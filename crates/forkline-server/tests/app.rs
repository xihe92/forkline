//! `cli::App`: loading and reloading plans and services from files.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use std::path::{Path, PathBuf};

use common::modules;
use forkline_engine::Request;
use forkline_server::cli::App;

/// A scratch directory removed on drop.
struct Dir(PathBuf);
impl Dir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("forkline-app-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn plan(suffix: &str) -> String {
    format!(
        "[plans.p]\nroot = \"decode\"\nterminal = \"error\"\nlinks = [\"decode -> work\", \"work -> encode\"]\n\
         [plans.p.nodes]\ndecode = {{ module = \"BodyDecoder\" }}\n\
         work = {{ module = \"Work\", config = {{ suffix = \"{suffix}\" }} }}\n\
         encode = {{ module = \"ReplyEncoder\" }}\nerror = {{ module = \"ErrorReply\" }}\n"
    )
}

fn http_service(name: &str) -> String {
    format!("[services.{name}]\nkind = \"http\"\nurl = \"http://127.0.0.1:9/\"\n")
}

fn service_names(engine: &forkline_engine::Engine) -> Vec<String> {
    engine.snapshot().services.iter().map(|(name, _)| name.to_owned()).collect()
}

async fn reply(engine: &forkline_engine::Engine) -> String {
    let reply = engine.call("p", Request::in_process("x")).await.unwrap();
    String::from_utf8(reply.body.to_vec()).unwrap()
}

#[tokio::test]
async fn reload_rebuilds_file_services_and_keeps_code_services() {
    let dir = Dir::new("reload");
    let plans = dir.write("plans.toml", &plan("-v1"));
    let services = dir.write("services.toml", &http_service("first"));
    let app = App::new(modules()).service("from_code", 7u32);

    let engine = app.load_engine(&plans, Some(&services)).unwrap();
    assert_eq!(service_names(&engine), ["first", "from_code"]);
    assert_eq!(reply(&engine).await, "x-v1");
    let hash = App::config_hash(&plans, Some(&services));

    dir.write("plans.toml", &plan("-v2"));
    dir.write("services.toml", &http_service("second"));
    assert_ne!(App::config_hash(&plans, Some(&services)), hash, "the hash follows the files");
    assert_eq!(app.reload(&engine, &plans, Some(&services)).unwrap(), 2);
    assert_eq!(service_names(&engine), ["from_code", "second"], "file services rebuilt, code services kept");
    assert_eq!(reply(&engine).await, "x-v2");
    assert_eq!(engine.snapshot().config_hash, App::config_hash(&plans, Some(&services)));

    // A broken services file rejects the reload; the running snapshot stays.
    dir.write("services.toml", "[services.bad]\nkind = \"carrier-pigeon\"\n");
    let err = app.reload(&engine, &plans, Some(&services)).unwrap_err();
    assert!(err.contains("carrier-pigeon"), "{err}");
    assert_eq!(engine.snapshot().version, 2);
    assert_eq!(reply(&engine).await, "x-v2");
}

#[test]
fn loading_reports_file_problems() {
    let dir = Dir::new("load");
    let app = App::new(modules());
    let missing = dir.0.join("missing.toml");

    let err = app.load_engine(&missing, None).unwrap_err();
    assert!(err.contains("missing.toml"), "{err}");

    let plans = dir.write("plans.toml", "[plans.p]\nroot = \"decode\"\n");
    assert!(app.load_engine(&plans, None).is_err(), "an incomplete plan is rejected");

    let plans = dir.write("plans.toml", &plan(""));
    let err = app.load_engine(&plans, Some(&missing)).unwrap_err();
    assert!(err.starts_with(&format!("cannot read {}", missing.display())), "{err}");

    let services = dir.write("services.toml", "services = 5\n");
    assert_eq!(
        app.load_engine(&plans, Some(&services)).unwrap_err(),
        format!("{}: `services` must be a table", services.display())
    );
    assert!(app.load_engine(&plans, Some(Path::new(&dir.write("s.toml", "")))).is_ok(), "an empty file is fine");
}
