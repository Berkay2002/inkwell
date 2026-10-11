//! Reading order and tag editing: every operation is one undo step, survives save and reload, and
//! leaves the ParentTree in agreement with the tree (`tags::problems` is empty after each one).

use super::tests::{Spec, check_eng, el, file, marked, open};
use super::*;
use std::sync::Arc;
use tags::{TagPatch, problems};

const ROOT_TREE: &str = "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [102 0 R 103 0 R 104 0 R]]>> /ParentTreeNextKey 1>>";

/// Three paragraphs (P, MCIDs 0-2) drawn top to bottom, in a tree with a valid ParentTree.
pub(super) fn doc3() -> Spec {
    let content = format!("{}{}{}", marked("P", 0, 700), marked("P", 1, 650), marked("P", 2, 600));
    Spec {
        cat: "/StructTreeRoot 100 0 R /MarkInfo <</Marked true>>".into(),
        pages: vec![("/StructParents 0".into(), content)],
        objs: vec![
            (100, ROOT_TREE.into()),
            (101, "<</Type/StructElem /S/Document /P 100 0 R /K [102 0 R 103 0 R 104 0 R]>>".into()),
            el(102, "P", 101, "0"),
            el(103, "P", 101, "1"),
            el(104, "P", 101, "2"),
        ],
        ..Spec::default()
    }
}

fn doc_of(path: &str) -> PdfDocument {
    PdfDocument::open(path).unwrap()
}

/// What a screen reader walks: the (page, MCID) pairs in tree order.
fn walk(path: &str) -> Vec<(u32, i32)> {
    let d = doc_of(path);
    Tree::read(&d, &tree::page_index(&d).unwrap()).unwrap().order
}

fn region_xrefs(eng: &Engine, id: DocId, page: u32) -> Vec<i32> {
    reading_order_doc(eng, id, page).unwrap().regions.iter().map(|r| r.xref).collect()
}

fn assert_consistent(eng: &Engine, id: DocId) {
    let p = tag_problems_doc(eng, id).unwrap();
    assert!(p.is_empty(), "ParentTree disagrees with the tags: {p:?}");
}

/// Saves, reopens the file and runs `f` on the reopened engine.
fn reload(eng: &Engine, id: DocId, path: &str) -> (Arc<Engine>, DocId) {
    eng.save(id, None).unwrap();
    eng.close(id);
    open(path)
}

#[test]
fn the_reading_order_lists_regions_with_text_and_a_box() {
    let path = file("ro-list", &doc3());
    let (eng, id) = open(&path);
    let ro = reading_order_doc(&eng, id, 0).unwrap();
    assert!(ro.tagged && ro.orphans.is_empty());
    let r = &ro.regions;
    assert_eq!(r.iter().map(|r| r.xref).collect::<Vec<_>>(), [102, 103, 104]);
    assert_eq!(r.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), ["Item 0", "Item 1", "Item 2"]);
    assert!(r.iter().all(|r| r.role == "P" && r.depth == 1 && r.parent == 101 && r.editable));
    // Drawn at y = 700, 650, 600 up the page: from the top, 92, 142, 192 (12 pt text).
    let b = r[0].bbox.expect("text has a box");
    assert!((b[0] - 72.0).abs() < 1.0 && b[1] > 80.0 && b[3] < 100.0 && b[2] > b[0] + 20.0, "{b:?}");
    assert!(r[1].bbox.unwrap()[1] > b[3] && r[2].bbox.unwrap()[1] > r[1].bbox.unwrap()[3]);
    assert!(reading_order_doc(&eng, id, 5).is_err());
    let v = serde_json::to_value(&ro).unwrap();
    assert_eq!(v["regions"][0]["rawRole"], "P");
}

#[test]
fn a_tj_array_and_hex_strings_give_the_text_and_the_extent() {
    let content = "/P <</MCID 0>> BDC BT /F1 12 Tf 72 700 Td [(Hel) -20 (lo ) <776F726C64>] TJ ET EMC";
    let mut s = doc3();
    s.pages[0].1 = content.into();
    s.objs.truncate(3);
    s.objs[1].1 = "<</Type/StructElem /S/Document /P 100 0 R /K [102 0 R]>>".into();
    s.objs[0].1 = "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [102 0 R]]>> /ParentTreeNextKey 1>>".into();
    let path = file("ro-tj", &s);
    let (eng, id) = open(&path);
    let ro = reading_order_doc(&eng, id, 0).unwrap();
    assert_eq!(ro.regions[0].text, "Hello world");
}

#[test]
fn marked_content_nobody_points_at_is_listed_as_an_orphan() {
    let mut s = doc3();
    s.pages[0].1 = format!("{}{}", s.pages[0].1, marked("P", 7, 500));
    let path = file("ro-orphan", &s);
    let (eng, id) = open(&path);
    let ro = reading_order_doc(&eng, id, 0).unwrap();
    assert_eq!(ro.orphans.len(), 1);
    assert_eq!((ro.orphans[0].mcid, ro.orphans[0].text.as_str()), (7, "Item 7"));
}

#[test]
fn a_reorder_is_one_step_and_survives_save_reload_and_matches_the_structure_walk() {
    let path = file("ro-reorder", &doc3());
    let (eng, id) = open(&path);
    assert_consistent(&eng, id);
    let steps = eng.history(id).unwrap().len();
    reorder_doc(&eng, id, 0, &[104, 102, 103], EditSource::User).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + 1);
    assert_eq!(eng.history(id).unwrap().last().unwrap().label, "Change reading order");
    assert_eq!(region_xrefs(&eng, id, 0), [104, 102, 103]);
    assert_consistent(&eng, id);
    // The tree order is also what the checker's reading-order rule sees: MCID 2 comes first.
    let r = check_eng(&eng, id);
    assert_eq!(r.status(Rule::ReadingOrder), Status::Failed);

    eng.undo(id).unwrap();
    assert_eq!(region_xrefs(&eng, id, 0), [102, 103, 104]);
    eng.redo(id).unwrap();

    let (eng2, id2) = reload(&eng, id, &path);
    assert_eq!(region_xrefs(&eng2, id2, 0), [104, 102, 103]);
    assert_eq!(walk(&path), [(0, 2), (0, 0), (0, 1)], "the structure walk reads the new order");
    assert_consistent(&eng2, id2);
    let texts: Vec<String> = reading_order_doc(&eng2, id2, 0).unwrap().regions.into_iter().map(|r| r.text).collect();
    assert_eq!(texts, ["Item 2", "Item 0", "Item 1"]);
}

