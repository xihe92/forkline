//! A fake backend for local runs of the horoscope example: serves `/time`,
//! `/horoscope/{sign}` and `/stock/{ticker}` on 127.0.0.1:9001.

#![forbid(unsafe_code)]

use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::extract::Path;
use axum::routing::get;

async fn time() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    format!("{secs}")
}

async fn horoscope(Path(sign): Path<String>) -> String {
    format!("{sign}: a good day to ship")
}

async fn stock(Path(ticker): Path<String>) -> String {
    let price = ticker.bytes().map(u64::from).sum::<u64>() % 500 + 10;
    format!("{ticker}: {price}.00")
}

#[tokio::main]
async fn main() -> ExitCode {
    let app = Router::new()
        .route("/time", get(time))
        .route("/horoscope/{sign}", get(horoscope))
        .route("/stock/{ticker}", get(stock));
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:9001").await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: cannot bind 127.0.0.1:9001: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("backend on http://127.0.0.1:9001");
    match axum::serve(listener, app).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
