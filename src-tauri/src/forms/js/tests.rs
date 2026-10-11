//! The script engine on its own: the object model, the AF functions, the limits and what happens when a
//! script misbehaves. (Scripts in a document, through the edit lane, are tested in `forms/tests.rs`.)

use super::*;
use crate::forms::af;
use std::time::Duration;

fn field(name: &str, value: &str) -> FieldState {
    FieldState {
        name: name.into(),
        kind: FieldType::Text,
        value: if value.is_empty() { Vec::new() } else { vec![value.into()] },
        default: Vec::new(),
        readonly: false,
        required: false,
        display: DISPLAY_VISIBLE,
        options: Vec::new(),
        char_limit: None,
        text_color: None,
        fill_color: None,
        stroke_color: None,
    }
}

fn job(event: Event, script: &str, fields: Vec<FieldState>, timeout: Duration) -> Job {
    Job {
        key: event.target.clone().unwrap_or_default(),
        event,
        script: script.into(),
        fields,
        doc: DocInfo { file_name: "form.pdf".into(), num_pages: 3, page: 0, info: vec![("title".into(), "A form".into())] },
        timeout,
    }
}

/// Runs `script` as the Format, Keystroke, Validate or Calculate event of field `Target` holding `value`.
fn run(s: &Scripts, name: &'static str, value: &str, script: &str) -> Outcome {
    s.run(job(Event::new(name, "Target", value), script, vec![field("Target", value), field("A", "5"), field("B", "7")], EVENT_TIME))
}

fn calc(s: &Scripts, script: &str) -> Outcome {
    s.run(job(Event::new("Calculate", "Target", ""), script, vec![field("Target", ""), field("A", "5"), field("B", "7")], CALC_TIME))
}

#[test]
fn a_number_format_wrapped_in_other_code_runs_the_same_native_formatter() {
    let s = Scripts::new();
    // Not exactly one AF call, so it is the script engine that runs it.
    let out = run(&s, "Format", "1234.5", "var symbol = \"$\"; AFNumber_Format(2, 0, 0, 0, symbol, true);");
    assert_eq!(out.error, None, "{:?}", s.log_text());
    assert_eq!(out.value, "$1,234.50");
    // And it is what the fast path gives.
    let f = af::parse_format("AFNumber_Format(2, 0, 0, 0, \"$\", true);").unwrap();
    assert_eq!(af::format_value(&f, "1234.5"), out.value);
    let out = run(&s, "Format", "-0.5", "if (event.value !== \"\") { AFPercent_Format(1, 0); }");
    assert_eq!(out.value, "-50.0%");
}

#[test]
fn a_date_keystroke_rejects_an_impossible_date_with_acrobats_message() {
    let s = Scripts::new();
    let out = run(&s, "Keystroke", "31/31/2020", "if (event.willCommit) { AFDate_KeystrokeEx(\"dd/mm/yyyy\"); }");
    assert!(!out.rc);
    assert!(out.alerts.iter().any(|a| a.message.contains("Invalid date")), "{:?}", out.alerts);
    let out = run(&s, "Keystroke", "05/04/2020", "if (event.willCommit) { AFDate_KeystrokeEx(\"dd/mm/yyyy\"); }");
    assert!(out.rc);
    assert!(out.alerts.is_empty());
    assert_eq!(out.value, "05/04/2020");
}

