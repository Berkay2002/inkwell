//! The on-disk store. Layout, crash-safety order and the privacy model are in docs/history.md.

use super::crypto::{self, Keys, Obj};
use super::identity::{self, FileId};
use super::retention::{plan_retention, shed_oldest};
use super::types::*;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;

const FORMAT: u32 = 1;

// Test-only failure point, for data-loss paths that are otherwise hard to hit.
#[cfg(test)]
thread_local! {
    pub(super) static FAIL_AT: std::cell::Cell<&'static str> = const { std::cell::Cell::new("") };
}

fn fault(_at: &str) -> Result<()> {
    #[cfg(test)]
    if FAIL_AT.with(|f| f.get()) == _at {
        return Err(std::io::Error::other(format!("injected failure at {_at}")).into());
    }
    Ok(())
}
/// FastCDC bounds (min, avg, max), picked by `history_bench` (docs/history.md).
pub const CDC: (u32, u32, u32) = (4 << 10, 16 << 10, 64 << 10);

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Splits bytes into content-defined chunks.
pub fn chunks(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    fastcdc::v2020::FastCDC::new(bytes, CDC.0, CDC.1, CDC.2).map(move |c| &bytes[c.offset..c.offset + c.length])
}

#[derive(Serialize, Deserialize)]
struct StoreFile {
    format: u32,
    policy: Policy,
}

/// identity.json: keyed hashes only (index key), plus the NTFS file id.
#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase", default)]
struct Identity {
    file_id: Option<FileId>,
    path_hash: Option<Hash>,
    pdf_ids: Vec<Hash>,
    /// Index-keyed hash of the head version's bytes (finds copies of the file).
    head_hash: Option<Hash>,
    keep_pre_redaction: bool,
}

#[derive(Serialize, Deserialize)]
struct Wal {
    version: VersionId,
    chunks: Vec<ChunkId>,
}

/// "Never keep history" record: any shared hash matches.
#[derive(Serialize, Deserialize, Clone)]
struct Tombstone {
    hashes: Vec<Hash>,
}

struct Entry {
    m: Manifest,
    broken: bool,
}

struct DocState {
    keys: Keys,
    identity: Identity,
    versions: BTreeMap<VersionId, Entry>,
    names: HashMap<VersionId, Option<String>>,
    /// A manifest or the names record failed to decrypt: prune and GC leave this doc alone.
    has_bad: bool,
}

impl DocState {
    fn head(&self) -> Option<&Manifest> {
        self.versions.values().next_back().map(|e| &e.m)
    }

    fn info(&self, e: &Entry) -> VersionInfo {
        let m = &e.m;
        VersionInfo {
            id: m.id,
            parents: m.parents.clone(),
            created: m.created,
            author: m.author.clone(),
            kind: m.kind.clone(),
            label: m.label.clone(),
            summary: m.summary.clone(),
            name: self.names.get(&m.id).cloned().unwrap_or_else(|| m.name.clone()),
            pages_touched: m.pages_touched.clone(),
            size: m.size,
            broken: e.broken,
        }
    }

    fn infos(&self) -> Vec<VersionInfo> {
        self.versions.values().rev().map(|e| self.info(e)).collect()
    }
}

struct Inner {
    root: PathBuf,
    _lock: File,
    index_key: Zeroizing<[u8; 32]>,
    policy: Mutex<Policy>,
    never: Mutex<Vec<Tombstone>>,
    /// Docs whose session marker was left by a process that died.
    stale: Mutex<HashSet<DocId>>,
}

type Slot = Arc<Mutex<Option<DocState>>>;

/// One per process. Every method is safe to call from any thread.
pub struct Store {
    inner: RwLock<Inner>,
    docs: Mutex<HashMap<DocId, Slot>>,
}

/// Removes the doc's session marker when dropped.
pub struct SessionGuard {
    store: Arc<Store>,
    doc: DocId,
}

impl SessionGuard {
    pub fn end(self) {}
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let inn = self.store.inner.read();
        let _ = fs::remove_file(doc_dir(&inn.root, self.doc).join("session"));
    }
}

pub fn default_root() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Inkwell").join("history")
}

fn doc_dir(root: &Path, doc: DocId) -> PathBuf {
    root.join("docs").join(doc.to_string())
}

fn manifest_path(dir: &Path, v: VersionId) -> PathBuf {
    dir.join("versions").join(format!("{v}.v"))
}

fn chunk_path(dir: &Path, c: ChunkId) -> PathBuf {
    let s = c.to_string();
    dir.join("chunks").join(&s[..2]).join(s)
}

fn is_temp(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.contains(".tmp-"))
}

/// Best effort: NTFS journals renames, but ask for the directory to be flushed anyway.
fn sync_dir(dir: &Path) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;
    if let Ok(f) = OpenOptions::new().write(true).custom_flags(FILE_FLAG_BACKUP_SEMANTICS).open(dir) {
        let _ = f.sync_all();
    }
}

