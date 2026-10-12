//! Links through the edit lane: draw, retarget, border, delete, links from addresses, the
//! address gate, undo and a round trip through save.

use super::*;
use crate::engine::fixture::{self, Fixture};
use crate::outline::tests::{dir, open_bytes, pdf_with, raw_pdf};

fn rev(eng: &Engine, id: DocId) -> u32 {
    eng.edit_state(id).unwrap().rev
}

fn go(eng: &Engine, id: DocId, op: LinkOp) -> LinkEdited {
    apply(eng, id, rev(eng, id), &op, EditSource::User).unwrap()
}

fn fixture_doc(test: &str, pages: usize) -> (std::sync::Arc<Engine>, DocId, String, std::path::PathBuf) {
    let d = dir("links", test);
    let (eng, id, path) = open_bytes(&d, "a.pdf", &fixture::build(&Fixture { pages, ..Default::default() }));
    (eng, id, path, d)
}

#[test]
fn the_gate_takes_web_and_mail_addresses_only() {
    assert_eq!(gate_url("  https://example.com/a?b=c#d ").unwrap(), "https://example.com/a?b=c#d");
    assert_eq!(gate_url("HTTP://Example.com").unwrap(), "http://Example.com");
    assert_eq!(gate_url("mailto:me@example.com?subject=Hi").unwrap(), "mailto:me@example.com?subject=Hi");
    assert_eq!(gate_url("https://example.com/ü").unwrap(), "https://example.com/%C3%BC");
    for bad in [
        "",
        "example.com",
        "javascript:alert(1)",
        "file:///C:/Windows/win.ini",
        "ftp://example.com",
        "https:///nohost",
        "https://",
        "https://exa mple.com",
        "https://example.com/\u{202E}gpj",
        "mailto:me@example.com?attach=C:\\secret.txt",
        "data:text/html,hi",
    ] {
        assert!(gate_url(bad).is_err(), "{bad:?} passed");
    }
    assert!(gate_url(&format!("https://example.com/{}", "a".repeat(MAX_URL))).is_err());
}

#[test]
fn draws_a_web_link_and_reads_it_back_through_save() {
    let (eng, id, path, _d) = fixture_doc("web", 2);
    let e = go(
        &eng,
        id,
        LinkOp::Add {
            page: 1,
            rect: [100.0, 200.0, 300.0, 230.0],
            target: NewTarget::Uri { uri: "https://example.com/docs".into() },
            border: None,
        },
    );
    assert_eq!(e.state.undo.as_deref(), Some("Add link on p. 2"));
    assert!(e.state.dirty);
    let l = list(&eng, id, 1).unwrap();
    assert_eq!(l.links.len(), 1);
    let k = &l.links[0];
    assert_eq!(k.id, e.id.unwrap());
    assert_eq!(k.target, Target::Uri { uri: "https://example.com/docs".into() });
    assert!(!k.border.visible, "a new link has no visible border unless asked");
    for (a, b) in k.rect.iter().zip([100.0, 200.0, 300.0, 230.0]) {
        assert!((a - b).abs() < 0.1, "{:?}", k.rect);
    }
    assert!(list(&eng, id, 0).unwrap().links.is_empty());

    eng.save(id, None).unwrap();
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    let l = list(&eng2, id2, 1).unwrap();
    assert_eq!(l.links.len(), 1);
    assert_eq!(l.links[0].target, Target::Uri { uri: "https://example.com/docs".into() });
}

#[test]
fn a_page_link_points_at_the_page_and_the_view() {
    let (eng, id, _, _d) = fixture_doc("page", 3);
    go(
        &eng,
        id,
        LinkOp::Add {
            page: 0,
            rect: [50.0, 50.0, 150.0, 80.0],
            target: NewTarget::Page { page: 2, at: Some([40.0, 300.0]) },
            border: Some(BorderSpec { visible: true, width: Some(2.0), color: Some([0.1, 0.2, 0.9]) }),
        },
    );
    let k = &list(&eng, id, 0).unwrap().links[0];
    match &k.target {
        Target::Page { page: 2, at: Some([x, y]) } => assert!((x - 40.0).abs() < 0.1 && (y - 300.0).abs() < 0.1),
        other => panic!("{other:?}"),
    }
    assert!(k.border.visible);
    assert!((k.border.width - 2.0).abs() < 0.01);
    assert_eq!((k.border.color[2] * 10.0).round() as i32, 9);
}

#[test]
fn retargets_moves_restyles_and_deletes_each_as_one_undo_step() {
    let (eng, id, _, _d) = fixture_doc("edit", 3);
    let id0 = go(
        &eng,
        id,
        LinkOp::Add {
            page: 0,
            rect: [50.0, 50.0, 150.0, 80.0],
            target: NewTarget::Uri { uri: "https://example.com".into() },
            border: None,
        },
    )
    .id
    .unwrap();
    go(&eng, id, LinkOp::SetTarget { page: 0, id: id0, target: NewTarget::Page { page: 1, at: None } });
    assert!(matches!(list(&eng, id, 0).unwrap().links[0].target, Target::Page { page: 1, .. }));
    go(&eng, id, LinkOp::SetRect { page: 0, id: id0, rect: [60.0, 60.0, 260.0, 100.0] });
    let r = list(&eng, id, 0).unwrap().links[0].rect;
    assert!((r[2] - 260.0).abs() < 0.1 && (r[3] - 100.0).abs() < 0.1);
    go(&eng, id, LinkOp::SetBorder { page: 0, id: id0, border: BorderSpec { visible: true, width: None, color: None } });
    assert!(list(&eng, id, 0).unwrap().links[0].border.visible);
    go(&eng, id, LinkOp::SetBorder { page: 0, id: id0, border: BorderSpec { visible: false, width: None, color: None } });
    assert!(!list(&eng, id, 0).unwrap().links[0].border.visible);
    let del = go(&eng, id, LinkOp::Delete { page: 0, ids: vec![id0] });
    assert_eq!(del.count, 1);
    assert!(list(&eng, id, 0).unwrap().links.is_empty());
    // Back through every step: delete, hide border, show border, move, retarget, add.
    eng.undo(id).unwrap();
    assert_eq!(list(&eng, id, 0).unwrap().links.len(), 1);
    eng.undo(id).unwrap();
    assert!(list(&eng, id, 0).unwrap().links[0].border.visible);
    eng.undo(id).unwrap();
    eng.undo(id).unwrap();
    eng.undo(id).unwrap();
    assert!(matches!(list(&eng, id, 0).unwrap().links[0].target, Target::Uri { .. }));
    eng.undo(id).unwrap();
    assert!(list(&eng, id, 0).unwrap().links.is_empty());
}

#[test]
fn refuses_unsafe_targets_small_rectangles_and_stale_changes() {
    let (eng, id, _, _d) = fixture_doc("refuse", 1);
    let add = |uri: &str, rect: [f32; 4], rev: u32| {
        apply(&eng, id, rev, &LinkOp::Add { page: 0, rect, target: NewTarget::Uri { uri: uri.into() }, border: None }, EditSource::User)
    };
    let r0 = rev(&eng, id);
    assert!(matches!(add("javascript:alert(1)", [10.0, 10.0, 100.0, 40.0], r0), Err(EngineError::Invalid(_))));
    assert!(matches!(add("https://example.com", [10.0, 10.0, 11.0, 40.0], r0), Err(EngineError::Invalid(_))));
    assert!(add("https://example.com", [10.0, 10.0, 100.0, 40.0], r0).is_ok());
    assert!(matches!(add("https://example.com", [10.0, 10.0, 100.0, 40.0], r0), Err(EngineError::Stale)));
    assert_eq!(list(&eng, id, 0).unwrap().links.len(), 1);
    assert!(matches!(list(&eng, id, 7), Err(EngineError::BadPage(7))));
}

