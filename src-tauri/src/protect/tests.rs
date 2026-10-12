use super::*;
use crate::engine::edit::{EditOp, EditSource};
use crate::engine::fixture::{self, Fixture};
use crate::engine::{DocId, Engine, EngineError};
use mupdf::Document;
use std::path::{Path, PathBuf};

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join(format!("protect-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn fixture_at(d: &Path, name: &str, pages: usize) -> String {
    let p = d.join(name);
    std::fs::write(&p, fixture::build(&Fixture { pages, lines: 2, ..Default::default() })).unwrap();
    p.to_string_lossy().into_owned()
}

fn spec() -> ProtectSpec {
    ProtectSpec {
        open_password: Some("open sesame".into()),
        permissions_password: Some("owner-secret".into()),
        printing: Printing::Low,
        changes: Changes::Comments,
        copy: false,
        accessibility: true,
        cipher: Cipher::Aes256,
    }
}

/// `spec()` with `f` applied (ProtectSpec wipes its passwords on drop, so no struct update syntax).
fn tweak(f: impl FnOnce(&mut ProtectSpec)) -> ProtectSpec {
    let mut s = spec();
    f(&mut s);
    s
}

fn open(eng: &Engine, path: &str) -> DocId {
    eng.open(path).unwrap().doc
}

/// Opens `path` the way the UI does: asked for a password, then given one.
fn open_with(eng: &Engine, path: &str, password: &str) -> DocId {
    let (id, r) = eng.open_pending(path);
    assert!(matches!(r, Err(EngineError::NeedsPassword)), "{r:?}");
    eng.authenticate(id, password).unwrap();
    id
}

fn invalid_code(r: Result<impl std::fmt::Debug, EngineError>) -> String {
    match r {
        Err(EngineError::Invalid(m)) => m,
        other => panic!("expected an Invalid error, got {other:?}"),
    }
}

/// The raw /P and /R of the file at `path`, read with a fresh MuPDF document.
fn encrypt_of(path: &str) -> (i32, i32) {
    let doc = mupdf::pdf::PdfDocument::open(path).unwrap();
    let enc = doc.trailer().unwrap().get_dict("Encrypt").unwrap().expect("an /Encrypt dictionary");
    let int = |k: &str| enc.get_dict(k).unwrap().unwrap().as_int().unwrap();
    (int("P"), int("R"))
}

#[test]
fn spec_validation() {
    let mut s = spec();
    assert_eq!(s.validate(), Ok(()));

    s.open_password = None;
    s.permissions_password = None;
    assert_eq!(s.validate(), Err(SpecError::NothingToSet));
    // empty strings are no password
    s.open_password = Some(String::new());
    assert_eq!(s.validate(), Err(SpecError::NothingToSet));

    let mut s = spec();
    s.permissions_password = s.open_password.clone();
    assert_eq!(s.validate(), Err(SpecError::SamePassword));

    let mut s = spec();
    s.open_password = Some("x".repeat(128));
    assert_eq!(s.validate(), Err(SpecError::TooLong));
    s.open_password = Some("x".repeat(127));
    assert_eq!(s.validate(), Ok(()));

    // restrictions need the permissions password
    let mut s = spec();
    s.permissions_password = None;
    assert_eq!(s.validate(), Err(SpecError::NeedsPermissionsPassword));
    // an open password alone, nothing restricted, is fine
    s.printing = Printing::High;
    s.changes = Changes::AllButExtract;
    s.copy = true;
    assert_eq!(s.validate(), Ok(()));

    let mut s = spec();
    s.cipher = Cipher::Aes128;
    s.open_password = Some("pässword".into());
    assert_eq!(s.validate(), Err(SpecError::PlainTextOnly));
    s.cipher = Cipher::Aes256;
    assert_eq!(s.validate(), Ok(()));
}

#[test]
fn rights_map_to_bits_and_back() {
    for printing in [Printing::None, Printing::Low, Printing::High] {
        for changes in [Changes::None, Changes::Pages, Changes::Forms, Changes::Comments, Changes::AllButExtract] {
            for copy in [false, true] {
                let s = tweak(|s| (s.printing, s.changes, s.copy, s.accessibility) = (printing, changes, copy, false));
                let r = s.rights();
                // as MuPDF writes it: only the permission bits survive, the reserved ones are set
                let p = (r.permission().bits() & 0xf3c) | 0xfffff0c0;
                let back = Rights::from_p(p as i32);
                assert_eq!(back, r, "{printing:?} {changes:?} copy={copy}");
                assert_eq!(back.changes(), changes);
                assert_eq!(back.printing, printing);
                // copying implies screen-reader access, as in Acrobat
                assert_eq!(back.accessibility, copy);
            }
        }
    }
    assert!(Rights::from_p(-4).unrestricted());
    assert!(!Rights::from_p(0xfffff0c0u32 as i32).unrestricted());
}

#[test]
fn bits_9_to_12_of_a_revision_2_file_follow_the_older_bits() {
    // print and copy only, 9 to 12 clear (the way an old writer leaves them)
    let p = 0xfffff0c0u32 as i32 | 0x4 | 0x10;
    let r3 = Rights::from_p_r(p, Some(3));
    assert!(!r3.forms && !r3.assemble && !r3.accessibility && r3.printing == Printing::Low);
    assert_eq!(Rights::from_p_r(p, None), r3, "an unknown revision is read as the current one");
    for r in [Some(2), Some(1)] {
        let old = Rights::from_p_r(p, r);
        // copy allows screen readers (bit 10 = 5) and sets printing quality; page changes follow modify (11 = 4)
        // and form filling follows annotate (9 = 6): neither is set here
        assert!(old.copy && old.accessibility && old.printing == Printing::High, "{r:?}");
        assert!(!old.modify && !old.assemble && !old.annotate && !old.forms, "{r:?}");
        assert!(!old.may_extract(), "copy without any change right is not extractable: {r:?}");
        // modify on: page changes follow it
        let m = Rights::from_p_r(p | 0x8, r);
        assert!(m.modify && m.assemble && !m.forms && m.may_extract(), "{r:?}");
        // annotate on: forms follow it; the stale bits 9 to 12 of the number are ignored
        let a = Rights::from_p_r(p | 0x20, r);
        assert!(a.annotate && a.forms && !a.assemble, "{r:?}");
        let stale = Rights::from_p_r(p | 0xf00, r);
        assert!(!stale.forms && !stale.assemble, "bits 9 to 12 as written do not count below revision 3: {r:?}");
    }
}

#[test]
fn debug_hides_passwords() {
    let s = format!("{:?}", spec());
    assert!(!s.contains("sesame") && !s.contains("owner-secret"), "{s}");
}

/// Protect, Save as, reopen: both passwords, the rights and the cipher survive on disk.
#[test]
fn aes256_round_trip_through_save_and_reopen() {
    let d = dir("aes256");
    let src = fixture_at(&d, "a.pdf", 3);
    let out = d.join("locked.pdf").to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &src);

    assert_eq!(eng.security_info(id).unwrap(), SecurityInfo::plain());
    let s = eng.protect(id, spec(), None, EditSource::User).unwrap();
    assert!(s.dirty);
    assert_eq!(s.undo.as_deref(), Some("Protect with a password (AES-256)"));
    let info = eng.security_info(id).unwrap();
    assert_eq!((info.origin, info.unsaved, info.method.as_deref()), (Origin::Set, true, Some("AES-256")));

    let (state, saved) = eng.save_recorded(id, Some(&out)).unwrap();
    assert!(!state.dirty);
    let saved = saved.unwrap();
    assert!(saved.moved);
    assert_eq!(saved.summary, vec!["Protect with a password (AES-256)".to_string()]);
    // the recorded bytes are the file
    assert_eq!(std::fs::read(&out).unwrap(), saved.bytes);
    assert!(!eng.security_info(id).unwrap().unsaved);
    // the original is untouched
    assert_eq!(std::fs::read(&src).unwrap(), fixture::build(&Fixture { pages: 3, lines: 2, ..Default::default() }));
    // the document keeps rendering from memory after the file under it became encrypted
    assert!(!eng.page_text(id, 2).unwrap().is_empty());

    // on disk: AES-256 revision 6, the P value of the sheet
    let (p, r) = encrypt_of(&out);
    assert_eq!(r, 6);
    let on_disk = Rights::from_p(p);
    assert_eq!((on_disk.printing, on_disk.changes(), on_disk.copy, on_disk.accessibility), (Printing::Low, Changes::Comments, false, true));
    assert!(on_disk.annotate && on_disk.forms && !on_disk.modify && !on_disk.assemble);

    // needs the open password; the permissions password also opens it
    let mut raw = Document::open(&out).unwrap();
    assert!(raw.needs_password().unwrap());
    assert!(!raw.authenticate("wrong").unwrap());
    assert!(raw.authenticate("open sesame").unwrap());
    assert_eq!(raw.page_count().unwrap(), 3);

    let eng2 = Engine::new(2, 64 << 20);
    let (id2, r) = eng2.open_pending(&out);
    assert!(matches!(r, Err(EngineError::NeedsPassword)));
    assert!(matches!(eng2.authenticate(id2, "nope"), Err(EngineError::BadPassword)));
    eng2.authenticate(id2, "open sesame").unwrap();
    let info = eng2.security_info(id2).unwrap();
    assert!(info.encrypted && info.open_password && info.restricted);
    assert_eq!((info.method.as_deref(), info.revision, info.origin), (Some("AES-256"), Some(6), Origin::File));
    // opened with the open password: restrictions hold, security cannot be changed without the other one
    assert!(!info.unlocked);
    assert_eq!(info.rights.printing, Printing::Low);
    assert!(!eng2.page_text(id2, 0).unwrap().is_empty());

    // opened with the permissions password: unlocked
    let eng3 = Engine::new(2, 64 << 20);
    let id3 = open_with(&eng3, &out, "owner-secret");
    assert!(eng3.security_info(id3).unwrap().unlocked);
}

