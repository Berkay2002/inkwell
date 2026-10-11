// Adapted from pdfcraft (MIT OR Apache-2.0), crates/preflight/src/xmp.rs @ b8ddf71
//! XMP metadata for the standards: reading the properties a check needs, and writing the
//! identification schemas (`pdfaid`, `pdfuaid`, `pdfxid`) together with the document information
//! it must agree with (ISO 19005 6.6 / 6.7).
//!
//! This is text handling, not an XML parser: it reads the two forms XMP writers produce (an
//! attribute on `rdf:Description`, or a property element, simple or inside `rdf:Alt`/`rdf:Seq`),
//! and writes new properties as a `rdf:Description` of their own, which XMP allows next to the
//! existing ones.

/// The value of a simple XMP property, written either as an attribute (`pdfaid:part="2"`) or as an
/// element (`<pdfaid:part>2</pdfaid:part>`, or inside an `rdf:Alt`/`rdf:Seq`: the first item).
pub fn value(xmp: &str, key: &str) -> Option<String> {
    for q in ['"', '\''] {
        let attr = format!("{key}={q}");
        let mut from = 0;
        while let Some(i) = xmp[from..].find(&attr) {
            let at = from + i;
            // `pdfaid:part=` and not the tail of another name.
            let boundary = xmp[..at].chars().next_back().is_none_or(|c| c.is_whitespace());
            let rest = &xmp[at + attr.len()..];
            if boundary {
                if let Some(e) = rest.find(q) {
                    return Some(unescape(&rest[..e]));
                }
            }
            from = at + attr.len();
        }
    }
    let (start, end) = element_span(xmp, key)?;
    let whole = &xmp[start..end];
    let open_end = whole.find('>')? + 1;
    if whole[..open_end].ends_with("/>") {
        return Some(String::new());
    }
    let close = whole.rfind("</")?;
    let inner = &whole[open_end..close];
    if let Some(li) = inner.find("<rdf:li") {
        let s = li + inner[li..].find('>')? + 1;
        let e = s + inner[s..].find("</rdf:li>")?;
        return Some(unescape(inner[s..e].trim()));
    }
    Some(unescape(inner.trim()))
}

/// `[start, end)` of the first `<key ...>...</key>` (or `<key .../>`) element.
fn element_span(xmp: &str, key: &str) -> Option<(usize, usize)> {
    let open = format!("<{key}");
    let mut from = 0;
    while let Some(i) = xmp[from..].find(&open) {
        let start = from + i;
        let after = start + open.len();
        match xmp[after..].chars().next() {
            Some(c) if c == '>' || c == '/' || c.is_whitespace() => {
                let tag_end = after + xmp[after..].find('>')? + 1;
                if xmp[..tag_end].ends_with("/>") {
                    return Some((start, tag_end));
                }
                let close = format!("</{key}>");
                let e = tag_end + xmp[tag_end..].find(&close)? + close.len();
                return Some((start, e));
            }
            _ => from = after,
        }
    }
    None
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Takes the property `key` out of `xmp`, in its attribute and element forms.
pub fn remove_prop(xmp: &mut String, key: &str) {
    for q in ['"', '\''] {
        let attr = format!("{key}={q}");
        while let Some(i) = xmp.find(&attr) {
            let boundary = xmp[..i].chars().next_back().is_none_or(|c| c.is_whitespace());
            let Some(e) = xmp[i + attr.len()..].find(q) else { break };
            if !boundary {
                break;
            }
            let end = i + attr.len() + e + 1;
            // The whitespace before the attribute goes with it.
            let start = xmp[..i].trim_end_matches([' ', '\t', '\r', '\n']).len();
            xmp.replace_range(start..end, "");
        }
    }
    while let Some((s, e)) = element_span(xmp, key) {
        let start = xmp[..s].trim_end_matches([' ', '\t']).len();
        xmp.replace_range(start..e, "");
    }
}

/// A PDF date (`D:YYYYMMDDHHmmSSOHH'mm'`) as an XMP (ISO 8601) date.
pub fn iso_date(pdf: &str) -> Option<String> {
    let d = pdf.trim().trim_start_matches("D:");
    let digits: String = d.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() < 4 {
        return None;
    }
    let part = |a: usize, b: usize, def: &str| digits.get(a..b).unwrap_or(def).to_string();
    let (y, mo, da, h, mi, s) = (part(0, 4, "0000"), part(4, 6, "01"), part(6, 8, "01"), part(8, 10, "00"), part(10, 12, "00"), part(12, 14, "00"));
    let rest = &d[digits.len()..];
    let tz = match rest.chars().next() {
        Some('Z') | None => "Z".to_string(),
        Some(sign @ ('+' | '-')) => {
            let t: String = rest[1..].chars().filter(|c| c.is_ascii_digit()).collect();
            let (th, tm) = (t.get(0..2).unwrap_or("00"), t.get(2..4).unwrap_or("00"));
            if th == "00" && tm == "00" { "Z".to_string() } else { format!("{sign}{th}:{tm}") }
        }
        _ => "Z".to_string(),
    };
    Some(format!("{y}-{mo}-{da}T{h}:{mi}:{s}{tz}"))
}

/// An XMP date with a `+00:00` zone spelled `Z`, for comparing it with [`iso_date`].
pub fn norm_date(x: &str) -> String {
    let x = x.trim();
    match x.strip_suffix("+00:00").or_else(|| x.strip_suffix("-00:00")) {
        Some(h) => format!("{h}Z"),
        None => x.to_string(),
    }
}

/// The document information entries XMP mirrors, as text.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct InfoProps {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub producer: Option<String>,
    pub creator: Option<String>,
    /// PDF date strings.
    pub created: Option<String>,
    pub modified: Option<String>,
}