#[test]
fn a_link_to_a_file_keeps_a_name_from_the_document_and_never_a_folder_of_this_machine() {
    let (eng, id, _, d) = fixture_doc("file", 1);
    let beside = d.join("appendix.pdf").to_string_lossy().into_owned();
    go(
        &eng,
        id,
        LinkOp::Add {
            page: 0,
            rect: [10.0, 10.0, 100.0, 40.0],
            target: NewTarget::File { file: beside, page: Some(2) },
            border: None,
        },
    );
    let elsewhere = std::env::temp_dir().join("inkwell-tests-other").join("notes.txt").to_string_lossy().into_owned();
    go(
        &eng,
        id,
        LinkOp::Add {
            page: 0,
            rect: [10.0, 50.0, 100.0, 80.0],
            target: NewTarget::File { file: elsewhere, page: None },
            border: None,
        },
    );
    let l = list(&eng, id, 0).unwrap().links;
    assert_eq!(l[0].target, Target::File { name: "appendix.pdf".into(), page: Some(2) });
    // Beside the document's folder, on the same drive: found by going up from it.
    assert_eq!(l[1].target, Target::File { name: "../../../inkwell-tests-other/notes.txt".into(), page: None });
}

#[test]
fn links_from_addresses_are_predicted_then_made_once() {
    let d = dir("links", "urls");
    let p1 = "BT /F1 12 Tf 72 700 Td (See https://example.com/guide, then write to help@example.org.) Tj ET\n\
              BT /F1 12 Tf 72 680 Td (And www.rust-lang.org/learn) Tj ET";
    let p2 = "BT /F1 12 Tf 72 700 Td (Nothing to see here, a plain sentence.) Tj ET";
    let (eng, id, _) = open_bytes(&d, "u.pdf", &pdf_with(&[p1, p2]));
    let p = preview_urls(&eng, id).unwrap();
    assert_eq!((p.count, p.pages), (3, 1), "{p:?}");
    assert_eq!(p.rows[0].url, "https://example.com/guide");
    let made = go(&eng, id, LinkOp::FromUrls);
    assert_eq!(made.count, 3);
    assert_eq!(made.state.undo.as_deref(), Some("Create links from web addresses"));
    let l = list(&eng, id, 0).unwrap().links;
    let uris: Vec<String> = l.iter().filter_map(|k| if let Target::Uri { uri } = &k.target { Some(uri.clone()) } else { None }).collect();
    assert_eq!(uris, ["https://example.com/guide", "mailto:help@example.org", "https://www.rust-lang.org/learn"]);
    // Each link sits over its own text: the first starts after "See " at the left margin.
    assert!(l[0].rect[0] > 72.0 && l[0].rect[2] < 400.0, "{:?}", l[0].rect);
    // Nothing is left to make: predicted 0, and asking for it is refused.
    assert_eq!(preview_urls(&eng, id).unwrap().count, 0);
    let r = apply(&eng, id, rev(&eng, id), &LinkOp::FromUrls, EditSource::User);
    assert!(matches!(r, Err(EngineError::Invalid(_))));
    // One undo takes all three away.
    eng.undo(id).unwrap();
    assert!(list(&eng, id, 0).unwrap().links.is_empty());
}

#[test]
fn a_destination_that_is_not_there_is_broken_and_the_audit_counts_it() {
    let (eng, id, _path, _d) = fixture_doc("broken", 3);
    go(&eng, id, LinkOp::Add { page: 0, rect: [50.0, 50.0, 150.0, 80.0], target: NewTarget::Page { page: 2, at: None }, border: None });
    go(&eng, id, LinkOp::Add { page: 1, rect: [50.0, 50.0, 150.0, 80.0], target: NewTarget::Uri { uri: "https://example.com/".into() }, border: None });
    let ok = audit(&eng, id).unwrap();
    assert_eq!((ok.total, ok.external, ok.broken), (2, 1, 0));

    // Point the first link at a named destination the document does not have.
    eng.apply_custom(id, "Break link".into(), EditSource::User, false, |d| {
        let page = d.find_page(0)?;
        let annots = page.get_dict("Annots")?.unwrap();
        let mut a = annots.get_array(0)?.unwrap();
        a.dict_put("Dest", d.new_object_from_str("(nowhere)")?)?;
        Ok(vec![0])
    })
    .unwrap();
    let l = list(&eng, id, 0).unwrap();
    match &l.links[0].target {
        Target::Broken { what } => assert!(what.contains("nowhere"), "{what}"),
        other => panic!("{other:?}"),
    }
    let a = audit(&eng, id).unwrap();
    assert_eq!((a.total, a.external, a.broken), (2, 1, 1));
    assert_eq!(a.rows.len(), 1);
    assert_eq!((a.rows[0].page, a.rows[0].id), (0, l.links[0].id));
}

#[test]
fn a_hidden_or_colourless_border_is_not_visible() {
    let (eng, id, _path, _d) = fixture_doc("hidden", 1);
    let e = go(
        &eng,
        id,
        LinkOp::Add {
            page: 0,
            rect: [50.0, 50.0, 150.0, 80.0],
            target: NewTarget::Uri { uri: "https://example.com/".into() },
            border: Some(BorderSpec { visible: true, width: Some(2.0), color: None }),
        },
    );
    assert!(list(&eng, id, 0).unwrap().links[0].border.visible);
    eng.apply_custom(id, "Hide link".into(), EditSource::User, false, |d| {
        let page = d.find_page(0)?;
        let annots = page.get_dict("Annots")?.unwrap();
        let mut a = annots.get_array(0)?.unwrap();
        a.dict_put("C", d.new_array()?)?;
        Ok(vec![0])
    })
    .unwrap();
    assert!(!list(&eng, id, 0).unwrap().links[0].border.visible, "/C [] draws nothing");
    assert_eq!(list(&eng, id, 0).unwrap().links[0].id, e.id.unwrap());
}

/// The `/AP /N` stream of the first annotation of page 1: its BBox size and the drawing, if there is one.
fn first_ap(eng: &Engine, id: DocId) -> Option<([f32; 2], String)> {
    eng.read_custom(id, |d| {
        let page = d.find_page(0)?;
        let annots = page.get_dict("Annots")?.unwrap();
        let a = annots.get_array(0)?.unwrap();
        let Some(ap) = a.get_dict("AP")? else { return Ok(None) };
        let n = ap.get_dict("N")?.unwrap();
        assert!(n.is_stream()?, "the appearance is a stream");
        let bbox = n.get_dict("BBox")?.and_then(|r| nums(&r, 4)).unwrap();
        let body = String::from_utf8_lossy(&n.read_stream()?).into_owned();
        Ok(Some(([bbox[2] - bbox[0], bbox[3] - bbox[1]], body)))
    })
    .unwrap()
}

#[test]
fn a_visible_border_has_an_appearance_stream_that_follows_the_link() {
    let (eng, id, _path, _d) = fixture_doc("ap", 1);
    let e = go(
        &eng,
        id,
        LinkOp::Add {
            page: 0,
            rect: [50.0, 50.0, 150.0, 80.0],
            target: NewTarget::Uri { uri: "https://example.com/".into() },
            border: Some(BorderSpec { visible: true, width: Some(2.0), color: Some([1.0, 0.0, 0.0]) }),
        },
    );
    let (size, body) = first_ap(&eng, id).expect("a visible border is drawn by an appearance stream");
    assert_eq!(size, [100.0, 30.0]);
    assert!(body.contains("1.000 0.000 0.000 RG") && body.contains("2.000 w"), "{body}");
    let link = e.id.unwrap();
    go(&eng, id, LinkOp::SetRect { page: 0, id: link, rect: [50.0, 50.0, 250.0, 100.0] });
    assert_eq!(first_ap(&eng, id).unwrap().0, [200.0, 50.0], "the stream is as big as the moved link");
    go(&eng, id, LinkOp::SetBorder { page: 0, id: link, border: BorderSpec { visible: false, width: None, color: None } });
    assert!(first_ap(&eng, id).is_none(), "no border, no stream");
    eng.undo(id).unwrap();
    assert!(first_ap(&eng, id).is_some(), "undo brings the border and its stream back");
}