#[test]
fn reordering_moves_only_what_has_to_move() {
    let path = file("ro-minimal", &doc3());
    let (eng, id) = open(&path);
    // 102 stays; 104 and 103 swap.
    reorder_doc(&eng, id, 0, &[102, 104, 103], EditSource::User).unwrap();
    assert_eq!(walk(&path), [(0, 0), (0, 1), (0, 2)], "the file is unchanged until saved");
    eng.save(id, None).unwrap();
    assert_eq!(walk(&path), [(0, 0), (0, 2), (0, 1)]);
    // The identity order and a wrong set are refused.
    let e = reorder_doc(&eng, id, 0, &[102, 104, 103], EditSource::User).unwrap_err();
    assert!(e.to_string().contains("already"), "{e}");
    assert!(reorder_doc(&eng, id, 0, &[102, 104], EditSource::User).is_err());
    assert!(reorder_doc(&eng, id, 0, &[102, 102, 104], EditSource::User).is_err());
}

#[test]
fn a_reversed_order_works_for_any_number_of_regions() {
    let n = 7;
    let mut content = String::new();
    let mut kids = String::new();
    let mut objs = vec![];
    for i in 0..n {
        content += &marked("P", i, 700 - 20 * i);
        kids += &format!("{} 0 R ", 110 + i);
        objs.push(el(110 + i as usize, "P", 101, &i.to_string()));
    }
    let nums: String = (0..n).map(|i| format!("{} 0 R ", 110 + i)).collect();
    objs.push((100, format!("<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [{nums}]]>> /ParentTreeNextKey 1>>")));
    objs.push((101, format!("<</Type/StructElem /S/Document /P 100 0 R /K [{kids}]>>")));
    let s = Spec { cat: "/StructTreeRoot 100 0 R /MarkInfo <</Marked true>>".into(), pages: vec![("/StructParents 0".into(), content)], objs, ..Spec::default() };
    let path = file("ro-reverse", &s);
    let (eng, id) = open(&path);
    let rev: Vec<i32> = (0..n).rev().map(|i| 110 + i).collect();
    reorder_doc(&eng, id, 0, &rev, EditSource::User).unwrap();
    assert_eq!(region_xrefs(&eng, id, 0), rev);
    assert_consistent(&eng, id);
    let (_, _) = reload(&eng, id, &path);
    assert_eq!(walk(&path).iter().map(|x| x.1).collect::<Vec<_>>(), (0..n).rev().collect::<Vec<_>>());
}

#[test]
fn a_region_inside_a_list_keeps_its_place_in_the_tree_when_others_move() {
    // Document > P(102), L(105) > LI(106) > LBody(107, MCID 1), P(104, MCID 2)
    let content = format!("{}{}{}", marked("P", 0, 700), marked("LBody", 1, 650), marked("P", 2, 600));
    let s = Spec {
        cat: "/StructTreeRoot 100 0 R /MarkInfo <</Marked true>>".into(),
        pages: vec![("/StructParents 0".into(), content)],
        objs: vec![
            (100, "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [102 0 R 107 0 R 104 0 R]]>> /ParentTreeNextKey 1>>".into()),
            (101, "<</Type/StructElem /S/Document /P 100 0 R /K [102 0 R 105 0 R 104 0 R]>>".into()),
            el(102, "P", 101, "0"),
            (105, "<</Type/StructElem /S/L /P 101 0 R /Pg 10 0 R /K [106 0 R]>>".into()),
            (106, "<</Type/StructElem /S/LI /P 105 0 R /Pg 10 0 R /K [107 0 R]>>".into()),
            el(107, "LBody", 106, "1"),
            el(104, "P", 101, "2"),
        ],
        ..Spec::default()
    };
    let path = file("ro-nested", &s);
    let (eng, id) = open(&path);
    assert_eq!(region_xrefs(&eng, id, 0), [102, 107, 104]);
    reorder_doc(&eng, id, 0, &[102, 104, 107], EditSource::User).unwrap();
    assert_eq!(region_xrefs(&eng, id, 0), [102, 104, 107]);
    assert_consistent(&eng, id);
    // The list is still a list: only the paragraph moved.
    let tags = tags_doc(&eng, id, None).unwrap();
    let doc = &tags[0];
    let kinds: Vec<&str> = doc.children.iter().map(|c| c.role.as_str()).collect();
    assert_eq!(kinds, ["P", "P", "L"]);
}

#[test]
fn a_role_change_is_one_step_and_survives_a_reload() {
    let path = file("role", &doc3());
    let (eng, id) = open(&path);
    let steps = eng.history(id).unwrap().len();
    set_role_doc(&eng, id, 103, "H2", EditSource::User).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + 1);
    assert_eq!(eng.history(id).unwrap().last().unwrap().label, "Change tag to H2");
    let ro = reading_order_doc(&eng, id, 0).unwrap();
    assert_eq!(ro.regions[1].role, "H2");
    // The heading rule now sees an H2 that skips H1.
    assert_eq!(check_eng(&eng, id).status(Rule::HeadingNesting), Status::Failed);
    // The same role again, an unknown type and a non-element are refused.
    assert!(set_role_doc(&eng, id, 103, "H2", EditSource::User).is_err());
    assert!(set_role_doc(&eng, id, 103, "Banana", EditSource::User).is_err());
    assert!(set_role_doc(&eng, id, 3, "P", EditSource::User).is_err());
    eng.undo(id).unwrap();
    assert_eq!(reading_order_doc(&eng, id, 0).unwrap().regions[1].role, "P");
    eng.redo(id).unwrap();
    let (eng2, id2) = reload(&eng, id, &path);
    assert_eq!(reading_order_doc(&eng2, id2, 0).unwrap().regions[1].role, "H2");
    assert_consistent(&eng2, id2);
    for (role, ok) in [("Figure", true), ("Table", true), ("L", true), ("LI", true), ("H6", true), ("P", true)] {
        assert_eq!(set_role_doc(&eng2, id2, 103, role, EditSource::User).is_ok(), ok, "{role}");
    }
}