/// temp in the same dir, fsync, rename. The directory is synced by the caller where it matters.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().expect("store paths have a parent");
    fs::create_dir_all(dir)?;
    let name = path.file_name().expect("file name").to_string_lossy();
    let tmp = dir.join(format!("{name}.tmp-{}", hex8()));
    let r = (|| {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(r?)
}

fn hex8() -> String {
    crypto::random::<4>().iter().map(|b| format!("{b:02x}")).collect()
}

/// Overwrite then delete (best effort against plain undelete; SSDs may keep old cells, see docs).
fn destroy_file(path: &Path) -> Result<()> {
    if let Ok(meta) = fs::metadata(path) {
        if let Ok(mut f) = OpenOptions::new().write(true).open(path) {
            let _ = f.write_all(&vec![0u8; meta.len() as usize]);
            let _ = f.sync_all();
        }
        fs::remove_file(path)?;
        if let Some(d) = path.parent() {
            sync_dir(d);
        }
    }
    Ok(())
}

/// `remove_file` / `remove_dir_all` where "already gone" is success.
fn remove_any(path: &Path) -> Result<()> {
    let r = if path.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) };
    match r {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(b) => Ok(Some(serde_json::from_slice(&b)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn lock_root(root: &Path) -> Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    fs::create_dir_all(root)?;
    OpenOptions::new().read(true).write(true).create(true).truncate(false).share_mode(0).open(root.join("lock")).map_err(
        |e| if e.raw_os_error() == Some(32) { HistoryError::Locked } else { e.into() },
    )
}

fn new_version_id(after: Option<&Manifest>) -> VersionId {
    let ms = now_ms().max(after.map_or(0, |m| m.id.millis() + 1));
    let mut b = [0u8; 16];
    b[..6].copy_from_slice(&ms.to_be_bytes()[2..]);
    b[6..].copy_from_slice(&crypto::random::<10>());
    VersionId(b)
}

impl Inner {
    fn ikey(&self, tag: &str, bytes: &[u8]) -> Hash {
        let mut h = blake3::Hasher::new_keyed(&self.index_key);
        h.update(tag.as_bytes());
        h.update(&[0]);
        h.update(bytes);
        Hash(*h.finalize().as_bytes())
    }

    fn fid_hash(&self, f: FileId) -> Hash {
        let mut b = f.volume.to_le_bytes().to_vec();
        b.extend_from_slice(&f.id.to_le_bytes());
        self.ikey("fid", &b)
    }

    /// The hashes a file is known by (for tombstones): path, file id, /ID.
    fn file_hashes(&self, path: &Path, bytes: Option<&[u8]>) -> Vec<Hash> {
        let mut v = vec![self.ikey("path", identity::normalized_path(path).as_bytes())];
        v.extend(identity::file_id(path).map(|f| self.fid_hash(f)));
        v.extend(bytes.and_then(identity::pdf_id).map(|id| self.ikey("id", &id)));
        v
    }

    fn excluded(&self, hashes: &[Hash]) -> bool {
        self.never.lock().iter().any(|t| t.hashes.iter().any(|h| hashes.contains(h)))
    }

    fn save_never(&self, never: &[Tombstone]) -> Result<()> {
        write_atomic(&self.root.join("never.json"), &serde_json::to_vec(never)?)?;
        sync_dir(&self.root);
        Ok(())
    }

    fn doc_ids(&self) -> Vec<DocId> {
        let mut v: Vec<DocId> = fs::read_dir(self.root.join("docs"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse().ok())
            .collect();
        v.sort();
        v
    }

    fn write_identity(&self, doc: DocId, id: &Identity) -> Result<()> {
        write_atomic(&doc_dir(&self.root, doc).join("identity.json"), &serde_json::to_vec(id)?)
    }

    /// Recovery for one doc: temp files, WAL, manifests. Returns its state.
    fn fsck_doc(&self, doc: DocId, rep: &mut FsckReport) -> Result<DocState> {
        let dir = doc_dir(&self.root, doc);
        if !dir.is_dir() {
            return Err(HistoryError::NotFound);
        }
        let mut files = Vec::new();
        walk(&dir, &mut files);
        for f in files.iter().filter(|f| is_temp(f)) {
            fs::remove_file(f)?;
            rep.temp_files_removed += 1;
        }
        let wrapped = match fs::read(dir.join("key.dpapi")) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // A shred that was interrupted after the key went: finish it.
                fs::remove_dir_all(&dir)?;
                return Err(HistoryError::NotFound);
            }
            Err(e) => return Err(e.into()),
        };
        let keys = Keys::new(&*crypto::unprotect(&wrapped)?);
        let identity: Identity = read_json(&dir.join("identity.json"))?.unwrap_or_default();

        if let Some(wal) = read_json::<Wal>(&dir.join("wal")).ok().flatten() {
            let ok = self
                .load_manifest(doc, &dir, &keys, wal.version)
                .is_ok_and(|m| m.chunks.iter().all(|(c, len)| read_chunk(doc, &dir, &keys, *c, *len).is_ok()));
            if ok {
                rep.wals_kept += 1;
            } else {
                let _ = fs::remove_file(manifest_path(&dir, wal.version));
                rep.wals_rolled_back += 1;
            }
        }
        // An unparseable WAL is a torn write of the WAL itself: nothing was written after it.
        let _ = fs::remove_file(dir.join("wal"));

        // Unreadable names would make named versions look prunable: mark the doc damaged instead.
        let (names, names_bad) = match fs::read(dir.join("names")) {
            Ok(b) => match keys.open(doc, Obj::Names, b"names", &b).and_then(|p| Ok(serde_json::from_slice(&p)?)) {
                Ok(n) => (n, false),
                Err(_) => (HashMap::new(), true),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (HashMap::new(), false),
            Err(e) => return Err(e.into()),
        };

        let mut st = DocState { keys, identity, versions: BTreeMap::new(), names, has_bad: names_bad };
        for e in fs::read_dir(dir.join("versions")).into_iter().flatten().flatten() {
            let p = e.path();
            let Some(v) = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<VersionId>().ok()) else { continue };
            match p.extension().and_then(|x| x.to_str()) {
                Some("v") => {}
                Some("bad") => {
                    st.has_bad = true;
                    continue;
                }
                _ => continue,
            }
            match self.load_manifest(doc, &dir, &st.keys, v) {
                Ok(m) => {
                    let broken = m.chunks.iter().any(|(c, _)| !chunk_path(&dir, *c).is_file());
                    rep.broken_versions += broken as u32;
                    st.versions.insert(v, Entry { m, broken });
                }
                Err(_) => {
                    // Never delete what we can't read: move it aside and report it.
                    fs::rename(&p, p.with_extension("bad"))?;
                    st.has_bad = true;
                    rep.bad_manifests += 1;
                }
            }
        }
        Ok(st)
    }

    fn load_manifest(&self, doc: DocId, dir: &Path, keys: &Keys, v: VersionId) -> Result<Manifest> {
        let b = fs::read(manifest_path(dir, v))?;
        let m: Manifest = serde_json::from_slice(&keys.open(doc, Obj::Manifest, &v.0, &b)?)?;
        if m.id != v {
            return Err(HistoryError::Integrity("manifest id mismatch".into()));
        }
        Ok(m)
    }

    fn read_version(&self, doc: DocId, st: &DocState, v: VersionId) -> Result<Vec<u8>> {
        let e = st.versions.get(&v).ok_or(HistoryError::NotFound)?;
        let dir = doc_dir(&self.root, doc);
        let mut out = Vec::with_capacity(e.m.size as usize);
        for (c, len) in &e.m.chunks {
            out.extend_from_slice(&read_chunk(doc, &dir, &st.keys, *c, *len)?);
        }
        if out.len() as u64 != e.m.size || st.keys.content_hash(&out) != e.m.content_hash {
            return Err(HistoryError::Integrity("version content does not match its hash".into()));
        }
        Ok(out)
    }

    fn commit(&self, doc: DocId, st: &mut DocState, bytes: &[u8], meta: NewVersion) -> Result<VersionInfo> {
        let content_hash = st.keys.content_hash(bytes);
        let head = st.head();
        let force = (meta.kind == Kind::Checkpoint && !meta.label.is_empty()) || meta.name.is_some();
        if let Some(h) = head.filter(|h| h.content_hash == content_hash && !force) {
            let id = h.id;
            // A save of what was just autosaved: the autosave becomes the Save (the explicit save is the
            // version people look for). Same id, time and content; the manifest is replaced atomically.
            if meta.kind == Kind::Save && h.kind == Kind::Autosave {
                let mut m = h.clone();
                (m.author, m.kind, m.label, m.summary, m.pages_touched) = (meta.author, meta.kind, meta.label, meta.summary, meta.pages_touched);
                let dir = doc_dir(&self.root, doc);
                write_atomic(&manifest_path(&dir, id), &st.keys.seal(doc, Obj::Manifest, &id.0, &serde_json::to_vec(&m)?))?;
                sync_dir(&dir.join("versions"));
                st.versions.get_mut(&id).unwrap().m = m;
            }
            return Ok(st.info(&st.versions[&id]));
        }
        let dir = doc_dir(&self.root, doc);
        let pieces: Vec<&[u8]> = chunks(bytes).collect();
        let ids: Vec<ChunkId> = pieces.iter().map(|p| st.keys.chunk_id(p)).collect();
        let m = Manifest {
            id: new_version_id(head),
            parents: head.map(|h| vec![h.id]).unwrap_or_default(),
            created: now_ms(),
            author: meta.author,
            kind: meta.kind,
            label: meta.label,
            summary: meta.summary,
            name: meta.name,
            pages_touched: meta.pages_touched,
            size: bytes.len() as u64,
            content_hash,
            chunks: ids.iter().zip(&pieces).map(|(c, p)| (*c, p.len() as u32)).collect(),
        };

        // 1. WAL, 2. missing chunks, 3. manifest, 4. directory sync, 5. drop the WAL.
        write_atomic(&dir.join("wal"), &serde_json::to_vec(&Wal { version: m.id, chunks: ids.clone() })?)?;
        sync_dir(&dir);
        let mut written = HashSet::new();
        for (c, p) in ids.iter().zip(&pieces) {
            let path = chunk_path(&dir, *c);
            if !path.is_file() && written.insert(*c) {
                write_atomic(&path, &st.keys.seal(doc, Obj::Chunk, &c.0, p))?;
            }
        }
        for d in written.iter().filter_map(|c| chunk_path(&dir, *c).parent().map(Path::to_path_buf)).collect::<HashSet<_>>() {
            sync_dir(&d);
        }
        write_atomic(&manifest_path(&dir, m.id), &st.keys.seal(doc, Obj::Manifest, &m.id.0, &serde_json::to_vec(&m)?))?;
        sync_dir(&dir.join("versions"));
        fault("wal-delete")?;
        fs::remove_file(dir.join("wal"))?;

        st.identity.head_hash = Some(self.ikey("content", bytes));
        if let Some(id) = identity::pdf_id(bytes).map(|i| self.ikey("id", &i)) {
            if !st.identity.pdf_ids.contains(&id) {
                st.identity.pdf_ids.push(id);
            }
        }
        // Only lookups depend on this; a crash here costs a "same document?" prompt at worst.
        let _ = self.write_identity(doc, &st.identity);

        let id = m.id;
        st.versions.insert(id, Entry { m, broken: false });
        Ok(st.info(&st.versions[&id]))
    }

    /// Deletes the doc's versions, chunks, names and WAL (not the key, not the identity).
    fn remove_contents(&self, doc: DocId) -> Result<()> {
        let dir = doc_dir(&self.root, doc);
        for sub in ["versions", "chunks", "names", "wal"] {
            remove_any(&dir.join(sub))?;
        }
        Ok(())
    }

    fn new_key(&self, doc: DocId) -> Result<Keys> {
        let key = Zeroizing::new(crypto::random::<32>());
        let dir = doc_dir(&self.root, doc);
        write_atomic(&dir.join("key.dpapi"), &crypto::protect(&key)?)?;
        sync_dir(&dir);
        Ok(Keys::new(&key))
    }

    /// Unique on-disk bytes of each doc's chunks (stat'ed once).
    fn chunk_sizes(&self, doc: DocId, st: &DocState) -> HashMap<ChunkId, u64> {
        let dir = doc_dir(&self.root, doc);
        let mut sizes = HashMap::new();
        for e in st.versions.values() {
            for (c, _) in &e.m.chunks {
                sizes.entry(*c).or_insert_with(|| fs::metadata(chunk_path(&dir, *c)).map_or(0, |m| m.len()));
            }
        }
        sizes
    }
}

fn read_chunk(doc: DocId, dir: &Path, keys: &Keys, c: ChunkId, len: u32) -> Result<Vec<u8>> {
    let b = fs::read(chunk_path(dir, c)).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => HistoryError::Integrity(format!("chunk {c} is missing")),
        _ => e.into(),
    })?;
    let p = keys.open(doc, Obj::Chunk, &c.0, &b)?;
    if p.len() != len as usize || keys.chunk_id(&p) != c {
        return Err(HistoryError::Integrity(format!("chunk {c} does not match its id")));
    }
    Ok(p)
}