#[test]
fn a_link_without_a_border_gets_no_appearance_stream() {
    let (eng, id, _path, _d) = fixture_doc("no-ap", 1);
    go(
        &eng,
        id,
        LinkOp::Add {
            page: 0,
            rect: [50.0, 50.0, 150.0, 80.0],
            target: NewTarget::Uri { uri: "https://example.com/".into() },
            border: None,
        },
    );
    assert!(first_ap(&eng, id).is_none());
}

#[test]
fn a_file_link_opens_only_a_pdf_found_from_the_document() {
    let d = dir("links", "filelink");
    let doc = d.join("a.pdf");
    let doc = doc.to_string_lossy().into_owned();
    std::fs::create_dir_all(d.join("sub")).unwrap();
    std::fs::write(d.join("b.pdf"), b"%PDF-1.4").unwrap();
    std::fs::write(d.join("sub").join("c.PDF"), b"%PDF-1.4").unwrap();
    std::fs::write(d.join("notes.txt"), b"x").unwrap();
    assert_eq!(resolve_file_link(&doc, "b.pdf").unwrap(), d.join("b.pdf"));
    assert_eq!(resolve_file_link(&doc, "sub/c.PDF").unwrap(), d.join("sub").join("c.PDF"));
    assert_eq!(resolve_file_link(&doc, "sub\\c.PDF").unwrap(), d.join("sub").join("c.PDF"), "either slash");
    // Up from the folder of the document is followed (the confirmation shows where it ends up).
    let in_sub = d.join("sub").join("x.pdf").to_string_lossy().into_owned();
    assert_eq!(resolve_file_link(&in_sub, "../b.pdf").unwrap(), d.join("b.pdf"));
    for refused in [
        "C:\\x\\b.pdf",
        "\\\\server\\share\\b.pdf",
        "/etc/b.pdf",
        "notes.txt",
        "missing.pdf",
        "../missing.pdf",
        "",
        "file:///b.pdf",
        "b.pdf\0.txt",
    ] {
        assert!(resolve_file_link(&doc, refused).is_err(), "{refused:?} must be refused");
    }
}

// ---- review fixes ----

/// A one-page file (612 x 792, `page_extra` is added to the page dictionary) whose annotations are objects 4..;
/// `catalog_extra` goes into the catalog and `more` after the annotations.
fn page_with_annots(page_extra: &str, catalog_extra: &str, annots: &[String], more: &[String]) -> Vec<u8> {
    let refs = (0..annots.len()).map(|i| format!("{} 0 R", 4 + i)).collect::<Vec<_>>().join(" ");
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] {page_extra} /Annots [{refs}] >>"),
    ];
    objs.extend(annots.iter().cloned());
    objs.extend(more.iter().cloned());
    raw_pdf(&objs)
}

fn link_annot(extra: &str) -> String {
    format!("<< /Type /Annot /Subtype /Link /Rect [100 600 300 650] /Border [0 0 0] {extra} >>")
}

fn opened(test: &str, bytes: &[u8]) -> (std::sync::Arc<Engine>, DocId, String, std::path::PathBuf) {
    let d = dir("links", test);
    let (eng, id, path) = open_bytes(&d, "a.pdf", bytes);
    (eng, id, path, d)
}

fn add_uri(page: u32, rect: [f32; 4], uri: &str, border: Option<BorderSpec>) -> LinkOp {
    LinkOp::Add { page, rect, target: NewTarget::Uri { uri: uri.into() }, border }
}

#[test]
fn the_gate_encodes_what_rfc_3986_disallows_and_decodes_parameter_names() {
    assert_eq!(gate_url("https://example.com/a\"b<c>d\\e^f`g{h|i}j").unwrap(), "https://example.com/a%22b%3Cc%3Ed%5Ce%5Ef%60g%7Bh%7Ci%7Dj");
    assert_eq!(gate_url("https://example.com/100%/a%20b%zz").unwrap(), "https://example.com/100%25/a%20b%25zz", "a % that starts no escape");
    assert_eq!(gate_url("https://example.com/ü?q=ö#é").unwrap(), "https://example.com/%C3%BC?q=%C3%B6#%C3%A9");
    for bad in [
        "mailto:me@example.com?%61ttach=C:\\x",
        "mailto:me@example.com?subject=Hi&%41TTACH=x",
        "mailto:me@example.com?subject=Hi;attachment=x",
        "mailto:me@example.com?%69nsert=x",
    ] {
        assert!(gate_url(bad).is_err(), "{bad:?} passed");
    }
    assert!(gate_url("mailto:me@example.com?subject=Attached%20is%20the%20file").is_ok(), "only parameter names count");
}

#[test]
fn a_file_link_is_named_from_the_folder_of_the_document() {
    let n = |doc: &str, file: &str| file_link_name(doc, file);
    assert_eq!(n("C:\\Docs\\a.pdf", "C:\\Docs\\b.pdf").unwrap(), "b.pdf");
    assert_eq!(n("C:\\Docs\\a.pdf", "c:\\docs\\Sub\\b.pdf").unwrap(), "Sub/b.pdf", "the drive and folders are compared without case");
    assert_eq!(n("\\\\?\\C:\\Docs\\a.pdf", "C:\\Docs\\b.pdf").unwrap(), "b.pdf", "a \\\\?\\ prefix is dropped");
    assert_eq!(n("C:\\Docs\\Sub\\a.pdf", "C:\\Docs\\b.pdf").unwrap(), "../b.pdf");
    assert_eq!(n("C:\\Docs\\a.pdf", "C:\\Other\\Deep\\b.pdf").unwrap(), "../Other/Deep/b.pdf");
    assert_eq!(n("/home/me/docs/a.pdf", "/home/me/pics/b.png").unwrap(), "../pics/b.png");
    for (doc, file) in [("C:\\Docs\\a.pdf", "D:\\Docs\\b.pdf"), ("C:\\Docs\\a.pdf", "\\\\server\\share\\b.pdf"), ("C:\\Docs\\a.pdf", "b.pdf")] {
        match n(doc, file) {
            Err(EngineError::Invalid(m)) => assert!(!m.is_empty()),
            other => panic!("{file}: {other:?}"),
        }
    }
}

#[test]
fn the_name_a_file_link_got_is_returned_and_one_above_the_folder_uses_dot_dot() {
    let (eng, id, _, d) = fixture_doc("file-name", 1);
    let up = d.parent().unwrap().join("up.pdf").to_string_lossy().into_owned();
    let e = go(
        &eng,
        id,
        LinkOp::Add { page: 0, rect: [10.0, 10.0, 100.0, 40.0], target: NewTarget::File { file: up, page: None }, border: None },
    );
    assert_eq!(e.name.as_deref(), Some("../up.pdf"));
    assert_eq!(list(&eng, id, 0).unwrap().links[0].target, Target::File { name: "../up.pdf".into(), page: Some(0) });
    let uri = go(&eng, id, add_uri(0, [10.0, 50.0, 100.0, 80.0], "https://example.com/", None));
    assert_eq!(uri.name, None);
}

