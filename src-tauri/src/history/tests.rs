//! Run with `cargo test --lib history -- --test-threads=1`.

use super::*;
use crate::engine::fixture::{self, Fixture};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-history-tests").join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// A fixture with a trailer /ID (the generated one has none).
fn pdf(pages: usize, id: &str) -> Vec<u8> {
    let b = fixture::build(&Fixture { pages, chapter_every: 0, needle_every: 0, lines: 30 });
    let at = b.windows(14).rposition(|w| w == b"/Root 1 0 R >>").unwrap() + 12;
    let mut b = b;
    b.splice(at..at, format!("/ID [<{id}> <{id}>] ").into_bytes());
    b
}

fn meta(kind: Kind, label: &str) -> NewVersion {
    NewVersion { author: Author::User, kind, label: label.into(), summary: vec![], name: None, pages_touched: vec![] }
}

/// Successive saves, each adding one annotation (incremental append or full rewrite).
fn edits(base: &[u8], n: usize, incremental: bool) -> Vec<Vec<u8>> {
    use mupdf::Rect;
    use mupdf::pdf::{PdfDocument, PdfWriteOptions};
    let mut out = vec![base.to_vec()];
    for i in 0..n {
        let doc = PdfDocument::from_bytes(out.last().unwrap()).unwrap();
        let pages = doc.page_count().unwrap();
        let mut page = doc.load_pdf_page(i as i32 % pages).unwrap();
        page.add_text_annotation(Rect::new(50.0, 50.0 + i as f32 * 10.0, 70.0, 70.0 + i as f32 * 10.0), &format!("note {i}"))
            .unwrap();
        let mut opts = PdfWriteOptions::default();
        opts.set_incremental(incremental);
        let mut buf = Vec::new();
        doc.write_to_with_options(&mut buf, opts).unwrap();
        out.push(buf);
    }
    out
}

