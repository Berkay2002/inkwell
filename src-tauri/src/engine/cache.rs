//! LRU tile cache bounded by bytes.

use super::DocId;
use lru::LruCache;
use std::sync::Arc;

/// Identifies one rendered tile. `scale` is the render scale in thousandths (the UI quantises zoom
/// into a few buckets so tiles are reused while zooming).
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct TileKey {
    pub doc: DocId,
    /// The document's edit generation when the tile was asked for.
    pub rev: u32,
    pub page: u32,
    pub rot: u8,
    pub scale: u32,
    pub tx: u16,
    pub ty: u16,
}

/// Straight (non-premultiplied) RGBA, opaque, row-major, no padding.
#[derive(Clone)]
pub struct Tile {
    pub w: u32,
    pub h: u32,
    pub rgba: Arc<Vec<u8>>,
}

pub struct TileCache {
    map: LruCache<TileKey, Tile>,
    bytes: usize,
    budget: usize,
}

impl TileCache {
    pub fn new(budget: usize) -> Self {
        Self { map: LruCache::unbounded(), bytes: 0, budget }
    }

    pub fn get(&mut self, k: &TileKey) -> Option<Tile> {
        self.map.get(k).cloned()
    }

    pub fn insert(&mut self, k: TileKey, t: Tile) {
        let n = t.rgba.len();
        if let Some(old) = self.map.put(k, t) {
            self.bytes -= old.rgba.len();
        }
        self.bytes += n;
        self.evict();
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    pub fn set_budget(&mut self, budget: usize) {
        self.budget = budget;
        self.evict();
    }

    pub fn drop_doc(&mut self, doc: DocId) {
        let keys: Vec<_> = self.map.iter().filter(|(k, _)| k.doc == doc).map(|(k, _)| *k).collect();
        for k in keys {
            if let Some(t) = self.map.pop(&k) {
                self.bytes -= t.rgba.len();
            }
        }
    }

    fn evict(&mut self) {
        // Always keep the newest entry, even if it alone exceeds the budget.
        while self.bytes > self.budget && self.map.len() > 1 {
            if let Some((_, t)) = self.map.pop_lru() {
                self.bytes -= t.rgba.len();
            }
        }
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
}