#[test]
fn a_link_may_not_launch_a_program_or_a_script() {
    let (eng, id, _, d) = fixture_doc("launch", 1);
    let target = |name: &str| NewTarget::File { file: d.join(name).to_string_lossy().into_owned(), page: None };
    for bad in [
        "setup.exe",
        "run.BAT",
        "x.ps1",
        "a.vbs",
        "trick.exe.",
        "noextension",
        "page.hta",
        "tool.lnk",
        "a.appref-ms",
        "a.settingcontent-ms",
        "a.application",
        "a.website",
        "a.library-ms",
        "a.search-ms",
        "a.chm",
        "a.mht",
        "a.xll",
        "a.wsc",
        "a.sct",
        "a.iso",
        "a.vhd",
        "macros.docm",
        "macros.xlsm",
        "old.doc",
        "report.pdf.exe",
    ] {
        let r = apply(
            &eng,
            id,
            rev(&eng, id),
            &LinkOp::Add { page: 0, rect: [10.0, 10.0, 100.0, 40.0], target: target(bad), border: None },
            EditSource::User,
        );
        assert!(matches!(r, Err(EngineError::Invalid(_))), "{bad}: {r:?}");
    }
    assert!(list(&eng, id, 0).unwrap().links.is_empty());
    for ok in ["report.docx", "notes.txt", "scan.png", "other.pdf"] {
        go(&eng, id, LinkOp::Add { page: 0, rect: [10.0, 10.0, 100.0, 40.0], target: target(ok), border: None });
    }
    assert_eq!(list(&eng, id, 0).unwrap().links.len(), 4);
}

#[test]
fn a_file_specification_has_a_byte_string_f_and_a_utf16_uf() {
    let (eng, id, _, d) = fixture_doc("filespec", 1);
    let file = d.join("b\u{fc}cher.docx").to_string_lossy().into_owned();
    go(&eng, id, LinkOp::Add { page: 0, rect: [10.0, 10.0, 100.0, 40.0], target: NewTarget::File { file, page: None }, border: None });
    let (f, uf) = eng
        .read_custom(id, |doc| {
            let a = doc.find_page(0)?.get_dict("Annots")?.unwrap().get_array(0)?.unwrap();
            let spec = a.get_dict("A")?.unwrap().get_dict("F")?.unwrap();
            Ok((spec.get_dict("F")?.unwrap().as_bytes()?, spec.get_dict("UF")?.unwrap().as_bytes()?))
        })
        .unwrap();
    assert_eq!(f, b"b\xfccher.docx", "F is PDFDocEncoding bytes, not UTF-16");
    assert_eq!(&uf[..2], &[0xFE, 0xFF], "UF is a text string in UTF-16BE");
    assert_eq!(list(&eng, id, 0).unwrap().links[0].target, Target::File { name: "b\u{fc}cher.docx".into(), page: None });
}

#[test]
fn an_address_too_small_to_link_is_not_counted() {
    let d = dir("links", "tiny-urls");
    let p = "BT /F1 1 Tf 72 700 Td (https://tiny.example.com) Tj ET\n\
             BT /F1 12 Tf 72 680 Td (https://big.example.org/x) Tj ET";
    let (eng, id, _) = open_bytes(&d, "t.pdf", &pdf_with(&[p]));
    let preview = preview_urls(&eng, id).unwrap();
    assert_eq!(preview.count, 1, "{preview:?}");
    assert_eq!(preview.rows[0].url, "https://big.example.org/x");
    let made = go(&eng, id, LinkOp::FromUrls);
    assert_eq!(made.count, preview.count, "what was predicted is what is made");
}

#[test]
fn links_on_a_rotated_and_cropped_page_keep_their_place() {
    let bytes = page_with_annots("/Rotate 90 /CropBox [50 100 450 700]", "", &[], &[]);
    let (eng, id, _, _d) = opened("rotated", &bytes);
    // The page is 600 x 400 as shown: a rectangle near its right edge.
    let want = [400.0, 50.0, 560.0, 90.0];
    let e = go(
        &eng,
        id,
        add_uri(0, want, "https://example.com/", Some(BorderSpec { visible: true, width: Some(2.0), color: Some([0.0, 0.0, 1.0]) })),
    );
    let k = list(&eng, id, 0).unwrap().links.into_iter().find(|l| Some(l.id) == e.id).unwrap();
    for (a, b) in k.rect.iter().zip(want) {
        assert!((a - b).abs() < 0.1, "{:?} vs {want:?}", k.rect);
    }
    // In user space the rectangle is turned a quarter: 40 wide and 160 high; the appearance is that size.
    let (size, _) = first_ap(&eng, id).unwrap();
    assert_eq!(size, [40.0, 160.0]);
    // A page link keeps its point through the same turn.
    let to = go(
        &eng,
        id,
        LinkOp::Add {
            page: 0,
            rect: [10.0, 10.0, 90.0, 40.0],
            target: NewTarget::Page { page: 0, at: Some([120.0, 80.0]) },
            border: None,
        },
    );
    let l = list(&eng, id, 0).unwrap().links.into_iter().find(|l| Some(l.id) == to.id).unwrap();
    match l.target {
        Target::Page { page: 0, at: Some(at) } => assert!((at[0] - 120.0).abs() < 0.1 && (at[1] - 80.0).abs() < 0.1, "{at:?}"),
        other => panic!("{other:?}"),
    }
    // Moving it works in the same space.
    let moved = [100.0, 100.0, 220.0, 140.0];
    go(&eng, id, LinkOp::SetRect { page: 0, id: to.id.unwrap(), rect: moved });
    let l = list(&eng, id, 0).unwrap().links.into_iter().find(|l| Some(l.id) == to.id).unwrap();
    for (a, b) in l.rect.iter().zip(moved) {
        assert!((a - b).abs() < 0.1, "{:?}", l.rect);
    }
}

#[test]
fn existing_launch_goto_remote_javascript_and_named_actions_survive_rect_and_border_edits() {
    let annots = vec![
        link_annot("/A << /S /Launch /F (readme.docx) >>"),
        link_annot("/A << /S /GoToR /F << /Type /Filespec /F (other.pdf) /UF (other.pdf) >> /D [2 /Fit] /NewWindow true >>"),
        link_annot("/A << /S /JavaScript /JS (app.alert\\(1\\)) >>"),
        link_annot("/A << /S /Named /N /NextPage >>"),
    ];
    let (eng, id, _, _d) = opened("actions", &page_with_annots("", "", &annots, &[]));
    let before = list(&eng, id, 0).unwrap().links;
    assert_eq!(before.len(), 4);
    assert_eq!(before[0].target, Target::File { name: "readme.docx".into(), page: None });
    assert_eq!(before[1].target, Target::File { name: "other.pdf".into(), page: Some(2) });
    assert!(matches!(&before[2].target, Target::Other { what } if what == "JavaScript"));
    assert!(matches!(&before[3].target, Target::Other { what } if what == "Named"));
    for l in &before {
        go(&eng, id, LinkOp::SetRect { page: 0, id: l.id, rect: [20.0, 20.0, 220.0, 70.0] });
        go(
            &eng,
            id,
            LinkOp::SetBorder { page: 0, id: l.id, border: BorderSpec { visible: true, width: Some(3.0), color: Some([1.0, 0.0, 0.0]) } },
        );
        go(&eng, id, LinkOp::SetBorder { page: 0, id: l.id, border: BorderSpec { visible: false, width: None, color: None } });
    }
    let after = list(&eng, id, 0).unwrap().links;
    for (a, b) in before.iter().zip(&after) {
        assert_eq!(a.id, b.id);
        assert_eq!(a.target, b.target, "the action is kept");
        assert_ne!(a.rect, b.rect, "and the rectangle did change");
    }
    let (js, goto_r_new_window, named) = eng
        .read_custom(id, |doc| {
            let annots = doc.find_page(0)?.get_dict("Annots")?.unwrap();
            let js = annots.get_array(2)?.unwrap().get_dict("A")?.unwrap().get_dict("JS")?.unwrap().as_string()?;
            let nw = annots.get_array(1)?.unwrap().get_dict("A")?.unwrap().get_dict("NewWindow")?.is_some();
            let n = annots.get_array(3)?.unwrap().get_dict("A")?.unwrap().get_dict("N")?.unwrap().as_name()?;
            Ok((js, nw, String::from_utf8_lossy(&n).into_owned()))
        })
        .unwrap();
    assert_eq!(js, "app.alert(1)");
    assert!(goto_r_new_window);
    assert_eq!(named, "NextPage");
}