#[test]
fn aes128_for_old_readers() {
    let d = dir("aes128");
    let src = fixture_at(&d, "a.pdf", 2);
    let out = d.join("old.pdf").to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &src);
    let s = tweak(|s| s.cipher = Cipher::Aes128);
    eng.protect(id, s, None, EditSource::User).unwrap();
    eng.save(id, Some(&out)).unwrap();
    assert_eq!(encrypt_of(&out).1, 4);
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = open_with(&eng2, &out, "open sesame");
    let info = eng2.security_info(id2).unwrap();
    assert_eq!((info.method.as_deref(), info.revision), (Some("AES-128"), Some(4)));
}

/// Only a permissions password: the file opens without asking, and the restrictions are known.
#[test]
fn permissions_password_only_opens_without_a_prompt() {
    let d = dir("owner-only");
    let src = fixture_at(&d, "a.pdf", 2);
    let out = d.join("restricted.pdf").to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &src);
    let s = ProtectSpec {
        open_password: None,
        permissions_password: Some("boss".into()),
        printing: Printing::None,
        changes: Changes::None,
        copy: false,
        accessibility: false,
        cipher: Cipher::Aes256,
    };
    eng.protect(id, s, None, EditSource::User).unwrap();
    eng.save(id, Some(&out)).unwrap();

    let eng2 = Engine::new(2, 64 << 20);
    let id2 = open(&eng2, &out);
    let info = eng2.security_info(id2).unwrap();
    assert!(info.encrypted && !info.open_password && info.restricted && !info.unlocked);
    assert_eq!(info.rights.printing, Printing::None);
    assert!(!info.rights.copy && !info.rights.modify);
    assert!(!eng2.page_text(id2, 0).unwrap().is_empty());
}