/// `(Info key, XMP key, value)` for each entry that is set.
pub fn mirrored(p: &InfoProps) -> Vec<(&'static str, &'static str, String)> {
    let mut v = Vec::new();
    let mut add = |info: &'static str, xmp: &'static str, val: &Option<String>| {
        if let Some(s) = val.as_ref().filter(|s| !s.is_empty()) {
            v.push((info, xmp, s.clone()));
        }
    };
    add("Title", "dc:title", &p.title);
    add("Author", "dc:creator", &p.author);
    add("Subject", "dc:description", &p.subject);
    add("Keywords", "pdf:Keywords", &p.keywords);
    add("Producer", "pdf:Producer", &p.producer);
    add("Creator", "xmp:CreatorTool", &p.creator);
    add("CreationDate", "xmp:CreateDate", &p.created);
    add("ModDate", "xmp:ModifyDate", &p.modified);
    v
}

fn prop_xml(xmp_key: &str, val: &str) -> String {
    match xmp_key {
        "dc:title" | "dc:description" => {
            format!("   <{xmp_key}><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></{xmp_key}>\n", esc(val))
        }
        "dc:creator" => format!("   <dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>\n", esc(val)),
        "xmp:CreateDate" | "xmp:ModifyDate" => {
            let d = iso_date(val).unwrap_or_else(|| val.to_string());
            format!("   <{xmp_key}>{}</{xmp_key}>\n", esc(&d))
        }
        _ => format!("   <{xmp_key}>{}</{xmp_key}>\n", esc(val)),
    }
}

/// One identification schema.
pub struct Ident {
    pub prefix: &'static str,
    pub ns: &'static str,
    pub props: Vec<(&'static str, String)>,
}

impl Ident {
    pub fn pdfa(part: u8, conformance: char) -> Ident {
        Ident { prefix: "pdfaid", ns: "http://www.aiim.org/pdfa/ns/id/", props: vec![("part", part.to_string()), ("conformance", conformance.to_string())] }
    }

    pub fn pdfua(part: u8) -> Ident {
        Ident { prefix: "pdfuaid", ns: "http://www.aiim.org/pdfua/ns/id/", props: vec![("part", part.to_string())] }
    }