#[test]
fn a_link_change_can_be_undone_and_redone() {
    let (eng, id, _, _d) = fixture_doc("redo", 2);
    let e = go(&eng, id, add_uri(0, [50.0, 50.0, 150.0, 80.0], "https://example.com/", None));
    let link = e.id.unwrap();
    go(&eng, id, LinkOp::SetBorder { page: 0, id: link, border: BorderSpec { visible: true, width: Some(2.0), color: None } });
    go(&eng, id, LinkOp::SetTarget { page: 0, id: link, target: NewTarget::Page { page: 1, at: None } });
    let end = list(&eng, id, 0).unwrap().links;
    eng.undo(id).unwrap();
    eng.undo(id).unwrap();
    eng.undo(id).unwrap();
    assert!(list(&eng, id, 0).unwrap().links.is_empty());
    eng.redo(id).unwrap();
    assert_eq!(list(&eng, id, 0).unwrap().links.len(), 1);
    eng.redo(id).unwrap();
    eng.redo(id).unwrap();
    assert_eq!(list(&eng, id, 0).unwrap().links, end);
    assert!(first_ap(&eng, id).is_some(), "the appearance came back with the redo");
    // A new change after an undo drops what was undone, and the list stays right.
    eng.undo(id).unwrap();
    go(&eng, id, LinkOp::Delete { page: 0, ids: vec![link] });
    assert!(eng.redo(id).unwrap().redo.is_none());
    assert!(list(&eng, id, 0).unwrap().links.is_empty());
}

#[test]
fn a_change_that_changes_nothing_makes_no_undo_step() {
    let (eng, id, _, _d) = fixture_doc("noop", 2);
    let uri = "https://example.com/";
    let e = go(&eng, id, add_uri(0, [50.0, 50.0, 150.0, 80.0], uri, Some(BorderSpec { visible: true, width: Some(2.0), color: Some([1.0, 0.0, 0.0]) })));
    let link = e.id.unwrap();
    let before = eng.edit_state(id).unwrap();
    let same = [
        LinkOp::SetRect { page: 0, id: link, rect: [50.0, 50.0, 150.0, 80.0] },
        LinkOp::SetBorder { page: 0, id: link, border: BorderSpec { visible: true, width: Some(2.0), color: Some([1.0, 0.0, 0.0]) } },
        LinkOp::SetTarget { page: 0, id: link, target: NewTarget::Uri { uri: uri.into() } },
        LinkOp::Delete { page: 0, ids: vec![] },
    ];
    for op in same {
        let s = apply(&eng, id, rev(&eng, id), &op, EditSource::User).unwrap().state;
        assert_eq!((s.rev, s.undo.clone()), (before.rev, before.undo.clone()), "{op:?}");
    }
    let s = go(&eng, id, LinkOp::SetRect { page: 0, id: link, rect: [50.0, 50.0, 160.0, 80.0] }).state;
    assert_ne!(s.rev, before.rev);
}

#[test]
fn a_certified_document_keeps_its_links() {
    for (p, allowed) in [(1, false), (2, false), (3, true)] {
        let cat = format!("/Perms << /DocMDP << /Type /Sig /Reference [ << /TransformMethod /DocMDP /TransformParams << /P {p} >> >> ] >> >>");
        let (eng, id, _, _d) = opened(&format!("certified-{p}"), &page_with_annots("", &cat, &[link_annot("/Dest [3 0 R /Fit]")], &[]));
        let r = apply(&eng, id, rev(&eng, id), &add_uri(0, [50.0, 50.0, 150.0, 80.0], "https://example.com/", None), EditSource::User);
        if allowed {
            assert!(r.is_ok(), "P {p}: {r:?}");
        } else {
            match r {
                Err(EngineError::Invalid(m)) => assert!(m.contains("certified"), "P {p}: {m}"),
                other => panic!("P {p}: {other:?}"),
            }
        }
    }
}

#[test]
fn deleting_a_link_takes_it_out_of_the_structure_tree() {
    let annots = vec![link_annot("/Dest [3 0 R /Fit] /StructParent 0")];
    let more = vec![
        "<< /Type /StructTreeRoot /K 6 0 R /ParentTree 7 0 R >>".to_string(),
        "<< /Type /StructElem /S /Link /P 5 0 R /Pg 3 0 R /K [ << /Type /OBJR /Obj 4 0 R /Pg 3 0 R >> ] >>".to_string(),
        "<< /Nums [ 0 6 0 R ] >>".to_string(),
    ];
    let (eng, id, _, _d) = opened("structure", &page_with_annots("", "/StructTreeRoot 5 0 R", &annots, &more));
    let link = list(&eng, id, 0).unwrap().links[0].id;
    go(&eng, id, LinkOp::Delete { page: 0, ids: vec![link] });
    let (nums, kids) = eng
        .read_custom(id, |doc| {
            let tree = doc.new_indirect(7, 0)?;
            let nums = tree.get_dict("Nums")?.unwrap().len()?;
            let k = doc.new_indirect(6, 0)?.get_dict("K")?.map_or(0, |k| k.len().unwrap_or(1));
            Ok((nums, k))
        })
        .unwrap();
    assert_eq!(nums, 0, "no entry for the deleted link is left in the parent tree");
    assert_eq!(kids, 0, "and the element that held it no longer refers to it");
    eng.undo(id).unwrap();
    assert_eq!(list(&eng, id, 0).unwrap().links.len(), 1, "undo brings it back");
}

#[test]
fn a_deleted_link_is_not_left_in_the_saved_bytes() {
    let (eng, id, path, _d) = fixture_doc("removing", 1);
    let marker = "https://example.com/zz-unique-marker-4711";
    let e = go(&eng, id, add_uri(0, [50.0, 50.0, 150.0, 80.0], marker, None));
    eng.save(id, None).unwrap();
    let has = |p: &str| {
        let b = std::fs::read(p).unwrap();
        b.windows(marker.len()).any(|w| w == marker.as_bytes())
    };
    assert!(has(&path), "saved with the link in it");
    go(&eng, id, LinkOp::Delete { page: 0, ids: vec![e.id.unwrap()] });
    eng.save(id, None).unwrap();
    assert!(!has(&path), "the deleted link's address is not kept in the file");
}

#[test]
fn deleting_a_link_of_a_signed_document_keeps_the_signed_bytes() {
    // A signature field with a value (object 5); /P 3 certification allows deleting links.
    let cat = "/AcroForm << /SigFlags 3 /Fields [5 0 R] >> /Perms << /DocMDP << /Type /Sig /Reference [ << /TransformMethod /DocMDP /TransformParams << /P 3 >> >> ] >> >>";
    let sig = "<< /FT /Sig /T (Signature1) /V << /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00> >> >>".to_string();
    let (eng, id, path, _d) = opened("signed-delete", &page_with_annots("", cat, &[link_annot("/A << /S /URI /URI (https://secret.example/) >>")], &[sig]));
    assert!(eng.read_custom(id, |d| Ok(crate::outline::signed(d))).unwrap(), "the signature is seen");
    let original = std::fs::read(&path).unwrap();
    let link = list(&eng, id, 0).unwrap().links[0].id;
    go(&eng, id, LinkOp::Delete { page: 0, ids: vec![link] });
    eng.save(id, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len() && saved.starts_with(&original), "the file was rewritten: the signature would no longer verify");
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    assert!(list(&eng2, id2, 0).unwrap().links.is_empty(), "the link is gone when it is read again");
}

