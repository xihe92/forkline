//! Named, typed services shared by modules.
//!
//! A service is any cloneable handle (usually a client wrapping a
//! `tower::Service` stack) registered under a name. Modules look services up
//! by name *and* type; asking for the wrong type is an error, not a panic.

use std::any::Any;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

#[derive(Clone)]
struct Entry {
    value: Arc<dyn Any + Send + Sync>,
    type_name: &'static str,
}

/// Named services, each stored with its concrete type. Cloning shares the services.
#[derive(Clone, Default)]
pub struct ServiceRegistry {
    entries: BTreeMap<String, Entry>,
}

/// A service lookup failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ServiceLookupError {
    /// No service of that name.
    #[error("service `{0}` is not registered")]
    Missing(String),
    /// The service exists with another type.
    #[error("service `{name}` is a `{actual}`, not a `{expected}`")]
    WrongType {
        /// Service name.
        name: String,
        /// The type asked for.
        expected: &'static str,
        /// The registered type.
        actual: &'static str,
    },
}

impl ServiceRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `value` under `name`, replacing any previous entry.
    pub fn insert<T: Clone + Send + Sync + 'static>(&mut self, name: impl Into<String>, value: T) {
        self.entries.insert(name.into(), Entry { value: Arc::new(value), type_name: std::any::type_name::<T>() });
    }

    /// Builder-style [`insert`](Self::insert).
    pub fn with<T: Clone + Send + Sync + 'static>(mut self, name: impl Into<String>, value: T) -> Self {
        self.insert(name, value);
        self
    }

    /// A clone of the service `name`, if it exists with type `T`.
    pub fn get<T: Clone + Send + Sync + 'static>(&self, name: &str) -> Result<T, ServiceLookupError> {
        let entry = self.entries.get(name).ok_or_else(|| ServiceLookupError::Missing(name.to_owned()))?;
        entry.value.downcast_ref::<T>().cloned().ok_or(ServiceLookupError::WrongType {
            name: name.to_owned(),
            expected: std::any::type_name::<T>(),
            actual: entry.type_name,
        })
    }

    /// True when a service of this name exists.
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Registered names and their types, sorted by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &'static str)> {
        self.entries.iter().map(|(n, e)| (n.as_str(), e.type_name))
    }

    /// Number of services.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when there are no services.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl fmt::Debug for ServiceRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct Client(u8);

    #[test]
    fn typed_lookup() {
        let reg = ServiceRegistry::new().with("a", Client(1));
        assert_eq!(reg.get::<Client>("a"), Ok(Client(1)));
        assert_eq!(reg.get::<Client>("b"), Err(ServiceLookupError::Missing("b".into())));
        let err = reg.get::<String>("a").unwrap_err().to_string();
        assert!(err.contains("not a `alloc::string::String`"), "{err}");
    }
}