#[test]
fn a_custom_role_in_the_role_map_is_allowed() {
    let mut s = doc3();
    s.objs[0].1 = "<</Type/StructTreeRoot /K 101 0 R /RoleMap <</Para /P>> /ParentTree <</Nums [0 [102 0 R 103 0 R 104 0 R]]>> /ParentTreeNextKey 1>>".into();
    let path = file("role-map", &s);
    let (eng, id) = open(&path);
    set_role_doc(&eng, id, 102, "Para", EditSource::User).unwrap();
    let r = &reading_order_doc(&eng, id, 0).unwrap().regions[0];
    assert_eq!((r.role.as_str(), r.raw_role.as_str()), ("P", "Para"));
}

#[test]
fn marking_a_region_as_artifact_takes_its_content_out_of_the_tree() {
    let path = file("artifact", &doc3());
    let (eng, id) = open(&path);
    let steps = eng.history(id).unwrap().len();
    artifact_doc(&eng, id, 103, EditSource::User).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + 1);
    assert_eq!(eng.history(id).unwrap().last().unwrap().label, "Mark as artifact");
    let ro = reading_order_doc(&eng, id, 0).unwrap();
    assert_eq!(ro.regions.iter().map(|r| r.xref).collect::<Vec<_>>(), [102, 104]);
    assert!(ro.orphans.is_empty(), "the content is an artifact now, not untagged");
    assert_consistent(&eng, id);
    // The checker sees nothing left outside the tree.
    assert_eq!(check_eng(&eng, id).status(Rule::ReadingOrder), Status::Passed);
    eng.undo(id).unwrap();
    assert_eq!(region_xrefs(&eng, id, 0), [102, 103, 104]);
    eng.redo(id).unwrap();

    let (eng2, id2) = reload(&eng, id, &path);
    assert_eq!(walk(&path), [(0, 0), (0, 2)]);
    assert_eq!(region_xrefs(&eng2, id2, 0), [102, 104]);
    assert_consistent(&eng2, id2);
    // The text is still on the page.
    let lines: Vec<String> = eng2.page_text(id2, 0).unwrap().into_iter().map(|l| l.t).collect();
    assert!(lines.iter().any(|l| l.contains("Item 1")), "{lines:?}");
    let d = doc_of(&path);
    let page = d.find_page(0).unwrap();
    let data = String::from_utf8(page_content(&page).unwrap()).unwrap();
    assert!(data.contains("/Artifact BMC") && !data.contains("<</MCID 1>>"), "{data}");
    // An artifact has no role of its own to change back to: the element is gone.
    assert!(artifact_doc(&eng2, id2, 103, EditSource::User).is_err());
}

#[test]
fn changing_the_role_to_artifact_is_the_same_edit() {
    let path = file("role-artifact", &doc3());
    let (eng, id) = open(&path);
    set_role_doc(&eng, id, 102, "Artifact", EditSource::User).unwrap();
    assert_eq!(region_xrefs(&eng, id, 0), [103, 104]);
    assert_consistent(&eng, id);
}

#[test]
fn artifacting_a_branch_takes_all_of_it_and_a_links_struct_parent() {
    let content = format!("{}{}", marked("P", 0, 700), marked("P", 1, 650));
    let s = Spec {
        cat: "/StructTreeRoot 100 0 R /MarkInfo <</Marked true>>".into(),
        pages: vec![("/StructParents 0 /Annots [120 0 R]".into(), content)],
        objs: vec![
            (100, "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [103 0 R 104 0 R] 1 105 0 R]>> /ParentTreeNextKey 2>>".into()),
            (101, "<</Type/StructElem /S/Document /P 100 0 R /K [102 0 R]>>".into()),
            (102, "<</Type/StructElem /S/Sect /P 101 0 R /Pg 10 0 R /K [103 0 R 104 0 R 105 0 R]>>".into()),
            el(103, "P", 102, "0"),
            el(104, "P", 102, "1"),
            (105, "<</Type/StructElem /S/Link /P 102 0 R /Pg 10 0 R /K [<</Type/OBJR /Obj 120 0 R /Pg 10 0 R>>]>>".into()),
            (120, "<</Type/Annot /Subtype/Link /Rect [72 640 180 660] /Border [0 0 0] /StructParent 1 /A <</S/URI /URI (https://example.com)>>>>".into()),
        ],
        ..Spec::default()
    };
    let path = file("artifact-branch", &s);
    let (eng, id) = open(&path);
    assert_consistent(&eng, id);
    artifact_doc(&eng, id, 102, EditSource::User).unwrap();
    assert!(region_xrefs(&eng, id, 0).is_empty());
    assert_consistent(&eng, id);
    let (_, _) = reload(&eng, id, &path);
    let d = doc_of(&path);
    let annot = d.new_indirect(120, 0).unwrap();
    assert!(annot.get_dict("StructParent").unwrap().is_none(), "the link has left the tree");
    assert!(walk(&path).is_empty());
}

