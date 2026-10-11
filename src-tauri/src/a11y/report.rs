//! The report as a file: HTML (Acrobat's shape: a summary, then a table per group) or a PDF set
//! with the same writer the comparison report uses.

use super::{Finding, Report, Result, RuleResult, Status, bad};
use crate::diff::report::{CHG, DEL, DIM, INK, MARGIN, PAGE_W, RULE, ADD, Writer};

/// Groups in report order (Acrobat's seven).
pub const GROUPS: [&str; 7] = ["Document", "Page content", "Forms", "Alternate text", "Tables", "Lists", "Headings"];

pub fn status_word(s: Status) -> &'static str {
    match s {
        Status::Passed => "Passed",
        Status::Failed => "Failed",
        Status::Manual => "Needs manual check",
        Status::Skipped => "Skipped",
    }
}

/// `(failed, manual, passed, skipped)`.
pub fn counts(r: &Report) -> (u32, u32, u32, u32) {
    let n = |s: Status| r.results.iter().filter(|x| x.status == s).count() as u32;
    (n(Status::Failed), n(Status::Manual), n(Status::Passed), n(Status::Skipped))
}

fn summary_line(r: &Report) -> String {
    let (f, m, p, s) = counts(r);
    let mut parts = vec![format!("{f} failed"), format!("{m} needs a manual check"), format!("{p} passed")];
    if s > 0 {
        parts.push(format!("{s} skipped"));
    }
    format!("{} rules checked: {}", r.results.len(), parts.join(", "))
}

fn where_of(f: &Finding) -> String {
    match f.page {
        Some(p) => format!("Page {}", p + 1),
        None => "Document".to_string(),
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn html(name: &str, date: &str, r: &Report) -> String {
    let (f, ..) = counts(r);
    let mut o = String::new();
    o.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>Accessibility report</title>\n<style>\n");
    o.push_str("body{font:14px/1.45 system-ui,'Segoe UI',sans-serif;color:#1a1c20;max-width:960px;margin:2rem auto;padding:0 1rem}\n");
    o.push_str("h1{font-size:1.6rem;margin:0 0 .25rem}h2{margin:2rem 0 .5rem}h3{margin:1.5rem 0 .4rem}\n");
    o.push_str("table{border-collapse:collapse;width:100%}th,td{border:1px solid #ccc;padding:.4rem .6rem;text-align:left;vertical-align:top}\n");
    o.push_str("th{background:#f2f2f2}.failed{color:#b3392f;font-weight:600}.passed{color:#2f7d4f}.manual{color:#b7791f}.skipped{color:#6b675f}\n");
    o.push_str("ul{margin:.2rem 0;padding-left:1.2rem}.dim{color:#6b675f}\n</style>\n</head>\n<body>\n");
    o.push_str(&format!("<h1>Accessibility report</h1>\n<p class=\"dim\">File: {} · Report created: {} · {} page{}</p>\n", esc(name), esc(date), r.pages, if r.pages == 1 { "" } else { "s" }));
    o.push_str("<h2>Summary</h2>\n");
    o.push_str(&format!(
        "<p>{}</p>\n<p>{}</p>\n",
        if f > 0 { "The checker found problems which may prevent the document from being fully accessible." } else { "The checker found no problems in this document." },
        esc(&summary_line(r))
    ));
    let (f, m, p, s) = counts(r);
    o.push_str(&format!("<ul>\n<li>Needs manual check: {m}</li>\n<li>Skipped: {s}</li>\n<li>Passed: {p}</li>\n<li>Failed: {f}</li>\n</ul>\n"));
    o.push_str("<h2>Detailed report</h2>\n");
    for g in GROUPS {
        let rows: Vec<&RuleResult> = r.results.iter().filter(|x| x.group == g).collect();
        if rows.is_empty() {
            continue;
        }
        o.push_str(&format!("<h3>{g}</h3>\n<table>\n<tr><th>Rule name</th><th>Status</th><th>Description</th></tr>\n"));
        for x in rows {
            let class = match x.status {
                Status::Passed => "passed",
                Status::Failed => "failed",
                Status::Manual => "manual",
                Status::Skipped => "skipped",
            };
            let mut desc = String::new();
            match x.status {
                Status::Passed => {}
                Status::Skipped => desc.push_str("Does not apply to this document."),
                _ => desc.push_str(&esc(x.hint)),
            }
            if !x.findings.is_empty() {
                desc.push_str("<ul>");
                for fi in &x.findings {
                    desc.push_str(&format!("<li>{}: {}</li>", esc(&where_of(fi)), esc(&fi.message)));
                }
                desc.push_str("</ul>");
            }
            o.push_str(&format!("<tr><td>{}</td><td class=\"{class}\">{}</td><td>{desc}</td></tr>\n", esc(x.name), status_word(x.status)));
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
    w.para(MARGIN, width, 20.0, true, INK, "Accessibility report").map_err(fail)?;
    w.para(MARGIN, width, 11.0, false, DIM, &format!("{name} · {date} · {} page{}", r.pages, if r.pages == 1 { "" } else { "s" })).map_err(fail)?;
    w.y -= 10.0;
    w.para(MARGIN, width, 12.0, false, INK, &summary_line(r)).map_err(fail)?;
    w.y -= 8.0;
    for g in GROUPS {
        let rows: Vec<&RuleResult> = r.results.iter().filter(|x| x.group == g).collect();
        if rows.is_empty() {
            continue;
        }
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
                Status::Skipped => DIM,
            };
            w.room(30.0).map_err(fail)?;
            w.y -= 15.0;
            let base = w.y;
            w.text(MARGIN + 4.0, base, 10.0, true, tone, status_word(x.status));
            w.text(MARGIN + 130.0, base, 11.0, true, INK, x.name);
            if matches!(x.status, Status::Failed | Status::Manual) {
                w.para(MARGIN + 130.0, width - 130.0, 9.5, false, DIM, x.hint).map_err(fail)?;
            }
            for fi in x.findings.iter().take(40) {
                w.para(MARGIN + 140.0, width - 140.0, 9.5, false, INK, &format!("{}: {}", where_of(fi), fi.message)).map_err(fail)?;
            }
            if x.findings.len() > 40 {
                w.para(MARGIN + 140.0, width - 140.0, 9.5, false, DIM, &format!("... and {} more", x.findings.len() - 40)).map_err(fail)?;
            }
            w.y -= 3.0;
        }
    }
    w.finish().map_err(fail)
}
