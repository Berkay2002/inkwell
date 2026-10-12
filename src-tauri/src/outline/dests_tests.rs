//! Named destinations: list, rename with its references, set to a view, add, delete, remove unused, the name tree
//! (sorted, `/Limits`, balanced), certified and signed documents, and that everything that resolved still does.

use super::tests::{dir, open_bytes, raw_pdf};
use super::*;
use std::path::PathBuf;
use std::sync::Arc;

/// Three pages. Bookmarks "One" (`/Dest (alpha)`) and "Two" (a GoTo to `/alpha`), two links on page 1 (`/Dest /alpha`,
/// a GoTo to `(alpha)`), and `extra` as objects 12.. . `catalog` is more catalog keys.
fn file(catalog: &str, extra: &[&str]) -> Vec<u8> {
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>";
    let mut objs: Vec<String> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R {catalog} >>"),
        "<< /Type /Pages /Kids [5 0 R 6 0 R 7 0 R] /Count 3 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /Outlines /First 8 0 R /Last 9 0 R /Count 2 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [10 0 R 11 0 R] >>".into(),
        page.into(),
        page.into(),
        "<< /Title (One) /Parent 4 0 R /Dest (alpha) /Next 9 0 R >>".into(),
        "<< /Title (Two) /Parent 4 0 R /A << /S /GoTo /D /alpha >> /Prev 8 0 R >>".into(),
        "<< /Type /Annot /Subtype /Link /Rect [10 10 50 30] /Border [0 0 0] /Dest /alpha >>".into(),
        "<< /Type /Annot /Subtype /Link /Rect [10 40 50 60] /Border [0 0 0] /A << /S /GoTo /D (alpha) >> >>".into(),
    ];
    objs.extend(extra.iter().map(|s| s.to_string()));
    raw_pdf(&objs)
}

const TREE: &str = "/Names << /Dests << /Names [(alpha) [5 0 R /Fit] (beta) [6 0 R /Fit] (gamma) [7 0 R /Fit]] >> >>";
const OLD: &str = "/Dests << /alpha [5 0 R /Fit] /beta [6 0 R /Fit] /gamma [7 0 R /Fit] >>";

fn open(name: &str, catalog: &str, extra: &[&str]) -> (Arc<Engine>, DocId, PathBuf) {
    let d = dir("dests", name);
    let (eng, id, path) = open_bytes(&d, "a.pdf", &file(catalog, extra));
    (eng, id, PathBuf::from(path))
}

fn rev(eng: &Engine, id: DocId) -> u32 {
    eng.edit_state(id).unwrap().rev
}

fn go(eng: &Engine, id: DocId, op: DestOp) -> DestEdited {
    dests::apply(eng, id, rev(eng, id), &op, EditSource::User).unwrap_or_else(|e| panic!("{op:?} failed: {e:?}"))
}

/// The refusal's message; the document is as it was.
fn refused(eng: &Engine, id: DocId, op: DestOp) -> String {
    let before = eng.edit_state(id).unwrap();
    match dests::apply(eng, id, rev(eng, id), &op, EditSource::User) {
        Err(EngineError::Invalid(m)) => {
            let after = eng.edit_state(id).unwrap();
            assert_eq!((after.rev, after.undo), (before.rev, before.undo), "{op:?} changed the document");
            m
        }
        other => panic!("{op:?}: {other:?}"),
    }
}

fn dl(eng: &Engine, id: DocId) -> DestList {
    dests::list(eng, id).unwrap()
}

fn names(l: &DestList) -> Vec<&str> {
    l.items.iter().map(|e| e.name.as_str()).collect()
}

fn entry<'a>(l: &'a DestList, name: &str) -> &'a DestEntry {
    l.items.iter().find(|e| e.name == name).unwrap_or_else(|| panic!("{name} is not listed: {:?}", names(l)))
}

fn page_of(e: &DestEntry) -> Option<u32> {
    match e.dest {
        Dest::Page { page, .. } => Some(page),
        _ => None,
    }
}

/// Where every bookmark and every link of every page goes, as the panels show it.
fn resolved(eng: &Engine, id: DocId) -> (Vec<Dest>, Vec<crate::links::Target>) {
    fn flat(items: &[Bookmark], out: &mut Vec<Dest>) {
        for b in items {
            out.push(b.dest.clone());
            flat(&b.children, out);
        }
    }
    let mut bm = Vec::new();
    flat(&super::list(eng, id).unwrap().items, &mut bm);
    let links = (0..3).flat_map(|p| crate::links::list(eng, id, p).unwrap().links.into_iter().map(|l| l.target)).collect();
    (bm, links)
}

/// The object `num`'s `key`, printed.
fn raw(eng: &Engine, id: DocId, num: i32, key: &str) -> String {
    eng.read_custom(id, |d| Ok(format!("{:?}", d.new_indirect(num, 0)?.get_dict(key)?.map(|o| o.resolve().ok().flatten().map(|r| r.to_string())))))
        .unwrap()
}