/// Other people's files: an edit and Save keep the encryption and the restrictions.
#[test]
fn saving_an_edit_keeps_the_restrictions() {
    let d = dir("keep");
    let src = fixture_at(&d, "a.pdf", 3);
    let locked = d.join("locked.pdf").to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &src);
    eng.protect(id, spec(), None, EditSource::User).unwrap();
    eng.save(id, Some(&locked)).unwrap();
    drop(eng);

    let eng = Engine::new(2, 64 << 20);
    let id = open_with(&eng, &locked, "open sesame");
    let before = encrypt_of(&locked);
    // an incremental edit, then a delete (full rewrite)
    eng.apply(id, EditOp::Rotate { pages: vec![0], quarter_turns: 1 }).unwrap();
    eng.save(id, None).unwrap();
    assert_eq!(encrypt_of(&locked), before);
    eng.apply(id, EditOp::Delete { pages: vec![2] }).unwrap();
    eng.save(id, None).unwrap();
    assert_eq!(encrypt_of(&locked), before);
    let mut raw = Document::open(&locked).unwrap();
    assert!(raw.authenticate("open sesame").unwrap());
    assert_eq!(raw.page_count().unwrap(), 2);
    // still restricted when read back
    let info = Engine::new(2, 64 << 20);
    let again = open_with(&info, &locked, "open sesame");
    assert!(info.security_info(again).unwrap().restricted);
}