fn cost_of(st: &DocState, sizes: &HashMap<ChunkId, u64>, keep: &HashSet<VersionId>) -> u64 {
    let mut seen = HashSet::new();
    keep.iter()
        .filter_map(|v| st.versions.get(v))
        .flat_map(|e| &e.m.chunks)
        .filter(|(c, _)| seen.insert(*c))
        .map(|(c, _)| sizes.get(c).copied().unwrap_or(0))
        .sum()
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)?.flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if e.file_type()?.is_dir() {
            copy_tree(&src, &dst)?;
        } else if e.file_name() != "lock" && !is_temp(&src) {
            fs::copy(&src, &dst)?;
            OpenOptions::new().write(true).open(&dst)?.sync_all()?;
        }
    }
    Ok(())
}

impl Store {
    /// Opens (or creates) the store at `root`, taking its lock file. A second process gets `Locked`.
    pub fn open(root: impl AsRef<Path>) -> Result<Store> {
        let root = root.as_ref().to_path_buf();
        if !root.join("store.json").exists() {
            if let Ok(to) = fs::read_to_string(root.join("moved-to.txt")) {
                return Err(HistoryError::Moved(to));
            }
        }
        let lock = lock_root(&root)?;
        for f in fs::read_dir(&root)?.flatten() {
            if is_temp(&f.path()) {
                fs::remove_file(f.path())?;
            }
        }
        let sf = match read_json::<StoreFile>(&root.join("store.json"))? {
            Some(sf) if sf.format > FORMAT => {
                return Err(HistoryError::Invalid(format!("history store format {} is newer than this Inkwell", sf.format)));
            }
            Some(sf) => sf,
            None => {
                let sf = StoreFile { format: FORMAT, policy: Policy::default() };
                write_atomic(&root.join("store.json"), &serde_json::to_vec_pretty(&sf)?)?;
                sf
            }
        };
        let index_key = match fs::read(root.join("index.key")) {
            Ok(b) => crypto::unprotect(&b)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let k = Zeroizing::new(crypto::random());
                write_atomic(&root.join("index.key"), &crypto::protect(&k)?)?;
                k
            }
            Err(e) => return Err(e.into()),
        };
        sync_dir(&root);
        let never = read_json(&root.join("never.json"))?.unwrap_or_default();
        let inner = Inner {
            root,
            _lock: lock,
            index_key,
            policy: Mutex::new(sf.policy),
            never: Mutex::new(never),
            stale: Mutex::new(HashSet::new()),
        };
        // We hold the lock, so every session marker belongs to a process that is gone.
        let stale = inner.doc_ids().into_iter().filter(|d| doc_dir(&inner.root, *d).join("session").exists()).collect();
        *inner.stale.lock() = stale;
        Ok(Store { inner: RwLock::new(inner), docs: Mutex::new(HashMap::new()) })
    }

    pub fn root(&self) -> PathBuf {
        self.inner.read().root.clone()
    }

    pub fn policy(&self) -> Policy {
        self.inner.read().policy.lock().clone()
    }

    pub fn set_policy(&self, p: Policy) -> Result<()> {
        let inn = self.inner.read();
        let sf = StoreFile { format: FORMAT, policy: p.clone() };
        write_atomic(&inn.root.join("store.json"), &serde_json::to_vec_pretty(&sf)?)?;
        *inn.policy.lock() = p;
        Ok(())
    }

    fn slot(&self, doc: DocId) -> Slot {
        self.docs.lock().entry(doc).or_default().clone()
    }

    /// Runs `f` under the doc's lock, running fsck on first use. If `f` fails, the cached state is
    /// dropped (disk and cache may disagree after a partial write), so the next use reloads via fsck.
    fn with_doc<R>(&self, doc: DocId, f: impl FnOnce(&Inner, &mut DocState) -> Result<R>) -> Result<R> {
        let inn = self.inner.read();
        let slot = self.slot(doc);
        let mut g = slot.lock();
        if g.is_none() {
            *g = Some(inn.fsck_doc(doc, &mut FsckReport::default())?);
        }
        let r = f(&inn, g.as_mut().expect("loaded above"));
        if r.is_err() {
            *g = None;
        }
        r
    }

    /// Runs recovery on every document now (otherwise it runs on each doc's first use). A doc
    /// that can't be opened (e.g. its key no longer unwraps) is listed in `skipped`.
    pub fn fsck(&self) -> Result<FsckReport> {
        let inn = self.inner.read();
        let mut rep = FsckReport::default();
        for doc in inn.doc_ids() {
            let slot = self.slot(doc);
            let mut g = slot.lock();
            *g = match inn.fsck_doc(doc, &mut rep) {
                Ok(st) => Some(st),
                Err(HistoryError::NotFound) => None,
                Err(_) => {
                    rep.skipped.push(doc);
                    None
                }
            };
        }
        Ok(rep)
    }

    /// Every document that has a history.
    pub fn docs(&self) -> Vec<DocId> {
        self.inner.read().doc_ids()
    }

    pub fn doc_for_file(&self, path: &Path) -> Result<Resolution> {
        let inn = self.inner.read();
        let bytes = fs::read(path)?;
        if inn.excluded(&inn.file_hashes(path, Some(&bytes))) {
            return Ok(Resolution::Excluded);
        }
        let ids: Vec<(DocId, Identity)> = inn
            .doc_ids()
            .into_iter()
            .filter_map(|d| Some((d, read_json(&doc_dir(&inn.root, d).join("identity.json")).ok()??)))
            .collect();
        let fid = identity::file_id(path);
        if let Some((d, _)) = ids.iter().find(|(_, i)| fid.is_some() && i.file_id == fid) {
            return Ok(Resolution::Known { doc: *d });
        }
        let content = inn.ikey("content", &bytes);
        let pid = identity::pdf_id(&bytes).map(|i| inn.ikey("id", &i));
        let ph = inn.ikey("path", identity::normalized_path(path).as_bytes());
        let same: Vec<DocId> = ids.iter().filter(|(_, i)| i.head_hash == Some(content)).map(|(d, _)| *d).collect();
        let by_id: Vec<&(DocId, Identity)> = ids.iter().filter(|(_, i)| pid.is_some_and(|p| i.pdf_ids.contains(&p))).collect();
        let known = match (same.as_slice(), by_id.iter().filter(|(_, i)| i.path_hash == Some(ph)).collect::<Vec<_>>().as_slice()) {
            ([d], _) => *d,
            ([], [(d, _)]) => *d,
            ([], _) if by_id.is_empty() => return Ok(Resolution::New),
            ([], _) => return Ok(Resolution::Ambiguous { candidates: by_id.iter().map(|(d, _)| *d).collect() }),
            (many, _) => return Ok(Resolution::Ambiguous { candidates: many.to_vec() }),
        };
        drop(inn);
        // Found by content or /ID + path: remember where the file is now.
        self.confirm_same(known, path)?;
        Ok(Resolution::Known { doc: known })
    }

    /// Starts a history for the file and records its current bytes (kind Import).
    pub fn attach(&self, path: &Path) -> Result<DocId> {
        let bytes = fs::read(path)?;
        let doc = {
            let inn = self.inner.read();
            if inn.excluded(&inn.file_hashes(path, Some(&bytes))) {
                return Err(HistoryError::NeverKeep);
            }
            let doc = DocId(crypto::random());
            inn.new_key(doc)?;
            let identity = Identity {
                file_id: identity::file_id(path),
                path_hash: Some(inn.ikey("path", identity::normalized_path(path).as_bytes())),
                ..Default::default()
            };
            inn.write_identity(doc, &identity)?;
            doc
        };
        let meta = NewVersion {
            author: Author::External,
            kind: Kind::Import,
            label: "Started keeping history".into(),
            summary: vec![],
            name: None,
            pages_touched: vec![],
        };
        self.commit(doc, &bytes, meta)?;
        Ok(doc)
    }

    /// The user answered "same document": the file at `path` now belongs to `doc`.
    pub fn confirm_same(&self, doc: DocId, path: &Path) -> Result<()> {
        let bytes = fs::read(path)?;
        self.with_doc(doc, |inn, st| {
            st.identity.file_id = identity::file_id(path);
            st.identity.path_hash = Some(inn.ikey("path", identity::normalized_path(path).as_bytes()));
            if let Some(id) = identity::pdf_id(&bytes).map(|i| inn.ikey("id", &i)) {
                if !st.identity.pdf_ids.contains(&id) {
                    st.identity.pdf_ids.push(id);
                }
            }
            inn.write_identity(doc, &st.identity)
        })
    }

    pub fn commit(&self, doc: DocId, bytes: &[u8], meta: NewVersion) -> Result<VersionInfo> {
        self.with_doc(doc, |inn, st| inn.commit(doc, st, bytes, meta))
    }

    pub fn autosave(&self, doc: DocId, bytes: &[u8]) -> Result<VersionInfo> {
        let meta = NewVersion {
            author: Author::Autosave,
            kind: Kind::Autosave,
            label: "Autosave".into(),
            summary: vec![],
            name: None,
            pages_touched: vec![],
        };
        self.commit(doc, bytes, meta)
    }

    /// Newest first.
    pub fn versions(&self, doc: DocId) -> Result<Vec<VersionInfo>> {
        self.with_doc(doc, |_, st| Ok(st.infos()))
    }

    /// Byte-exact content, verified against the version's hash.
    pub fn read(&self, doc: DocId, v: VersionId) -> Result<Vec<u8>> {
        self.with_doc(doc, |inn, st| inn.read_version(doc, st, v))
    }

    /// Records `v`'s content as a new version on top. Writing the file is the caller's job.
    pub fn restore(&self, doc: DocId, v: VersionId, author: Author) -> Result<VersionInfo> {
        self.with_doc(doc, |inn, st| {
            let bytes = inn.read_version(doc, st, v)?;
            let meta = NewVersion {
                author,
                kind: Kind::Restore { from: v },
                label: "Restored an earlier version".into(),
                summary: vec![],
                name: None,
                pages_touched: vec![],
            };
            inn.commit(doc, st, &bytes, meta)
        })
    }

    /// Writes `v` to `dest` atomically. Refuses to replace an existing file unless `overwrite`.
    pub fn save_copy(&self, doc: DocId, v: VersionId, dest: &Path, overwrite: bool) -> Result<()> {
        use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW};
        let bytes = self.read(doc, v)?;
        let dir = dest.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let tmp = dir.join(format!(".{}.tmp-{}", dest.file_name().unwrap_or_default().to_string_lossy(), hex8()));
        let r = (|| {
            let mut f = File::create(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            drop(f);
            let wide = |p: &Path| {
                use std::os::windows::ffi::OsStrExt;
                p.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>()
            };
            let flags = MOVEFILE_WRITE_THROUGH | if overwrite { MOVEFILE_REPLACE_EXISTING } else { 0 };
            // SAFETY: both are NUL-terminated wide strings that outlive the call.
            if unsafe { MoveFileExW(wide(&tmp).as_ptr(), wide(dest).as_ptr(), flags) } == 0 {
                let e = std::io::Error::last_os_error();
                return Err(if matches!(e.raw_os_error(), Some(80 | 183)) { HistoryError::Exists } else { e.into() });
            }
            Ok(())
        })();
        if r.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        r
    }

    /// Names (or un-names with `None`) a version. Named versions are kept forever.
    pub fn name(&self, doc: DocId, v: VersionId, name: Option<String>) -> Result<VersionInfo> {
        self.with_doc(doc, |inn, st| {
            if !st.versions.contains_key(&v) {
                return Err(HistoryError::NotFound);
            }
            let mut names = st.names.clone();
            names.insert(v, name.filter(|n| !n.trim().is_empty()));
            let sealed = st.keys.seal(doc, Obj::Names, b"names", &serde_json::to_vec(&names)?);
            write_atomic(&doc_dir(&inn.root, doc).join("names"), &sealed)?;
            st.names = names;
            Ok(st.info(&st.versions[&v]))
        })
    }

    /// If the file differs from the head, records it as "Changed outside Inkwell".
    pub fn check_external(&self, doc: DocId, path: &Path) -> Result<Option<VersionInfo>> {
        let bytes = fs::read(path)?;
        self.with_doc(doc, |inn, st| {
            if st.head().is_some_and(|h| h.content_hash == st.keys.content_hash(&bytes)) {
                return Ok(None);
            }
            let meta = NewVersion {
                author: Author::External,
                kind: Kind::External,
                label: "Changed outside Inkwell".into(),
                summary: vec![],
                name: None,
                pages_touched: vec![],
            };
            inn.commit(doc, st, &bytes, meta).map(Some)
        })
    }

    /// Marks the doc as open in this process until the guard drops.
    pub fn begin_session(self: &Arc<Self>, doc: DocId) -> Result<SessionGuard> {
        self.with_doc(doc, |inn, _| {
            write_atomic(&doc_dir(&inn.root, doc).join("session"), std::process::id().to_string().as_bytes())?;
            inn.stale.lock().remove(&doc);
            Ok(())
        })?;
        Ok(SessionGuard { store: self.clone(), doc })
    }

    /// After a crash: the newest autosave, if it is newer than the last save / outside change.
    /// Call before `begin_session` (which clears the crash flag).
    pub fn pending_recovery(&self, doc: DocId) -> Result<Option<VersionInfo>> {
        if !self.inner.read().stale.lock().contains(&doc) {
            return Ok(None);
        }
        self.with_doc(doc, |_, st| {
            let on_disk = |k: &Kind| matches!(k, Kind::Save | Kind::External | Kind::Import);
            let last_disk = st.versions.values().rev().find(|e| on_disk(&e.m.kind)).map(|e| e.m.id);
            let auto = st.versions.values().rev().find(|e| e.m.kind == Kind::Autosave);
            Ok(auto.filter(|a| Some(a.m.id) > last_disk).map(|a| st.info(a)))
        })
    }

    /// Crypto-shred: the key goes first, then everything else; the identity is forgotten.
    pub fn shred(&self, doc: DocId) -> Result<()> {
        let inn = self.inner.read();
        let slot = self.slot(doc);
        let mut g = slot.lock();
        let dir = doc_dir(&inn.root, doc);
        if !dir.exists() {
            return Err(HistoryError::NotFound);
        }
        *g = None;
        destroy_file(&dir.join("key.dpapi"))?;
        fs::remove_dir_all(&dir)?;
        sync_dir(&inn.root.join("docs"));
        inn.stale.lock().remove(&doc);
        drop(g);
        self.docs.lock().remove(&doc);
        Ok(())
    }

    pub fn delete_history(&self, doc: DocId) -> Result<()> {
        self.shred(doc)
    }

    /// After the redacted file was written, in place of the Save commit, as one locked operation
    /// (so `check_external` can't slip an unredacted file in between). `keep_history = false`
    /// (default): shred every earlier version, start a fresh key and record `redacted` as the first
    /// version. `true`: keep it, flag the doc "pre-redaction history kept", and record `redacted`.
    pub fn redactions_applied(&self, doc: DocId, keep_history: bool, redacted: &[u8], meta: NewVersion) -> Result<VersionInfo> {
        self.with_doc(doc, |inn, st| {
            if keep_history {
                st.identity.keep_pre_redaction = true;
                inn.write_identity(doc, &st.identity)?;
                return inn.commit(doc, st, redacted, meta);
            }
            let dir = doc_dir(&inn.root, doc);
            // The new key goes beside the old one first, so the doc is keyless only between the
            // destroy and the rename (a crash there: fsck finishes the shred).
            let key = Zeroizing::new(crypto::random::<32>());
            let tmp = dir.join(format!("key.dpapi.tmp-{}", hex8()));
            let mut f = File::create(&tmp)?;
            f.write_all(&crypto::protect(&key)?)?;
            f.sync_all()?;
            drop(f);
            // Forget the old key and versions in memory before anything is deleted (drop zeroizes).
            st.keys = Keys::new(&key);
            st.versions.clear();
            st.names.clear();
            st.has_bad = false;
            st.identity.head_hash = None;
            st.identity.keep_pre_redaction = false;
            destroy_file(&dir.join("key.dpapi"))?;
            fault("rekey")?;
            fs::rename(&tmp, dir.join("key.dpapi"))?;
            sync_dir(&dir);
            inn.remove_contents(doc)?;
            inn.write_identity(doc, &st.identity)?;
            inn.commit(doc, st, redacted, meta)
        })
    }

    pub fn keeps_pre_redaction(&self, doc: DocId) -> Result<bool> {
        self.with_doc(doc, |_, st| Ok(st.identity.keep_pre_redaction))
    }

    /// "Never keep history for this file": shreds its history and records a keyed-hash tombstone
    /// (path, file id, /ID) so it is never recorded again. `false` lifts it (by path). A path that
    /// resolves `Ambiguous` fails with `HistoryError::Ambiguous(candidates)` and writes nothing:
    /// ask which history, then call again with `Target::Doc`.
    pub fn set_never_keep(&self, target: Target, never: bool) -> Result<()> {
        let (hashes, doc) = {
            let inn = self.inner.read();
            match &target {
                Target::Path(p) => {
                    let bytes = fs::read(p).ok();
                    (inn.file_hashes(p, bytes.as_deref()), None)
                }
                Target::Doc(d) => {
                    let id: Identity = read_json(&doc_dir(&inn.root, *d).join("identity.json"))?.ok_or(HistoryError::NotFound)?;
                    let mut h: Vec<Hash> = id.path_hash.into_iter().chain(id.pdf_ids).collect();
                    h.extend(id.file_id.and_then(identity::usable).map(|f| inn.fid_hash(f)));
                    (h, Some(*d))
                }
            }
        };
        let doc = match (doc, &target) {
            (Some(d), _) => Some(d),
            // Ambiguous: writing the tombstone would leave the matching history behind. Ask first.
            (None, Target::Path(p)) if never => match self.doc_for_file(p)? {
                Resolution::Known { doc } => Some(doc),
                Resolution::Ambiguous { candidates } => return Err(HistoryError::Ambiguous(candidates)),
                Resolution::New | Resolution::Excluded => None,
            },
            _ => None,
        };
        let inn = self.inner.read();
        let mut list = inn.never.lock().clone();
        if never {
            list.push(Tombstone { hashes });
        } else {
            list.retain(|t| !t.hashes.iter().any(|h| hashes.contains(h)));
        }
        inn.save_never(&list)?;
        *inn.never.lock() = list;
        drop(inn);
        if let Some(d) = doc.filter(|_| never) {
            self.shred(d)?;
        }
        Ok(())
    }

    /// Applies the retention policy (and the caps), then GC. Docs that can't be opened are left
    /// alone and listed in `skipped`.
    pub fn prune(&self) -> Result<Maintenance> {
        let policy = self.policy();
        let now = now_ms();
        let mut out = Maintenance::default();
        let mut total: Vec<(DocId, Vec<VersionInfo>, HashMap<ChunkId, u64>)> = Vec::new();
        for doc in self.docs() {
            let r = self.with_doc(doc, |inn, st| {
                // Plan from the manifests on disk, not a possibly stale cache.
                *st = inn.fsck_doc(doc, &mut FsckReport::default())?;
                if st.has_bad {
                    return Ok((0, Vec::new(), HashMap::new()));
                }
                let sizes = inn.chunk_sizes(doc, st);
                let keep = plan_retention(&st.infos(), &policy, now, |k| cost_of(st, &sizes, k));
                let n = remove_versions(inn, doc, st, |v| !keep.contains(v))?;
                Ok((n, st.infos(), sizes))
            });
            if let Some((n, vs, sizes)) = per_doc(r, doc, &mut out.skipped) {
                out.pruned += n;
                total.push((doc, vs, sizes));
            }
        }
        if policy.total_cap_bytes > 0 {
            // ponytail: recomputes the whole cost per dropped version (O(n²)); index chunk refcounts if stores get huge.
            let mut keep: HashSet<(DocId, VersionId)> =
                total.iter().flat_map(|(d, vs, _)| vs.iter().map(move |v| (*d, v.id))).collect();
            let cands = total
                .iter()
                .flat_map(|(d, vs, _)| {
                    let head = vs.first().map(|v| v.id);
                    vs.iter().filter(move |v| Some(v.id) != head && v.name.is_none()).map(move |v| (v.id.millis(), (*d, v.id)))
                })
                .collect();
            let chunk_lists: HashMap<DocId, HashMap<VersionId, Vec<ChunkId>>> = total
                .iter()
                .map(|(d, _, _)| {
                    let m = self.with_doc(*d, |_, st| {
                        Ok(st.versions.iter().map(|(v, e)| (*v, e.m.chunks.iter().map(|c| c.0).collect())).collect())
                    });
                    (*d, m.unwrap_or_default())
                })
                .collect();
            let cost = |keep: &HashSet<(DocId, VersionId)>| -> u64 {
                let mut seen = HashSet::new();
                let mut sum = 0;
                for (d, v) in keep {
                    let sizes = &total.iter().find(|t| t.0 == *d).expect("doc listed").2;
                    for c in chunk_lists.get(d).and_then(|m| m.get(v)).into_iter().flatten() {
                        if seen.insert((*d, *c)) {
                            sum += sizes.get(c).copied().unwrap_or(0);
                        }
                    }
                }
                sum
            };
            shed_oldest(cands, &mut keep, |k| cost(k) > policy.total_cap_bytes);
            for (doc, vs, _) in &total {
                let drop: HashSet<VersionId> = vs.iter().map(|v| v.id).filter(|v| !keep.contains(&(*doc, *v))).collect();
                if !drop.is_empty() {
                    let r = self.with_doc(*doc, |inn, st| remove_versions(inn, *doc, st, |v| drop.contains(v)));
                    out.pruned += per_doc(r, *doc, &mut out.skipped).unwrap_or(0);
                }
            }
        }
        let g = self.gc()?;
        out.freed_bytes = g.freed_bytes;
        for d in g.skipped {
            if !out.skipped.contains(&d) {
                out.skipped.push(d);
            }
        }
        Ok(out)
    }

    /// Mark-and-sweep: deletes chunk files no manifest of their doc references. Runs under each
    /// doc's lock, so no commit of that doc is in flight; the live set comes from the manifests on
    /// disk (reloaded), not the cache. Docs with a pending WAL or an unreadable manifest are left
    /// alone; docs that can't be opened are listed in `skipped`.
    pub fn gc(&self) -> Result<Maintenance> {
        let mut out = Maintenance::default();
        for doc in self.docs() {
            let r = self.with_doc(doc, |inn, st| {
                let dir = doc_dir(&inn.root, doc);
                if dir.join("wal").exists() {
                    return Ok(0);
                }
                *st = inn.fsck_doc(doc, &mut FsckReport::default())?;
                if st.has_bad {
                    return Ok(0);
                }
                let live: HashSet<String> =
                    st.versions.values().flat_map(|e| e.m.chunks.iter().map(|c| c.0.to_string())).collect();
                let mut files = Vec::new();
                walk(&dir.join("chunks"), &mut files);
                let mut n = 0;
                for f in files {
                    let name = f.file_name().and_then(|s| s.to_str()).unwrap_or_default().to_string();
                    if !is_temp(&f) && !live.contains(&name) {
                        n += fs::metadata(&f).map_or(0, |m| m.len());
                        fs::remove_file(&f)?;
                    }
                }
                Ok(n)
            });
            out.freed_bytes += per_doc(r, doc, &mut out.skipped).unwrap_or(0);
        }
        Ok(out)
    }

    pub fn usage(&self) -> Result<Usage> {
        let mut u = Usage::default();
        for doc in self.docs() {
            let r = self.with_doc(doc, |inn, st| {
                let mut files = Vec::new();
                walk(&doc_dir(&inn.root, doc), &mut files);
                let chunks = files.iter().filter(|f| f.parent().and_then(|p| p.parent()).is_some_and(|p| p.ends_with("chunks"))).count();
                Ok(DocUsage {
                    doc: Some(doc),
                    bytes: files.iter().map(|f| fs::metadata(f).map_or(0, |m| m.len())).sum(),
                    versions: st.versions.len() as u64,
                    chunks: chunks as u64,
                })
            });
            if let Some(d) = per_doc(r, doc, &mut u.skipped) {
                u.total += d.bytes;
                u.chunks += d.chunks;
                u.versions += d.versions;
                u.per_doc.push(d);
            }
        }
        Ok(u)
    }

    /// Moves the store: copy, verify every manifest and chunk at the new place, switch, then
    /// remove the old tree (leaving `moved-to.txt`). On any failure the old store is untouched.
    /// After the switch, every old key is destroyed first, then the rest is removed; returns the
    /// old files that could not be removed (empty when the old tree is clean).
    pub fn relocate(&self, new_root: &Path) -> Result<Vec<PathBuf>> {
        let mut inn = self.inner.write();
        let old = inn.root.clone();
        let new_root = std::path::absolute(new_root)?;
        let old_abs = std::path::absolute(&old)?;
        if new_root.starts_with(&old_abs) || old_abs.starts_with(&new_root) {
            return Err(HistoryError::Invalid("the new location must be outside the current one".into()));
        }
        if fs::read_dir(&new_root).is_ok_and(|mut d| d.next().is_some()) {
            return Err(HistoryError::Invalid("the new location is not empty".into()));
        }
        let lock = lock_root(&new_root)?;
        let r = copy_tree(&old, &new_root).and_then(|_| verify_tree(&new_root));
        if let Err(e) = r {
            drop(lock);
            let _ = fs::remove_dir_all(&new_root);
            return Err(e);
        }
        let sf = StoreFile { format: FORMAT, policy: inn.policy.lock().clone() };
        write_atomic(&new_root.join("store.json"), &serde_json::to_vec_pretty(&sf)?)?;
        sync_dir(&new_root);
        let old_lock = std::mem::replace(&mut inn._lock, lock);
        inn.root = new_root.clone();
        // The move has happened: from here on, failures only leave old files behind, so they
        // must not be reported as a failed move (the caller would keep pointing at the old place).
        let _ = fs::write(old.join("moved-to.txt"), new_root.to_string_lossy().as_bytes());
        let mut left = Vec::new();
        // Keys first: whatever then can't be deleted is undecryptable.
        let mut keys = vec![old.join("index.key")];
        keys.extend(fs::read_dir(old.join("docs")).into_iter().flatten().flatten().map(|e| e.path().join("key.dpapi")));
        for k in keys {
            if destroy_file(&k).is_err() {
                left.push(k);
            }
        }
        for e in fs::read_dir(&old).into_iter().flatten().flatten() {
            let p = e.path();
            if matches!(e.file_name().to_str(), Some("lock" | "moved-to.txt")) {
                continue;
            }
            // File by file, so one file held open doesn't keep the rest.
            let mut files = vec![p.clone()];
            if p.is_dir() {
                files.clear();
                walk(&p, &mut files);
            }
            left.extend(files.into_iter().filter(|f| remove_any(f).is_err()));
            let _ = remove_any(&p);
        }
        drop(old_lock);
        let _ = fs::remove_file(old.join("lock"));
        left.sort();
        left.dedup();
        Ok(left)
    }
}