/// Walks `/Names /Dests` as a reader would and holds it to the rules: sorted keys with no duplicates, kids indirect,
/// `/Limits` on every node but the root and equal to what is below it. Returns the keys in order and the depth of the leaves (all equal).
fn check_tree(eng: &Engine, id: DocId) -> (Vec<Vec<u8>>, usize) {
    eng.read_custom(id, |d| {
        fn walk(
            node: &PdfObject,
            is_root: bool,
            depth: usize,
            out: &mut Vec<Vec<u8>>,
            leaves: &mut Vec<usize>,
        ) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
            let kids = node.get_dict("Kids").unwrap();
            let names = node.get_dict("Names").unwrap();
            assert!(kids.is_none() || names.is_none(), "a node has both /Kids and /Names");
            let (mut lo, mut hi) = (None, None);
            if let Some(k) = kids {
                assert!(k.len().unwrap() > 0, "an inner node has kids");
                for i in 0..k.len().unwrap() as i32 {
                    let kid = k.get_array(i).unwrap().unwrap();
                    assert!(kid.is_indirect().unwrap(), "kids are indirect references");
                    let (l, h) = walk(&kid, false, depth + 1, out, leaves);
                    lo = lo.or(l);
                    hi = h;
                }
            } else if let Some(n) = names {
                assert_eq!(n.len().unwrap() % 2, 0, "names come in pairs");
                assert!(is_root || n.len().unwrap() > 0, "no node but the root is empty");
                leaves.push(depth);
                for i in 0..(n.len().unwrap() / 2) as i32 {
                    let k = n.get_array(2 * i).unwrap().unwrap().as_bytes().unwrap();
                    out.push(k.clone());
                    lo = lo.or(Some(k.clone()));
                    hi = Some(k);
                }
            }
            let limits = node.get_dict("Limits").unwrap();
            if is_root {
                assert!(limits.is_none(), "the root has no /Limits");
            } else {
                let l = limits.expect("every node but the root has /Limits");
                let get = |i| l.get_array(i).unwrap().unwrap().as_bytes().unwrap();
                assert_eq!((Some(get(0)), Some(get(1))), (lo.clone(), hi.clone()), "/Limits is what lies below the node");
            }
            (lo, hi)
        }
        let root = d.catalog()?.get_dict("Names")?.unwrap().get_dict("Dests")?.unwrap();
        let (mut keys, mut leaves) = (Vec::new(), Vec::new());
        walk(&root, true, 0, &mut keys, &mut leaves);
        assert!(keys.windows(2).all(|w| w[0] < w[1]), "keys are sorted and unique");
        assert!(leaves.windows(2).all(|w| w[0] == w[1]), "every leaf is at the same depth: {leaves:?}");
        Ok((keys, leaves.first().copied().unwrap_or(0)))
    })
    .unwrap()
}

fn many(n: usize) -> String {
    let pairs: String = (0..n).map(|i| format!("(k{i:04}) [{} 0 R /Fit] ", 5 + i % 3)).collect();
    format!("/Names << /Dests << /Names [{pairs}] >> >>")
}

#[test]
fn lists_both_sources_by_name_with_page_and_uses() {
    let cat = "/Dests << /alpha [6 0 R /Fit] /zeta [7 0 R /Fit] >> /Names << /Dests << /Names [(beta) [5 0 R /Fit] (zeta) [5 0 R /Fit]] >> >>";
    let (eng, id, _) = open("list", cat, &[]);
    let l = dl(&eng, id);
    assert_eq!(names(&l), ["alpha", "beta", "zeta"]);
    assert!(l.both && !l.truncated && l.frozen.is_none() && l.locked.is_none() && !l.signed && !l.unchecked);
    let alpha = entry(&l, "alpha");
    assert_eq!((page_of(alpha), alpha.in_dests, alpha.in_tree, alpha.shadowed), (Some(1), true, false, false));
    // two bookmarks and two links name it
    assert_eq!(alpha.refs, 4, "{:?}", alpha.used_by);
    // MuPDF reads /Dests alone: the tree-only name is listed, marked, and its page is its own
    let beta = entry(&l, "beta");
    assert_eq!((page_of(beta), beta.in_dests, beta.in_tree, beta.shadowed), (Some(0), false, true, true));
    // a name in both is one row and MuPDF's source (/Dests) gives the page
    let zeta = entry(&l, "zeta");
    assert_eq!((page_of(zeta), zeta.in_dests, zeta.in_tree), (Some(2), true, true));
    assert_eq!(zeta.refs, 0);
}

