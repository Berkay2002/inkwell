//! Files that lie: marked-content ids that are huge or negative, cycles, trees cut at the guards,
//! content drawn by forms, oversized ToUnicode maps, nested marked content, and moves that would
//! break a table or a list. None may crash, allocate without bound, or leave a half-written tree.

use super::tests::{Spec, check_eng, el, file, marked, open, stream};
use super::tests_structure::doc3;
use super::*;
use std::sync::Arc;

fn assert_consistent(eng: &Engine, id: DocId) {
    let p = tag_problems_doc(eng, id).unwrap();
    assert!(p.is_empty(), "{p:?}");
}

fn reload(eng: &Engine, id: DocId, path: &str) -> (Arc<Engine>, DocId) {
    eng.save(id, None).unwrap();
    eng.close(id);
    open(path)
}

fn steps(eng: &Engine, id: DocId) -> usize {
    eng.history(id).unwrap().len()
}

/// doc3 with element 102's content replaced by `k`.
fn doc3_k(k: &str) -> Spec {
    let mut s = doc3();
    s.objs[2] = el(102, "P", 101, k);
    s
}

#[test]
fn a_huge_mcid_is_not_allocated_and_the_tree_is_not_edited() {
    let path = file("h-huge", &doc3_k("2147483646"));
    let (eng, id) = open(&path);
    // Reading works, the huge reference is simply not listed.
    let ro = reading_order_doc(&eng, id, 0).unwrap();
    assert!(ro.regions.iter().all(|r| !r.mcids.contains(&2147483646)));
    // The validator says why edits are off; repair and every edit refuse, without allocating.
    let p = tag_problems_doc(&eng, id).unwrap();
    assert!(p.iter().any(|x| x.message.contains("can't be repaired or edited")), "{p:?}");
    let n = steps(&eng, id);
    assert!(fix_doc(&eng, id, &Fix::ParentTree, EditSource::User).is_err());
    assert!(set_role_doc(&eng, id, 103, "H1", EditSource::User).is_err());
    assert!(artifact_doc(&eng, id, 103, EditSource::User).is_err());
    assert!(merge_doc(&eng, id, 103, 104, EditSource::User).is_err());
    assert!(reorder_doc(&eng, id, 0, &[104, 103], EditSource::User).is_err());
    assert_eq!(steps(&eng, id), n, "no step was added");
    // The checker reports it as a failure of the parent-tree rule.
    assert_eq!(check_eng(&eng, id).status(Rule::ParentTree), Status::Failed);
}

#[test]
fn a_negative_mcid_does_not_land_on_entry_zero() {
    for k in ["-1", "-2147483648", "1000001"] {
        let path = file("h-neg", &doc3_k(k));
        let (eng, id) = open(&path);
        assert!(reading_order_doc(&eng, id, 0).is_ok());
        let before = steps(&eng, id);
        assert!(fix_doc(&eng, id, &Fix::ParentTree, EditSource::User).is_err(), "{k}");
        assert!(artifact_doc(&eng, id, 103, EditSource::User).is_err(), "{k}");
        assert_eq!(steps(&eng, id), before);
        // Entry 0 of the page's ParentTree array still names 102.
        eng.save(id, None).ok();
        let d = PdfDocument::open(&path).unwrap();
        let root = d.catalog().unwrap().get_dict("StructTreeRoot").unwrap().unwrap();
        let pt = tags::ParentTree::read(&root).unwrap();
        assert_eq!(pt.arrays[&0][0], Some(102), "{k}");
    }
}

#[test]
fn set_mcid_and_split_refuse_ids_out_of_range() {
    let mut pt = tags::ParentTree::default();
    for bad in [-1, i32::MIN, tree::MAX_MCID + 1, i32::MAX] {
        assert!(pt.set_mcid_for_test(0, bad, Some(5)).is_err(), "{bad}");
    }
    assert!(pt.set_mcid_for_test(0, tree::MAX_MCID, Some(5)).is_ok());
    assert_eq!(pt.arrays[&0].len(), tree::MAX_MCID as usize + 1);
    let path = file("h-split-range", &doc3());
    let (eng, id) = open(&path);
    assert!(split_doc(&eng, id, 102, -1, None, EditSource::User).is_err());
    assert!(split_doc(&eng, id, 102, i32::MAX, None, EditSource::User).is_err());
}