#[test]
fn merging_two_regions_joins_their_content_in_tree_order() {
    let path = file("merge", &doc3());
    let (eng, id) = open(&path);
    let steps = eng.history(id).unwrap().len();
    merge_doc(&eng, id, 102, 103, EditSource::User).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + 1);
    let ro = reading_order_doc(&eng, id, 0).unwrap();
    assert_eq!(ro.regions.iter().map(|r| r.xref).collect::<Vec<_>>(), [102, 104]);
    assert_eq!(ro.regions[0].mcids, [0, 1]);
    assert_eq!(ro.regions[0].text, "Item 0 Item 1");
    assert_consistent(&eng, id);
    // Merging into the later one puts the earlier content first.
    eng.undo(id).unwrap();
    merge_doc(&eng, id, 104, 102, EditSource::User).unwrap();
    let ro = reading_order_doc(&eng, id, 0).unwrap();
    assert_eq!(ro.regions.iter().map(|r| (r.xref, r.mcids.clone())).collect::<Vec<_>>(), [(103, vec![1]), (104, vec![0, 2])]);
    assert_consistent(&eng, id);
    let (eng2, id2) = reload(&eng, id, &path);
    assert_consistent(&eng2, id2);
    assert!(merge_doc(&eng2, id2, 104, 104, EditSource::User).is_err());
}

#[test]
fn merging_three_regions_is_one_undo_step() {
    let path = file("merge-many", &doc3());
    let (eng, id) = open(&path);
    let steps = eng.history(id).unwrap().len();
    merge_many_doc(&eng, id, 102, &[103, 104], EditSource::User).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + 1);
    let ro = reading_order_doc(&eng, id, 0).unwrap();
    assert_eq!(ro.regions.iter().map(|r| (r.xref, r.mcids.clone())).collect::<Vec<_>>(), [(102, vec![0, 1, 2])]);
    assert_consistent(&eng, id);
    eng.undo(id).unwrap();
    assert_eq!(region_xrefs(&eng, id, 0), [102, 103, 104]);
    // A refusal part way (a missing region) leaves the document as it was and adds no step.
    let steps = eng.history(id).unwrap().len();
    assert!(merge_many_doc(&eng, id, 102, &[103, 999], EditSource::User).is_err());
    assert_eq!(eng.history(id).unwrap().len(), steps);
    assert_eq!(region_xrefs(&eng, id, 0), [102, 103, 104]);
    assert!(merge_many_doc(&eng, id, 102, &[], EditSource::User).is_err());
}

#[test]
fn splitting_a_region_makes_a_sibling_with_the_rest() {
    let mut s = doc3();
    s.pages[0].1 = "/P <</MCID 0>> BDC BT /F1 12 Tf 72 700 Td (One) Tj ET EMC /P <</MCID 1>> BDC BT /F1 12 Tf 72 680 Td (Two) Tj ET EMC /P <</MCID 2>> BDC BT /F1 12 Tf 72 660 Td (Three) Tj ET EMC".into();
    s.objs[2] = el(102, "P", 101, "[0 1 2]");
    s.objs.truncate(3);
    s.objs[0].1 = "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [102 0 R 102 0 R 102 0 R]]>> /ParentTreeNextKey 1>>".into();
    s.objs[1].1 = "<</Type/StructElem /S/Document /P 100 0 R /K [102 0 R]>>".into();
    let path = file("split", &s);
    let (eng, id) = open(&path);
    assert_consistent(&eng, id);
    let steps = eng.history(id).unwrap().len();
    split_doc(&eng, id, 102, 1, Some("H1"), EditSource::User).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + 1);
    let ro = reading_order_doc(&eng, id, 0).unwrap();
    assert_eq!(ro.regions.len(), 2);
    assert_eq!((ro.regions[0].xref, ro.regions[0].mcids.clone(), ro.regions[0].role.as_str()), (102, vec![0], "P"));
    assert_eq!((ro.regions[1].mcids.clone(), ro.regions[1].role.as_str()), (vec![1, 2], "H1"));
    assert_eq!(ro.regions[1].parent, 101);
    assert_eq!(ro.regions[1].text, "Two Three");
    assert_consistent(&eng, id);
    let (eng2, id2) = reload(&eng, id, &path);
    assert_eq!(walk(&path), [(0, 0), (0, 1), (0, 2)]);
    assert_consistent(&eng2, id2);
    // Nothing would stay in the first part; an MCID the element lacks.
    assert!(split_doc(&eng2, id2, 102, 0, None, EditSource::User).is_err());
    assert!(split_doc(&eng2, id2, 102, 9, None, EditSource::User).is_err());
}

fn table_doc() -> Spec {
    // Table 102 > TR 103 > TH 104 (MCID 0), TD 105 (MCID 1)
    let content = format!("{}{}", marked("TH", 0, 700), marked("TD", 1, 680));
    Spec {
        cat: "/StructTreeRoot 100 0 R /MarkInfo <</Marked true>>".into(),
        pages: vec![("/StructParents 0".into(), content)],
        objs: vec![
            (100, "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [104 0 R 105 0 R]]>> /ParentTreeNextKey 1>>".into()),
            (101, "<</Type/StructElem /S/Document /P 100 0 R /K [102 0 R]>>".into()),
            (102, "<</Type/StructElem /S/Table /P 101 0 R /Pg 10 0 R /K [103 0 R]>>".into()),
            (103, "<</Type/StructElem /S/TR /P 102 0 R /Pg 10 0 R /K [104 0 R 105 0 R]>>".into()),
            el(104, "TH", 103, "0"),
            el(105, "TD", 103, "1"),
        ],
        ..Spec::default()
    }
}