#[test]
fn a_keystroke_while_typing_checks_the_partial_entry_and_on_commit_the_full_rule() {
    let s = Scripts::new();
    let mut ev = Event::new("Keystroke", "Target", "12");
    ev.will_commit = false;
    ev.change = "x".into();
    ev.sel_start = 2.0;
    ev.sel_end = 2.0;
    let out = s.run(job(ev, "AFNumber_Keystroke(2, 0, 0, 0, \"\", true);", vec![field("Target", "12")], EVENT_TIME));
    assert!(!out.rc, "a letter is not a number");
    let mut ev = Event::new("Keystroke", "Target", "12");
    ev.will_commit = false;
    ev.change = "3".into();
    ev.sel_start = 2.0;
    ev.sel_end = 2.0;
    let out = s.run(job(ev, "AFNumber_Keystroke(2, 0, 0, 0, \"\", true);", vec![field("Target", "12")], EVENT_TIME));
    assert!(out.rc);
    // On commit "$1,000" is a number once the currency is allowed for.
    let out = run(&s, "Keystroke", "$1,000", "if (event.willCommit) AFNumber_Keystroke(0, 0, 0, 0, \"$\", true);");
    assert!(out.rc, "{:?} {:?}", out.alerts, s.log_text());
    assert_eq!(out.value, "1000");
}

#[test]
fn a_range_validation_gives_acrobats_message_and_rejects() {
    let s = Scripts::new();
    let script = "if (event.value !== \"\") { AFRange_Validate(true, 0, true, 100); }";
    let out = run(&s, "Validate", "150", script);
    assert!(!out.rc);
    assert!(out.alerts.iter().any(|a| a.message.contains("less than or equal to 100")), "{:?}", out.alerts);
    assert!(run(&s, "Validate", "50", script).rc);
}

#[test]
fn a_calculation_reads_other_fields_and_sets_event_value() {
    let s = Scripts::new();
    let out = calc(&s, "var a = this.getField(\"A\"), b = this.getField(\"B\"); event.value = a.value + b.value;");
    assert_eq!(out.error, None, "{:?}", s.log_text());
    assert_eq!(out.value, "12", "digit strings read as numbers");
    let out = calc(&s, "AFSimple_Calculate(\"SUM\", new Array(\"A\", \"B\"));");
    assert_eq!(out.value, "12");
    let out = calc(&s, "AFSimple_Calculate(\"PRD\", [\"A\", \"B\"]);");
    assert_eq!(out.value, "35");
}

#[test]
fn a_script_writing_another_field_reports_it_as_changed() {
    let s = Scripts::new();
    let out = calc(&s, "this.getField(\"A\").value = \"9\"; this.getField(\"B\").readonly = true; this.getField(\"B\").fillColor = color.red;");
    assert_eq!(out.error, None, "{:?}", s.log_text());
    let names: Vec<_> = out.changed.iter().map(|f| f.state.name.as_str()).collect();
    assert_eq!(names, ["A", "B"]);
    assert_eq!(out.changed[0].state.value, ["9"]);
    assert!(out.changed[1].state.readonly);
    assert_eq!(out.changed[1].state.fill_color.as_deref().map(|c| c[0].as_str()), Some("RGB"));
    // Only what the script set is reported as set.
    assert_eq!(out.changed[0].props, BTreeSet::from([Prop::Value]));
    assert_eq!(out.changed[1].props, BTreeSet::from([Prop::ReadOnly, Prop::FillColor]));
}

#[test]
fn an_endless_loop_times_out_in_time_and_leaves_a_usable_runtime() {
    let s = Scripts::new();
    let t = Instant::now();
    let out = calc(&s, "while (true) {}");
    let took = t.elapsed();
    assert_eq!(out.error, Some(ScriptError::Timeout));
    assert!(took < Duration::from_millis(1500), "{took:?}");
    assert!(out.rc && out.value.is_empty(), "the event result is as if no script ran");
    // That field's script is off for the session ...
    assert!(calc(&s, "event.value = \"x\";").skipped);
    // ... but another field's runs on the same runtime.
    let other = s.run(job(Event::new("Calculate", "Other", ""), "event.value = String(1 + 1);", vec![field("Other", "")], CALC_TIME));
    assert_eq!(other.value, "2");
    assert!(s.log().iter().any(|l| l.message.contains("stopped")), "{:?}", s.log_text());
}