#[test]
fn a_cycle_is_read_once_and_blocks_edits() {
    // 102 holds 103 holds 102.
    let mut s = doc3();
    s.objs[1].1 = "<</Type/StructElem /S/Document /P 100 0 R /K [102 0 R]>>".into();
    s.objs[2] = (102, "<</Type/StructElem /S/P /P 101 0 R /Pg 10 0 R /K [0 103 0 R]>>".into());
    s.objs[3] = (103, "<</Type/StructElem /S/P /P 102 0 R /Pg 10 0 R /K [1 102 0 R]>>".into());
    let path = file("h-cycle", &s);
    let (eng, id) = open(&path);
    assert!(reading_order_doc(&eng, id, 0).is_ok());
    assert!(tags_doc(&eng, id, None).is_ok());
    assert!(set_role_doc(&eng, id, 102, "H1", EditSource::User).is_err());
    assert!(!tag_problems_doc(&eng, id).unwrap().is_empty());
    let r = check_eng(&eng, id);
    assert_eq!(r.status(Rule::ParentTree), Status::Failed);
}

#[test]
fn a_tree_cut_at_the_depth_guard_is_not_repaired_into_a_hole() {
    // 300 nested Sect elements with the paragraph at the bottom.
    let depth = 300usize;
    let mut s = doc3();
    s.objs.truncate(2);
    s.objs[1].1 = "<</Type/StructElem /S/Document /P 100 0 R /K [200 0 R]>>".into();
    for i in 0..depth {
        let (me, next) = (200 + i, 201 + i);
        let kid = if i + 1 == depth { "0".to_string() } else { format!("{next} 0 R") };
        s.objs.push((me, format!("<</Type/StructElem /S/Sect /P {} 0 R /Pg 10 0 R /K [{kid}]>>", if i == 0 { 101 } else { me - 1 })));
    }
    s.objs[0].1 = format!("<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [{} 0 R]]>> /ParentTreeNextKey 1>>", 200 + depth - 1);
    let path = file("h-deep", &s);
    let (eng, id) = open(&path);
    let before = steps(&eng, id);
    assert!(fix_doc(&eng, id, &Fix::ParentTree, EditSource::User).is_err(), "repair would null the entry for the paragraph it can't see");
    assert!(set_role_doc(&eng, id, 200, "Div", EditSource::User).is_err());
    assert_eq!(steps(&eng, id), before);
    assert!(tag_problems_doc(&eng, id).unwrap().iter().any(|p| p.message.contains("deeper or larger")));
}

#[test]
fn a_parent_tree_with_entries_that_cant_be_kept_is_never_rewritten() {
    // Entry 1 of the page array is a direct dictionary; key 7 has a value that is neither array nor reference.
    let mut s = doc3();
    s.objs[0].1 = "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [102 0 R <</Odd true>> 104 0 R] 7 42]>> /ParentTreeNextKey 8>>".into();
    let path = file("h-pt-skipped", &s);
    let (eng, id) = open(&path);
    assert!(tag_problems_doc(&eng, id).unwrap().iter().any(|p| p.message.contains("can't be read")));
    let before = steps(&eng, id);
    assert!(fix_doc(&eng, id, &Fix::ParentTree, EditSource::User).is_err());
    assert!(artifact_doc(&eng, id, 104, EditSource::User).is_err());
    assert!(split_doc(&eng, id, 102, 0, None, EditSource::User).is_err());
    assert!(merge_doc(&eng, id, 102, 104, EditSource::User).is_err());
    assert_eq!(steps(&eng, id), before);
}

