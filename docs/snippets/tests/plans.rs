//! The book's plans pass every check `serve` runs, and do what the book says.

#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use forkline::engine::{Engine, Request};
use forkline::plan::PlansFile;
use forkline::{Pool, ServiceRegistry, Status};
use forkline_docs_snippets::registry;
use forkline_server::cli::App;

fn file(name: &str) -> String {
    format!("{}/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn the_books_plans_pass_every_check() {
    App::new(registry().unwrap()).assert_files_valid(Some(Path::new(&file("server.toml"))), file("plans.toml"), None);
}

fn engine() -> Engine {
    let plans = PlansFile::load(file("plans.toml")).unwrap();
    let services = ServiceRegistry::new().with_pool("pricing", Pool::new("pricing", 2, 16).unwrap());
    Engine::with_services(Arc::new(registry().unwrap()), &plans, Arc::new(services)).unwrap()
}

async fn call(plan: &str, body: &str) -> (Status, serde_json::Value) {
    let reply = engine().call(plan, Request::in_process(body.to_owned())).await.unwrap();
    (reply.status, serde_json::from_slice(&reply.body).unwrap())
}

const ORDER: &str =
    r#"{"id":"A1","flag":2,"items":[{"sku":"tea","quantity":2,"unit_cents":350}],"notify":["sms","email"]}"#;

#[tokio::test]
async fn quote_takes_the_branch_the_flag_picks() {
    let (status, body) = call("quote", ORDER).await;
    assert_eq!(status, Status::Ok);
    assert_eq!(body["price_cents"], 700 + 1499); // express
    let (_, body) = call("quote", &ORDER.replace(r#""flag":2"#, r#""flag":1"#)).await;
    assert_eq!(body["price_cents"], 700 + 499);
}

#[tokio::test]
async fn a_bad_flag_is_a_bad_request_answered_by_the_terminal() {
    let (status, body) = call("quote", &ORDER.replace(r#""flag":2"#, r#""flag":7"#)).await;
    assert_eq!(status, Status::Error(forkline::ErrorKind::BadRequest));
    assert_eq!(body["error"], "bad_request");
    assert_eq!(body["node"], "route");
}

#[tokio::test]
async fn notify_runs_every_listed_channel_and_joins_the_receipts() {
    let (status, body) = call("notify", ORDER).await;
    assert_eq!(status, Status::Ok);
    assert_eq!(body["price_cents"], 700 + 499);
    assert_eq!(body["receipts"], serde_json::json!(["email: order A1", "sms: order A1"]));
    let (_, body) = call("notify", &ORDER.replace(r#"["sms","email"]"#, "[]")).await;
    assert_eq!(body["receipts"], serde_json::json!([]));
}

#[tokio::test]
async fn checkout_prices_through_the_subplan_on_the_pool() {
    let (status, body) = call("checkout", ORDER).await;
    assert_eq!(status, Status::Ok);
    assert_eq!(body["price_cents"], 700);
}

#[test]
fn the_kv_kind_builds_from_services_toml() {
    let services = forkline_services::ServicesFile::from_toml_str(
        "[services.sessions]\nkind = \"kv\"\naddr = \"10.0.0.5:6379\"\nprefix = \"s:\"\ntimeout_ms = 20\n",
    )
    .unwrap();
    let mut built = ServiceRegistry::new();
    forkline_docs_snippets::kinds::service_kinds().build_into(&services, &mut built).unwrap();
    let kv: forkline_docs_snippets::kinds::KvClient = built.get("sessions").unwrap();
    assert_eq!(kv.prefix, "s:");
}