#[test]
fn deleting_a_link_of_an_unsigned_document_still_leaves_its_address_out_of_the_file() {
    let (eng, id, path, _d) = opened("unsigned-delete", &page_with_annots("", "", &[link_annot("/A << /S /URI /URI (https://secret.example/) >>")], &[]));
    assert!(!eng.read_custom(id, |d| Ok(crate::outline::signed(d))).unwrap());
    let link = list(&eng, id, 0).unwrap().links[0].id;
    go(&eng, id, LinkOp::Delete { page: 0, ids: vec![link] });
    eng.save(id, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(!String::from_utf8_lossy(&saved).contains("secret.example"), "a deleted link's address does not stay in the file");
}

// ---- highlight style, inline links, remove all ----

/// Objects: 1 catalog, 2 pages, 3 the page (`annots` is the literal `/Annots` value), then `more` from 4.
fn doc_with(annots: &str, catalog_extra: &str, more: &[String]) -> Vec<u8> {
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots {annots} >>"),
    ];
    objs.extend(more.iter().cloned());
    raw_pdf(&objs)
}

const INLINE: &str = "<< /Type /Annot /Subtype /Link /Rect [100 600 300 650] /Border [0 0 0] /A << /S /URI /URI (https://inline.example/) >> >>";

/// For each entry of page 0's `/Annots`: is it written inline (not a reference)?
fn direct_entries(eng: &Engine, id: DocId) -> Vec<bool> {
    eng.read_custom(id, |d| {
        let p = d.find_page(0)?;
        let arr = p.get_dict("Annots")?.unwrap();
        let arr = arr.resolve()?.unwrap_or(arr);
        (0..arr.len()? as i32).map(|i| Ok(!arr.get_array(i)?.unwrap().is_indirect()?)).collect::<Result<Vec<bool>>>()
    })
    .unwrap()
}

fn uri_at(l: &PageLinks, i: usize) -> String {
    match &l.links[i].target {
        Target::Uri { uri } => uri.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_highlight_style_is_read_set_and_kept_through_save_for_all_four() {
    let links: Vec<String> = ["", "/H /N", "/H /O", "/H /P", "/H /Z"].iter().map(|h| link_annot(&format!("/A << /S /URI /URI (https://e.example/) >> {h}"))).collect();
    let (eng, id, path, _d) = opened("highlight-read", &page_with_annots("", "", &links, &[]));
    let read: Vec<Highlight> = list(&eng, id, 0).unwrap().links.iter().map(|l| l.highlight).collect();
    assert_eq!(read, [Highlight::Invert, Highlight::None, Highlight::Outline, Highlight::Inset, Highlight::Invert], "absent and unknown are Invert");

    let link = list(&eng, id, 0).unwrap().links[0].id;
    let before = eng.edit_state(id).unwrap();
    let s = go(&eng, id, LinkOp::SetHighlight { page: 0, id: link, highlight: Highlight::Invert }).state;
    assert_eq!((s.rev, s.undo.clone()), (before.rev, before.undo.clone()), "Invert on a link with no /H is no change");
    let want = [Highlight::None, Highlight::Outline, Highlight::Inset, Highlight::Invert];
    for h in want {
        let e = go(&eng, id, LinkOp::SetHighlight { page: 0, id: link, highlight: h });
        assert_eq!(e.state.undo.as_deref(), Some("Change link highlight on p. 1"));
        assert_eq!(list(&eng, id, 0).unwrap().links[0].highlight, h);
    }
    // Each change is its own step, and undo goes back one at a time.
    eng.undo(id).unwrap();
    assert_eq!(list(&eng, id, 0).unwrap().links[0].highlight, Highlight::Inset);
    eng.redo(id).unwrap();
    // Moving the link does not touch its style.
    go(&eng, id, LinkOp::SetHighlight { page: 0, id: link, highlight: Highlight::Outline });
    go(&eng, id, LinkOp::SetRect { page: 0, id: link, rect: [10.0, 10.0, 90.0, 40.0] });
    assert_eq!(list(&eng, id, 0).unwrap().links[0].highlight, Highlight::Outline);
    eng.save(id, None).unwrap();
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    let back: Vec<Highlight> = list(&eng2, id2, 0).unwrap().links.iter().map(|l| l.highlight).collect();
    assert_eq!(back, [Highlight::Outline, Highlight::None, Highlight::Outline, Highlight::Inset, Highlight::Invert]);
}

#[test]
fn a_link_written_inline_in_annots_is_listed_followed_and_audited() {
    let annots = format!("[ {INLINE} 4 0 R << /Type /Annot /Subtype /Link /Rect [100 500 300 550] /Dest (nowhere) >> ]");
    let more = vec![link_annot("/Dest [3 0 R /Fit]")];
    let (eng, id, _, _d) = opened("inline-list", &doc_with(&annots, "", &more));
    assert_eq!(direct_entries(&eng, id), [true, false, true], "the fixture really holds inline dictionaries");
    let l = list(&eng, id, 0).unwrap();
    assert_eq!(l.links.len(), 3);
    assert_eq!(l.links[0].id, -1);
    assert_eq!(uri_at(&l, 0), "https://inline.example/");
    assert!(l.links[1].id > 0 && matches!(l.links[1].target, Target::Page { page: 0, .. }));
    assert_eq!(l.links[2].id, -3);
    assert!(matches!(l.links[2].target, Target::Broken { .. }), "{:?}", l.links[2].target);
    let a = audit(&eng, id).unwrap();
    assert_eq!((a.total, a.external, a.broken), (3, 1, 1));
    assert_eq!(a.rows[0].id, -3);
}

#[test]
fn an_inline_link_is_edited_in_place_each_change_one_exact_undo_step() {
    let annots = format!("[ {INLINE} 4 0 R ]");
    let more = vec![link_annot("/Dest [3 0 R /Fit]")];
    let (eng, id, path, _d) = opened("inline-edit", &doc_with(&annots, "", &more));
    let start = list(&eng, id, 0).unwrap().links;
    let ops = [
        LinkOp::SetTarget { page: 0, id: -1, target: NewTarget::Uri { uri: "https://changed.example/".into() } },
        LinkOp::SetRect { page: 0, id: -1, rect: [20.0, 30.0, 220.0, 80.0] },
        LinkOp::SetBorder { page: 0, id: -1, border: BorderSpec { visible: true, width: Some(2.0), color: Some([1.0, 0.0, 0.0]) } },
        LinkOp::SetHighlight { page: 0, id: -1, highlight: Highlight::Inset },
    ];
    let mut states = vec![start.clone()];
    for op in ops {
        let e = go(&eng, id, op);
        assert!(e.state.dirty);
        states.push(list(&eng, id, 0).unwrap().links);
        assert_eq!(states.last().unwrap()[0].id, -1, "the link keeps its number");
        assert_eq!(states.last().unwrap()[1], start[1], "the other link is untouched");
    }
    let end = states.last().unwrap().clone();
    assert_eq!(end[0].target, Target::Uri { uri: "https://changed.example/".into() });
    assert!(end[0].border.visible && end[0].highlight == Highlight::Inset);
    assert!((end[0].rect[2] - 220.0).abs() < 0.1);
    assert_eq!(direct_entries(&eng, id), [true, false], "it is still written where it was");
    // Undo goes back through exactly the lists it came through.
    for want in states.iter().rev().skip(1) {
        eng.undo(id).unwrap();
        assert_eq!(&list(&eng, id, 0).unwrap().links, want);
    }
    for want in states.iter().skip(1) {
        eng.redo(id).unwrap();
        assert_eq!(&list(&eng, id, 0).unwrap().links, want);
    }
    eng.save(id, None).unwrap();
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    assert_eq!(list(&eng2, id2, 0).unwrap().links, end, "the edit is in the saved file");
}

#[test]
fn deleting_inline_and_indirect_links_together_takes_the_right_entries() {
    let annots = format!("[ {INLINE} 4 0 R {INLINE} 5 0 R {INLINE} ]");
    let more = vec![link_annot("/Dest [3 0 R /Fit]"), "<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (keep) >>".to_string()];
    let (eng, id, _, _d) = opened("inline-delete", &doc_with(&annots, "", &more));
    let l = list(&eng, id, 0).unwrap();
    assert_eq!(l.links.iter().map(|k| k.id).collect::<Vec<_>>(), [-1, 4, -3, -5]);
    // Not in order, and a repeat: all resolved against the list the person saw.
    let e = go(&eng, id, LinkOp::Delete { page: 0, ids: vec![-5, 4, -1, -5] });
    assert_eq!(e.count, 3);
    let left = list(&eng, id, 0).unwrap();
    assert_eq!(left.links.len(), 1);
    assert_eq!(direct_entries(&eng, id), [true, false], "the inline link and the note that were not named are still there");
    eng.undo(id).unwrap();
    assert_eq!(list(&eng, id, 0).unwrap(), PageLinks { rev: eng.edit_state(id).unwrap().rev, ..l });
}

fn two_page_doc(annots_1: &str, annots_2: &str, catalog_extra: &str, more: &[String]) -> Vec<u8> {
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>"),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots {annots_1} >>"),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots {annots_2} >>"),
    ];
    objs.extend(more.iter().cloned());
    raw_pdf(&objs)
}

