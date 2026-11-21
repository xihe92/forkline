//! The interface! macro through the public `forkline` path.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use bytes::Bytes;
use forkline::{Access, Interface, RequestMap};

forkline::interface! {
    /// Every access kind.
    pub struct AllIo {
        /// Read only.
        in     svc_response: String,
        in     svc_time_taken: i64,
        inout  total_response: String,
        out    first: bool,
        unset  scratch: Vec<u8>,
    }
}

forkline::interface! {
    struct Empty0 {}
}

forkline::interface! {
    pub(crate) struct Trailing { in body: Bytes, }
}

#[test]
fn the_contract_lists_every_key_in_order() {
    let c = AllIo::CONTRACT;
    let names: Vec<_> = c.iter().map(|k| k.name).collect();
    assert_eq!(names, ["svc_response", "svc_time_taken", "total_response", "first", "scratch"]);
    let access: Vec<_> = c.iter().map(|k| k.access).collect();
    assert_eq!(access, [Access::In, Access::In, Access::InOut, Access::Out, Access::Unset]);
    assert_eq!((c[1].type_id)(), std::any::TypeId::of::<i64>());
    assert_eq!((c[4].type_name)(), "alloc::vec::Vec<u8>");
    assert!(Empty0::CONTRACT.is_empty());
    assert_eq!(Trailing::CONTRACT.len(), 1);
}

#[test]
fn accessors_read_and_write_through_the_bound_slots() {
    // Bind contract positions to non-identity slots, as a plan with renames would.
    let slots = [4, 0, 3, 1, 2];
    let mut map = RequestMap::with_slots(5);
    map.set(4, String::from("resp"));
    map.set(0, 42_i64);
    map.set(2, vec![1_u8]);
    map.clear_dirty();

    let mut io = AllIo::bind(&mut map, &slots);
    assert_eq!(io.svc_response().map(String::as_str), Some("resp"));
    assert_eq!(io.svc_time_taken(), Some(&42));
    assert_eq!(io.total_response(), None);
    assert_eq!(io.total_response_mut(), None, "no value to mutate yet");
    io.set_total_response("a".into());
    io.total_response_mut().unwrap().push('b');
    io.set_first(true);
    io.unset_scratch();

    assert_eq!(map.get::<String>(3).map(String::as_str), Some("ab"));
    assert_eq!(map.get::<bool>(1), Some(&true));
    assert!(!map.contains(2));
    assert_eq!(map.dirty_slots().collect::<Vec<_>>(), vec![1, 2, 3]);
}

#[test]
fn a_short_binding_reads_as_absent_instead_of_panicking() {
    let mut map = RequestMap::with_slots(1);
    map.set(0, String::from("x"));
    let mut io = AllIo::bind(&mut map, &[0]);
    assert!(io.svc_response().is_some());
    assert_eq!(io.svc_time_taken(), None);
    io.set_first(true); // ignored, no slot
}

#[test]
fn compile_errors() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