#[test]
fn a_missing_page_is_broken_and_an_undecodable_name_is_still_listed() {
    // the key <41E97F> is not UTF-8 and has no PDFDocEncoding text (0x7F)
    let cat = "/Dests << /alpha [99 0 R /Fit] /#41#E9#7F [5 0 R /Fit] >>";
    let (eng, id, _) = open("broken", cat, &[]);
    let l = dl(&eng, id);
    assert!(matches!(entry(&l, "alpha").dest, Dest::Broken { .. }), "{:?}", entry(&l, "alpha").dest);
    assert!(names(&l).contains(&"A\\xE9\\x7F"), "{:?}", names(&l));
}

#[test]
fn rename_in_the_tree_updates_every_reference_and_is_one_undo_step() {
    let cat = format!("{TREE} /OpenAction (alpha)");
    let (eng, id, _) = open("rename_tree", &cat, &[]);
    let before = resolved(&eng, id);
    assert_eq!(before.0.len(), 2);
    let done = go(&eng, id, DestOp::Rename { name: "alpha".into(), to: "  omega ".into() });
    assert_eq!(done.state.undo.as_deref(), Some("Rename destination \"alpha\" to \"omega\""));
    assert_eq!(names(&dl(&eng, id)), ["beta", "gamma", "omega"]);
    assert_eq!(resolved(&eng, id), before, "every bookmark and link goes where it went");
    // the references hold the new name, in the kind they were
    let (bm, link) = (raw(&eng, id, 8, "Dest"), raw(&eng, id, 10, "Dest"));
    assert!(bm.contains("(omega)") && link.contains("/omega"), "{bm} {link}");
    assert!(raw(&eng, id, 1, "OpenAction").contains("(omega)"));
    check_tree(&eng, id);
    // one step back restores all of it
    eng.undo(id).unwrap();
    assert_eq!(names(&dl(&eng, id)), ["alpha", "beta", "gamma"]);
    assert_eq!(resolved(&eng, id), before);
    assert!(raw(&eng, id, 10, "Dest").contains("/alpha"));
}

#[test]
fn rename_in_the_old_dictionary_and_in_both_sources_at_once() {
    let (eng, id, _) = open("rename_old", OLD, &[]);
    let before = resolved(&eng, id);
    go(&eng, id, DestOp::Rename { name: "alpha".into(), to: "omega".into() });
    assert_eq!(names(&dl(&eng, id)), ["beta", "gamma", "omega"]);
    assert_eq!(resolved(&eng, id), before);
    assert!(eng.read_custom(id, |d| Ok(d.catalog()?.get_dict("Dests")?.unwrap().get_dict("alpha")?.is_none())).unwrap());

    // a name that lives in both places is renamed in both
    let cat = "/Dests << /alpha [5 0 R /Fit] >> /Names << /Dests << /Names [(alpha) [5 0 R /Fit]] >> >>";
    let (eng, id, _) = open("rename_both", cat, &[]);
    let before = resolved(&eng, id);
    go(&eng, id, DestOp::Rename { name: "alpha".into(), to: "omega".into() });
    let l = dl(&eng, id);
    assert_eq!(names(&l), ["omega"]);
    assert_eq!((l.items[0].in_dests, l.items[0].in_tree), (true, true));
    assert_eq!(resolved(&eng, id), before);
    assert_eq!(check_tree(&eng, id).0, vec![b"omega".to_vec()]);
}

#[test]
fn a_non_ascii_name_goes_into_the_tree_and_is_refused_in_the_old_dictionary() {
    let (eng, id, _) = open("nonascii_tree", TREE, &[]);
    let before = resolved(&eng, id);
    go(&eng, id, DestOp::Rename { name: "alpha".into(), to: "caf\u{e9}".into() });
    assert!(names(&dl(&eng, id)).contains(&"caf\u{e9}"));
    assert_eq!(resolved(&eng, id), before, "a name and a string reference both still find it");
    check_tree(&eng, id);

    let (eng, id, _) = open("nonascii_old", OLD, &[]);
    let why = refused(&eng, id, DestOp::Rename { name: "alpha".into(), to: "caf\u{e9}".into() });
    assert!(why.contains("ASCII"), "{why}");
}

#[test]
fn a_rename_is_refused_when_a_script_may_use_the_name() {
    let cat = "/Names << /Dests << /Names [(alpha) [5 0 R /Fit] (beta) [6 0 R /Fit]] >> /JavaScript << /Names [(init) 12 0 R] >> >>";
    let (eng, id, _) = open("rename_js", cat, &["<< /S /JavaScript /JS (this.gotoNamedDest(\"alpha\");) >>"]);
    let why = refused(&eng, id, DestOp::Rename { name: "alpha".into(), to: "omega".into() });
    assert!(why.contains("JavaScript") && why.contains("alpha"), "{why}");
    assert!(dl(&eng, id).items.iter().all(|e| e.name != "beta" || e.maybe.is_none()), "only the mentioned name is in doubt");
    assert!(entry(&dl(&eng, id), "alpha").maybe.is_some());
    // a name the script does not mention renames
    go(&eng, id, DestOp::Rename { name: "beta".into(), to: "delta".into() });
}

