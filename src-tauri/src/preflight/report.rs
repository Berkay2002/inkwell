// Adapted from pdfcraft (MIT OR Apache-2.0), crates/preflight/src/lib.rs @ b8ddf71
//! The preflight report as a file: HTML (a summary, then a table per group, like the accessibility
//! report) or a PDF set with the same writer the comparison report uses.

use super::{Finding, Report, Result, RuleResult, Status, bad};
use crate::diff::report::{ADD, CHG, DEL, DIM, INK, MARGIN, PAGE_W, RULE, Writer};

pub fn status_word(s: Status) -> &'static str {
    match s {
        Status::Passed => "Passed",
        Status::Failed => "Failed",
        Status::Manual => "Needs manual check",
        Status::Info => "Information",
    }
}

/// `(failed, manual, info, passed)`.
pub fn counts(r: &Report) -> (u32, u32, u32, u32) {
    let n = |s: Status| r.results.iter().filter(|x| x.status == s).count() as u32;
    (n(Status::Failed), n(Status::Manual), n(Status::Info), n(Status::Passed))
}

fn summary_line(r: &Report) -> String {
    let (f, m, i, p) = counts(r);
    let mut parts = vec![format!("{f} failed"), format!("{m} needs a manual check"), format!("{p} passed")];
    if i > 0 {
        parts.push(format!("{i} for information"));
    }
    format!("{} rules checked: {}", r.results.len(), parts.join(", "))
}

fn verdict(r: &Report) -> String {
    let label = r.profile.label();
    if r.conforms {
        let manual = r.results.iter().any(|x| x.status == Status::Manual);
        if manual {
            format!("No rule failed for {label}; some checks need a person to look.")
        } else {
            format!("No rule failed for {label}.")
        }
    } else {
        format!("The document does not meet {label}.")
    }
}

const COVERAGE: &str = "This is not a veraPDF validation. Inkwell checks the rules listed here, read from the file's objects; clauses not listed are not checked. See docs/pdfa.md for the list.";

