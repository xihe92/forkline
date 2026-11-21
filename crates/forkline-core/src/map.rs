//! The per-request attribute map.
//!
//! Key names are interned to slot numbers when a plan is compiled, so every
//! lookup at request time is an array index. Values sit behind `Arc`, which
//! makes copying the map at a fork one pointer copy per slot. Each map keeps a
//! bitset of the slots written or unset since it was created (or since the
//! last [`RequestMap::clear_dirty`]); joins use it to apply the merge rules.

use std::any::Any;
use std::fmt;
use std::sync::Arc;

use fixedbitset::FixedBitSet;

/// Index of an interned key in a compiled plan.
pub type Slot = u32;

/// A type-erased map value.
pub type Value = Arc<dyn Any + Send + Sync>;

/// The per-request attribute map that every node reads and writes.
#[derive(Clone)]
pub struct RequestMap {
    values: Vec<Option<Value>>,
    dirty: FixedBitSet,
}

impl RequestMap {
    /// An empty map with room for `slots` keys.
    pub fn with_slots(slots: usize) -> Self {
        Self { values: vec![None; slots], dirty: FixedBitSet::with_capacity(slots) }
    }

    /// Number of slots.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// True when the map has no slots.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// True when the slot holds a value.
    pub fn contains(&self, slot: Slot) -> bool {
        matches!(self.values.get(slot as usize), Some(Some(_)))
    }

    /// Borrow the value in `slot`, if present and of type `T`.
    pub fn get<T: 'static>(&self, slot: Slot) -> Option<&T> {
        self.values.get(slot as usize)?.as_ref()?.downcast_ref::<T>()
    }

    /// Mutably borrow the value in `slot`, copying it first if a fork shares it.
    ///
    /// Marks the slot as written.
    pub fn get_mut<T: Clone + Send + Sync + 'static>(&mut self, slot: Slot) -> Option<&mut T> {
        let value = self.values.get_mut(slot as usize)?.as_mut()?;
        if Arc::get_mut(value).is_none() {
            let copy: T = value.downcast_ref::<T>()?.clone();
            *value = Arc::new(copy);
        }
        self.dirty.grow(slot as usize + 1);
        self.dirty.insert(slot as usize);
        Arc::get_mut(value)?.downcast_mut::<T>()
    }

    /// Store `value` in `slot` and mark it written. Out-of-range slots are ignored.
    pub fn set<T: Send + Sync + 'static>(&mut self, slot: Slot, value: T) {
        self.set_raw(slot, Some(Arc::new(value)));
    }

    /// Clear `slot` and mark it written.
    pub fn unset(&mut self, slot: Slot) {
        self.set_raw(slot, None);
    }

    /// The raw value in `slot`, for the engine's merge step.
    pub fn raw(&self, slot: Slot) -> Option<&Value> {
        self.values.get(slot as usize)?.as_ref()
    }

    /// Replace the raw value in `slot` and mark it written.
    pub fn set_raw(&mut self, slot: Slot, value: Option<Value>) {
        if let Some(cell) = self.values.get_mut(slot as usize) {
            *cell = value;
            self.dirty.grow(slot as usize + 1);
            self.dirty.insert(slot as usize);
        }
    }

    /// True when `slot` was written or unset since the last clear.
    pub fn is_dirty(&self, slot: Slot) -> bool {
        self.dirty.contains(slot as usize)
    }

    /// Slots written or unset since the last clear, in ascending order.
    pub fn dirty_slots(&self) -> impl Iterator<Item = Slot> + '_ {
        // Slot counts are bounded by the plan's key table, which is far below u32::MAX.
        self.dirty.ones().filter_map(|i| Slot::try_from(i).ok())
    }

    /// Forget which slots were written.
    pub fn clear_dirty(&mut self) {
        self.dirty.clear();
    }
}

impl fmt::Debug for RequestMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let present: Vec<usize> = self.values.iter().enumerate().filter_map(|(i, v)| v.as_ref().map(|_| i)).collect();
        f.debug_struct("RequestMap")
            .field("slots", &self.values.len())
            .field("present", &present)
            .field("dirty", &self.dirty.ones().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_and_type_mismatch() {
        let mut m = RequestMap::with_slots(3);
        m.set(0, String::from("hi"));
        m.set(1, 7_i64);
        assert_eq!(m.get::<String>(0).map(String::as_str), Some("hi"));
        assert_eq!(m.get::<i64>(1), Some(&7));
        assert_eq!(m.get::<i64>(0), None, "wrong type reads as absent");
        assert_eq!(m.get::<i64>(2), None);
        assert_eq!(m.get::<i64>(99), None, "out of range reads as absent");
    }

    #[test]
    fn dirty_tracking() {
        let mut m = RequestMap::with_slots(4);
        m.set(1, 1_u8);
        m.unset(3);
        assert_eq!(m.dirty_slots().collect::<Vec<_>>(), vec![1, 3]);
        m.clear_dirty();
        assert_eq!(m.dirty_slots().count(), 0);
        assert!(m.contains(1));
        assert!(!m.contains(3));
    }

    #[test]
    fn get_mut_copies_shared_values() {
        let mut base = RequestMap::with_slots(1);
        base.set(0, vec![1, 2]);
        base.clear_dirty();
        let mut fork = base.clone();
        fork.get_mut::<Vec<i32>>(0).unwrap().push(3);
        assert_eq!(base.get::<Vec<i32>>(0), Some(&vec![1, 2]), "the original is untouched");
        assert_eq!(fork.get::<Vec<i32>>(0), Some(&vec![1, 2, 3]));
        assert!(fork.is_dirty(0));
        assert!(!base.is_dirty(0));
    }

    #[test]
    fn out_of_range_writes_are_ignored() {
        let mut m = RequestMap::with_slots(1);
        m.set(5, 1_u8);
        m.unset(5);
        assert_eq!(m.dirty_slots().count(), 0);
    }
}