/// Objects from 5: a link (5), a highlight annotation (6), an outline root (7) and its item (8).
fn remove_all_fixture() -> Vec<u8> {
    let more = vec![
        link_annot("/A << /S /URI /URI (https://indirect.example/zz-marker-9) >>"),
        "<< /Type /Annot /Subtype /Highlight /Rect [10 10 90 30] /QuadPoints [10 30 90 30 10 10 90 10] /C [1 1 0] >>".to_string(),
        "<< /Type /Outlines /First 8 0 R /Last 8 0 R /Count 1 >>".to_string(),
        "<< /Title (Start) /Parent 7 0 R /Dest [3 0 R /Fit] >>".to_string(),
    ];
    // Page 1: an inline link, the shared one, the highlight. Page 2: the shared one again and an inline one.
    two_page_doc(&format!("[ {INLINE} 5 0 R 6 0 R ]"), &format!("[ 5 0 R {INLINE} ]"), "/Outlines 7 0 R", &more)
}

#[test]
fn remove_all_links_takes_every_link_in_one_step_and_keeps_the_rest() {
    let d = dir("links", "remove-all");
    let (eng, id, path) = open_bytes(&d, "a.pdf", &remove_all_fixture());
    let has = |p: &str, m: &str| String::from_utf8_lossy(&std::fs::read(p).unwrap()).contains(m);
    let before: Vec<PageLinks> = (0..2).map(|p| list(&eng, id, p).unwrap()).collect();
    assert_eq!((before[0].links.len(), before[1].links.len()), (2, 2));
    assert_eq!(audit(&eng, id).unwrap().total, 4);
    let outline = crate::outline::list(&eng, id).unwrap().items.len();
    assert_eq!(outline, 1);

    let e = go(&eng, id, LinkOp::RemoveAll);
    assert_eq!(e.count, 4, "as many as the audit counted: the shared one is on two pages");
    assert_eq!(e.count, before.iter().map(|p| p.links.len() as u32).sum::<u32>());
    assert_eq!(e.state.undo.as_deref(), Some("Remove all links"));
    for p in 0..2 {
        assert!(list(&eng, id, p).unwrap().links.is_empty(), "page {p}");
    }
    assert_eq!(crate::outline::list(&eng, id).unwrap().items.len(), outline, "the outline stays");
    let page1 = eng
        .read_custom(id, |d| {
            let arr = d.find_page(0)?.get_dict("Annots")?.unwrap();
            let arr = arr.resolve()?.unwrap_or(arr);
            let sub = arr.get_array(0)?.unwrap().get_dict("Subtype")?.map(|s| s.as_name().map(|n| String::from_utf8_lossy(&n).into_owned()));
            Ok((arr.len()?, sub))
        })
        .unwrap();
    assert_eq!((page1.0, page1.1.and_then(|s| s.ok()).as_deref()), (1, Some("Highlight")), "the highlight annotation stays");

    // One step: undo brings all four back, as they were.
    eng.undo(id).unwrap();
    for (p, was) in before.iter().enumerate() {
        let now = list(&eng, id, p as u32).unwrap();
        assert_eq!(now.links, was.links, "page {p}");
    }
    eng.redo(id).unwrap();
    eng.save(id, None).unwrap();
    assert!(!has(&path, "zz-marker-9") && !has(&path, "inline.example"), "a removed link's address is not kept in the file");
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    assert_eq!(audit(&eng2, id2).unwrap().total, 0);
    assert_eq!(crate::outline::list(&eng2, id2).unwrap().items.len(), 1);
    // Nothing left: asking again is no step.
    let s = eng.edit_state(id).unwrap();
    let again = apply(&eng, id, rev(&eng, id), &LinkOp::RemoveAll, EditSource::User).unwrap().state;
    assert_eq!((again.rev, again.undo), (s.rev, s.undo));
}

#[test]
fn a_certified_document_takes_neither_remove_all_nor_a_highlight_change() {
    let cat = "/Perms << /DocMDP << /Type /Sig /Reference [ << /TransformMethod /DocMDP /TransformParams << /P 2 >> >> ] >> >>";
    let (eng, id, _, _d) = opened("certified-depth", &page_with_annots("", cat, &[link_annot("/Dest [3 0 R /Fit]"), INLINE.to_string()], &[]));
    let l = list(&eng, id, 0).unwrap();
    assert!(l.locked.as_deref().is_some_and(|m| m.contains("certified")), "the list says why");
    let before = eng.edit_state(id).unwrap();
    for op in [
        LinkOp::RemoveAll,
        LinkOp::SetHighlight { page: 0, id: l.links[0].id, highlight: Highlight::None },
        LinkOp::SetHighlight { page: 0, id: l.links[1].id, highlight: Highlight::None },
        LinkOp::Delete { page: 0, ids: vec![l.links[1].id] },
    ] {
        match apply(&eng, id, rev(&eng, id), &op, EditSource::User) {
            Err(EngineError::Invalid(m)) => assert!(m.contains("certified"), "{op:?}: {m}"),
            other => panic!("{op:?}: {other:?}"),
        }
        let s = eng.edit_state(id).unwrap();
        assert_eq!((s.rev, s.undo, s.dirty), (before.rev, before.undo.clone(), before.dirty), "{op:?} left a step");
    }
}

