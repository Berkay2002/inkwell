//! Rich text of a comment (`/RC`, XFDF `<contents-richtext>`): a small XHTML subset with CSS in `style`.
//!
//! It is hostile input and is never executed or fetched. It is read with the XML reader of `crate::exchange`
//! (no DOCTYPE, no entities but the five), then rebuilt from a whitelist: only the elements and `style`
//! properties below survive, links lose their address, `script`, `style` and `head` are dropped with what they
//! hold, and everything else is unwrapped to its text. What comes out is a `<body>` element Inkwell wrote itself.
//! Over [`MAX_RICH`] or deeper than [`MAX_NEST`], the rich text is dropped and the plain `/Contents` stays.

use super::ap::Why;
use crate::exchange::{self, Node, esc};
use std::fmt::Write as _;

/// Sanitized rich text, in bytes.
pub(crate) const MAX_RICH: usize = 128 << 10;
/// Nested elements.
const MAX_NEST: usize = 16;
const MAX_ELEMENTS: usize = 2000;

pub(super) const TOO_BIG: Why = "rich text over the size limit, or nested too deep";
pub(super) const DAMAGED: Why = "rich text that is not valid XML";

const BODY_OPEN: &str = "<body xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\" xfa:APIVersion=\"Acrobat:8.0.0\" xfa:spec=\"2.0.2\">";

/// `style` properties that are kept.
const PROPS: [&str; 20] = [
    "font-size",
    "font-family",
    "font-weight",
    "font-style",
    "font-stretch",
    "color",
    "text-align",
    "text-decoration",
    "text-indent",
    "line-height",
    "vertical-align",
    "background-color",
    "margin-left",
    "margin-right",
    "margin-top",
    "margin-bottom",
    "letter-spacing",
    "xfa-spacerun",
    "xfa-tab-stops",
    "text-transform",
];

/// Elements that are kept (with `body` as the root).
const KEPT: [&str; 11] = ["p", "span", "b", "i", "u", "s", "br", "div", "ul", "ol", "li"];
/// Elements dropped with everything inside.
const GONE: [&str; 10] = ["script", "style", "head", "title", "object", "embed", "iframe", "applet", "svg", "math"];
const BLOCK: [&str; 4] = ["p", "div", "li", "br"];

pub(crate) struct Rich {
    /// The `<body>` element.
    pub xml: String,
    /// Its text, with a line break at each paragraph.
    pub text: String,
}

struct Walk {
    out: String,
    text: String,
    elements: usize,
}

fn style_of(v: &str) -> String {
    let mut kept = Vec::new();
    for decl in v.split(';') {
        let Some((k, val)) = decl.split_once(':') else { continue };
        let (k, val) = (k.trim().to_ascii_lowercase(), val.trim());
        let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, ' ' | '#' | '.' | ',' | '%' | '\'' | '_' | '-');
        if PROPS.contains(&k.as_str()) && !val.is_empty() && val.len() <= 80 && val.chars().all(plain) {
            kept.push(format!("{k}:{val}"));
        }
    }
    kept.join(";")
}

impl Walk {
    fn newline(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
    }

    fn text_of(&mut self, s: &str) -> Result<(), Why> {
        if s.is_empty() {
            return Ok(());
        }
        self.text.push_str(s);
        self.out.push_str(&esc(s));
        if self.out.len() > MAX_RICH {
            return Err(TOO_BIG);
        }
        Ok(())
    }

    /// The text before an element's first child, then each child with the text that follows it.
    fn content(&mut self, n: &Node, depth: usize) -> Result<(), Why> {
        if depth > MAX_NEST {
            return Err(TOO_BIG);
        }
        let tails: usize = n.kids.iter().map(|k| k.tail.len()).sum();
        let lead = n.text.get(..n.text.len().saturating_sub(tails)).unwrap_or("");
        // Layout whitespace between the blocks of a body or a list is not text.
        let container = matches!(n.name.as_str(), "body" | "ul" | "ol");
        let text = |w: &mut Walk, s: &str| if container && s.trim().is_empty() { Ok(()) } else { w.text_of(s) };
        text(self, lead)?;
        for k in &n.kids {
            self.element(k, depth + 1)?;
            text(self, &k.tail)?;
        }
        Ok(())
    }

    fn element(&mut self, n: &Node, depth: usize) -> Result<(), Why> {
        self.elements += 1;
        if self.elements > MAX_ELEMENTS || depth > MAX_NEST {
            return Err(TOO_BIG);
        }
        let name = n.name.as_str();
        if GONE.contains(&name) {
            return Ok(());
        }
        if !KEPT.contains(&name) {
            // Links, images and anything unknown: what they hold stays, the element does not.
            return self.content(n, depth);
        }
        let mut open = format!("<{name}");
        if let Some(s) = n.attr("style").map(style_of).filter(|s| !s.is_empty()) {
            let _ = write!(open, " style=\"{}\"", esc(&s));
        }
        if let Some(d) = n.attr("dir").filter(|d| matches!(*d, "ltr" | "rtl")) {
            let _ = write!(open, " dir=\"{d}\"");
        }
        if name == "br" {
            self.out.push_str(&open);
            self.out.push_str("/>");
            self.newline();
            return Ok(());
        }
        open.push('>');
        self.out.push_str(&open);
        let block = BLOCK.contains(&name);
        if block {
            self.newline();
        }
        self.content(n, depth)?;
        let _ = write!(self.out, "</{name}>");
        if block {
            self.newline();
        }
        Ok(())
    }
}

/// Rebuilds the `<body>` element `body`.
pub(crate) fn sanitize(body: &Node) -> Result<Rich, Why> {
    let mut w = Walk { out: String::from(BODY_OPEN), text: String::new(), elements: 0 };
    w.content(body, 1)?;
    w.out.push_str("</body>");
    if w.out.len() > MAX_RICH {
        return Err(TOO_BIG);
    }
    let text = w.text.trim_end_matches('\n').to_string();
    Ok(Rich { xml: w.out, text })
}

/// Rich text held as a string or stream's bytes (an FDF's `/RC`).
pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Rich, Why> {
    let bytes = exchange::utf8_of(bytes).map_err(|_| DAMAGED)?;
    let root = exchange::parse_xml(&bytes).map_err(|_| DAMAGED)?;
    if root.name != "body" {
        return Err(DAMAGED);
    }
    sanitize(&root)
}