#[test]
fn remove_security_needs_the_permissions_password() {
    let d = dir("remove");
    let src = fixture_at(&d, "a.pdf", 2);
    let locked = d.join("locked.pdf").to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &src);
    eng.protect(id, spec(), None, EditSource::User).unwrap();
    eng.save(id, Some(&locked)).unwrap();
    drop(eng);

    let eng = Engine::new(2, 64 << 20);
    let id = open_with(&eng, &locked, "open sesame");
    assert_eq!(invalid_code(eng.remove_security(id, None, EditSource::User)), "needs-permissions-password");
    // wrong, and the open password (which only lifts nothing)
    assert_eq!(invalid_code(eng.remove_security(id, Some("guess"), EditSource::User)), "bad-permissions-password");
    assert_eq!(invalid_code(eng.remove_security(id, Some("open sesame"), EditSource::User)), "bad-permissions-password");
    // nothing changed: no step was recorded
    assert!(!eng.edit_state(id).unwrap().dirty);

    let s = eng.remove_security(id, Some("owner-secret"), EditSource::User).unwrap();
    assert_eq!(s.undo.as_deref(), Some("Remove password security"));
    assert_eq!(eng.security_info(id).unwrap().origin, Origin::Removed);
    eng.save(id, None).unwrap();

    // the file opens without a password, and carries no /Encrypt
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = open(&eng2, &locked);
    assert_eq!(eng2.security_info(id2).unwrap(), SecurityInfo::plain());
    assert!(mupdf::pdf::PdfDocument::open(&locked).unwrap().trailer().unwrap().get_dict("Encrypt").unwrap().is_none());
    assert!(!eng2.page_text(id2, 1).unwrap().is_empty());
}

/// A session that opened the file with the permissions password changes security without asking.
#[test]
fn opened_as_owner_removes_without_asking() {
    let d = dir("owner");
    let src = fixture_at(&d, "a.pdf", 2);
    let locked = d.join("locked.pdf").to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &src);
    eng.protect(id, spec(), None, EditSource::User).unwrap();
    eng.save(id, Some(&locked)).unwrap();
    drop(eng);

    let eng = Engine::new(2, 64 << 20);
    let id = open_with(&eng, &locked, "owner-secret");
    eng.remove_security(id, None, EditSource::User).unwrap();
    // and a change of password is an edit like any other
    let s = tweak(|s| s.open_password = Some("new one".into()));
    let st = eng.protect(id, s, None, EditSource::User).unwrap();
    assert_eq!(st.undo.as_deref(), Some("Protect with a password (AES-256)"));
}

#[test]
fn changing_the_security_of_a_restricted_file_needs_the_permissions_password() {
    let d = dir("change");
    let src = fixture_at(&d, "a.pdf", 2);
    let locked = d.join("locked.pdf").to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &src);
    eng.protect(id, spec(), None, EditSource::User).unwrap();
    eng.save(id, Some(&locked)).unwrap();
    drop(eng);

    let eng = Engine::new(2, 64 << 20);
    let id = open_with(&eng, &locked, "open sesame");
    let relaxed = tweak(|s| (s.printing, s.changes, s.copy) = (Printing::High, Changes::AllButExtract, true));
    assert_eq!(invalid_code(eng.protect(id, relaxed.clone(), None, EditSource::User)), "needs-permissions-password");
    let s = eng.protect(id, relaxed, Some("owner-secret"), EditSource::User).unwrap();
    assert_eq!(s.undo.as_deref(), Some("Change password security (AES-256)"));
    eng.save(id, None).unwrap();
    let (p, _) = encrypt_of(&locked);
    assert!(Rights::from_p(p).unrestricted());
}

#[test]
fn undo_returns_to_the_unprotected_file() {
    let d = dir("undo");
    let src = fixture_at(&d, "a.pdf", 2);
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &src);
    eng.protect(id, spec(), None, EditSource::User).unwrap();
    assert!(eng.edit_state(id).unwrap().dirty);
    let s = eng.undo(id).unwrap();
    assert!(!s.dirty);
    assert_eq!(eng.security_info(id).unwrap(), SecurityInfo::plain());
    // nothing to save: the file is exactly as it was
    assert!(eng.save_recorded(id, None).unwrap().1.is_none());
    let s = eng.redo(id).unwrap();
    assert!(s.dirty);
    assert_eq!(eng.security_info(id).unwrap().origin, Origin::Set);
}