#[test]
fn remove_all_links_of_a_signed_document_stays_incremental() {
    let cat = "/AcroForm << /SigFlags 3 /Fields [5 0 R] >>";
    let sig = "<< /FT /Sig /T (Signature1) /V << /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00> >> >>".to_string();
    let annots = format!("[ {INLINE} 4 0 R ]");
    let more = vec![link_annot("/A << /S /URI /URI (https://secret.example/) >>"), sig];
    let bytes = doc_with(&annots, cat, &more);
    let (eng, id, path, _d) = opened("signed-remove-all", &bytes);
    let l = list(&eng, id, 0).unwrap();
    assert!(l.signed && l.locked.is_none());
    let original = std::fs::read(&path).unwrap();
    assert_eq!(go(&eng, id, LinkOp::RemoveAll).count, 2);
    eng.save(id, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len() && saved.starts_with(&original), "the signed bytes stay the start of the file");
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    assert!(list(&eng2, id2, 0).unwrap().links.is_empty());
}

#[test]
fn a_hostile_annots_array_is_read_as_far_as_the_cap_and_never_half_removed() {
    // 60,000 entries that are not links, then a link; the same object listed many times; an array that holds itself.
    let junk = "0 ".repeat(MAX_ANNOTS + 10_000);
    let (eng, id, _, _d) = opened("hostile-long", &doc_with(&format!("[ {junk} 4 0 R ]"), "", &[link_annot("/Dest [3 0 R /Fit]")]));
    assert!(list(&eng, id, 0).unwrap().links.is_empty(), "past the cap nothing is read");
    let before = eng.edit_state(id).unwrap();
    match apply(&eng, id, rev(&eng, id), &LinkOp::RemoveAll, EditSource::User) {
        Err(EngineError::Invalid(m)) => assert!(m.contains("more annotations"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(eng.edit_state(id).unwrap().rev, before.rev);

    let repeated = "4 0 R ".repeat(10_000);
    let (eng, id, _, _d) = opened("hostile-repeat", &doc_with(&format!("[ {repeated} ]"), "", &[link_annot("/Dest [3 0 R /Fit]")]));
    assert_eq!(list(&eng, id, 0).unwrap().links.len(), 1, "one object listed ten thousand times is one link");
    assert_eq!(go(&eng, id, LinkOp::RemoveAll).count, 1);
    assert!(list(&eng, id, 0).unwrap().links.is_empty(), "and every entry for it went");

    // /Annots is object 4, an array that holds itself, a nested array and a link (5).
    let more = vec!["[ 4 0 R [ 4 0 R ] 5 0 R 5 0 R ]".to_string(), link_annot("/Dest [3 0 R /Fit]")];
    let (eng, id, _, _d) = opened("hostile-loop", &doc_with("4 0 R", "", &more));
    assert_eq!(list(&eng, id, 0).unwrap().links.len(), 1);
    assert_eq!(audit(&eng, id).unwrap().total, 1);
    assert_eq!(go(&eng, id, LinkOp::RemoveAll).count, 1);
}

// ---- round 2 ----

#[test]
fn a_certification_that_cannot_be_read_still_lists_the_links_as_locked() {
    // A form with more fields than are checked: `certified` is an error, never a shorter answer.
    let fields = "5 0 R ".repeat(20_001);
    let cat = format!("/AcroForm << /Fields [ {fields} ] >>");
    let (eng, id, _, _d) = opened("unreadable-certification", &page_with_annots("", &cat, &[link_annot("/Dest [3 0 R /Fit]")], &["<< /FT /Tx /T (a) >>".into()]));
    assert!(eng.read_custom(id, |d| Ok(certified(d).is_err())).unwrap(), "the fixture's certification is unreadable");
    let l = list(&eng, id, 0).unwrap();
    assert_eq!(l.links.len(), 1, "the links are still listed");
    assert!(l.locked.as_deref().is_some_and(|m| m.contains("certified")), "{:?}", l.locked);
    let before = eng.edit_state(id).unwrap();
    for op in [LinkOp::RemoveAll, LinkOp::SetHighlight { page: 0, id: l.links[0].id, highlight: Highlight::None }] {
        assert!(matches!(apply(&eng, id, rev(&eng, id), &op, EditSource::User), Err(EngineError::Invalid(_))), "{op:?}");
    }
    assert_eq!(eng.edit_state(id).unwrap().rev, before.rev);
}

#[test]
fn a_dictionary_that_is_a_form_field_too_is_not_a_link_to_list_or_delete() {
    let cat = "/AcroForm << /Fields [5 0 R 6 0 R] >>";
    let annots = [
        link_annot("/Dest [3 0 R /Fit]"),
        "<< /Type /Annot /Subtype /Link /FT /Sig /T (Signature1) /Rect [10 10 90 30] >>".to_string(),
        "<< /Type /Annot /Subtype /Link /Parent 7 0 R /Rect [10 40 90 60] >>".to_string(),
    ];
    let more = vec!["<< /FT /Btn /T (group) /Kids [6 0 R] >>".to_string()];
    let inline = "<< /Type /Annot /Subtype /Link /FT /Tx /T (x) /Rect [10 70 90 90] >>";
    let bytes = {
        let mut objs = vec![
            format!("<< /Type /Catalog /Pages 2 0 R {cat} >>"),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [ 4 0 R 5 0 R 6 0 R {inline} ] >>"),
        ];
        objs.extend(annots);
        objs.extend(more);
        raw_pdf(&objs)
    };
    let (eng, id, _, _d) = opened("field-as-link", &bytes);
    let l = list(&eng, id, 0).unwrap();
    assert_eq!(l.links.len(), 1, "only the plain link is listed: {:?}", l.links);
    assert_eq!(audit(&eng, id).unwrap().total, 1);
    // Naming a field's number to Delete finds no link, and Remove all leaves the fields where they are.
    assert!(matches!(
        apply(&eng, id, rev(&eng, id), &LinkOp::Delete { page: 0, ids: vec![5] }, EditSource::User),
        Err(EngineError::Stale)
    ));
    assert_eq!(go(&eng, id, LinkOp::RemoveAll).count, 1);
    let kept = eng.read_custom(id, |d| Ok(d.find_page(0)?.get_dict("Annots")?.unwrap().len()?)).unwrap();
    assert_eq!(kept, 3, "the three field widgets stay in /Annots");
}

#[test]
fn a_parent_tree_entry_that_is_not_the_links_own_stays_when_the_link_goes() {
    // The link's /StructParent key is also the key of a page's /StructParents: its value is an array of elements, not
    // an element that holds an object reference to the link.
    for (name, value) in [("page-array", "[ 6 0 R ]"), ("other-element", "6 0 R")] {
        let annots = vec![link_annot("/Dest [3 0 R /Fit] /StructParent 0")];
        let more = vec![
            "<< /Type /StructTreeRoot /K 6 0 R /ParentTree 7 0 R >>".to_string(),
            "<< /Type /StructElem /S /P /P 5 0 R /Pg 3 0 R /K [ 0 1 ] >>".to_string(),
            format!("<< /Nums [ 0 {value} ] >>"),
        ];
        let (eng, id, _, _d) = opened(&format!("structure-{name}"), &page_with_annots("", "/StructTreeRoot 5 0 R", &annots, &more));
        let link = list(&eng, id, 0).unwrap().links[0].id;
        go(&eng, id, LinkOp::Delete { page: 0, ids: vec![link] });
        let (nums, kids) = eng
            .read_custom(id, |doc| {
                let nums = doc.new_indirect(7, 0)?.get_dict("Nums")?.unwrap().len()?;
                let k = doc.new_indirect(6, 0)?.get_dict("K")?.map_or(0, |k| k.len().unwrap_or(1));
                Ok((nums, k))
            })
            .unwrap();
        assert_eq!((nums, kids), (2, 2), "{name}: the entry and the element's content are untouched");
        assert!(list(&eng, id, 0).unwrap().links.is_empty());
    }
}
