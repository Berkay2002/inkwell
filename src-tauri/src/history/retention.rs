//! Which versions to keep: all for `all_days`, then one per day for `daily_months`, then one per
//! month; named versions and the head always; then the oldest unnamed go first while over a cap.

use super::types::{Policy, VersionId, VersionInfo};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

const DAY: u64 = 86_400_000;

/// `cost(keep)` = bytes the kept set occupies (shared chunks counted once).
pub fn plan_retention(
    vs: &[VersionInfo],
    p: &Policy,
    now: u64,
    cost: impl Fn(&HashSet<VersionId>) -> u64,
) -> HashSet<VersionId> {
    let Some(head) = vs.iter().map(|v| v.id).max() else { return HashSet::new() };
    let pinned = |v: &VersionInfo| v.id == head || v.name.is_some();
    let all_until = p.all_days as u64 * DAY;
    let daily_until = all_until + p.daily_months as u64 * 30 * DAY;

    let mut keep = HashSet::new();
    // Bucket -> newest version in it. ponytail: UTC days/months; use the local zone if users notice.
    let mut buckets: HashMap<(u8, i64), VersionId> = HashMap::new();
    for v in vs {
        let age = now.saturating_sub(v.created);
        if pinned(v) || age <= all_until {
            keep.insert(v.id);
            continue;
        }
        let day = (v.created / DAY) as i64;
        let bucket = if age <= daily_until { (0, day) } else { (1, month_of(day)) };
        let e = buckets.entry(bucket).or_insert(v.id);
        if v.id > *e {
            *e = v.id;
        }
    }
    keep.extend(buckets.into_values());

    if p.per_doc_cap_bytes > 0 {
        let cands = vs.iter().filter(|v| !pinned(v)).map(|v| (v.id, v.id)).collect();
        shed_oldest(cands, &mut keep, |k| cost(k) > p.per_doc_cap_bytes);
    }
    keep
}

/// Drops candidates oldest first (by `order`) while `over(keep)`. Shared by the per-doc and the
/// total cap.
pub fn shed_oldest<K: Eq + Hash + Clone, O: Ord>(
    mut cands: Vec<(O, K)>,
    keep: &mut HashSet<K>,
    mut over: impl FnMut(&HashSet<K>) -> bool,
) {
    cands.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, k) in cands {
        if !over(keep) {
            break;
        }
        keep.remove(&k);
    }
}

/// Days since 1970-01-01 -> months since 1970-01 (civil calendar, H. Hinnant's algorithm).
pub(super) fn month_of(days: i64) -> i64 {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y - 1970) * 12 + (m - 1)
}