fn all_files(root: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    fn walk(d: &Path, v: &mut Vec<PathBuf>) {
        for e in fs::read_dir(d).into_iter().flatten().flatten() {
            if e.file_type().unwrap().is_dir() {
                walk(&e.path(), v);
            } else {
                v.push(e.path());
            }
        }
    }
    walk(root, &mut v);
    v
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn setup(name: &str, bytes: &[u8]) -> (PathBuf, PathBuf, Store, DocId) {
    let dir = tmp(name);
    let file = dir.join("doc.pdf");
    fs::write(&file, bytes).unwrap();
    let root = dir.join("store");
    let s = Store::open(&root).unwrap();
    let doc = s.attach(&file).unwrap();
    (dir, root, s, doc)
}

#[test]
fn byte_exact_restore_of_every_version() {
    let base = pdf(30, "00112233445566778899aabbccddeeff");
    let mut seq = edits(&base, 6, true);
    seq.extend(edits(seq.last().unwrap(), 4, false).into_iter().skip(1));
    let (_d, _r, s, doc) = setup("exact", &seq[0]);
    let mut ids = vec![s.versions(doc).unwrap()[0].id];
    for (i, b) in seq.iter().enumerate().skip(1) {
        ids.push(s.commit(doc, b, meta(Kind::Save, &format!("save {i}"))).unwrap().id);
    }
    assert_eq!(s.versions(doc).unwrap().len(), seq.len());
    for (id, b) in ids.iter().zip(&seq) {
        assert!(s.read(doc, *id).unwrap() == *b);
    }
    // Restore creates a new version on top, never deletes.
    let r = s.restore(doc, ids[2], Author::User).unwrap();
    assert_eq!(r.kind, Kind::Restore { from: ids[2] });
    assert_eq!(r.parents, vec![*ids.last().unwrap()]);
    assert!(s.read(doc, r.id).unwrap() == seq[2]);
    assert_eq!(s.versions(doc).unwrap().len(), seq.len() + 1);
    // Reopening reads the same.
    drop(s);
    let s = Store::open(&_r).unwrap();
    for (id, b) in ids.iter().zip(&seq) {
        assert!(s.read(doc, *id).unwrap() == *b);
    }
}

#[test]
fn commit_dedups_unless_labelled_checkpoint() {
    let (_d, _r, s, doc) = setup("dedup-head", &pdf(3, "01"));
    let head = s.versions(doc).unwrap()[0].id;
    let b = s.read(doc, head).unwrap();
    assert_eq!(s.commit(doc, &b, meta(Kind::Save, "same")).unwrap().id, head);
    assert_eq!(s.commit(doc, &b, meta(Kind::Checkpoint, "")).unwrap().id, head);
    let c = s.commit(doc, &b, meta(Kind::Checkpoint, "Assistant turn")).unwrap();
    assert_ne!(c.id, head);
    assert_eq!(s.versions(doc).unwrap().len(), 2);
    assert!(s.read(doc, c.id).unwrap() == b);
}

#[test]
fn dedup_ratio_on_incremental_saves() {
    // 20 incremental saves of a 200-page document: each appends a few KB.
    let seq = edits(&pdf(200, "aa"), 20, true);
    let (_d, _r, s, doc) = setup("ratio", &seq[0]);
    let first = s.usage().unwrap().total;
    for b in &seq[1..] {
        s.commit(doc, b, meta(Kind::Save, "save")).unwrap();
    }
    let total = s.usage().unwrap().total;
    let logical: usize = seq.iter().map(Vec::len).sum();
    let per_version = (total - first) / 20;
    eprintln!("first {first} B, then {per_version} B/version; logical {logical} B, stored {total} B");
    // Each save adds < 1 KB of PDF; the store may spend about one chunk plus a manifest on it.
    assert!(per_version < 40 << 10, "{per_version} B per incremental save");
    assert!((total as f64) < logical as f64 / 10.0);
}

fn vi(id_ms: u64, name: Option<&str>) -> VersionInfo {
    let mut b = [0u8; 16];
    b[..6].copy_from_slice(&id_ms.to_be_bytes()[2..]);
    VersionInfo {
        id: VersionId(b),
        parents: vec![],
        created: id_ms,
        author: Author::User,
        kind: Kind::Save,
        label: String::new(),
        summary: vec![],
        name: name.map(Into::into),
        pages_touched: vec![],
        size: 0,
        broken: false,
    }
}

#[test]
fn retention_plan_on_two_year_timeline() {
    const H: u64 = 3_600_000;
    const D: u64 = 24 * H;
    let now = 1_790_000_000_000u64;
    // Every 5 hours for two years; one named version 400 days ago.
    let mut vs: Vec<VersionInfo> = (0..(730 * 24 / 5)).map(|i| vi(now - 730 * D + i * 5 * H, None)).collect();
    let named = vi(now - 400 * D + 1, Some("Sent to client"));
    vs.push(named.clone());
    let p = Policy { per_doc_cap_bytes: 0, total_cap_bytes: 0, ..Policy::default() };
    let keep = plan_retention(&vs, &p, now, |_| 0);
    let head = vs.iter().map(|v| v.id).max().unwrap();
    assert!(keep.contains(&head) && keep.contains(&named.id));
    for v in &vs {
        if now - v.created <= 30 * D {
            assert!(keep.contains(&v.id), "recent version pruned");
        }
    }
    let kept: Vec<&VersionInfo> = vs.iter().filter(|v| keep.contains(&v.id) && v.name.is_none()).collect();
    // Daily window: exactly one per day that had versions.
    let daily: Vec<u64> = kept.iter().filter(|v| (30 * D + 1..=210 * D).contains(&(now - v.created))).map(|v| v.created / D).collect();
    let days: HashSet<u64> = daily.iter().copied().collect();
    assert_eq!(daily.len(), days.len(), "two versions kept on one day");
    assert!(days.len() >= 179);
    // Monthly beyond: one per calendar month.
    let monthly: Vec<i64> =
        kept.iter().filter(|v| now - v.created > 210 * D).map(|v| retention::month_of((v.created / D) as i64)).collect();
    let months: HashSet<i64> = monthly.iter().copied().collect();
    assert_eq!(monthly.len(), months.len());
    assert!((17..=19).contains(&months.len()), "{} months", months.len());
    // Calendar sanity.
    assert_eq!(retention::month_of(0), 0);
    assert_eq!(retention::month_of(31), 1);
    assert_eq!(retention::month_of(365 + 366 + 365 + 59), 38); // 1973-03-01
}

#[test]
fn caps_prune_oldest_unnamed_first() {
    let now = 1_790_000_000_000u64;
    let mut vs: Vec<VersionInfo> = (0..10).map(|i| vi(now - 10_000 + i * 1000, None)).collect();
    vs[0].name = Some("kept".into());
    // Each version costs 100 bytes; cap 450 -> 4 versions plus... named + head can't go.
    let p = Policy { per_doc_cap_bytes: 450, total_cap_bytes: 0, ..Policy::default() };
    let keep = plan_retention(&vs, &p, now, |k| k.len() as u64 * 100);
    let kept: Vec<usize> = (0..10).filter(|i| keep.contains(&vs[*i].id)).collect();
    assert_eq!(kept, vec![0, 7, 8, 9]);

    // Through the store, with real sizes: the per-doc cap drops the oldest unnamed versions.
    let (_d, _r, s, doc) = setup("caps", &pdf(5, "02"));
    let mut b = pdf(5, "02");
    let mut ids = vec![s.versions(doc).unwrap()[0].id];
    for i in 0..6 {
        b.extend_from_slice(&crate::history::crypto::random::<4096>());
        b.extend_from_slice(format!("\n% {i}\n").as_bytes());
        ids.push(s.commit(doc, &b, meta(Kind::Save, "s")).unwrap().id);
    }
    s.name(doc, ids[1], Some("named".into())).unwrap();
    let total = s.usage().unwrap().total;
    s.set_policy(Policy { per_doc_cap_bytes: total * 6 / 10, ..Policy::default() }).unwrap();
    let pruned = s.prune().unwrap();
    let left: Vec<VersionId> = s.versions(doc).unwrap().iter().map(|v| v.id).collect();
    assert!(pruned > 0);
    assert!(left.contains(&ids[1]) && left.contains(ids.last().unwrap()));
    // Whatever went, it went oldest first among the unnamed.
    let gone: Vec<usize> = (0..ids.len()).filter(|i| !left.contains(&ids[*i])).collect();
    let unnamed: Vec<usize> = (0..ids.len()).filter(|i| *i != 1).collect();
    assert_eq!(gone, unnamed[..gone.len()].to_vec());
    for v in &left {
        s.read(doc, *v).unwrap();
    }
}

#[test]
fn gc_removes_only_unreferenced_chunks() {
    let (_d, root, s, doc) = setup("gc", &pdf(20, "03"));
    let mut b = pdf(20, "03");
    let mut ids = vec![];
    for i in 0..4 {
        b.extend_from_slice(&crate::history::crypto::random::<70000>());
        b.extend_from_slice(format!("\n% {i}\n").as_bytes());
        ids.push(s.commit(doc, &b, meta(Kind::Save, "s")).unwrap().id);
    }
    // A stray chunk nothing references.
    let stray = root.join("docs").join(doc.to_string()).join("chunks").join("ff").join("ff".repeat(32));
    fs::create_dir_all(stray.parent().unwrap()).unwrap();
    fs::write(&stray, b"junk").unwrap();
    let before = s.usage().unwrap().chunks;
    assert_eq!(s.gc().unwrap(), 4);
    assert!(!stray.exists());
    assert_eq!(s.usage().unwrap().chunks, before - 1);
    // Prune the oldest two by policy (all_days 0, daily 0 => monthly keeps one); every survivor reads.
    s.set_policy(Policy { all_days: 0, daily_months: 0, per_doc_cap_bytes: 0, total_cap_bytes: 0 }).unwrap();
    s.prune().unwrap();
    let left = s.versions(doc).unwrap();
    assert!(left.len() < 5);
    assert!(s.usage().unwrap().chunks < before - 1, "unreferenced chunks of pruned versions stay");
    for v in left {
        s.read(doc, v.id).unwrap();
    }
}

#[test]
fn no_plaintext_at_rest_and_shred_leaves_nothing() {
    let base = pdf(10, "0badcafe");
    let marker = b"Inkwell fixture page 7";
    assert!(contains(&base, marker));
    let (dir, root, s, doc) = setup("shred", &base);
    let seq = edits(&base, 2, false);
    for b in &seq[1..] {
        s.commit(doc, b, meta(Kind::Save, "s")).unwrap();
    }
    s.name(doc, s.versions(doc).unwrap()[0].id, Some("Secret name ZQX".into())).unwrap();
    let doc_dir = root.join("docs").join(doc.to_string());
    for f in all_files(&root).into_iter().filter(|f| !f.ends_with("lock")) {
        let b = fs::read(&f).unwrap();
        for m in [&marker[..], b"quick brown fox", b"Secret name ZQX", dir.to_string_lossy().as_bytes(), b"doc.pdf"] {
            assert!(!contains(&b, m), "{f:?} holds plaintext {:?}", String::from_utf8_lossy(m));
        }
    }
    s.shred(doc).unwrap();
    assert!(!doc_dir.exists());
    assert!(all_files(&root).iter().all(|f| !f.starts_with(&doc_dir)));
    assert!(s.versions(doc).is_err());
    assert_eq!(s.doc_for_file(&dir.join("doc.pdf")).unwrap(), Resolution::New);
}

#[test]
fn redaction_purge_destroys_old_key() {
    let (_d, root, s, doc) = setup("redact", &pdf(4, "04"));
    let key = root.join("docs").join(doc.to_string()).join("key.dpapi");
    let old_key = fs::read(&key).unwrap();
    let old = s.versions(doc).unwrap()[0].id;
    s.redactions_applied(doc, false).unwrap();
    assert!(s.versions(doc).unwrap().is_empty());
    assert!(s.read(doc, old).is_err());
    assert_ne!(fs::read(&key).unwrap(), old_key);
    assert!(fs::read_dir(root.join("docs").join(doc.to_string()).join("chunks")).is_err());
    let red = pdf(4, "04").into_iter().chain(*b"% redacted\n").collect::<Vec<u8>>();
    let v = s.commit(doc, &red, meta(Kind::Save, "Applied redactions")).unwrap();
    assert!(s.read(doc, v.id).unwrap() == red);
    assert!(!s.keeps_pre_redaction(doc).unwrap());

    s.redactions_applied(doc, true).unwrap();
    assert!(s.keeps_pre_redaction(doc).unwrap());
    assert_eq!(s.versions(doc).unwrap().len(), 1);
}

#[test]
fn never_keep_and_tamper() {
    let (dir, root, s, doc) = setup("never", &pdf(4, "05"));
    // Tampered chunk -> read errors, never wrong bytes.
    let v = s.versions(doc).unwrap()[0].id;
    let chunk = all_files(&root.join("docs").join(doc.to_string()).join("chunks"))[0].clone();
    let mut b = fs::read(&chunk).unwrap();
    let n = b.len();
    b[n / 2] ^= 1;
    fs::write(&chunk, &b).unwrap();
    assert!(matches!(s.read(doc, v), Err(HistoryError::Integrity(_))));
    b[n / 2] ^= 1;
    fs::write(&chunk, &b).unwrap();
    assert!(s.read(doc, v).is_ok());
    // A truncated manifest is moved aside (not deleted) and reported.
    let man = all_files(&root.join("docs").join(doc.to_string()).join("versions"))[0].clone();
    let mb = fs::read(&man).unwrap();
    fs::write(&man, &mb[..mb.len() - 1]).unwrap();
    let rep = s.fsck().unwrap();
    assert_eq!(rep.bad_manifests, 1);
    assert!(man.with_extension("bad").exists());
    assert!(s.versions(doc).unwrap().is_empty());
    assert_eq!(s.gc().unwrap(), 0, "GC must not touch a damaged doc");

    let file = dir.join("doc.pdf");
    s.set_never_keep(Target::Path(file.clone()), true).unwrap();
    assert_eq!(s.doc_for_file(&file).unwrap(), Resolution::Excluded);
    assert!(matches!(s.attach(&file), Err(HistoryError::NeverKeep)));
    assert!(!root.join("docs").join(doc.to_string()).exists());
    assert!(!contains(&fs::read(root.join("never.json")).unwrap(), b"doc.pdf"));
    // Survives a restart, and can be lifted.
    drop(s);
    let s = Store::open(&root).unwrap();
    assert_eq!(s.doc_for_file(&file).unwrap(), Resolution::Excluded);
    s.set_never_keep(Target::Path(file.clone()), false).unwrap();
    assert_eq!(s.doc_for_file(&file).unwrap(), Resolution::New);
}

#[test]
fn identity_survives_rename_move_and_copy() {
    let (dir, _root, s, doc) = setup("identity", &pdf(6, "1234abcd"));
    let file = dir.join("doc.pdf");
    let renamed = dir.join("renamed.pdf");
    fs::rename(&file, &renamed).unwrap();
    assert_eq!(s.doc_for_file(&renamed).unwrap(), Resolution::Known { doc });
    let sub = dir.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let moved = sub.join("moved.pdf");
    fs::rename(&renamed, &moved).unwrap();
    assert_eq!(s.doc_for_file(&moved).unwrap(), Resolution::Known { doc });
    // A copy elsewhere has a new file id: it resolves through its content (= head) and /ID.
    let other = dir.join("other");
    fs::create_dir_all(&other).unwrap();
    let copy = other.join("copy.pdf");
    fs::copy(&moved, &copy).unwrap();
    assert_eq!(s.doc_for_file(&copy).unwrap(), Resolution::Known { doc });
    // Same /ID, other content, other path: ask.
    let edited = other.join("edited.pdf");
    let mut b = fs::read(&copy).unwrap();
    b.extend_from_slice(b"% changed elsewhere\n");
    fs::write(&edited, &b).unwrap();
    assert_eq!(s.doc_for_file(&edited).unwrap(), Resolution::Ambiguous { candidates: vec![doc] });
    s.confirm_same(doc, &edited).unwrap();
    assert_eq!(s.doc_for_file(&edited).unwrap(), Resolution::Known { doc });
    let ext = s.check_external(doc, &edited).unwrap().unwrap();
    assert_eq!(ext.kind, Kind::External);
    assert!(s.check_external(doc, &edited).unwrap().is_none());
    // An unrelated file is new.
    let unrelated = other.join("new.pdf");
    fs::write(&unrelated, pdf(2, "99")).unwrap();
    assert_eq!(s.doc_for_file(&unrelated).unwrap(), Resolution::New);
    assert_eq!(identity::pdf_id(&pdf(1, "beef")), Some(vec![0xbe, 0xef]));
}

#[test]
fn relocate_keeps_every_version_readable() {
    let (dir, root, s, doc) = setup("relocate", &pdf(8, "06"));
    let seq = edits(&pdf(8, "06"), 3, true);
    let mut ids = vec![s.versions(doc).unwrap()[0].id];
    for b in &seq[1..] {
        ids.push(s.commit(doc, b, meta(Kind::Save, "s")).unwrap().id);
    }
    s.name(doc, ids[1], Some("first".into())).unwrap();
    let new_root = dir.join("moved-store");
    assert!(s.relocate(&root.join("inside")).is_err());
    s.relocate(&new_root).unwrap();
    assert_eq!(s.root(), std::path::absolute(&new_root).unwrap());
    for (id, b) in ids.iter().zip(&seq) {
        assert!(s.read(doc, *id).unwrap() == *b);
    }
    assert!(matches!(Store::open(&root), Err(HistoryError::Moved(_))));
    drop(s);
    let s = Store::open(&new_root).unwrap();
    assert_eq!(s.versions(doc).unwrap().iter().find(|v| v.id == ids[1]).unwrap().name.as_deref(), Some("first"));
    for (id, b) in ids.iter().zip(&seq) {
        assert!(s.read(doc, *id).unwrap() == *b);
    }
}

#[test]
fn lock_save_copy_and_crash_recovery() {
    let (dir, root, s, doc) = setup("session", &pdf(3, "07"));
    assert!(matches!(Store::open(&root), Err(HistoryError::Locked)));
    let s = Arc::new(s);
    let head = s.versions(doc).unwrap()[0].id;
    let dest = dir.join("copy.pdf");
    s.save_copy(doc, head, &dest, false).unwrap();
    assert!(fs::read(&dest).unwrap() == s.read(doc, head).unwrap());
    assert!(matches!(s.save_copy(doc, head, &dest, false), Err(HistoryError::Exists)));
    s.save_copy(doc, head, &dest, true).unwrap();

    // Clean session: nothing to recover.
    let g = s.begin_session(doc).unwrap();
    let mut b = s.read(doc, head).unwrap();
    b.extend_from_slice(b"% unsaved\n");
    s.autosave(doc, &b).unwrap();
    g.end();
    drop(s);
    let s = Arc::new(Store::open(&root).unwrap());
    assert!(s.pending_recovery(doc).unwrap().is_none());

    // Crash: the process dies with the session marker still there.
    let g = s.begin_session(doc).unwrap();
    b.extend_from_slice(b"% more\n");
    let auto = s.autosave(doc, &b).unwrap();
    let marker = root.join("docs").join(doc.to_string()).join("session");
    let pid = fs::read(&marker).unwrap();
    drop(g);
    drop(s);
    fs::write(&marker, pid).unwrap();
    let s = Arc::new(Store::open(&root).unwrap());
    assert_eq!(s.pending_recovery(doc).unwrap().map(|v| v.id), Some(auto.id));
    assert!(s.read(doc, auto.id).unwrap() == b);
    // Saved after the autosave: nothing to offer.
    b.extend_from_slice(b"% saved\n");
    s.commit(doc, &b, meta(Kind::Save, "Save")).unwrap();
    assert!(s.pending_recovery(doc).unwrap().is_none());
}

/// Child of `kill_mid_write`: commits forever until killed.
#[test]
#[ignore]
fn kill_child() {
    let Some(root) = std::env::var_os("INKWELL_KILL_ROOT") else { return };
    let root = PathBuf::from(root);
    let s = Store::open(root.join("store")).unwrap();
    let doc = match s.docs().first() {
        Some(d) => *d,
        None => s.attach(&root.join("src.pdf")).unwrap(),
    };
    for i in 0.. {
        let n = s.versions(doc).unwrap().len() + i;
        let v = s.commit(doc, &kill_gen(n), meta(Kind::Save, &format!("i={n}"))).unwrap();
        if n % 3 == 0 {
            s.name(doc, v.id, Some(format!("n{n}"))).unwrap();
        }
        if n % 7 == 0 {
            s.autosave(doc, &kill_gen(n + 100_000)).unwrap();
        }
    }
}

fn kill_gen(n: usize) -> Vec<u8> {
    static BASE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let mut b = BASE.get_or_init(|| pdf(40, "08")).clone();
    let mut block = vec![0u8; 24 << 10];
    blake3::Hasher::new().update(&n.to_le_bytes()).finalize_xof().fill(&mut block);
    let at = n.wrapping_mul(7919) % b.len();
    b.splice(at..at, block);
    b
}

#[test]
fn kill_mid_write() {
    let dir = tmp("kill");
    fs::write(dir.join("src.pdf"), kill_gen(usize::MAX / 2)).unwrap();
    let exe = std::env::current_exe().unwrap();
    let mut max_versions = 0;
    let mut interrupted = 0;
    // Versions are immutable: check each one after the kill that follows its commit, and all at the end.
    let mut checked = HashSet::new();
    for round in 0..50 {
        let mut child = std::process::Command::new(&exe)
            .args(["history::tests::kill_child", "--exact", "--ignored", "--test-threads=1"])
            .env("INKWELL_KILL_ROOT", &dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let wait = 150 + u16::from_le_bytes(crypto::random::<2>()) as u64 % 400;
        std::thread::sleep(std::time::Duration::from_millis(wait));
        child.kill().unwrap();
        child.wait().unwrap();

        let root = dir.join("store");
        if !root.join("store.json").exists() {
            continue;
        }
        let s = Store::open(&root).unwrap();
        let rep = s.fsck().unwrap();
        assert_eq!(rep.bad_manifests, 0, "round {round}: {rep:?}");
        interrupted += (rep.temp_files_removed + rep.wals_kept + rep.wals_rolled_back > 0) as u32;
        assert!(all_files(&root).iter().all(|f| !f.to_string_lossy().contains(".tmp-")), "round {round}: temp files left");
        for d in s.docs() {
            let vs = s.versions(d).unwrap();
            max_versions = max_versions.max(vs.len());
            for v in vs {
                assert!(!v.broken, "round {round}: broken version");
                if round < 49 && !checked.insert(v.id) {
                    continue;
                }
                let got = s.read(d, v.id).unwrap();
                if let Some(n) = v.label.strip_prefix("i=") {
                    assert!(got == kill_gen(n.parse().unwrap()), "round {round}: wrong bytes");
                }
            }
        }
    }
    eprintln!("{max_versions} versions; {interrupted} of 50 kills landed mid-write");
    assert!(max_versions >= 20, "the child barely committed ({max_versions} versions)");
    assert!(interrupted >= 5, "kills never landed mid-write ({interrupted})");
}