/// Edits after protecting are saved encrypted too (every save is a rewrite with the settings).
#[test]
fn edits_after_protecting_stay_protected() {
    let d = dir("after");
    let src = fixture_at(&d, "a.pdf", 3);
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &src);
    eng.protect(id, spec(), None, EditSource::User).unwrap();
    eng.apply(id, EditOp::Rotate { pages: vec![1], quarter_turns: 1 }).unwrap();
    eng.save(id, None).unwrap();
    assert_eq!(encrypt_of(&src).1, 6);
    // a second edit and save: still one encryption, the same rights
    eng.apply(id, EditOp::Delete { pages: vec![0] }).unwrap();
    eng.save(id, None).unwrap();
    let mut raw = Document::open(&src).unwrap();
    assert!(raw.authenticate("open sesame").unwrap());
    assert_eq!(raw.page_count().unwrap(), 2);
    // the open document is unharmed
    assert!(!eng.page_text(id, 0).unwrap().is_empty());
}

#[test]
fn a_file_without_security_has_nothing_to_remove() {
    let d = dir("none");
    let src = fixture_at(&d, "a.pdf", 1);
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &src);
    assert!(matches!(eng.remove_security(id, None, EditSource::User), Err(EngineError::Invalid(_))));
    assert_eq!(invalid_code(eng.protect(id, ProtectSpec { open_password: None, permissions_password: None, ..spec() }, None, EditSource::User)), "nothing-to-set");
}

#[test]
fn the_file_on_disk_is_read_with_its_new_password_by_the_tools() {
    // Compress and the like read the file on disk: after Save wrote a new password it is the new one.
    let d = dir("file-password");
    let src = fixture_at(&d, "a.pdf", 2);
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &src);
    assert_eq!(eng.file_password(id).unwrap(), None);

    eng.protect(id, spec(), None, EditSource::User).unwrap();
    // set but not written yet: the file on disk is still the plain one
    assert_eq!(eng.file_password(id).unwrap(), None);
    eng.save(id, None).unwrap();
    assert_eq!(eng.file_password(id).unwrap().as_ref().map(|p| p.as_str()), Some("open sesame"));
    let audit = crate::compress::audit_file(Path::new(&src), eng.file_password(id).unwrap().as_ref().map(|p| p.as_str())).unwrap();
    assert!(audit.total > 0);

    // removed and saved: no password again
    eng.remove_security(id, Some("owner-secret"), EditSource::User).unwrap();
    eng.save(id, None).unwrap();
    assert_eq!(eng.file_password(id).unwrap(), None);
    assert!(crate::compress::audit_file(Path::new(&src), None).is_ok());
}

/// What a fresh MuPDF document of `path` grants for `password` (2 user, 4 owner, 6 both, 0 neither).
fn access_of(path: &str, password: &str) -> i32 {
    mupdf::pdf::PdfDocument::open(path).unwrap().authenticate_access(password).unwrap()
}

/// An open password and nothing else must leave the file's owner password equal to it, as Acrobat does:
/// with an empty owner password MuPDF hashes "" into /O, and qpdf, pikepdf and pypdf accept "" as the
/// owner password, so anyone could lift the restrictions of a file that was never meant to have any.
#[test]
fn open_only_protection_makes_the_open_password_the_owner_password() {
    for cipher in [Cipher::Aes256, Cipher::Aes128] {
        let d = dir(&format!("owner-eq-{}", if cipher == Cipher::Aes256 { 256 } else { 128 }));
        let src = fixture_at(&d, "a.pdf", 2);
        let out = d.join("locked.pdf").to_string_lossy().into_owned();
        let eng = Engine::new(2, 64 << 20);
        let id = open(&eng, &src);
        let open_only = tweak(|s| {
            (s.permissions_password, s.printing, s.changes, s.copy, s.accessibility, s.cipher) =
                (None, Printing::High, Changes::AllButExtract, true, true, cipher);
        });
        eng.protect(id, open_only, None, EditSource::User).unwrap();
        eng.save(id, Some(&out)).unwrap();

        // /O is the hash of the open password: MuPDF's owner check, run with a non-empty guess, says so
        // (before the fix it was 2: the open password was only the user password).
        assert_eq!(access_of(&out, "open sesame"), 6);
        assert_eq!(access_of(&out, "") & 4, 0);
        assert_eq!(access_of(&out, "not it") & 4, 0);

        // and the session that opened it with that password may change the security, as the owner
        let eng2 = Engine::new(2, 64 << 20);
        let id2 = open_with(&eng2, &out, "open sesame");
        let info = eng2.security_info(id2).unwrap();
        assert!(info.encrypted && info.unlocked && !info.restricted, "{info:?}");
        eng2.remove_security(id2, None, EditSource::User).unwrap();
    }
}