/// Element 102 draws through form 140 (key 5 on the stream) and holds MCID 1 on the page.
fn stm_doc(pt: &str) -> Spec {
    let mut s = doc3();
    s.objs.truncate(2);
    s.objs[0].1 = format!("<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [{pt}]>> /ParentTreeNextKey 6>>");
    s.objs[1].1 = "<</Type/StructElem /S/Document /P 100 0 R /K [102 0 R 103 0 R]>>".into();
    s.objs.push((102, "<</Type/StructElem /S/P /P 101 0 R /Pg 10 0 R /K [0 <</Type/MCR /Pg 10 0 R /Stm 140 0 R /MCID 0>>]>>".into()));
    s.objs.push(el(103, "P", 101, "1"));
    s.objs.push((140, stream("/Type/XObject /Subtype/Form /BBox [0 0 10 10] /StructParents 5", "")));
    s
}

#[test]
fn content_drawn_by_a_form_is_checked_split_and_not_artifacted_away() {
    let good = "0 [102 0 R 103 0 R] 5 [102 0 R]";
    let path = file("h-stm", &stm_doc(good));
    let (eng, id) = open(&path);
    assert_consistent(&eng, id);
    // Marking it as artifact from here would leave the stream's entry naming a removed element: refused.
    let before = steps(&eng, id);
    assert!(artifact_doc(&eng, id, 102, EditSource::User).is_err());
    assert_eq!(steps(&eng, id), before);
    // Splitting moves the stream's MCR with the rest, and its ParentTree entry follows.
    split_doc(&eng, id, 102, 0, None, EditSource::User).unwrap_err(); // first item: nothing would stay
    // The form's MCR is the second kid, but only page content is a split point: add a page item first.
    let mut s = stm_doc("0 [102 0 R 102 0 R 103 0 R] 5 [102 0 R]");
    s.objs.retain(|o| o.0 != 102);
    s.objs.push((102, "<</Type/StructElem /S/P /P 101 0 R /Pg 10 0 R /K [0 1 <</Type/MCR /Pg 10 0 R /Stm 140 0 R /MCID 0>>]>>".into()));
    s.objs.retain(|o| o.0 != 103);
    s.objs.push(el(103, "P", 101, "2"));
    let path = file("h-stm-split", &s);
    let (eng, id) = open(&path);
    assert_consistent(&eng, id);
    split_doc(&eng, id, 102, 1, None, EditSource::User).unwrap();
    assert_consistent(&eng, id);
    let (eng2, id2) = reload(&eng, id, &path);
    assert_consistent(&eng2, id2);
}

#[test]
fn a_wrong_entry_on_a_forms_key_is_reported_and_repaired() {
    let path = file("h-stm-bad", &stm_doc("0 [102 0 R 103 0 R] 5 [103 0 R]"));
    let (eng, id) = open(&path);
    let p = tag_problems_doc(&eng, id).unwrap();
    assert!(p.iter().any(|x| x.message.contains("form object 140")), "{p:?}");
    fix_doc(&eng, id, &Fix::ParentTree, EditSource::User).unwrap();
    assert_consistent(&eng, id);
}

#[test]
fn an_oversized_to_unicode_map_is_capped() {
    // 20,000 ranges of 65,536 codes each would be over a billion entries.
    let mut cmap = String::from("/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n1 begincodespacerange <0000> <FFFF> endcodespacerange\n");
    for _ in 0..20 {
        cmap.push_str("1000 beginbfrange\n");
        for _ in 0..1000 {
            cmap.push_str("<0000> <FFFF> <0041>\n");
        }
        cmap.push_str("endbfrange\n");
    }
    cmap.push_str("endcmap end end");
    let mut s = doc3();
    s.font = "<</Type/Font/Subtype/Type1/BaseFont/Helvetica/ToUnicode 150 0 R>>".into();
    s.objs.push((150, stream("", &cmap)));
    let path = file("h-cmap", &s);
    let (eng, id) = open(&path);
    let t = std::time::Instant::now();
    let ro = reading_order_doc(&eng, id, 0).unwrap();
    assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
    assert_eq!(ro.regions.len(), 3);
    // The map that was kept decodes (every code maps up from 'A' here), so the preview is not empty.
    assert!(!ro.regions[0].text.is_empty());
}