/// `/Dests` holds the key FE FF, which MuPDF's viewer forms from any non-ASCII string (the UTF-16 text cut at its first
/// 0): the bookmark's string "caf\u{e9}" reaches it by a spelling no rename follows, so the rename is refused and says where.
#[test]
fn a_reference_spelled_another_way_refuses_the_rename() {
    let cat = "/Dests << /#FE#FF [6 0 R /Fit] >>";
    let d = dir("dests", "spelled");
    let mut objs = file(cat, &[]);
    // the first bookmark names "caf\u{e9}" as UTF-16
    let text = String::from_utf8_lossy(&objs).replace("/Dest (alpha)", "/Dest <FEFF00630061006600E9>");
    objs = text.into_bytes();
    let (eng, id, _p) = open_bytes(&d, "a.pdf", &objs);
    let before = resolved(&eng, id);
    assert!(matches!(before.0[0], Dest::Page { page: 1, .. }), "{:?}", before.0[0]);
    let key = dl(&eng, id).items[0].name.clone();
    let why = refused(&eng, id, DestOp::Rename { name: key, to: "x".into() });
    assert!(why.contains("bookmark \"One\"") && why.contains("left as it is"), "{why}");
}

#[test]
fn set_view_writes_the_page_the_point_and_the_zoom() {
    let (eng, id, _) = open("set_view", TREE, &[]);
    let done = go(&eng, id, DestOp::SetView { name: "alpha".into(), page: 2, at: Some([36.0, 100.0]), zoom: Some(1.5) });
    assert_eq!(done.state.undo.as_deref(), Some("Set destination \"alpha\""));
    let l = dl(&eng, id);
    match &entry(&l, "alpha").dest {
        Dest::Page { page, at: Some([x, y]) } => assert_eq!((*page, x.round(), y.round()), (2, 36.0, 100.0)),
        other => panic!("{other:?}"),
    }
    let zoom = eng
        .read_custom(id, |d| {
            let arr = d.catalog()?.get_dict("Names")?.unwrap().get_dict("Dests")?.unwrap().get_dict("Names")?.unwrap().get_array(1)?.unwrap();
            Ok((name_of(arr.get_array(1)?), number(&arr.get_array(4)?)))
        })
        .unwrap();
    assert_eq!(zoom, ("XYZ".to_string(), Some(1.5)));
    // the same view again changes nothing
    let r = rev(&eng, id);
    let again = go(&eng, id, DestOp::SetView { name: "alpha".into(), page: 2, at: Some([36.0, 100.0]), zoom: Some(1.5) });
    assert_eq!(again.state.rev, r);
    check_tree(&eng, id);
    // the bookmarks that named it follow it
    assert!(matches!(resolved(&eng, id).0[0], Dest::Page { page: 2, .. }));
    // one step undoes it
    eng.undo(id).unwrap();
    assert_eq!(page_of(entry(&dl(&eng, id), "alpha")), Some(0));
}

#[test]
fn set_view_keeps_a_dictionary_value_and_sets_every_place_a_name_lives() {
    let cat = "/Dests << /alpha << /D [5 0 R /Fit] >> >> /Names << /Dests << /Names [(alpha) [6 0 R /Fit]] >> >>";
    let (eng, id, _) = open("set_view_dict", cat, &[]);
    go(&eng, id, DestOp::SetView { name: "alpha".into(), page: 2, at: None, zoom: None });
    let (old, tree) = eng
        .read_custom(id, |d| {
            let c = d.catalog()?;
            let old = c.get_dict("Dests")?.unwrap().get_dict("alpha")?.unwrap();
            let tree = c.get_dict("Names")?.unwrap().get_dict("Dests")?.unwrap().get_dict("Names")?.unwrap().get_array(1)?.unwrap();
            Ok((old.is_dict()? && old.get_dict("D")?.is_some(), tree.is_array()?))
        })
        .unwrap();
    assert!(old, "the /D dictionary is kept");
    assert!(tree);
    assert_eq!(page_of(entry(&dl(&eng, id), "alpha")), Some(2));
}

#[test]
fn delete_takes_a_name_out_and_its_users_become_broken() {
    let (eng, id, _) = open("delete", TREE, &[]);
    let done = go(&eng, id, DestOp::Delete { name: "alpha".into() });
    assert_eq!(done.count, 1);
    assert_eq!(names(&dl(&eng, id)), ["beta", "gamma"]);
    let (bm, links) = resolved(&eng, id);
    assert!(matches!(bm[0], Dest::Broken { .. }), "{:?}", bm[0]);
    assert!(matches!(links[0], crate::links::Target::Broken { .. }), "{:?}", links[0]);
    check_tree(&eng, id);
    eng.undo(id).unwrap();
    assert_eq!(names(&dl(&eng, id)), ["alpha", "beta", "gamma"]);
    assert!(matches!(resolved(&eng, id).0[0], Dest::Page { .. }));
}