/// One doc's result in a store-wide pass: NotFound (shredded meanwhile) is nothing; any other
/// error (e.g. a key DPAPI no longer unwraps) skips the doc instead of failing the whole pass.
fn per_doc<T>(r: Result<T>, doc: DocId, skipped: &mut Vec<DocId>) -> Option<T> {
    match r {
        Ok(t) => Some(t),
        Err(HistoryError::NotFound) => None,
        Err(_) => {
            if !skipped.contains(&doc) {
                skipped.push(doc);
            }
            None
        }
    }
}

fn remove_versions(inn: &Inner, doc: DocId, st: &mut DocState, drop: impl Fn(&VersionId) -> bool) -> Result<usize> {
    let head = st.head().map(|h| h.id);
    let named = |v: &VersionId| st.names.get(v).cloned().unwrap_or_else(|| st.versions[v].m.name.clone()).is_some();
    let gone: Vec<VersionId> = st.versions.keys().copied().filter(|v| drop(v) && Some(*v) != head && !named(v)).collect();
    let dir = doc_dir(&inn.root, doc);
    for v in &gone {
        fs::remove_file(manifest_path(&dir, *v))?;
        st.versions.remove(v);
    }
    if !gone.is_empty() {
        sync_dir(&dir.join("versions"));
    }
    Ok(gone.len())
}

/// Every manifest decrypts and every chunk it references checks, under `root`.
fn verify_tree(root: &Path) -> Result<()> {
    crypto::unprotect(&fs::read(root.join("index.key"))?)?;
    for e in fs::read_dir(root.join("docs")).into_iter().flatten().flatten() {
        let Some(doc) = e.file_name().to_str().and_then(|s| s.parse::<DocId>().ok()) else { continue };
        let dir = e.path();
        let keys = Keys::new(&*crypto::unprotect(&fs::read(dir.join("key.dpapi"))?)?);
        let mut seen = HashSet::new();
        for v in fs::read_dir(dir.join("versions")).into_iter().flatten().flatten() {
            let p = v.path();
            if p.extension().and_then(|x| x.to_str()) != Some("v") {
                continue;
            }
            let id: VersionId = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default().parse()?;
            let m: Manifest = serde_json::from_slice(&keys.open(doc, Obj::Manifest, &id.0, &fs::read(&p)?)?)?;
            for (c, len) in m.chunks {
                if seen.insert(c) && chunk_path(&dir, c).is_file() {
                    read_chunk(doc, &dir, &keys, c, len)?;
                }
            }
        }
    }
    Ok(())
}