#[test]
fn tag_attributes_are_edited_one_step_each_and_kept_by_save() {
    let path = file("tags", &table_doc());
    let (eng, id) = open(&path);
    let steps = eng.history(id).unwrap().len();
    let patch = |f: &dyn Fn(&mut TagPatch)| {
        let mut p = TagPatch::default();
        f(&mut p);
        p
    };
    edit_tag_doc(&eng, id, 105, &patch(&|p| p.alt = Some("Revenue, 2026".into())), EditSource::User).unwrap();
    edit_tag_doc(&eng, id, 105, &patch(&|p| p.actual_text = Some("USD 4.2M".into())), EditSource::User).unwrap();
    edit_tag_doc(&eng, id, 105, &patch(&|p| p.lang = Some("sv-SE".into())), EditSource::User).unwrap();
    edit_tag_doc(&eng, id, 104, &patch(&|p| p.scope = Some("Column".into())), EditSource::User).unwrap();
    edit_tag_doc(&eng, id, 105, &patch(&|p| p.headers = Some(vec![104])), EditSource::User).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + 5, "one step each");
    let t = tags_doc(&eng, id, None).unwrap();
    let cells = &t[0].children[0].children[0].children;
    let (th, td) = (&cells[0], &cells[1]);
    assert_eq!(td.alt.as_deref(), Some("Revenue, 2026"));
    assert_eq!(td.actual_text.as_deref(), Some("USD 4.2M"));
    assert_eq!(td.lang.as_deref(), Some("sv-SE"));
    assert_eq!(th.scope.as_deref(), Some("Column"));
    assert_eq!(td.headers, [104], "the header cell is named by its ID");
    assert!(th.id.is_some());

    let (eng2, id2) = reload(&eng, id, &path);
    let t = tags_doc(&eng2, id2, None).unwrap();
    let cells = &t[0].children[0].children[0].children;
    assert_eq!(cells[1].alt.as_deref(), Some("Revenue, 2026"));
    assert_eq!(cells[1].headers, [104]);
    assert_eq!(cells[0].scope.as_deref(), Some("Column"));
    // Text is removed by an empty value; a no-op, bad values and misplaced attributes are refused.
    edit_tag_doc(&eng2, id2, 105, &patch(&|p| p.alt = Some(String::new())), EditSource::User).unwrap();
    assert_eq!(tags_doc(&eng2, id2, None).unwrap()[0].children[0].children[0].children[1].alt, None);
    assert!(edit_tag_doc(&eng2, id2, 105, &TagPatch::default(), EditSource::User).is_err());
    assert!(edit_tag_doc(&eng2, id2, 105, &patch(&|p| p.lang = Some("not a tag!".into())), EditSource::User).is_err());
    assert!(edit_tag_doc(&eng2, id2, 105, &patch(&|p| p.scope = Some("Row".into())), EditSource::User).is_err(), "only TH has a scope");
    assert!(edit_tag_doc(&eng2, id2, 104, &patch(&|p| p.scope = Some("Diagonal".into())), EditSource::User).is_err());
    assert!(edit_tag_doc(&eng2, id2, 105, &patch(&|p| p.headers = Some(vec![105])), EditSource::User).is_err(), "headers are TH cells");
    assert!(edit_tag_doc(&eng2, id2, 102, &patch(&|p| p.headers = Some(vec![104])), EditSource::User).is_err(), "only cells list headers");
    // A role in the same patch.
    edit_tag_doc(&eng2, id2, 105, &patch(&|p| p.role = Some("TH".into())), EditSource::User).unwrap();
    assert_eq!(tags_doc(&eng2, id2, None).unwrap()[0].children[0].children[0].children[1].role, "TH");
}

#[test]
fn an_existing_attribute_array_is_extended_not_replaced() {
    let mut s = table_doc();
    s.objs[4] = (104, "<</Type/StructElem /S/TH /P 103 0 R /Pg 10 0 R /K 0 /A [<</O/Layout /Placement/Block>> 1]>>".into());
    let path = file("tags-attr-array", &s);
    let (eng, id) = open(&path);
    edit_tag_doc(&eng, id, 104, &TagPatch { scope: Some("Both".into()), ..TagPatch::default() }, EditSource::User).unwrap();
    let (_, _) = reload(&eng, id, &path);
    let d = doc_of(&path);
    let th = d.new_indirect(104, 0).unwrap();
    let a = th.get_dict("A").unwrap().unwrap();
    assert!(a.is_array().unwrap());
    let layout = a.get_array(0).unwrap().unwrap();
    assert_eq!(name(&layout, "O").unwrap().as_deref(), Some("Layout"), "the Layout attributes are kept");
    let t = Tree::read(&d, &tree::page_index(&d).unwrap()).unwrap();
    assert_eq!(t.elems.iter().find(|e| e.xref == 104).unwrap().attrs.get("Scope").map(|s| s.trim_start_matches('/')), Some("Both"));
}

#[test]
fn a_node_moves_to_another_parent_and_the_tree_stays_consistent() {
    let mut s = doc3();
    s.objs.push((105, "<</Type/StructElem /S/Sect /P 101 0 R /Pg 10 0 R /K []>>".into()));
    s.objs[1].1 = "<</Type/StructElem /S/Document /P 100 0 R /K [102 0 R 103 0 R 104 0 R 105 0 R]>>".into();
    let path = file("move", &s);
    let (eng, id) = open(&path);
    let steps = eng.history(id).unwrap().len();
    move_tag_doc(&eng, id, 102, 105, None, false, EditSource::User).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + 1);
    let t = tags_doc(&eng, id, None).unwrap();
    let kinds: Vec<(i32, usize)> = t[0].children.iter().map(|c| (c.xref, c.children.len())).collect();
    assert_eq!(kinds, [(103, 0), (104, 0), (105, 1)]);
    assert_eq!(t[0].children[2].children[0].xref, 102);
    assert_consistent(&eng, id);
    // Not into itself or its own kid.
    assert!(move_tag_doc(&eng, id, 105, 102, None, false, EditSource::User).is_err());
    assert!(move_tag_doc(&eng, id, 105, 105, None, false, EditSource::User).is_err());
    // To the top of the tree, first.
    move_tag_doc(&eng, id, 104, 0, Some(101), false, EditSource::User).unwrap();
    assert_eq!(tags_doc(&eng, id, None).unwrap()[0].xref, 104);
    let (eng2, id2) = reload(&eng, id, &path);
    assert_consistent(&eng2, id2);
    assert_eq!(walk(&path), [(0, 2), (0, 1), (0, 0)]);
}