    pub fn pdfx(version: &str) -> Ident {
        Ident { prefix: "pdfxid", ns: "http://www.npes.org/pdfx/ns/id/", props: vec![("GTS_PDFXVersion", version.to_string())] }
    }
}

const HEAD: &str = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n";
const TAIL: &str = " </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>";

fn description(info: &InfoProps, ids: &[Ident]) -> String {
    let mut ns = String::from("    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n    xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\"\n    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"");
    let mut body = String::new();
    for id in ids {
        ns.push_str(&format!("\n    xmlns:{}=\"{}\"", id.prefix, id.ns));
        for (k, v) in &id.props {
            body.push_str(&format!("   <{}:{k}>{}</{}:{k}>\n", id.prefix, esc(v), id.prefix));
        }
    }
    for (_, key, val) in mirrored(info) {
        body.push_str(&prop_xml(key, &val));
    }
    format!("  <rdf:Description rdf:about=\"\"\n{ns}>\n{body}  </rdf:Description>\n")
}

/// A new packet holding the identification and the mirrored document information.
pub fn fresh(info: &InfoProps, ids: &[Ident]) -> String {
    format!("{HEAD}{}{TAIL}", description(info, ids))
}

/// `existing` with the identification and the mirrored properties set (what else it holds is kept);
/// a packet that cannot be extended is replaced by a fresh one.
pub fn merge(existing: &str, info: &InfoProps, ids: &[Ident]) -> String {
    let Some(at) = existing.rfind("</rdf:RDF>") else { return fresh(info, ids) };
    if !existing.contains("<rdf:RDF") {
        return fresh(info, ids);
    }
    let mut x = existing.to_string();
    for id in ids {
        for (k, _) in &id.props {
            remove_prop(&mut x, &format!("{}:{k}", id.prefix));
        }
    }
    for (_, key, _) in mirrored(info) {
        remove_prop(&mut x, key);
    }
    let at = x.rfind("</rdf:RDF>").unwrap_or(at);
    x.insert_str(at, &description(info, ids));
    x
}

/// `(part, conformance)` of `pdfaid`, when the packet identifies a PDF/A part.
pub fn pdfa_id(xmp: &str) -> Option<(u8, String)> {
    let part = value(xmp, "pdfaid:part")?.trim().parse().ok()?;
    Some((part, value(xmp, "pdfaid:conformance").unwrap_or_default().trim().to_uppercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_attribute_and_element_forms() {
        let a = "<rdf:Description pdfaid:part=\"2\" pdfaid:conformance='B'/>";
        assert_eq!(value(a, "pdfaid:part").as_deref(), Some("2"));
        assert_eq!(value(a, "pdfaid:conformance").as_deref(), Some("B"));
        let e = "<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">A &amp; B</rdf:li></rdf:Alt></dc:title><pdfaid:part>3</pdfaid:part>";
        assert_eq!(value(e, "dc:title").as_deref(), Some("A & B"));
        assert_eq!(value(e, "pdfaid:part").as_deref(), Some("3"));
        assert_eq!(value(e, "pdfuaid:part"), None);
        assert_eq!(pdfa_id(e), Some((3, String::new())));
    }

    #[test]
    fn pdf_dates_become_iso_dates() {
        assert_eq!(iso_date("D:20240105093000+01'00'").as_deref(), Some("2024-01-05T09:30:00+01:00"));
        assert_eq!(iso_date("D:20240105").as_deref(), Some("2024-01-05T00:00:00Z"));
        assert_eq!(iso_date("D:20240105120000Z").as_deref(), Some("2024-01-05T12:00:00Z"));
        assert_eq!(iso_date("D:20240105120000+00'00'").as_deref(), Some("2024-01-05T12:00:00Z"));
        assert_eq!(iso_date("junk"), None);
        assert_eq!(norm_date("2024-01-05T12:00:00+00:00"), "2024-01-05T12:00:00Z");
    }

    #[test]
    fn merge_keeps_other_properties_and_replaces_the_old_identification() {
        let old = "<?xpacket begin=\"x\"?><x:xmpmeta><rdf:RDF><rdf:Description rdf:about=\"\" pdfaid:part=\"1\" pdfaid:conformance=\"B\" xmpMM:DocumentID=\"uuid:1\">\
<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Old</rdf:li></rdf:Alt></dc:title></rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>";
        let info = InfoProps { title: Some("New".into()), ..InfoProps::default() };
        let m = merge(old, &info, &[Ident::pdfa(2, 'B')]);
        assert_eq!(pdfa_id(&m), Some((2, "B".into())));
        assert_eq!(value(&m, "dc:title").as_deref(), Some("New"));
        assert_eq!(value(&m, "xmpMM:DocumentID").as_deref(), Some("uuid:1"));
        assert_eq!(m.matches("pdfaid:part").count(), 2, "one element, opened and closed");
        assert!(!m.contains(">Old<"));
    }

    #[test]
    fn a_packet_without_rdf_is_replaced() {
        let m = merge("garbage", &InfoProps::default(), &[Ident::pdfua(1)]);
        assert_eq!(value(&m, "pdfuaid:part").as_deref(), Some("1"));
    }
}
