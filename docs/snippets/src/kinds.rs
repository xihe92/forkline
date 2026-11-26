//! A service kind registered in code, for a backend Forkline has no client for.

use forkline::ServiceRegistry;
use forkline_server::cli::App;
use forkline_services::{ServiceConfig, ServiceKinds};
use serde::Deserialize;

// ANCHOR: kind
/// The settings a `kind = "kv"` service takes from `services.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KvSettings {
    /// Server address.
    pub addr: String,
    /// Prefix put in front of every key.
    #[serde(default)]
    pub prefix: String,
}

/// A client for a key-value store. Modules resolve it by name, like any service.
#[derive(Clone, Debug)]
pub struct KvClient {
    /// Server address.
    pub addr: String,
    /// Key prefix.
    pub prefix: String,
}

/// The built-in kinds (`http`, `grpc`) plus `kv`.
pub fn service_kinds() -> ServiceKinds {
    ServiceKinds::default().register("kv", |name: &str, config: &ServiceConfig, registry: &mut ServiceRegistry| {
        let settings: KvSettings = config.settings()?;
        registry.insert(name, KvClient { addr: settings.addr, prefix: settings.prefix });
        Ok(())
    })
}

/// The server app with the extra kind.
pub fn app() -> Result<App, forkline::RegistryError> {
    Ok(App::new(crate::registry()?).service_kinds(service_kinds()))
}
// ANCHOR_END: kind