#[test]
fn add_goes_where_the_names_live_and_never_creates_or_removes_dests() {
    // only the tree: into the tree
    let (eng, id, _) = open("add_tree", TREE, &[]);
    go(&eng, id, DestOp::Add { name: "delta".into(), page: 1, at: None, zoom: None });
    assert_eq!(names(&dl(&eng, id)), ["alpha", "beta", "delta", "gamma"]);
    assert_eq!(check_tree(&eng, id).0.len(), 4);
    assert!(eng.read_custom(id, |d| Ok(d.catalog()?.get_dict("Dests")?.is_none())).unwrap(), "/Dests would hide the tree");
    assert!(refused(&eng, id, DestOp::Add { name: "alpha".into(), page: 0, at: None, zoom: None }).contains("already exists"));
    eng.undo(id).unwrap();
    assert_eq!(names(&dl(&eng, id)), ["alpha", "beta", "gamma"]);

    // only /Dests: MuPDF reads nothing else, so it stays there (ASCII)
    let (eng, id, _) = open("add_old", OLD, &[]);
    go(&eng, id, DestOp::Add { name: "delta".into(), page: 1, at: None, zoom: None });
    let l = dl(&eng, id);
    assert!(entry(&l, "delta").in_dests && !entry(&l, "delta").in_tree);
    assert!(refused(&eng, id, DestOp::Add { name: "caf\u{e9}".into(), page: 0, at: None, zoom: None }).contains("ASCII"));
    assert!(eng.read_custom(id, |d| Ok(d.catalog()?.get_dict("Names")?.is_none())).unwrap(), "no tree was made");

    // both: into the tree, listed as not read by MuPDF
    let cat = "/Dests << /alpha [5 0 R /Fit] >> /Names << /Dests << /Names [(beta) [6 0 R /Fit]] >> >>";
    let (eng, id, _) = open("add_both", cat, &[]);
    go(&eng, id, DestOp::Add { name: "delta".into(), page: 1, at: None, zoom: None });
    let l = dl(&eng, id);
    assert!(entry(&l, "delta").in_tree && entry(&l, "delta").shadowed);

    // none at all: a tree is made, and /Dests is not
    let (eng, id, _) = open("add_none", "", &[]);
    go(&eng, id, DestOp::Add { name: "delta".into(), page: 1, at: Some([10.0, 10.0]), zoom: None });
    assert_eq!(names(&dl(&eng, id)), ["delta"]);
    check_tree(&eng, id);
    assert!(eng.read_custom(id, |d| Ok(d.catalog()?.get_dict("Dests")?.is_none())).unwrap());
}

#[test]
fn deleting_every_name_of_the_old_dictionary_keeps_the_dictionary() {
    let (eng, id, _) = open("empty_old", OLD, &[]);
    for n in ["alpha", "beta", "gamma"] {
        go(&eng, id, DestOp::Delete { name: n.into() });
    }
    assert!(dl(&eng, id).items.is_empty());
    assert!(eng.read_custom(id, |d| Ok(d.catalog()?.get_dict("Dests")?.is_some())).unwrap(), "an empty /Dests still stands");
}

#[test]
fn remove_unused_keeps_what_is_referenced_or_in_doubt() {
    let cat = "/Names << /Dests << /Names [(alpha) [5 0 R /Fit] (chained) [6 0 R /Fit] (target) [6 0 R /Fit] (viajs) [7 0 R /Fit] (zunused) [7 0 R /Fit] (zzunused) [7 0 R /Fit]] >> /JavaScript << /Names [(init) 12 0 R] >> >>";
    let (eng, id, _) = open("unused", cat, &["<< /S /JavaScript /JS (this.getNthDestName(0); gotoNamedDest(\"viajs\")) >>"]);
    let before = resolved(&eng, id);
    let done = go(&eng, id, DestOp::RemoveUnused);
    assert_eq!(done.count, 4, "{:?}", done.kept);
    assert_eq!(names(&dl(&eng, id)), ["alpha", "viajs"]);
    // all four went in one step
    eng.undo(id).unwrap();
    assert_eq!(dl(&eng, id).items.len(), 6);
    eng.redo(id).unwrap();
    assert_eq!(names(&dl(&eng, id)), ["alpha", "viajs"]);
    assert!(done.kept.iter().any(|k| k.contains("viajs") && k.contains("JavaScript")), "{:?}", done.kept);
    assert_eq!(resolved(&eng, id), before);
    check_tree(&eng, id);
    // nothing left to remove: no step
    let r = rev(&eng, id);
    assert_eq!(go(&eng, id, DestOp::RemoveUnused).state.rev, r);
}

