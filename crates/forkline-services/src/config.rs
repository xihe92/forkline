//! `services.toml` and the service kinds that turn it into clients.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use forkline_core::ServiceRegistry;

use crate::grpc::{GrpcClient, GrpcSettings};
use crate::http::{HttpClient, HttpSettings};
use crate::stack::StackConfig;

/// One service's configuration: its kind, layer settings, and the rest of
/// its table for the kind to interpret.
#[derive(Clone, Debug)]
pub struct ServiceConfig {
    /// Service kind (`http`, `grpc`, or one registered in code).
    pub kind: String,
    /// Layer settings.
    pub stack: StackConfig,
    /// Kind-specific settings.
    pub settings: toml::Table,
}

impl ServiceConfig {
    /// Deserialize the kind-specific settings into `T`.
    pub fn settings<T: serde::de::DeserializeOwned>(&self) -> Result<T, String> {
        toml::Value::Table(self.settings.clone()).try_into::<T>().map_err(|e| e.to_string())
    }
}

/// A whole services file.
#[derive(Clone, Debug, Default)]
pub struct ServicesFile {
    /// Services by name.
    pub services: BTreeMap<String, ServiceConfig>,
}

impl ServicesFile {
    /// Parse from a string.
    pub fn from_toml_str(text: &str) -> Result<Self, String> {
        let mut root: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
        let services = match root.remove("services") {
            None => toml::Table::new(),
            Some(toml::Value::Table(t)) => t,
            Some(_) => return Err("`services` must be a table".into()),
        };
        if let Some(key) = root.keys().next() {
            return Err(format!("unknown top-level key `{key}`"));
        }
        let mut out = BTreeMap::new();
        for (name, value) in services {
            let toml::Value::Table(mut table) = value else {
                return Err(format!("service `{name}` must be a table"));
            };
            let kind = match table.remove("kind") {
                Some(toml::Value::String(k)) => k,
                Some(_) => return Err(format!("service `{name}`: `kind` must be a string")),
                None => return Err(format!("service `{name}`: missing `kind`")),
            };
            let mut stack_table = toml::Table::new();
            for key in StackConfig::KEYS {
                if let Some(v) = table.remove(*key) {
                    stack_table.insert((*key).to_owned(), v);
                }
            }
            let stack: StackConfig =
                toml::Value::Table(stack_table).try_into().map_err(|e| format!("service `{name}`: {e}"))?;
            out.insert(name, ServiceConfig { kind, stack, settings: table });
        }
        Ok(Self { services: out })
    }

    /// Read and parse a file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::from_toml_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

type KindFactory = Box<dyn Fn(&str, &ServiceConfig, &mut ServiceRegistry) -> Result<(), String> + Send + Sync>;

/// The service kinds a binary knows how to build. `http` and `grpc` are built in.
pub struct ServiceKinds {
    kinds: BTreeMap<String, KindFactory>,
}

impl Default for ServiceKinds {
    fn default() -> Self {
        Self::empty()
            .register("http", |name, config, registry| {
                let settings: HttpSettings = config.settings()?;
                registry.insert(name, HttpClient::from_settings(name, &settings, &config.stack)?);
                Ok(())
            })
            .register("grpc", |name, config, registry| {
                let settings: GrpcSettings = config.settings()?;
                registry.insert(name, GrpcClient::from_settings(name, &settings, &config.stack)?);
                Ok(())
            })
    }
}

impl ServiceKinds {
    /// No kinds at all.
    pub fn empty() -> Self {
        Self { kinds: BTreeMap::new() }
    }

    /// Register a kind. The factory inserts whatever client type it builds.
    pub fn register(
        mut self,
        kind: impl Into<String>,
        factory: impl Fn(&str, &ServiceConfig, &mut ServiceRegistry) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.kinds.insert(kind.into(), Box::new(factory));
        self
    }

    /// Build every service in `file` into `registry` (which may already hold
    /// services added in code). Fails with every problem found.
    pub fn build_into(&self, file: &ServicesFile, registry: &mut ServiceRegistry) -> Result<(), Vec<String>> {
        let mut problems = Vec::new();
        for (name, config) in &file.services {
            match self.kinds.get(&config.kind) {
                Some(factory) => {
                    if let Err(e) = factory(name, config, registry) {
                        problems.push(format!("service `{name}`: {e}"));
                    }
                }
                None => problems.push(format!(
                    "service `{name}`: unknown kind `{}` (known: {})",
                    config.kind,
                    self.kinds.keys().cloned().collect::<Vec<_>>().join(", ")
                )),
            }
        }
        if problems.is_empty() { Ok(()) } else { Err(problems) }
    }

