//! Module tests, as the book's "Testing modules" page shows them.

#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use forkline::{ErrorKind, Pool, ServiceRegistry};
use forkline_docs_snippets::{ByFlag, Delivery, Item, Order, OrderDecoder, Receipts, Subtotal};
use forkline_plan::OnConflict;
use forkline_testkit::{Harness, TestError, inputs};

fn order(flag: u32) -> Order {
    Order {
        id: "A1".into(),
        flag,
        items: vec![Item { sku: "tea".into(), quantity: 2, unit_cents: 350 }],
        notify: vec![],
    }
}

// ANCHOR: module
#[tokio::test]
async fn delivery_adds_its_charge() -> Result<(), TestError> {
    let out = Harness::<Delivery>::from_config("charge_cents = 499", ServiceRegistry::new())?
        .inputs(inputs! { order: order(1) })
        .max_poll(Duration::from_millis(10)) // fail if one poll blocks for longer
        .run_module()
        .await?;
    assert_eq!(out.get::<u64>("price_cents"), Some(&1199));
    assert_eq!(out.changed(), ["price_cents"]);
    Ok(())
}
// ANCHOR_END: module

// ANCHOR: decode_branch
#[test]
fn a_bad_body_is_a_bad_request() {
    let err = Harness::new(OrderDecoder).run_decode("not json").unwrap_err();
    assert!(matches!(err, TestError::Node(e) if e.kind() == ErrorKind::BadRequest));
}

#[test]
fn the_flag_picks_the_branch() -> Result<(), TestError> {
    assert_eq!(Harness::new(ByFlag).inputs(inputs! { order: order(2) }).run_branch()?, "express");
    Ok(())
}
// ANCHOR_END: decode_branch

// ANCHOR: join
#[test]
fn receipts_collects_every_path() -> Result<(), TestError> {
    let out = Harness::new(Receipts)
        .fork_base(inputs! { price_cents: 700u64 })
        .path(inputs! { sms_receipt: String::from("sms: order A1") })
        .path(inputs! { email_receipt: String::from("email: order A1") })
        .on_conflict(OnConflict::Error)
        .run_join()?;
    let receipts = out.outputs.get::<Vec<String>>("receipts").unwrap();
    assert_eq!(receipts, &["email: order A1", "sms: order A1"]);
    assert_eq!(out.outputs.get::<u64>("price_cents"), Some(&700)); // carried through
    assert!(out.conflicts.is_empty());
    Ok(())
}
// ANCHOR_END: join

// ANCHOR: pool
#[tokio::test]
async fn subtotal_runs_on_its_pool() -> Result<(), TestError> {
    let pools = ServiceRegistry::new().with_pool("pricing", Pool::new("pricing", 1, 4).unwrap());
    let out = Harness::<Subtotal>::from_config("pool = \"pricing\"", pools)?
        .inputs(inputs! { order: order(1) })
        .run_module()
        .await?;
    assert_eq!(out.get::<u64>("price_cents"), Some(&700));
    Ok(())
}
// ANCHOR_END: pool