#[test]
fn moving_a_node_keeps_its_page_when_it_relied_on_an_inherited_one() {
    // Two pages; element 112 has no /Pg of its own (page 1 comes from its parent 111).
    let mut s = doc3();
    s.pages.push(("/StructParents 1".into(), marked("P", 0, 700)));
    s.objs[0].1 = "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [102 0 R 103 0 R 104 0 R] 1 [112 0 R]]>> /ParentTreeNextKey 2>>".into();
    s.objs[1].1 = "<</Type/StructElem /S/Document /P 100 0 R /K [102 0 R 103 0 R 104 0 R 111 0 R]>>".into();
    s.objs.push((111, "<</Type/StructElem /S/Sect /P 101 0 R /Pg 12 0 R /K [112 0 R]>>".into()));
    s.objs.push((112, "<</Type/StructElem /S/P /P 111 0 R /K 0>>".into()));
    let path = file("move-pg", &s);
    let (eng, id) = open(&path);
    assert_consistent(&eng, id);
    move_tag_doc(&eng, id, 112, 101, Some(102), false, EditSource::User).unwrap();
    assert_consistent(&eng, id);
    let (_, _) = reload(&eng, id, &path);
    let order = walk(&path);
    assert_eq!(order[0], (1, 0), "{order:?}");
}

#[test]
fn the_validator_finds_a_broken_parent_tree_and_repair_rebuilds_it() {
    let mut s = doc3();
    // MCID 1 is mapped to the wrong element, the page has no ParentTree entry for MCID 2.
    s.objs[0].1 = "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [102 0 R 102 0 R]]>>>>".into();
    let path = file("repair", &s);
    let (eng, id) = open(&path);
    let p = tag_problems_doc(&eng, id).unwrap();
    assert!(p.iter().any(|x| x.message.contains("MCID 1")) && p.iter().any(|x| x.message.contains("MCID 2")), "{p:?}");
    assert!(p.iter().any(|x| x.message.contains("ParentTreeNextKey")));
    let steps = eng.history(id).unwrap().len();
    fix_doc(&eng, id, &Fix::ParentTree, EditSource::User).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + 1);
    assert_consistent(&eng, id);
    let (eng2, id2) = reload(&eng, id, &path);
    assert_consistent(&eng2, id2);
    assert!(fix_doc(&eng2, id2, &Fix::ParentTree, EditSource::User).is_err(), "nothing to repair");
}

#[test]
fn a_parent_tree_in_kids_form_is_read_and_flattened_on_write() {
    let mut s = doc3();
    s.objs[0].1 = "<</Type/StructTreeRoot /K 101 0 R /ParentTree 130 0 R /ParentTreeNextKey 1>>".into();
    s.objs.push((130, "<</Kids [131 0 R] /Limits [0 0]>>".into()));
    s.objs.push((131, "<</Nums [0 [102 0 R 103 0 R 104 0 R]] /Limits [0 0]>>".into()));
    let path = file("ptree-kids", &s);
    let (eng, id) = open(&path);
    assert_consistent(&eng, id);
    artifact_doc(&eng, id, 102, EditSource::User).unwrap();
    assert_consistent(&eng, id);
    let (eng2, id2) = reload(&eng, id, &path);
    assert_eq!(region_xrefs(&eng2, id2, 0), [103, 104]);
}

#[test]
fn an_untagged_document_has_nothing_to_edit() {
    let path = file("untagged", &Spec::default());
    let (eng, id) = open(&path);
    let ro = reading_order_doc(&eng, id, 0).unwrap();
    assert!(!ro.tagged && ro.regions.is_empty());
    assert!(tags_doc(&eng, id, None).unwrap().is_empty());
    assert!(set_role_doc(&eng, id, 5, "P", EditSource::User).is_err());
    assert!(problems(&open_copy(&eng, id).unwrap()).unwrap().is_empty());
}

#[test]
fn the_tags_of_one_page_leave_out_the_other_branches() {
    let mut s = doc3();
    s.pages.push(("/StructParents 1".into(), marked("P", 0, 700)));
    s.objs[0].1 = "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [102 0 R 103 0 R 104 0 R] 1 [112 0 R]]>> /ParentTreeNextKey 2>>".into();
    s.objs[1].1 = "<</Type/StructElem /S/Document /P 100 0 R /K [102 0 R 103 0 R 104 0 R 111 0 R]>>".into();
    s.objs.push((111, "<</Type/StructElem /S/Sect /P 101 0 R /Pg 12 0 R /K [112 0 R]>>".into()));
    s.objs.push((112, "<</Type/StructElem /S/P /P 111 0 R /K 0>>".into()));
    let path = file("tags-page", &s);
    let (eng, id) = open(&path);
    let p1 = tags_doc(&eng, id, Some(0)).unwrap();
    assert_eq!(p1[0].children.iter().map(|c| c.xref).collect::<Vec<_>>(), [102, 103, 104]);
    let p2 = tags_doc(&eng, id, Some(1)).unwrap();
    assert_eq!(p2[0].children.iter().map(|c| c.xref).collect::<Vec<_>>(), [111]);
    assert_eq!(p2[0].children[0].children[0].xref, 112);
    assert_eq!(region_xrefs(&eng, id, 1), [112]);
}