#[test]
fn nested_marked_content_is_artifacted_and_the_checker_says_what_is_left() {
    // MCID 1 sits inside MCID 0's marked content.
    let content = "/P <</MCID 0>> BDC BT /F1 12 Tf 72 700 Td (outer) Tj ET /Span <</MCID 1>> BDC BT /F1 12 Tf 72 680 Td (inner) Tj ET EMC EMC";
    let mut s = doc3();
    s.pages[0].1 = content.into();
    s.objs.truncate(2);
    s.objs[0].1 = "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [102 0 R 103 0 R]]>> /ParentTreeNextKey 1>>".into();
    s.objs[1].1 = "<</Type/StructElem /S/Document /P 100 0 R /K [102 0 R 103 0 R]>>".into();
    s.objs.push(el(102, "P", 101, "0"));
    s.objs.push(el(103, "Span", 101, "1"));
    let path = file("h-nested", &s);
    let (eng, id) = open(&path);
    assert_consistent(&eng, id);
    artifact_doc(&eng, id, 103, EditSource::User).unwrap();
    assert_consistent(&eng, id);
    // An artifact inside tagged content is now what the content-marking rule reports.
    let r = check_eng(&eng, id);
    assert_eq!(r.status(Rule::ContentMarking), Status::Failed);
    assert!(r.result(Rule::ContentMarking).findings[0].message.contains("inside tagged content"));
    // Artifacting the outer one too leaves nothing in the tree.
    artifact_doc(&eng, id, 102, EditSource::User).unwrap();
    assert_consistent(&eng, id);
    assert!(reading_order_doc(&eng, id, 0).unwrap().regions.is_empty());
}

fn table_and_p() -> Spec {
    // Document > [Table(102) > TR(103) > TD(104, MCID 0), P(105, MCID 1)]
    let content = format!("{}{}", marked("TD", 0, 700), marked("P", 1, 680));
    Spec {
        cat: "/StructTreeRoot 100 0 R /MarkInfo <</Marked true>>".into(),
        pages: vec![("/StructParents 0".into(), content)],
        objs: vec![
            (100, "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [104 0 R 105 0 R]]>> /ParentTreeNextKey 1>>".into()),
            (101, "<</Type/StructElem /S/Document /P 100 0 R /K [102 0 R 105 0 R]>>".into()),
            (102, "<</Type/StructElem /S/Table /P 101 0 R /Pg 10 0 R /K [103 0 R]>>".into()),
            (103, "<</Type/StructElem /S/TR /P 102 0 R /Pg 10 0 R /K [104 0 R]>>".into()),
            el(104, "TD", 103, "0"),
            el(105, "P", 101, "1"),
        ],
        ..Spec::default()
    }
}

#[test]
fn a_reorder_never_moves_a_paragraph_into_a_row_or_a_cell_out_of_one() {
    let path = file("h-fit", &table_and_p());
    let (eng, id) = open(&path);
    let before = steps(&eng, id);
    let was: Vec<i32> = reading_order_doc(&eng, id, 0).unwrap().regions.iter().map(|r| r.xref).collect();
    assert_eq!(was, [104, 105]);
    let e = reorder_doc(&eng, id, 0, &[105, 104], EditSource::User).unwrap_err();
    assert!(e.to_string().contains("can't be moved"), "{e}");
    assert_eq!(steps(&eng, id), before);
    let now: Vec<i32> = reading_order_doc(&eng, id, 0).unwrap().regions.iter().map(|r| r.xref).collect();
    assert_eq!(now, was, "nothing moved");
    assert_consistent(&eng, id);
}