#[test]
fn three_timeouts_turn_every_script_of_the_document_off_until_it_is_turned_on_again() {
    let s = Scripts::new();
    for i in 0..3 {
        let name = format!("F{i}");
        let out = s.run(job(Event::new("Format", &name, "1"), "for (;;) {}", vec![field(&name, "1")], Duration::from_millis(100)));
        assert_eq!(out.error, Some(ScriptError::Timeout));
    }
    assert!(!s.usable());
    assert!(s.enabled(), "the person did not turn them off");
    assert!(run(&s, "Format", "1", "event.value = \"x\";").skipped);
    s.set_enabled(true);
    assert!(s.usable());
    assert_eq!(run(&s, "Format", "1", "event.value = \"x\";").value, "x");
}

#[test]
fn a_memory_bomb_fails_softly() {
    let s = Scripts::new();
    let out = calc(&s, "var big = \"x\".repeat(1 << 28); event.value = big;");
    assert!(matches!(out.error, Some(ScriptError::Memory) | Some(ScriptError::Script(_))), "{:?}", out.error);
    assert!(out.rc && out.value.is_empty());
    let out = calc(&s, "var a = []; for (;;) a.push(new Array(1 << 16).fill(1));");
    assert!(matches!(out.error, Some(ScriptError::Memory) | Some(ScriptError::Script(_))), "{:?}", out.error);
    // The runtime is still there.
    let other = s.run(job(Event::new("Calculate", "Other", ""), "event.value = \"ok\";", vec![field("Other", "")], CALC_TIME));
    assert_eq!(other.value, "ok");
}

#[test]
fn deep_recursion_is_an_error_not_a_crash() {
    let s = Scripts::new();
    let out = calc(&s, "function f(n) { return f(n + 1) + 1; } event.value = String(f(0));");
    assert!(matches!(out.error, Some(ScriptError::Script(_)) | Some(ScriptError::Memory)), "{:?}", out.error);
    assert_eq!(calc(&s, "event.value = \"alive\";").value, "alive");
}

#[test]
fn a_document_level_function_is_called_from_a_field_script() {
    let s = Scripts::new();
    s.doc_scripts(|| DocScripts::new(vec![("a lib".into(), "function triple(x) { return x * 3; } var rate = 0.5;".into())]));
    let out = calc(&s, "event.value = triple(14) + rate;");
    assert_eq!(out.error, None, "{:?}", s.log_text());
    assert_eq!(out.value, "42.5");
}

#[test]
fn document_scripts_run_in_the_order_given_and_a_failing_one_does_not_stop_the_rest() {
    let s = Scripts::new();
    s.doc_scripts(|| {
        DocScripts::new(vec![
            ("a".into(), "var order = \"a\";".into()),
            ("b".into(), "order += \"b\";".into()),
            ("c".into(), "nope(".into()),
            ("d".into(), "order += \"d\";".into()),
        ])
    });
    let out = calc(&s, "event.value = order;");
    assert_eq!(out.value, "abd");
    assert!(s.log().iter().any(|l| l.message.contains("document script \"c\"")), "{:?}", s.log_text());
}

#[test]
fn a_global_persists_from_one_event_to_the_next() {
    let s = Scripts::new();
    let script = "globalThis.hits = (globalThis.hits || 0) + 1; event.value = String(hits);";
    assert_eq!(calc(&s, script).value, "1");
    assert_eq!(calc(&s, script).value, "2");
    // `var` in a field script is a global too, and `let` stays in the script.
    calc(&s, "var kept = 11; let gone = 12;");
    assert_eq!(calc(&s, "event.value = String(kept) + typeof gone;").value, "11undefined");
}

#[test]
fn scripts_turned_off_are_not_run() {
    let s = Scripts::new();
    s.set_enabled(false);
    assert!(!s.enabled() && !s.usable());
    let out = run(&s, "Format", "1", "event.value = \"ran\";");
    assert!(out.skipped);
    assert_eq!(out.value, "1");
    s.set_enabled(true);
    assert_eq!(run(&s, "Format", "1", "event.value = \"ran\";").value, "ran");
}