#[test]
fn merging_across_pages_keeps_each_items_page() {
    let mut s = doc3();
    s.pages.push(("/StructParents 1".into(), marked("P", 0, 700)));
    s.objs[0].1 = "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [102 0 R 103 0 R 104 0 R] 1 [111 0 R]]>> /ParentTreeNextKey 2>>".into();
    s.objs[1].1 = "<</Type/StructElem /S/Document /P 100 0 R /K [102 0 R 103 0 R 104 0 R 111 0 R]>>".into();
    s.objs.push((111, "<</Type/StructElem /S/P /P 101 0 R /Pg 12 0 R /K 0>>".into()));
    let path = file("merge-pages", &s);
    let (eng, id) = open(&path);
    merge_doc(&eng, id, 104, 111, EditSource::User).unwrap();
    assert_consistent(&eng, id);
    let (eng2, id2) = reload(&eng, id, &path);
    assert_consistent(&eng2, id2);
    assert_eq!(walk(&path), [(0, 0), (0, 1), (0, 2), (1, 0)]);
    let on_p2 = reading_order_doc(&eng2, id2, 1).unwrap();
    assert_eq!((on_p2.regions[0].xref, on_p2.regions[0].text.as_str()), (104, "Item 0"));
    let on_p1 = reading_order_doc(&eng2, id2, 0).unwrap();
    assert_eq!(on_p1.regions.last().unwrap().mcids, [2]);
}

#[test]
fn autotagged_documents_can_be_edited_and_stay_consistent() {
    let s = super::tests::untagged_article_for_structure();
    let path = file("autotag-edit", &s);
    let (eng, id) = open(&path);
    fix_doc(&eng, id, &Fix::Autotag, EditSource::User).unwrap();
    assert_consistent(&eng, id);
    let ro = reading_order_doc(&eng, id, 0).unwrap();
    assert!(ro.regions.len() >= 4, "{:?}", ro.regions.iter().map(|r| (&r.role, &r.text)).collect::<Vec<_>>());
    let mut order: Vec<i32> = ro.regions.iter().map(|r| r.xref).collect();
    order.reverse();
    // Whole elements move; a list's items are separate regions inside it, so a full reversal may be
    // impossible. Either way the document is left consistent.
    match reorder_doc(&eng, id, 0, &order, EditSource::User) {
        Ok(_) => assert_eq!(region_xrefs(&eng, id, 0), order),
        Err(_) => assert_eq!(region_xrefs(&eng, id, 0), ro.regions.iter().map(|r| r.xref).collect::<Vec<_>>()),
    }
    assert_consistent(&eng, id);
    let first = ro.regions[0].xref;
    artifact_doc(&eng, id, first, EditSource::User).unwrap();
    assert_consistent(&eng, id);
    let r = check_eng(&eng, id);
    assert_eq!(r.status(Rule::ParentTree), Status::Passed, "{:?}", r.result(Rule::ParentTree).findings);
    let (eng2, id2) = reload(&eng, id, &path);
    assert_consistent(&eng2, id2);
}

#[test]
fn q_restores_the_text_state_for_the_next_region() {
    // The first region sets 24 pt in F1 inside q ... Q; the second relies on the 12 pt set before q.
    let content = "BT /F1 12 Tf ET /P <</MCID 0>> BDC q BT /F1 24 Tf 72 700 Td (Big) Tj ET Q EMC /P <</MCID 1>> BDC BT 72 650 Td (Small) Tj ET EMC";
    let mut s = doc3();
    s.pages[0].1 = content.into();
    s.objs.truncate(4);
    s.objs[1].1 = "<</Type/StructElem /S/Document /P 100 0 R /K [102 0 R 103 0 R]>>".into();
    s.objs[0].1 = "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [102 0 R 103 0 R]]>> /ParentTreeNextKey 1>>".into();
    let path = file("q-state", &s);
    let (eng, id) = open(&path);
    let r = reading_order_doc(&eng, id, 0).unwrap().regions;
    let h = |i: usize| r[i].bbox.map(|b| b[3] - b[1]).unwrap();
    // 24 pt text is about twice as tall as 12 pt; the second region must be measured at 12 pt.
    assert!(h(0) > 1.7 * h(1), "{} vs {}", h(0), h(1));
    assert_eq!(r[1].text, "Small");
}

#[test]
fn a_tag_moves_beside_a_sibling_in_a_parent_that_mixes_content_and_elements() {
    // Document /K [mcid 0, A, mcid 2, B]: the raw array holds content items between the elements.
    let content: String = (0..4).map(|m| marked("P", m, 700 - 50 * m)).collect();
    let s = Spec {
        cat: "/StructTreeRoot 100 0 R /MarkInfo <</Marked true>>".into(),
        pages: vec![("/StructParents 0".into(), content)],
        objs: vec![
            (100, "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [101 0 R 102 0 R 101 0 R 103 0 R]]>> /ParentTreeNextKey 1>>".into()),
            (101, "<</Type/StructElem /S/Document /P 100 0 R /Pg 10 0 R /K [0 102 0 R 2 103 0 R]>>".into()),
            el(102, "P", 101, "1"),
            el(103, "P", 101, "3"),
        ],
        ..Spec::default()
    };
    let path = file("move-mixed", &s);
    let (eng, id) = open(&path);
    assert_consistent(&eng, id);
    let tree_order = |eng: &Engine, id: DocId| -> Vec<i32> {
        eng.save(id, None).unwrap();
        walk(&path).into_iter().map(|(_, m)| m).collect()
    };
    let mv = |xref, parent, anchor, after| move_tag_doc(&eng, id, xref, parent, anchor, after, EditSource::User);
    // Down: A after B, past the content item between them.
    mv(102, 101, Some(103), true).unwrap();
    assert_eq!(tree_order(&eng, id), [0, 2, 3, 1]);
    eng.undo(id).unwrap();
    // Up: B before A, ahead of the content item that sits before A.
    mv(103, 101, Some(102), false).unwrap();
    assert_eq!(tree_order(&eng, id), [0, 3, 1, 2]);
    eng.undo(id).unwrap();
    // In: B as the last kid of A (A's /K is a single item).
    mv(103, 102, None, false).unwrap();
    assert_eq!(tree_order(&eng, id), [0, 1, 3, 2]);
    let t = tags_doc(&eng, id, None).unwrap();
    assert_eq!(t[0].children.iter().map(|c| c.xref).collect::<Vec<_>>(), [102]);
    assert_eq!(t[0].children[0].children.iter().map(|c| c.xref).collect::<Vec<_>>(), [103]);
    // Out: B back into the Document, right before A.
    mv(103, 101, Some(102), false).unwrap();
    assert_eq!(tree_order(&eng, id), [0, 3, 1, 2]);
    assert_consistent(&eng, id);
    eng.undo(id).unwrap();
    eng.undo(id).unwrap();
    assert_eq!(tree_order(&eng, id), [0, 1, 2, 3]);
    // Nothing changes: last kid to the end, itself, an anchor that is not a kid. No step is recorded.
    let steps = eng.history(id).unwrap().len();
    assert!(mv(103, 101, None, false).is_err());
    assert!(mv(102, 101, Some(102), true).is_err());
    assert!(mv(102, 0, Some(103), false).is_err(), "103 is not a kid of the top");
    assert_eq!(eng.history(id).unwrap().len(), steps);
    assert_eq!(tree_order(&eng, id), [0, 1, 2, 3]);
    // Past the content item in front of B is a real change.
    mv(102, 101, Some(103), false).unwrap();
    assert_eq!(tree_order(&eng, id), [0, 2, 1, 3]);
}