#[test]
fn a_list_item_stays_in_its_list() {
    // Document > [L(102) > LI(103) > LBody(104, MCID 0), P(105, MCID 1)]
    let content = format!("{}{}", marked("LBody", 0, 700), marked("P", 1, 680));
    let s = Spec {
        cat: "/StructTreeRoot 100 0 R /MarkInfo <</Marked true>>".into(),
        pages: vec![("/StructParents 0".into(), content)],
        objs: vec![
            (100, "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [104 0 R 105 0 R]]>> /ParentTreeNextKey 1>>".into()),
            (101, "<</Type/StructElem /S/Document /P 100 0 R /K [102 0 R 105 0 R]>>".into()),
            (102, "<</Type/StructElem /S/L /P 101 0 R /Pg 10 0 R /K [103 0 R]>>".into()),
            (103, "<</Type/StructElem /S/LI /P 102 0 R /Pg 10 0 R /K [104 0 R]>>".into()),
            el(104, "LBody", 103, "0"),
            el(105, "P", 101, "1"),
        ],
        ..Spec::default()
    };
    let path = file("h-fit-list", &s);
    let (eng, id) = open(&path);
    assert!(reorder_doc(&eng, id, 0, &[105, 104], EditSource::User).is_err());
    assert_consistent(&eng, id);
}

#[test]
fn a_reorder_that_would_change_the_order_on_another_page_is_refused() {
    // 111 holds MCID 1 on page 1 and MCID 0 on page 2; 113 is a paragraph on page 2 that sits between
    // them in the tree. Moving 111 ahead of 102 on page 1 would put it ahead of 113 on page 2 as well.
    let mut s = doc3();
    s.pages.push(("/StructParents 1".into(), format!("{}{}", marked("P", 0, 700), marked("P", 1, 680))));
    s.objs[0].1 = "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [102 0 R 111 0 R 104 0 R] 1 [111 0 R 113 0 R]]>> /ParentTreeNextKey 2>>".into();
    s.objs[1].1 = "<</Type/StructElem /S/Document /P 100 0 R /K [102 0 R 113 0 R 111 0 R 104 0 R]>>".into();
    s.objs.retain(|o| o.0 != 103);
    s.objs.push((111, "<</Type/StructElem /S/P /P 101 0 R /Pg 10 0 R /K [1 <</Type/MCR /Pg 12 0 R /MCID 0>>]>>".into()));
    s.objs.push((113, "<</Type/StructElem /S/P /P 101 0 R /Pg 12 0 R /K 1>>".into()));
    let path = file("h-span", &s);
    let (eng, id) = open(&path);
    assert_consistent(&eng, id);
    let p1: Vec<i32> = reading_order_doc(&eng, id, 1).unwrap().regions.iter().map(|r| r.xref).collect();
    assert_eq!(p1, [113, 111]);
    let before = steps(&eng, id);
    let e = reorder_doc(&eng, id, 0, &[111, 102, 104], EditSource::User).unwrap_err();
    assert!(e.to_string().contains("page 2"), "{e}");
    assert_eq!(steps(&eng, id), before);
    // A move that leaves page 2's order alone is fine.
    reorder_doc(&eng, id, 0, &[102, 104, 111], EditSource::User).unwrap();
    assert_consistent(&eng, id);
}

