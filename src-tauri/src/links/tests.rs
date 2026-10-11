//! Links through the edit lane: draw, retarget, border, delete, links from addresses, the
//! address gate, undo and a round trip through save.

use super::*;
use crate::engine::fixture::{self, Fixture};
use crate::outline::tests::{dir, open_bytes, pdf_with};

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
fn a_link_to_a_file_keeps_a_relative_name_and_never_a_folder_of_this_machine() {
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
    assert_eq!(l[1].target, Target::File { name: "notes.txt".into(), page: None });
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