    /// Build every service in `file` into a new registry.
    pub fn build(&self, file: &ServicesFile) -> Result<ServiceRegistry, Vec<String>> {
        let mut registry = ServiceRegistry::new();
        self.build_into(file, &mut registry)?;
        Ok(registry)
    }
}

impl fmt::Debug for ServiceKinds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.kinds.keys()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parses_kinds_layers_and_settings() {
        let file = ServicesFile::from_toml_str(
            r#"
            [services.horoscope]
            kind = "http"
            url = "http://127.0.0.1:1/"
            timeout_ms = 40
            retries = 1
            breaker = { failures = 3, open_ms = 500 }
            [services.ledger]
            kind = "grpc"
            url = "http://127.0.0.1:2"
            "#,
        )
        .unwrap();
        let h = &file.services["horoscope"];
        assert_eq!(h.kind, "http");
        assert_eq!(h.stack.timeout_ms, Some(40));
        assert_eq!(h.stack.retries, 1);
        assert_eq!(h.settings.len(), 1);
        let registry = ServiceKinds::default().build(&file).unwrap();
        assert!(registry.get::<HttpClient>("horoscope").is_ok());
        assert!(registry.get::<GrpcClient>("ledger").is_ok());
    }

    #[test]
    fn reports_every_problem() {
        let file = ServicesFile::from_toml_str(
            r#"
            [services.a]
            kind = "http"
            url = "not a url"
            [services.b]
            kind = "redis"
            [services.c]
            kind = "http"
            url = "http://x/"
            colour = "red"
            "#,
        )
        .unwrap();
        let problems = ServiceKinds::default().build(&file).unwrap_err();
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems[0].contains("invalid url"), "{problems:?}");
        assert!(problems[1].contains("unknown kind `redis`"), "{problems:?}");
        assert!(problems[2].contains("colour"), "{problems:?}");
    }

    #[test]
    fn rejects_bad_shapes() {
        assert!(ServicesFile::from_toml_str("[services.a]\nurl = \"x\"\n").unwrap_err().contains("missing `kind`"));
        assert!(ServicesFile::from_toml_str("[services.a]\nkind = \"http\"\nretries = \"x\"\n").is_err());
        assert!(ServicesFile::from_toml_str("[other]\n").unwrap_err().contains("unknown top-level key"));
        assert_eq!(ServicesFile::from_toml_str("services = 1\n").unwrap_err(), "`services` must be a table");
        assert_eq!(ServicesFile::from_toml_str("[services]\na = 1\n").unwrap_err(), "service `a` must be a table");
        assert_eq!(
            ServicesFile::from_toml_str("[services.a]\nkind = 1\n").unwrap_err(),
            "service `a`: `kind` must be a string"
        );
        assert!(ServicesFile::from_toml_str("not toml").is_err());
        let err = ServicesFile::load("/nonexistent/services.toml").unwrap_err();
        assert!(err.starts_with("cannot read /nonexistent/services.toml:"), "{err}");
    }

    #[test]
    fn grpc_urls_are_checked_and_code_kinds_can_be_added() {
        let file = ServicesFile::from_toml_str("[services.g]\nkind = \"grpc\"\nurl = \"not a url\"\n").unwrap();
        let problems = ServiceKinds::default().build(&file).unwrap_err();
        assert!(problems[0].starts_with("service `g`: invalid url `not a url`"), "{problems:?}");

        let file = ServicesFile::from_toml_str("[services.n]\nkind = \"number\"\nvalue = 7\n").unwrap();
        let kinds = ServiceKinds::empty().register("number", |name, config, registry| {
            let value = config.settings.get("value").and_then(toml::Value::as_integer).ok_or("missing value")?;
            registry.insert(name, value);
            Ok(())
        });
        let registry = kinds.build(&file).unwrap();
        assert_eq!(registry.get::<i64>("n").unwrap(), 7);
        let problems = ServiceKinds::empty().build(&file).unwrap_err();
        assert!(problems[0].contains("unknown kind `number`"), "{problems:?}");
    }
}