#[test]
fn unsupported_calls_return_undefined_and_are_logged() {
    let s = Scripts::new();
    let out = calc(&s, "var r = app.setTimeOut(\"1\", 10); var m = this.mailDoc({}); event.value = typeof r + String(m) + typeof this.flattenPages;");
    assert_eq!(out.error, None, "{:?}", s.log_text());
    assert_eq!(out.value, "objectundefinedfunction", "a timer handle comes back, and nothing ever fires");
    let log = s.log_text();
    assert!(log.iter().any(|m| m.contains("app.setTimeOut") && m.contains("is not supported")), "{log:?}");
    assert!(log.iter().any(|m| m.contains("mailDoc is not supported")), "{log:?}");
}

#[test]
fn alerts_and_requests_come_back_to_the_ui_and_nothing_is_done() {
    let s = Scripts::new();
    let out = calc(
        &s,
        "app.alert(\"Hello\", 3, 0, \"T\"); app.launchURL(\"https://example.com/x\"); app.launchURL(\"file:///c:/windows/system32/calc.exe\");
         app.launchURL(\"javascript:alert(1)\"); this.submitForm(\"https://example.com/post\"); app.beep(0);",
    );
    assert_eq!(out.error, None, "{:?}", s.log_text());
    assert_eq!(out.alerts.len(), 1);
    assert_eq!(out.alerts[0].message, "Hello");
    assert!(out.requests.contains(&Request::LaunchUrl { url: "https://example.com/x".into() }), "{:?}", out.requests);
    assert!(out.requests.iter().any(|r| matches!(r, Request::Submit { .. })));
    assert!(out.requests.contains(&Request::Beep));
    assert!(!out.requests.iter().any(|r| matches!(r, Request::LaunchUrl { url } if !url.starts_with("http"))), "{:?}", out.requests);
}

#[test]
fn util_formats_like_acrobat() {
    let s = Scripts::new();
    let out = calc(
        &s,
        "event.value = [util.printf(\"%,0.2f|%5d|%s|%x\", 1234.567, 42, \"s\", 255), util.printx(\"(999) 999-9999\", \"5551234567\"),
         util.printd(\"yyyy-mm-dd\", new Date(2020, 3, 5)), String(util.scand(\"dd/mm/yyyy\", \"05/04/2020\").getMonth())].join(\"|\");",
    );
    assert_eq!(out.error, None, "{:?}", s.log_text());
    assert_eq!(out.value, "1,234.57|   42|s|FF|(555) 123-4567|2020-04-05|3");
    let out = calc(&s, "try { util.printf(\"%999999999d\", 1); event.value = \"no\"; } catch (e) { event.value = e.name; }");
    assert_eq!(out.value, "RangeError");
}

#[test]
fn the_document_object_model_answers_like_acrobat() {
    let s = Scripts::new();
    let out = calc(
        &s,
        "event.value = [this.numFields, this.getNthFieldName(1), this.numPages, this.info.title, this.documentFileName, app.viewerType,
         this.getField(\"Nope\"), typeof event.target, event.targetName, event.name, event.type].join(\"|\");",
    );
    assert_eq!(out.error, None, "{:?}", s.log_text());
    assert_eq!(out.value, "3|A|3|A form|form.pdf|Reader||object|Target|Calculate|Field");
}

#[test]
fn a_script_that_is_too_long_is_refused_and_logged() {
    let s = Scripts::new();
    let big = format!("var s = '{}';", "x".repeat(MAX_SCRIPT_BYTES));
    let out = calc(&s, &big);
    assert!(matches!(out.error, Some(ScriptError::Refused(_))));
    assert!(s.log_text().iter().any(|m| m.contains("too long")));
}

#[test]
fn a_host_string_is_cut_at_64_kib() {
    let s = Scripts::new();
    let out = calc(&s, "event.value = \"y\".repeat(1 << 20);");
    assert_eq!(out.error, None);
    assert_eq!(out.value.len(), MAX_TEXT_BYTES);
}

#[test]
fn the_log_keeps_the_last_256_lines() {
    let s = Scripts::new();
    for i in 0..300 {
        s.note(LogLevel::Info, None, format!("line {i}"));
    }
    let log = s.log_text();
    assert_eq!(log.len(), MAX_LOG_LINES);
    assert_eq!(log.last().map(String::as_str), Some("line 299"));
}

#[test]
fn a_syntax_error_is_logged_with_the_field_and_changes_nothing() {
    let s = Scripts::new();
    let out = run(&s, "Format", "1", "event.value = ;");
    assert!(matches!(out.error, Some(ScriptError::Script(_))));
    assert_eq!(out.value, "1");
    assert!(s.log().iter().any(|l| l.field.as_deref() == Some("Target") && l.level == LogLevel::Error), "{:?}", s.log_text());
}

#[test]
fn a_worker_that_stops_answering_is_given_up_on_and_scripts_cannot_be_turned_on_again_this_session() {
    let s = Scripts::new();
    let t = Instant::now();
    // A host call that does not come back: the interrupt cannot reach it.
    let out = s.run(job(Event::new("Format", "Target", "1"), "__sleep(6000);", vec![field("Target", "1")], Duration::from_millis(100)));
    assert_eq!(out.error, Some(ScriptError::Lost));
    assert!(t.elapsed() < Duration::from_millis(3500), "{:?}", t.elapsed());
    assert!(!s.usable());
    assert!(!s.can_enable(), "the stuck thread is the only one this file gets");
    assert!(s.log_text().iter().any(|m| m.contains("stopped answering")), "{:?}", s.log_text());
    // Turning them on again is refused, with a message, and nothing runs.
    assert!(!s.set_enabled(true));
    assert!(!s.usable());
    assert!(s.log_text().iter().any(|m| m.contains("cannot be turned on again")), "{:?}", s.log_text());
    assert!(run(&s, "Format", "1", "event.value = \"fresh\";").skipped);
}

#[test]
fn a_panic_in_a_host_function_is_an_error_not_an_abort_and_the_runtime_is_replaced() {
    let s = Scripts::new();
    s.doc_scripts(|| DocScripts::new(vec![("lib".into(), "var fromDoc = 41;".into())]));
    let out = run(&s, "Format", "1", "__boom(); event.value = \"unreachable\";");
    assert_eq!(out.error, Some(ScriptError::Panic));
    assert_eq!(out.value, "1", "the event result is as if no script ran");
    assert!(s.log_text().iter().any(|m| m.contains("restarted")), "{:?}", s.log_text());
    // A fresh runtime, with the document scripts run again.
    let out = run(&s, "Format", "1", "event.value = String(fromDoc + 1);");
    assert_eq!(out.error, None, "{:?}", s.log_text());
    assert_eq!(out.value, "42");
}

#[test]
fn three_engine_panics_take_the_scripts_down_like_three_timeouts() {
    let s = Scripts::new();
    for _ in 0..3 {
        assert!(s.usable());
        assert_eq!(run(&s, "Format", "1", "__boom();").error, Some(ScriptError::Panic));
    }
    assert!(!s.usable(), "{:?}", s.log_text());
    assert!(run(&s, "Format", "1", "event.value = \"x\";").skipped);
}

#[test]
fn a_file_whose_worker_was_lost_stays_refused_when_it_is_opened_again() {
    let path = "forms-js-test-lost-file-A.pdf";
    let s = Scripts::for_path(path);
    let out = s.run(job(Event::new("Format", "Target", "1"), "__sleep(6000);", vec![field("Target", "1")], Duration::from_millis(100)));
    assert_eq!(out.error, Some(ScriptError::Lost));
    // The same path again, with the first closed: no worker is started for it.
    drop(s);
    let again = Scripts::for_path(path);
    assert!(!again.usable() && !again.can_enable());
    assert!(!again.set_enabled(true));
    assert!(run(&again, "Format", "1", "event.value = \"fresh\";").skipped);
    assert!(again.log_text().iter().any(|m| m.contains("until the app exits")), "{:?}", again.log_text());
    // Another file is not affected.
    let other = Scripts::for_path("forms-js-test-lost-file-B.pdf");
    assert_eq!(run(&other, "Format", "1", "event.value = \"ok\";").value, "ok");
}

#[test]
fn a_slow_rebuild_after_a_panic_is_not_taken_for_a_lost_worker() {
    let s = Scripts::new();
    // Document scripts that take a while to run again, so the rebuild after a panic is slow.
    s.doc_scripts(|| DocScripts::new(vec![("slow".into(), "var t = Date.now(); while (Date.now() - t < 700) {}".into())]));
    let out = run(&s, "Format", "1", "__boom();");
    assert_eq!(out.error, Some(ScriptError::Panic));
    // The worker is still rebuilding (~0.7 s) when this job arrives: the wait for it counts from the rebuild's end, so
    // the 100 ms limit is not spent on it (the allowance is OPEN_TIME + GRACE on top, set while the flag is up).
    let out = s.run(job(Event::new("Format", "Target", "1"), "event.value = \"after\";", vec![field("Target", "1")], Duration::from_millis(100)));
    assert_eq!(out.error, None, "{:?}", s.log_text());
    assert_eq!(out.value, "after");
    assert!(s.can_enable() && s.usable());
}

#[test]
fn a_script_that_throws_the_word_interrupted_is_an_error_not_a_timeout() {
    let s = Scripts::new();
    for _ in 0..4 {
        let out = run(&s, "Format", "1", "throw new Error(\"interrupted\");");
        assert!(matches!(out.error, Some(ScriptError::Script(_))), "{:?}", out.error);
    }
    // Four of them are not four strikes: nothing was taken down, and the field still runs.
    assert!(s.usable());
    assert_eq!(run(&s, "Format", "1", "event.value = \"ok\";").value, "ok");
}

#[test]
fn a_regular_expression_that_backtracks_for_ever_stops_at_the_deadline() {
    let s = Scripts::new();
    let t = Instant::now();
    let out = s.run(job(
        Event::new("Format", "Target", "1"),
        "var s = ''; for (var i = 0; i < 40; i++) s += 'a'; /^(a+)+$/.test(s + 'b');",
        vec![field("Target", "1")],
        Duration::from_millis(200),
    ));
    assert_eq!(out.error, Some(ScriptError::Timeout), "{:?}", s.log_text());
    assert!(t.elapsed() < Duration::from_millis(1500), "{:?}", t.elapsed());
}

#[test]
fn atomics_wait_cannot_block_the_worker() {
    let s = Scripts::new();
    let t = Instant::now();
    let out = run(
        &s,
        "Format",
        "1",
        "try { Atomics.wait(new Int32Array(new SharedArrayBuffer(16)), 0, 0, 60000); event.value = 'waited'; } catch (e) { event.value = 'threw'; }",
    );
    assert_eq!(out.error, None, "{:?}", s.log_text());
    assert_eq!(out.value, "threw");
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
}

#[test]
fn promise_callbacks_run_inside_the_script_that_queued_them_and_never_after_its_deadline() {
    let s = Scripts::new();
    // A callback runs within its own script's time.
    let out = run(&s, "Format", "1", "Promise.resolve().then(function () { event.value = 'then'; });");
    assert_eq!(out.value, "then");
    // A chain that never ends is cut at the deadline ...
    let t = Instant::now();
    let out = s.run(job(
        Event::new("Format", "Other", "1"),
        "var n = 0; (function again() { n++; globalThis.leaked = n; Promise.resolve().then(again); })();",
        vec![field("Other", "1")],
        Duration::from_millis(200),
    ));
    assert_eq!(out.error, Some(ScriptError::Timeout), "{:?}", s.log_text());
    assert!(t.elapsed() < Duration::from_millis(1500), "{:?}", t.elapsed());
    // ... and none of it is left to run in the next script's time: that script starts on a fresh runtime.
    let out = run(&s, "Format", "1", "var before = typeof leaked; var spin = 0; for (var i = 0; i < 2000; i++) spin++; event.value = before + ':' + (typeof leaked);");
    assert_eq!(out.error, None, "{:?}", s.log_text());
    assert_eq!(out.value, "undefined:undefined");
}

#[test]
fn a_document_script_that_assigns_an_af_function_by_a_computed_name_gets_the_engine_path() {
    for src in [
        "this[\"AFNumber\" + \"_Format\"] = function () {};",
        "globalThis['A' + 'FNumber_Format'] = function () {};",
        "Object.defineProperty(this, 'AFNumber_Format', { value: function () {} });",
        "Object.assign(this, { AFNumber_Format: function () {} });",
        "var k = 'AFNumber_Format'; this[k] = function () {};",
        "function AFNumber_Format() {}",
        "this.AFNumber_Format = function () {};",
        "const AFNumber_Format = () => {};",
    ] {
        assert!(DocScripts::new(vec![("s".into(), src.into())]).overrides_af, "{src}");
    }
    for src in ["var rate = 0.5; function vat(x) { return x * 1.25; }", "AFNumber_Format(2, 0, 0, 0, '$', true);"] {
        assert!(!DocScripts::new(vec![("s".into(), src.into())]).overrides_af, "{src}");
    }
}

#[test]
fn bidi_controls_are_stripped_from_the_links_a_script_asks_for() {
    let s = Scripts::new();
    let out = run(&s, "Format", "1", "app.launchURL('https://example.com/\\u202Egpj.exe'); app.launchURL('mailto:a@b.se\\u2066');");
    assert_eq!(
        out.requests,
        vec![Request::LaunchUrl { url: "https://example.com/gpj.exe".into() }, Request::LaunchUrl { url: "mailto:a@b.se".into() }]
    );
}

#[test]
fn the_runtime_has_no_way_to_reach_files_the_network_or_the_process() {
    let s = Scripts::new();
    let out = calc(
        &s,
        "event.value = [typeof require, typeof process, typeof fetch, typeof XMLHttpRequest, typeof std, typeof os, typeof import_, typeof Deno,
         typeof setTimeout, typeof __h, typeof Net, typeof app.execMenuItem].join(\",\");",
    );
    assert_eq!(out.error, None, "{:?}", s.log_text());
    assert_eq!(out.value, "undefined,undefined,undefined,undefined,undefined,undefined,undefined,undefined,undefined,undefined,undefined,function");
}

#[test]
fn the_fast_path_and_the_script_engine_agree_on_acrobat_generated_scripts() {
    let s = Scripts::new();
    // Prefixing a statement takes the script off the fast path, so it runs in the engine.
    let in_engine = |src: &str| format!("var viaEngine = true;\n{src}");

    let formats: [(&str, &[&str]); 10] = [
        ("AFNumber_Format(2, 0, 0, 0, \"\", true);", &["1234.5", "-3", "0", "", "1e3", "abc"]),
        ("AFNumber_Format(0, 1, 1, 0, \"\u{20ac}\", false);", &["1234567.891", "-42"]),
        ("AFNumber_Format(2, 2, 2, 0, \"$\", true)", &["-1234.5", "0.005"]),
        ("AFPercent_Format(2, 0);", &["0.256", "1", "-0.5"]),
        ("AFDate_FormatEx(\"dd/mm/yyyy\");", &["2020-04-05", "04/05/2020", "nonsense"]),
        ("AFDate_Format(3);", &["2020-04-05", "12/31/1999"]),
        ("AFTime_FormatEx(\"HH:MM\");", &["13:45:10", "1:05 pm"]),
        ("AFSpecial_Format(0);", &["12345", "1234"]),
        ("AFSpecial_Format(2);", &["5551234567", "555123"]),
        ("AFSpecial_Format(3);", &["123456789"]),
    ];
    for (src, values) in formats {
        let f = af::parse_format(src).unwrap_or_else(|| panic!("not a format: {src}"));
        for v in values {
            let out = run(&s, "Format", v, &in_engine(src));
            assert_eq!(out.error, None, "{src} {v}: {:?}", s.log_text());
            assert_eq!(out.value, af::format_value(&f, v), "{src} on {v:?}");
        }
    }

    let keystrokes: [(&str, &[&str]); 7] = [
        ("AFNumber_Keystroke(2, 0, 0, 0, \"\", true);", &["12.5", "abc", "1,000", ""]),
        ("AFNumber_Keystroke(0, 0, 0, 0, \"$\", true);", &["$1,000", "1000"]),
        ("AFPercent_Keystroke(2, 0);", &["25", "x"]),
        ("AFDate_KeystrokeEx(\"mm/dd/yyyy\");", &["12/31/2020", "13/45/2020", ""]),
        ("AFTime_KeystrokeEx(\"HH:MM\");", &["13:45", "25:99"]),
        ("AFSpecial_Keystroke(0);", &["12345", "1234", "abcde"]),
        ("AFSpecial_KeystrokeEx(\"999-99\");", &["123-45", "12345", "abc-de"]),
    ];
    for (src, values) in keystrokes {
        let f = af::parse_keystroke(src).unwrap_or_else(|| panic!("not a keystroke: {src}"));
        for v in values {
            let out = run(&s, "Keystroke", v, &in_engine(src));
            match af::keystroke(&f, "Target", v) {
                Ok(stored) => assert!(out.rc && out.value == stored, "{src} on {v:?}: native stores {stored:?}, engine rc={} value={:?} {:?}", out.rc, out.value, out.alerts),
                Err(msg) => assert!(!out.rc && out.alerts.iter().any(|a| a.message == msg), "{src} on {v:?}: native rejects with {msg:?}, engine rc={} {:?}", out.rc, out.alerts),
            }
        }
    }

    let validates: [(&str, &[&str]); 3] = [
        ("AFRange_Validate(true, 1, true, 10);", &["5", "0", "11", "", "abc"]),
        ("AFRange_Validate(true, 0, false, 0);", &["-1", "1"]),
        ("AFRange_Validate(false, 0, true, 99.5);", &["99.5", "99.6"]),
    ];
    for (src, values) in validates {
        let v = af::parse_validate(src).unwrap_or_else(|| panic!("not a validate: {src}"));
        for value in values {
            let out = run(&s, "Validate", value, &in_engine(src));
            match af::validate(&v, value) {
                Ok(()) => assert!(out.rc, "{src} on {value:?}: {:?}", out.alerts),
                Err(msg) => assert!(!out.rc && out.alerts.iter().any(|a| a.message == msg), "{src} on {value:?}: native {msg:?}, engine {:?}", out.alerts),
            }
        }
    }

    // Calculations: the engine reads the same field values the native code does.
    for op in ["SUM", "PRD", "AVG", "MIN", "MAX"] {
        let src = format!("AFSimple_Calculate(\"{op}\", new Array(\"A\", \"B\"));");
        let c = af::parse_calculate(&src).unwrap();
        let lookup = |n: &str| match n {
            "A" => vec!["5".to_string()],
            "B" => vec!["7".to_string()],
            _ => Vec::new(),
        };
        let native = af::calculate(&c, &lookup).unwrap();
        assert_eq!(calc(&s, &in_engine(&src)).value, native, "{op}");
    }
}