#[test]
fn merging_header_cells_keeps_the_headers_of_the_cells_that_name_them() {
    // Table > TR(120) > TH(121 ID a), TH(122 ID b), TD(123 Headers [b])
    let content = format!("{}{}{}", marked("TH", 0, 700), marked("TH", 1, 690), marked("TD", 2, 680));
    let s = Spec {
        cat: "/StructTreeRoot 100 0 R /MarkInfo <</Marked true>>".into(),
        pages: vec![("/StructParents 0".into(), content)],
        objs: vec![
            (100, "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [121 0 R 122 0 R 123 0 R]]>> /ParentTreeNextKey 1>>".into()),
            (101, "<</Type/StructElem /S/Document /P 100 0 R /K [119 0 R]>>".into()),
            (119, "<</Type/StructElem /S/Table /P 101 0 R /Pg 10 0 R /K [120 0 R]>>".into()),
            (120, "<</Type/StructElem /S/TR /P 119 0 R /Pg 10 0 R /K [121 0 R 122 0 R 123 0 R]>>".into()),
            (121, "<</Type/StructElem /S/TH /P 120 0 R /Pg 10 0 R /ID (a) /K 0>>".into()),
            (122, "<</Type/StructElem /S/TH /P 120 0 R /Pg 10 0 R /ID (b) /K 1>>".into()),
            (123, "<</Type/StructElem /S/TD /P 120 0 R /Pg 10 0 R /K 2 /A <</O/Table /Headers [(b)]>>>>".into()),
        ],
        ..Spec::default()
    };
    let path = file("h-headers", &s);
    let (eng, id) = open(&path);
    merge_doc(&eng, id, 121, 122, EditSource::User).unwrap();
    assert_consistent(&eng, id);
    let t = tags_doc(&eng, id, None).unwrap();
    let cells = &t[0].children[0].children[0].children;
    assert_eq!(cells.len(), 2);
    assert_eq!(cells[1].headers, [121], "the cell now names the merged header");
    assert_eq!(report(&eng, id).status(Rule::TableStructure), Status::Passed);
    // Merging into a header without an ID hands the ID over.
    let mut s2 = s.clone();
    s2.objs.retain(|o| o.0 != 121);
    s2.objs.push((121, "<</Type/StructElem /S/TH /P 120 0 R /Pg 10 0 R /K 0>>".into()));
    let path = file("h-headers-2", &s2);
    let (eng, id) = open(&path);
    merge_doc(&eng, id, 121, 122, EditSource::User).unwrap();
    let t = tags_doc(&eng, id, None).unwrap();
    let cells = &t[0].children[0].children[0].children;
    assert_eq!((cells[0].id.as_deref(), cells[1].headers.clone()), (Some("b"), vec![121]));
}

fn report(eng: &Engine, id: DocId) -> Report {
    check_eng(eng, id)
}

#[test]
fn repair_rewrites_a_wrong_parent_pointer() {
    // Everything is mapped right in the ParentTree; only 103's /P names the wrong element.
    let mut s = doc3();
    s.objs[3] = (103, "<</Type/StructElem /S/P /P 102 0 R /Pg 10 0 R /K 1>>".into());
    let path = file("h-wrong-p", &s);
    let (eng, id) = open(&path);
    let p = tag_problems_doc(&eng, id).unwrap();
    assert_eq!(p.len(), 1, "{p:?}");
    fix_doc(&eng, id, &Fix::ParentTree, EditSource::User).unwrap();
    assert_consistent(&eng, id);
    assert_eq!(check_eng(&eng, id).status(Rule::ParentTree), Status::Passed);
    let (eng2, id2) = reload(&eng, id, &path);
    assert_consistent(&eng2, id2);
}

#[test]
fn repair_gives_pages_that_share_a_key_a_key_each() {
    // Two pages both use key 0; page 2's paragraph is mapped under it too.
    let mut s = doc3();
    s.pages.push(("/StructParents 0".into(), marked("P", 0, 700)));
    s.objs[0].1 = "<</Type/StructTreeRoot /K 101 0 R /ParentTree <</Nums [0 [102 0 R 103 0 R 104 0 R]]>> /ParentTreeNextKey 1>>".into();
    s.objs[1].1 = "<</Type/StructElem /S/Document /P 100 0 R /K [102 0 R 103 0 R 104 0 R 111 0 R]>>".into();
    s.objs.push((111, "<</Type/StructElem /S/P /P 101 0 R /Pg 12 0 R /K 0>>".into()));
    let path = file("h-shared-key", &s);
    let (eng, id) = open(&path);
    assert!(!tag_problems_doc(&eng, id).unwrap().is_empty());
    fix_doc(&eng, id, &Fix::ParentTree, EditSource::User).unwrap();
    assert_consistent(&eng, id);
    let (eng2, id2) = reload(&eng, id, &path);
    assert_consistent(&eng2, id2);
    let d = PdfDocument::open(&path).unwrap();
    let keys: Vec<i32> = (0..2).map(|p| d.find_page(p).unwrap().get_dict("StructParents").unwrap().unwrap().as_int().unwrap()).collect();
    assert_ne!(keys[0], keys[1]);
}