/// A field that is on no page, a field's `/AA`, and a GoTo reached through `/Next` of another action are all users.
#[test]
fn what_forms_additional_actions_and_next_chains_use_is_kept_and_followed() {
    let cat = "/AcroForm << /Fields [12 0 R 13 0 R] >> /OpenAction << /S /URI /URI (http://example.com) /Next << /S /GoTo /D (chained) >> >> \
               /Names << /Dests << /Names [(aa) [5 0 R /Fit] (alpha) [5 0 R /Fit] (chained) [6 0 R /Fit] (orphan) [7 0 R /Fit] (target) [6 0 R /Fit]] >> >>";
    let extra = [
        "<< /FT /Tx /T (f1) /A << /S /GoTo /D (target) >> >>",
        "<< /FT /Btn /T (f2) /AA << /U << /S /GoTo /D (aa) >> >> >>",
    ];
    let (eng, id, _) = open("forms", cat, &extra);
    let l = dl(&eng, id);
    for n in ["aa", "chained", "target"] {
        assert_eq!(entry(&l, n).refs, 1, "{n}: {:?}", entry(&l, n).used_by);
    }
    assert_eq!(entry(&l, "orphan").refs, 0);
    assert!(!l.unchecked);

    let done = go(&eng, id, DestOp::RemoveUnused);
    assert_eq!(done.count, 1, "{:?}", done.kept);
    assert_eq!(names(&dl(&eng, id)), ["aa", "alpha", "chained", "target"]);

    go(&eng, id, DestOp::Rename { name: "target".into(), to: "t2".into() });
    go(&eng, id, DestOp::Rename { name: "aa".into(), to: "a2".into() });
    go(&eng, id, DestOp::Rename { name: "chained".into(), to: "c2".into() });
    assert!(raw(&eng, id, 12, "A").contains("(t2)"), "{}", raw(&eng, id, 12, "A"));
    assert!(raw(&eng, id, 13, "AA").contains("(a2)"), "{}", raw(&eng, id, 13, "AA"));
    assert!(raw(&eng, id, 1, "OpenAction").contains("(c2)"), "{}", raw(&eng, id, 1, "OpenAction"));
    assert_eq!(names(&dl(&eng, id)), ["a2", "alpha", "c2", "t2"]);
}

#[test]
fn a_name_only_another_destination_chains_to_is_kept() {
    // `via` is another name (a later hop); it makes `real` used
    let cat = "/Dests << /via /real /real [5 0 R /Fit] /spare [6 0 R /Fit] >>";
    let (eng, id, _) = open("chain", cat, &[]);
    let l = dl(&eng, id);
    assert_eq!(page_of(entry(&l, "via")), Some(0));
    assert_eq!(entry(&l, "real").refs, 1);
    go(&eng, id, DestOp::RemoveUnused);
    // `via` itself nothing uses; `real` stays only while it does, `spare` goes
    assert_eq!(names(&dl(&eng, id)), ["real"]);
}

#[test]
fn a_rename_follows_a_chained_destination() {
    let cat = "/Dests << /via /real /real [5 0 R /Fit] >>";
    let (eng, id, _) = open("chain_rename", cat, &[]);
    go(&eng, id, DestOp::Rename { name: "real".into(), to: "actual".into() });
    let l = dl(&eng, id);
    assert_eq!(names(&l), ["actual", "via"]);
    assert_eq!(page_of(entry(&l, "via")), Some(0));
}