/// With both passwords the open password is the user password only.
#[test]
fn the_open_password_is_not_the_owner_when_a_permissions_password_is_set() {
    let d = dir("owner-neq");
    let src = fixture_at(&d, "a.pdf", 1);
    let out = d.join("locked.pdf").to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &src);
    eng.protect(id, spec(), None, EditSource::User).unwrap();
    eng.save(id, Some(&out)).unwrap();
    assert_eq!(access_of(&out, "open sesame"), 2);
    assert_eq!(access_of(&out, "owner-secret") & 4, 4);
}

/// A file with its own owner password stays the owner's even when it restricts nothing (P = everything):
/// the rights alone must not decide who may change or remove the security.
#[test]
fn an_owner_password_with_every_right_allowed_still_guards_the_security() {
    let d = dir("owner-all");
    let src = fixture_at(&d, "a.pdf", 2);
    let locked = d.join("locked.pdf").to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &src);
    let all_allowed = tweak(|s| {
        (s.open_password, s.printing, s.changes, s.copy, s.accessibility) =
            (None, Printing::High, Changes::AllButExtract, true, true);
    });
    eng.protect(id, all_allowed, None, EditSource::User).unwrap();
    eng.save(id, Some(&locked)).unwrap();
    drop(eng);

    // opens without a prompt, restricts nothing, and is not the session's to change
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &locked);
    let info = eng.security_info(id).unwrap();
    assert!(info.encrypted && !info.restricted && !info.unlocked, "{info:?}");
    assert_eq!(invalid_code(eng.remove_security(id, None, EditSource::User)), "needs-permissions-password");
    assert_eq!(invalid_code(eng.remove_security(id, Some(""), EditSource::User)), "needs-permissions-password");
    assert_eq!(invalid_code(eng.remove_security(id, Some("guess"), EditSource::User)), "bad-permissions-password");
    let changed = tweak(|s| s.open_password = Some("new open".into()));
    assert_eq!(invalid_code(eng.protect(id, changed, None, EditSource::User)), "needs-permissions-password");
    assert!(!eng.edit_state(id).unwrap().dirty);
    eng.remove_security(id, Some("owner-secret"), EditSource::User).unwrap();
}

/// A failed save takes the protect step back: nothing stays pending, a later plain Save does not encrypt
/// the file, and a retry is one step.
#[test]
fn a_failed_protect_and_save_leaves_nothing_pending() {
    let d = dir("rollback");
    let src = fixture_at(&d, "a.pdf", 2);
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &src);

    let nowhere = d.join("no-such-folder").join("x.pdf").to_string_lossy().into_owned();
    assert!(eng.protect_and_save(id, spec(), None, Some(&nowhere)).is_err());
    let st = eng.edit_state(id).unwrap();
    assert!(!st.dirty && st.undo.is_none(), "{st:?}");
    assert_eq!(eng.security_info(id).unwrap(), SecurityInfo::plain());

    // Ctrl+S afterwards writes the document as it is: no protection nobody asked for
    eng.save(id, None).unwrap();
    assert!(mupdf::pdf::PdfDocument::open(&src).unwrap().trailer().unwrap().get_dict("Encrypt").unwrap().is_none());
    assert_eq!(eng.file_password(id).unwrap(), None);

    // the retry is one step, and undoing it goes back to the plain file
    let out = d.join("locked.pdf").to_string_lossy().into_owned();
    let (st, saved) = eng.protect_and_save(id, spec(), None, Some(&out)).unwrap();
    assert!(!st.dirty && saved.is_some());
    assert_eq!(st.undo.as_deref(), Some("Protect with a password (AES-256)"));
    assert_eq!(access_of(&out, "open sesame"), 2);
    eng.undo(id).unwrap();
    assert_eq!(eng.security_info(id).unwrap(), SecurityInfo::plain());
}

