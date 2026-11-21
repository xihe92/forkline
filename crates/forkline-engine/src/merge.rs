//! The merge rules applied when a join fires.
//!
//! Every map involved carries a dirty bitset relative to the same base (the
//! map at the join's owning divergence point):
//!
//! 1. Keys the join module wrote keep the join's value.
//! 2. Keys no path changed carry through from the base.
//! 3. Keys changed on exactly one path take that path's value (including
//!    removal, if the path unset it).
//! 4. Keys changed on two or more paths and not written by the join are
//!    conflicts. With [`OnConflict::Drop`] they are removed from the merged
//!    map; with [`OnConflict::Error`] the caller fails the request.

use forkline_core::{FixedBitSet, RequestMap, Slot};
use forkline_plan::OnConflict;

/// Merge `arrivals` into `merged`.
///
/// `merged` must start as a clean copy of the base and hold only the join
/// module's writes since. Returns the conflicting slots, in ascending order.
/// Afterwards `merged`'s dirty set is every slot that differs from the base.
pub fn merge_paths(merged: &mut RequestMap, arrivals: &[RequestMap], on_conflict: OnConflict) -> Vec<Slot> {
    let join_written = merged.dirty_bits().clone();
    let mut changed = FixedBitSet::with_capacity(merged.len());
    for arrival in arrivals {
        changed.union_with(arrival.dirty_bits());
    }
    let mut conflicts = Vec::new();
    for index in changed.ones() {
        let Ok(slot) = Slot::try_from(index) else { continue };
        if join_written.contains(index) {
            continue;
        }
        let mut writers = arrivals.iter().filter(|a| a.is_dirty(slot));
        match (writers.next(), writers.next()) {
            (Some(only), None) => merged.set_raw(slot, only.raw(slot).cloned()),
            (Some(_), Some(_)) => {
                conflicts.push(slot);
                if on_conflict == OnConflict::Drop {
                    merged.unset(slot);
                }
            }
            _ => {}
        }
    }
    conflicts
}