#[test]
fn the_tree_stays_sorted_with_limits_through_inserts_and_deletes_over_several_levels() {
    // 1100 entries in one leaf: more than a two-level tree holds
    let (eng, id, _) = open("big_tree", &many(1100), &[]);
    assert_eq!(dl(&eng, id).items.len(), 1100);
    // an insert at the front, the middle and the end
    for n in ["a-first", "k0555x", "zzz-last"] {
        go(&eng, id, DestOp::Add { name: n.into(), page: 0, at: None, zoom: None });
        let (keys, depth) = check_tree(&eng, id);
        assert!(depth >= 2, "a tree of this size has inner nodes below the root: {depth}");
        assert!(keys.contains(&n.as_bytes().to_vec()));
    }
    assert_eq!(check_tree(&eng, id).0.len(), 1103);
    // deletes: one from a leaf, then a whole range (leaves that empty go away, limits follow)
    go(&eng, id, DestOp::Delete { name: "k0003".into() });
    for i in 100..400 {
        go(&eng, id, DestOp::Delete { name: format!("k{i:04}") });
    }
    let (keys, _) = check_tree(&eng, id);
    assert_eq!(keys.len(), 1103 - 301);
    assert!(!keys.contains(&b"k0200".to_vec()));
    // every one still resolves by name
    let l = dl(&eng, id);
    assert!(l.items.iter().all(|e| matches!(e.dest, Dest::Page { .. })));
    // a rename moves a key to its place
    go(&eng, id, DestOp::Rename { name: "k0900".into(), to: "a-second".into() });
    let (keys, _) = check_tree(&eng, id);
    assert!(keys.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(&keys[..2], [b"a-first".to_vec(), b"a-second".to_vec()]);
}

#[test]
fn a_two_level_tree_that_was_given_loses_an_emptied_leaf_and_all_names_leave_an_empty_root() {
    let cat = "/Names << /Dests 12 0 R >>";
    let extra = [
        "<< /Kids [13 0 R 14 0 R] >>",
        "<< /Limits [(a) (b)] /Names [(a) [5 0 R /Fit] (b) [5 0 R /Fit]] >>",
        "<< /Limits [(c) (c)] /Names [(c) [6 0 R /Fit]] >>",
    ];
    let (eng, id, _) = open("two_level", cat, &extra);
    assert_eq!(names(&dl(&eng, id)), ["a", "b", "c"]);
    go(&eng, id, DestOp::Delete { name: "c".into() });
    let (keys, _) = check_tree(&eng, id);
    assert_eq!(keys, vec![b"a".to_vec(), b"b".to_vec()]);
    for n in ["a", "b"] {
        go(&eng, id, DestOp::Delete { name: n.into() });
    }
    assert!(dl(&eng, id).items.is_empty());
    assert!(check_tree(&eng, id).0.is_empty());
    go(&eng, id, DestOp::Add { name: "again".into(), page: 0, at: None, zoom: None });
    assert_eq!(names(&dl(&eng, id)), ["again"]);
}

#[test]
fn a_name_renamed_into_another_sources_name_is_refused() {
    let cat = "/Dests << /alpha [5 0 R /Fit] >> /Names << /Dests << /Names [(beta) [6 0 R /Fit]] >> >>";
    let (eng, id, _) = open("clash", cat, &[]);
    assert!(refused(&eng, id, DestOp::Rename { name: "alpha".into(), to: "beta".into() }).contains("already exists"));
    assert!(refused(&eng, id, DestOp::Rename { name: "alpha".into(), to: "   ".into() }).contains("needs a name"));
    assert!(matches!(
        dests::apply(&eng, id, rev(&eng, id), &DestOp::Rename { name: "nothing".into(), to: "x".into() }, EditSource::User),
        Err(EngineError::Stale)
    ));
}

#[test]
fn a_stale_change_is_refused() {
    let (eng, id, _) = open("stale", TREE, &[]);
    let r = rev(&eng, id);
    go(&eng, id, DestOp::Delete { name: "beta".into() });
    let e = dests::apply(&eng, id, r, &DestOp::Delete { name: "gamma".into() }, EditSource::User);
    assert!(matches!(e, Err(EngineError::Stale)), "{e:?}");
}

#[test]
fn a_certified_document_refuses_every_change_and_says_so_in_the_list() {
    let cert = "/Perms << /DocMDP << /Type /Sig /Reference [ << /TransformMethod /DocMDP /TransformParams << /P 3 >> >> ] >> >>";
    let (eng, id, _) = open("certified", &format!("{TREE} {cert}"), &[]);
    let l = dl(&eng, id);
    assert!(l.locked.as_deref().is_some_and(|m| m.contains("certified")), "{:?}", l.locked);
    for op in [
        DestOp::Rename { name: "alpha".into(), to: "omega".into() },
        DestOp::SetView { name: "alpha".into(), page: 1, at: None, zoom: None },
        DestOp::Delete { name: "alpha".into() },
        DestOp::Add { name: "new".into(), page: 0, at: None, zoom: None },
        DestOp::RemoveUnused,
    ] {
        let why = refused(&eng, id, op);
        assert!(why.contains("certified"), "{why}");
    }
}

#[test]
fn a_signed_document_saves_incrementally() {
    let sig = "<< /FT /Sig /T (Sig1) /V << /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00> >> >>";
    let cat = format!("{TREE} /AcroForm << /SigFlags 3 /Fields [12 0 R] >>");
    let (eng, id, path) = open("signed", &cat, &[sig]);
    let l = dl(&eng, id);
    assert!(l.signed && l.locked.is_none());
    let original = std::fs::read(&path).unwrap();
    go(&eng, id, DestOp::Delete { name: "beta".into() });
    go(&eng, id, DestOp::Rename { name: "alpha".into(), to: "omega".into() });
    eng.save(id, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len() && saved.starts_with(&original), "the signed bytes were rewritten");
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(path.to_str().unwrap()).unwrap().doc;
    assert_eq!(names(&dl(&eng2, id2)), ["gamma", "omega"]);
}

#[test]
fn an_unsigned_document_is_rewritten_without_a_deleted_name() {
    let cat = "/Names << /Dests << /Names [(alpha) [5 0 R /Fit] (secretdest) [6 0 R /Fit]] >> >>";
    let (eng, id, path) = open("unsigned", cat, &[]);
    go(&eng, id, DestOp::Delete { name: "secretdest".into() });
    eng.save(id, None).unwrap();
    assert!(!String::from_utf8_lossy(&std::fs::read(&path).unwrap()).contains("secretdest"));
}

#[test]
fn a_tree_with_a_shape_no_writer_makes_is_listed_and_left_alone() {
    // a node reached from two places, a cycle back to the root, a child that is not indirect, an odd /Names
    let cases: [(&str, String, Vec<&str>); 4] = [
        ("shared", "/Names << /Dests 12 0 R >>".into(), vec!["<< /Kids [13 0 R 13 0 R] >>", "<< /Limits [(a) (a)] /Names [(a) [5 0 R /Fit]] >>"]),
        ("cycle", "/Names << /Dests 12 0 R >>".into(), vec!["<< /Kids [13 0 R] >>", "<< /Limits [(a) (a)] /Kids [12 0 R] /Names [(a) [5 0 R /Fit]] >>"]),
        ("direct", "/Names << /Dests 12 0 R >>".into(), vec!["<< /Kids [<< /Limits [(a) (a)] /Names [(a) [5 0 R /Fit]] >>] >>"]),
        ("odd", "/Names << /Dests 12 0 R >>".into(), vec!["<< /Names [(a) [5 0 R /Fit] (b)] >>"]),
    ];
    for (name, cat, extra) in cases {
        let (eng, id, _) = open(&format!("shape_{name}"), &cat, &extra);
        let l = dl(&eng, id);
        assert!(l.frozen.is_some(), "{name}");
        for op in [DestOp::Delete { name: "a".into() }, DestOp::Add { name: "z".into(), page: 0, at: None, zoom: None }, DestOp::RemoveUnused] {
            let why = refused(&eng, id, op);
            assert!(why.contains("leaves the named destinations as they are"), "{name}: {why}");
        }
    }
}

#[test]
fn caps_are_stated_and_nothing_is_edited_past_them() {
    // a chain of nodes deeper than MAX_DEPTH
    let depth = dests::MAX_DEPTH + 3;
    let mut extra: Vec<String> = Vec::new();
    for i in 0..depth {
        let me = 12 + i;
        extra.push(if i + 1 < depth {
            format!("<< /Kids [{} 0 R] /Limits [(a) (a)] >>", me + 1)
        } else {
            "<< /Limits [(a) (a)] /Names [(a) [5 0 R /Fit]] >>".to_string()
        });
    }
    let extra_ref: Vec<&str> = extra.iter().map(String::as_str).collect();
    let (eng, id, _) = open("deep", "/Names << /Dests 12 0 R >>", &extra_ref);
    let l = dl(&eng, id);
    assert!(l.truncated && l.frozen.as_deref().is_some_and(|m| m.contains("12 levels")), "{:?}", l.frozen);
    assert!(refused(&eng, id, DestOp::Delete { name: "a".into() }).contains("leaves the named destinations as they are"));

    // more entries than are read
    let (eng, id, _) = open("wide", &many(dests::MAX_ENTRIES + 1), &[]);
    let l = dl(&eng, id);
    assert!(l.truncated);
    assert_eq!(l.items.len(), dests::MAX_ENTRIES);
    assert!(refused(&eng, id, DestOp::Add { name: "zzz".into(), page: 0, at: None, zoom: None }).contains("at most 20000"));
}

/// Whatever resolved before resolves the same after every kind of change to other names.
#[test]
fn what_resolved_before_resolves_the_same_after_every_change() {
    let cat = "/Names << /Dests << /Names [(alpha) [5 0 R /Fit] (beta) [6 0 R /Fit] (gamma) [7 0 R /Fit]] >> >>";
    let (eng, id, _) = open("consistency", cat, &[]);
    let before = resolved(&eng, id);
    assert!(before.0.iter().all(|d| matches!(d, Dest::Page { .. })));
    go(&eng, id, DestOp::Add { name: "new".into(), page: 2, at: None, zoom: None });
    assert_eq!(resolved(&eng, id), before);
    go(&eng, id, DestOp::SetView { name: "gamma".into(), page: 0, at: Some([5.0, 5.0]), zoom: None });
    assert_eq!(resolved(&eng, id), before);
    go(&eng, id, DestOp::Rename { name: "beta".into(), to: "bravo".into() });
    assert_eq!(resolved(&eng, id), before);
    go(&eng, id, DestOp::Delete { name: "gamma".into() });
    assert_eq!(resolved(&eng, id), before);
    go(&eng, id, DestOp::RemoveUnused);
    assert_eq!(resolved(&eng, id), before);
    assert_eq!(names(&dl(&eng, id)), ["alpha"]);
    // and the one that is renamed carries its users along
    go(&eng, id, DestOp::Rename { name: "alpha".into(), to: "first".into() });
    assert_eq!(resolved(&eng, id), before);
}