// ---- signed and certified documents --------------------------------------------------------------

const SIGNATURE: &str = "<</FT/Sig /T(Signature1) /V <</Type/Sig /Filter/Adobe.PPKLite /SubFilter/adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00>>>>>";

/// `doc3` signed (a signature field with a value), not certified.
fn signed_doc() -> Spec {
    let mut s = doc3();
    s.cat.push_str(" /AcroForm <</SigFlags 3 /Fields [120 0 R]>>");
    s.objs.push((120, SIGNATURE.into()));
    s
}

/// `doc3` certified with DocMDP `/P` `p`.
fn certified_doc(p: u8) -> Spec {
    let mut s = signed_doc();
    s.cat.push_str(&format!(" /Perms <</DocMDP <</Type/Sig /Reference [<</TransformMethod/DocMDP /TransformParams <</P {p}>>>>]>>>>"));
    s
}

fn certified_refusal(r: Result<EditState>, what: &str) {
    match r {
        Err(EngineError::Invalid(m)) => assert!(m.contains("certified"), "{what}: {m}"),
        other => panic!("{what}: expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_certified_document_refuses_every_structure_edit_and_fix() {
    for p in 1..=3 {
        let path = file(&format!("certified-{p}"), &certified_doc(p));
        let (eng, id) = open(&path);
        let r = check_eng(&eng, id);
        assert!(r.certified && r.signed, "P {p}");
        let steps = eng.history(id).unwrap().len();
        let u = || EditSource::User;
        let alt = TagPatch { alt: Some("x".into()), ..TagPatch::default() };
        certified_refusal(reorder_doc(&eng, id, 0, &[104, 102, 103], u()), "reorder");
        certified_refusal(set_role_doc(&eng, id, 102, "H1", u()), "set role");
        certified_refusal(artifact_doc(&eng, id, 102, u()), "artifact");
        certified_refusal(merge_doc(&eng, id, 102, 103, u()), "merge");
        certified_refusal(merge_many_doc(&eng, id, 102, &[103, 104], u()), "merge many");
        certified_refusal(split_doc(&eng, id, 102, 1, None, u()), "split");
        certified_refusal(edit_tag_doc(&eng, id, 102, &alt, u()), "edit tag");
        certified_refusal(move_tag_doc(&eng, id, 104, 101, Some(102), false, u()), "move tag");
        certified_refusal(fix_doc(&eng, id, &Fix::Title { title: Some("T".into()) }, u()), "title");
        certified_refusal(fix_doc(&eng, id, &Fix::Autotag, u()), "autotag");
        certified_refusal(fix_doc(&eng, id, &Fix::Many { fixes: vec![Fix::TabOrder] }, u()), "many");
        assert_eq!(eng.history(id).unwrap().len(), steps, "P {p}: a refusal adds no step");
        assert_eq!(region_xrefs(&eng, id, 0), [102, 103, 104]);
    }
}

#[test]
fn editing_a_signed_document_leaves_its_signed_bytes_alone() {
    let path = file("signed-edit", &signed_doc());
    let original = std::fs::read(&path).unwrap();
    let (eng, id) = open(&path);
    let r = check_eng(&eng, id);
    assert!(r.signed && !r.certified, "signed, not certified");
    reorder_doc(&eng, id, 0, &[104, 102, 103], EditSource::User).unwrap();
    edit_tag_doc(&eng, id, 102, &TagPatch { alt: Some("A".into()), ..TagPatch::default() }, EditSource::User).unwrap();
    fix_doc(&eng, id, &Fix::Title { title: Some("Signed title".into()) }, EditSource::User).unwrap();
    let (eng2, id2) = reload(&eng, id, &path);
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len() && saved.starts_with(&original), "an incremental save: the signed bytes are the start of the file");
    assert_eq!(region_xrefs(&eng2, id2, 0), [104, 102, 103], "the edit is in the saved file");
    assert!(check_eng(&eng2, id2).signed);
}

#[test]
fn a_merge_is_bounded_and_an_anchor_has_an_object_number() {
    let path = file("bounds", &doc3());
    let (eng, id) = open(&path);
    let steps = eng.history(id).unwrap().len();
    let many = vec![103; 100];
    match merge_many_doc(&eng, id, 102, &many, EditSource::User) {
        Err(EngineError::Invalid(m)) => assert!(m.contains("100"), "{m}"),
        other => panic!("{other:?}"),
    }
    for anchor in [0, -1] {
        match move_tag_doc(&eng, id, 104, 101, Some(anchor), false, EditSource::User) {
            Err(EngineError::Invalid(m)) => assert!(m.contains("object number"), "{m}"),
            other => panic!("anchor {anchor}: {other:?}"),
        }
    }
    assert_eq!(eng.history(id).unwrap().len(), steps);
}