/// Pages and content may not leave a restricted file as an unprotected one without its permissions password.
#[test]
fn extract_split_and_merge_respect_the_restrictions() {
    let d = dir("extract");
    let src = fixture_at(&d, "a.pdf", 3);
    // permissions password only (opens without a prompt), no copying, no page changes
    let locked = d.join("locked.pdf").to_string_lossy().into_owned();
    // both passwords, same rights, to open as the owner
    let locked2 = d.join("locked2.pdf").to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &src);
    eng.protect(id, tweak(|s| s.open_password = None), None, EditSource::User).unwrap();
    eng.save(id, Some(&locked)).unwrap();
    eng.undo(id).unwrap();
    eng.protect(id, spec(), None, EditSource::User).unwrap();
    eng.save(id, Some(&locked2)).unwrap();
    drop(eng);

    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &locked);
    let out = d.join("out.pdf").to_string_lossy().into_owned();
    assert_eq!(invalid_code(eng.ensure_extractable(id).map(|_| 0)), "needs-permissions-password");
    assert_eq!(invalid_code(eng.extract(id, &[0], &out, false)), "needs-permissions-password");
    assert_eq!(invalid_code(eng.split(id, 1).map(|_| 0)), "needs-permissions-password");
    assert!(!Path::new(&out).exists());
    match eng.merge(std::slice::from_ref(&locked), &out) {
        Err(EngineError::Invalid(m)) => assert!(m.contains("doesn't allow copying"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(!Path::new(&out).exists());
    // taking the security off (staged, with the permissions password) frees the pages to leave
    eng.remove_security(id, Some("owner-secret"), EditSource::User).unwrap();
    eng.ensure_extractable(id).unwrap();
    eng.extract(id, &[0], &out, false).unwrap();
    assert_eq!(Document::open(&out).unwrap().page_count().unwrap(), 1);

    // the session that holds the permissions password is the owner's
    let eng = Engine::new(2, 64 << 20);
    let id = open_with(&eng, &locked2, "owner-secret");
    eng.ensure_extractable(id).unwrap();
    let out2 = d.join("out2.pdf").to_string_lossy().into_owned();
    eng.extract(id, &[1, 2], &out2, false).unwrap();
    assert_eq!(Document::open(&out2).unwrap().page_count().unwrap(), 2);
    // while the open password alone is not enough
    let eng = Engine::new(2, 64 << 20);
    let id = open_with(&eng, &locked2, "open sesame");
    assert_eq!(invalid_code(eng.extract(id, &[0], &out, false)), "needs-permissions-password");
}

/// Passwords are put in NFKC before they are written, and control characters (a NUL cuts a C string
/// short) are refused.
#[test]
fn passwords_are_normalised_and_control_characters_refused() {
    let mut s = tweak(|s| {
        s.open_password = Some("cafe\u{301}-pass".into());
        s.permissions_password = Some("\u{ff21}\u{ff22}\u{ff23}-owner".into());
    });
    s.normalize();
    assert_eq!(s.open(), Some("caf\u{e9}-pass"));
    assert_eq!(s.owner(), Some("ABC-owner"));
    s.validate().unwrap();

    for bad in ["a\0b-secret", "tab\there", "del\u{7f}x", "c1\u{85}x"] {
        let s = tweak(|s| s.open_password = Some(bad.into()));
        assert_eq!(s.validate().unwrap_err().code(), "control-character", "{bad:?}");
    }

    // end to end: typed decomposed, opens with the composed form (what Acrobat's field produces)
    let d = dir("nfkc");
    let src = fixture_at(&d, "a.pdf", 1);
    let out = d.join("locked.pdf").to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &src);
    let typed = tweak(|s| {
        (s.open_password, s.permissions_password, s.printing, s.changes, s.copy) =
            (Some("cafe\u{301}-pass".into()), None, Printing::High, Changes::AllButExtract, true);
    });
    eng.protect(id, typed, None, EditSource::User).unwrap();
    eng.save(id, Some(&out)).unwrap();
    assert_ne!(access_of(&out, "caf\u{e9}-pass") & 2, 0);

    // MuPDF does no SASLprep: opening tries the password as typed, then its NFKC form, so typing it
    // decomposed (or composed, as Acrobat's field gives it) both open the file
    for typed in ["cafe\u{301}-pass", "caf\u{e9}-pass"] {
        let eng2 = Engine::new(2, 64 << 20);
        let id2 = open_with(&eng2, &out, typed);
        assert!(eng2.security_info(id2).unwrap().encrypted);
        assert!(!eng2.page_text(id2, 0).unwrap().is_empty());
    }
    // a wrong password is still wrong
    let eng2 = Engine::new(2, 64 << 20);
    let (id2, _) = eng2.open_pending(&out);
    assert!(matches!(eng2.authenticate(id2, "cafe-pass"), Err(EngineError::BadPassword)));

    // the permissions password too: typed full-width, written as "OWNER-secret", and checked the same way
    let both = d.join("both.pdf").to_string_lossy().into_owned();
    let id = open(&eng, &src);
    let owner_typed = "\u{ff2f}\u{ff37}\u{ff2e}\u{ff25}\u{ff32}-secret";
    eng.protect(id, tweak(|s| s.permissions_password = Some(owner_typed.into())), None, EditSource::User).unwrap();
    eng.save(id, Some(&both)).unwrap();
    assert_eq!(access_of(&both, "OWNER-secret") & 4, 4);
    let eng3 = Engine::new(2, 64 << 20);
    let id3 = open_with(&eng3, &both, "open sesame");
    assert_eq!(invalid_code(eng3.remove_security(id3, Some("guess"), EditSource::User)), "bad-permissions-password");
    eng3.remove_security(id3, Some(owner_typed), EditSource::User).unwrap();
}

/// SASLprep's mapping step: a password typed with a non-ASCII space or an invisible mark opens a file written
/// with the plain one (what Acrobat and qpdf hash); the form as typed is still tried first.
#[test]
fn a_password_with_odd_spaces_or_invisible_marks_opens_the_file_under_its_mapped_form() {
    // NFKC leaves U+1680 and the B.1 characters alone, so only the mapping step reaches the plain form
    assert_eq!(crate::protect::nfkc_form("ab\u{1680}cd\u{ad}-ef\u{200d}\u{fe0f}").as_deref().map(String::as_str), Some("ab cd-ef"));
    assert_eq!(crate::protect::nfkc_form("a\u{a0}b\u{3000}c").as_deref().map(String::as_str), Some("a b c"));
    assert_eq!(crate::protect::nfkc_form("a\u{200b}b").as_deref().map(String::as_str), Some("ab"), "U+200B is in B.1 and C.1.2: nothing");
    assert!(crate::protect::nfkc_form("plain ascii-pass").is_none());
    assert!(crate::protect::nfkc_form("\u{200b}\u{ad}").is_none(), "all invisible: nothing to retry with");

    let d = dir("saslprep-map");
    let src = fixture_at(&d, "a.pdf", 1);
    let out = d.join("locked.pdf").to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &src);
    let plain = tweak(|s| {
        (s.open_password, s.permissions_password, s.printing, s.changes, s.copy) =
            (Some("ab cd-ef".into()), None, Printing::High, Changes::AllButExtract, true);
    });
    eng.protect(id, plain, None, EditSource::User).unwrap();
    eng.save(id, Some(&out)).unwrap();

    for typed in ["ab cd-ef", "ab\u{1680}cd\u{ad}-ef", "ab\u{200c}\u{a0}cd-ef\u{fe0f}"] {
        let eng2 = Engine::new(2, 64 << 20);
        let id2 = open_with(&eng2, &out, typed);
        assert!(eng2.security_info(id2).unwrap().encrypted, "{typed:?}");
        assert!(!eng2.page_text(id2, 0).unwrap().is_empty(), "{typed:?}");
    }
    // a different password is still wrong, and so is one that differs by more than the mapped characters
    let eng2 = Engine::new(2, 64 << 20);
    let (id2, _) = eng2.open_pending(&out);
    assert!(matches!(eng2.authenticate(id2, "abcd-ef"), Err(EngineError::BadPassword)));
}

/// The step and the save of Protect are one operation under one lock: edits made at the same time
/// neither get between them nor leave the step behind when the save fails.
#[test]
fn protect_and_save_cannot_be_interleaved_with_other_edits() {
    let d = dir("atomic");
    let src = fixture_at(&d, "a.pdf", 4);
    let nowhere = d.join("no-such-folder").join("x.pdf").to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = open(&eng, &src);
    std::thread::scope(|s| {
        let protecting = s.spawn(|| {
            for _ in 0..12 {
                assert!(eng.protect_and_save(id, spec(), None, Some(&nowhere)).is_err());
            }
        });
        let editing = s.spawn(|| {
            for i in 0..12u32 {
                eng.apply(id, EditOp::Rotate { pages: vec![i % 4], quarter_turns: 1 }).unwrap();
            }
        });
        protecting.join().unwrap();
        editing.join().unwrap();
    });
    // no security step survived, wherever the rotations landed
    assert_eq!(eng.security_info(id).unwrap(), SecurityInfo::plain());
    let mut undone = 0;
    while eng.edit_state(id).unwrap().undo.is_some() {
        eng.undo(id).unwrap();
        undone += 1;
    }
    assert_eq!(undone, 12);
}
