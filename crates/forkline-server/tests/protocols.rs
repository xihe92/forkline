//! Protocol-specific behaviour the listener contract suite (`contract.rs`)
//! doesn't cover: the TCP adapter's owed responses, abandoned pipelines and
//! in-flight limit, a failing error node, and gRPC's own deadline handling.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use common::{grpc_call, plans, read_frame, registry, send_frame, start};
use forkline_engine::Engine;
use forkline_server::Hooks;
use tonic::Code;

const ALL_LISTENERS: &str = "[[http]]\naddr = \"127.0.0.1:0\"\nroutes = { \"/p\" = \"p\" }\nmax_body_bytes = 64\n\
     [[grpc]]\naddr = \"127.0.0.1:0\"\nmethods = { P = \"p\" }\n[[tcp]]\naddr = \"127.0.0.1:0\"\nplan = \"p\"\n";

fn engine(budget_ms: u64) -> Arc<Engine> {
    Arc::new(Engine::new(registry(), &plans(&[("p", "", 0, budget_ms)])).unwrap())
}

#[tokio::test]
async fn tcp_sends_the_responses_it_owes_before_closing_on_an_oversize_frame() {
    let s = start(ALL_LISTENERS, engine(1000), Hooks::default()).await;
    let mut conn = s.tcp().await;
    send_frame(&mut conn, b"50|owed").await;
    // Only the header: the server rejects the frame by its declared length.
    tokio::io::AsyncWriteExt::write_all(&mut conn, &(2u32 << 20).to_be_bytes()).await.unwrap();
    assert_eq!(read_frame(&mut conn).await.unwrap(), (0, Bytes::from("owed")));
    assert!(read_frame(&mut conn).await.is_none(), "then the connection closes");
    drop(conn);
    s.stop().await;
}

#[tokio::test]
async fn a_tcp_client_leaving_mid_pipeline_does_not_disturb_the_server() {
    let s = start(ALL_LISTENERS, engine(1000), Hooks::default()).await;
    let mut gone = s.tcp().await;
    send_frame(&mut gone, b"100|never read").await;
    send_frame(&mut gone, b"100|never read").await;
    drop(gone);
    tokio::time::sleep(Duration::from_millis(150)).await;

    let mut conn = s.tcp().await;
    send_frame(&mut conn, b"still here").await;
    assert_eq!(read_frame(&mut conn).await.unwrap(), (0, Bytes::from("still here")));
    drop(conn);
    s.stop().await;
}

#[tokio::test]
async fn tcp_reads_no_further_than_max_in_flight() {
    async fn two_slow_requests(max_in_flight: usize) -> Duration {
        let config =
            format!("[[tcp]]\naddr = \"127.0.0.1:0\"\nplan = \"p\"\nmax_in_flight_per_connection = {max_in_flight}\n");
        let s = start(&config, engine(5000), Hooks::default()).await;
        let mut conn = s.tcp().await;
        let started = Instant::now();
        send_frame(&mut conn, b"200|a").await;
        send_frame(&mut conn, b"200|b").await;
        assert_eq!(read_frame(&mut conn).await.unwrap(), (0, Bytes::from("a")));
        assert_eq!(read_frame(&mut conn).await.unwrap(), (0, Bytes::from("b")));
        let took = started.elapsed();
        drop(conn);
        s.stop().await;
        took
    }
    let one = two_slow_requests(1).await;
    assert!(one >= Duration::from_millis(390), "one at a time: {one:?}");
    let two = two_slow_requests(2).await;
    assert!(two < Duration::from_millis(390), "both at once: {two:?}");
}

#[tokio::test]
async fn a_failing_error_node_is_an_internal_error_on_every_protocol() {
    let engine = Arc::new(Engine::new(registry(), &common::broken_terminal_plan("p")).unwrap());
    let s = start(ALL_LISTENERS, engine, Hooks::default()).await;

    let res = reqwest::Client::new().post(s.http("/p")).body("fail").send().await.unwrap();
    assert_eq!(res.status(), 500);
    assert_eq!(res.text().await.unwrap(), "internal error");

    let status = grpc_call(s.grpc(), "P", "fail").await.unwrap_err();
    assert_eq!((status.code(), status.message()), (Code::Internal, "internal error"));

    let mut conn = s.tcp().await;
    send_frame(&mut conn, b"fail").await;
    assert_eq!(read_frame(&mut conn).await.unwrap(), (2, Bytes::from("internal error")));

    // Requests that succeed never touch the error node.
    send_frame(&mut conn, b"fine").await;
    assert_eq!(read_frame(&mut conn).await.unwrap(), (0, Bytes::from("fine")));
    drop(conn);
    s.stop().await;
}

#[tokio::test]
async fn a_grpc_request_whose_deadline_already_passed_never_runs() {
    // Two things race here, and both refuse the request without running it:
    // tonic enforces grpc-timeout outside our handler (CANCELLED, "Timeout
    // expired"), and when the body is already readable the engine sheds the
    // late request itself (RESOURCE_EXHAUSTED, "shed").
    let engine = Arc::new(Engine::new(registry(), &common::detached_plan("d", "expired-on-arrival", 0, 1000)).unwrap());
    let s = start("[[grpc]]\naddr = \"127.0.0.1:0\"\nmethods = { D = \"d\" }\n", engine, Hooks::default()).await;
    let channel =
        tonic::transport::Endpoint::from_shared(format!("http://{}", s.grpc())).unwrap().connect().await.unwrap();
    let mut client = tonic::client::Grpc::new(channel);
    client.ready().await.unwrap();
    let mut request = tonic::Request::new(Bytes::from("x"));
    request.metadata_mut().insert("grpc-timeout", "1n".parse().unwrap());
    let path = http::uri::PathAndQuery::from_static("/forkline.v1.Plans/D");
    let status = client.unary(request, path, forkline_server::grpc::BytesCodec).await.unwrap_err();
    let outcome = (status.code(), status.message());
    assert!(
        outcome == (Code::Cancelled, "Timeout expired") || outcome == (Code::ResourceExhausted, "shed"),
        "{outcome:?}"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(common::marks("expired-on-arrival"), 0, "the plan never ran");
    drop(client);
    s.stop().await;
}