fn where_of(f: &Finding) -> String {
    match (f.page, &f.object) {
        (Some(p), Some(o)) => format!("Page {} · {o}", p + 1),
        (Some(p), None) => format!("Page {}", p + 1),
        (None, Some(o)) => o.clone(),
        (None, None) => "Document".to_string(),
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Groups in the order they first appear.
fn groups(r: &Report) -> Vec<&'static str> {
    let mut g: Vec<&'static str> = Vec::new();
    for x in &r.results {
        if !g.contains(&x.group) {
            g.push(x.group);
        }
    }
    g
}

pub fn html(name: &str, date: &str, r: &Report) -> String {
    let title = format!("Preflight report: {}", r.profile.label());
    let mut o = String::new();
    o.push_str(&format!("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n<style>\n", esc(&title)));
    o.push_str("body{font:14px/1.45 system-ui,'Segoe UI',sans-serif;color:#1a1c20;max-width:960px;margin:2rem auto;padding:0 1rem}\n");
    o.push_str("h1{font-size:1.6rem;margin:0 0 .25rem}h2{margin:2rem 0 .5rem}h3{margin:1.5rem 0 .4rem}\n");
    o.push_str("table{border-collapse:collapse;width:100%}th,td{border:1px solid #ccc;padding:.4rem .6rem;text-align:left;vertical-align:top}\n");
    o.push_str("th{background:#f2f2f2}.failed{color:#b3392f;font-weight:600}.passed{color:#2f7d4f}.manual{color:#b7791f}.info{color:#6b675f}\n");
    o.push_str("ul{margin:.2rem 0;padding-left:1.2rem}.dim{color:#6b675f}\n</style>\n</head>\n<body>\n");
    o.push_str(&format!("<h1>{}</h1>\n<p class=\"dim\">File: {} · Report created: {} · {} page{}</p>\n", esc(&title), esc(name), esc(date), r.pages, if r.pages == 1 { "" } else { "s" }));
    if let Some(c) = &r.claimed {
        o.push_str(&format!("<p class=\"dim\">The file's metadata claims {}.</p>\n", esc(c)));
    }
    o.push_str(&format!("<h2>Summary</h2>\n<p>{}</p>\n<p>{}</p>\n<p class=\"dim\">{}</p>\n", esc(&verdict(r)), esc(&summary_line(r)), esc(COVERAGE)));
    o.push_str("<h2>Detailed report</h2>\n");
    for g in groups(r) {
        let rows: Vec<&RuleResult> = r.results.iter().filter(|x| x.group == g).collect();
        o.push_str(&format!("<h3>{}</h3>\n<table>\n<tr><th>Rule</th><th>Clause</th><th>Status</th><th>Description</th></tr>\n", esc(g)));
        for x in rows {
            let class = match x.status {
                Status::Passed => "passed",
                Status::Failed => "failed",
                Status::Manual => "manual",
                Status::Info => "info",
            };
            let mut desc = String::new();
            if x.status != Status::Passed {
                desc.push_str(&esc(x.hint));
            }
            if !x.findings.is_empty() {
                desc.push_str("<ul>");
                for fi in &x.findings {
                    let note = fi.note.as_deref().filter(|n| *n != super::check::NOTE_MANUAL).map(|n| format!(" (needs {})", esc(n))).unwrap_or_default();
                    if fi.page.is_none() && fi.object.is_none() {
                        desc.push_str(&format!("<li>{}</li>", esc(&fi.message)));
                    } else {
                        desc.push_str(&format!("<li>{}: {}{note}</li>", esc(&where_of(fi)), esc(&fi.message)));
                    }
                }
                desc.push_str("</ul>");
            }
            o.push_str(&format!("<tr><td>{}</td><td>{}</td><td class=\"{class}\">{}</td><td>{desc}</td></tr>\n", esc(x.name), esc(x.clause), status_word(x.status)));
        }
        o.push_str("</table>\n");
    }
    o.push_str("</body>\n</html>\n");
    o
}

pub fn pdf(name: &str, date: &str, r: &Report) -> Result<Vec<u8>> {
    let fail = |e: crate::diff::DiffError| bad(format!("could not write the report: {e}"));
    let mut w = Writer::new().map_err(fail)?;
    let width = PAGE_W - 2.0 * MARGIN;
    w.page().map_err(fail)?;
    w.para(MARGIN, width, 20.0, true, INK, &format!("Preflight report: {}", r.profile.label())).map_err(fail)?;
    w.para(MARGIN, width, 11.0, false, DIM, &format!("{name} · {date} · {} page{}", r.pages, if r.pages == 1 { "" } else { "s" })).map_err(fail)?;
    w.y -= 10.0;
    w.para(MARGIN, width, 12.0, true, if r.conforms { ADD } else { DEL }, &verdict(r)).map_err(fail)?;
    w.para(MARGIN, width, 11.0, false, INK, &summary_line(r)).map_err(fail)?;
    w.para(MARGIN, width, 9.5, false, DIM, COVERAGE).map_err(fail)?;
    w.y -= 8.0;
    for g in groups(r) {
        let rows: Vec<&RuleResult> = r.results.iter().filter(|x| x.group == g).collect();
        w.room(40.0).map_err(fail)?;
        w.y -= 12.0;
        w.para(MARGIN, width, 13.0, true, INK, g).map_err(fail)?;
        w.fill([MARGIN, w.y - 2.0, MARGIN + width, w.y - 1.2], RULE);
        w.y -= 4.0;
        for x in rows {
            let tone = match x.status {
                Status::Passed => ADD,
                Status::Failed => DEL,
                Status::Manual => CHG,
                Status::Info => DIM,
            };
            w.room(30.0).map_err(fail)?;
            w.y -= 15.0;
            let base = w.y;
            w.text(MARGIN + 4.0, base, 10.0, true, tone, status_word(x.status));
            w.text(MARGIN + 130.0, base, 11.0, true, INK, x.name);
            if x.status != Status::Passed {
                w.para(MARGIN + 130.0, width - 130.0, 9.5, false, DIM, &format!("{} ({})", x.hint, x.clause)).map_err(fail)?;
            }
            for fi in x.findings.iter().take(40) {
                let line = if fi.page.is_none() && fi.object.is_none() { fi.message.clone() } else { format!("{}: {}", where_of(fi), fi.message) };
                w.para(MARGIN + 140.0, width - 140.0, 9.5, false, INK, &line).map_err(fail)?;
            }
            if x.findings.len() > 40 {
                w.para(MARGIN + 140.0, width - 140.0, 9.5, false, DIM, &format!("... and {} more", x.findings.len() - 40)).map_err(fail)?;
            }
            w.y -= 3.0;
        }
    }
    w.finish().map_err(fail)
}
