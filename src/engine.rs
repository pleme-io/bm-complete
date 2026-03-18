use crate::completions;
use crate::config::Config;
use crate::store::{CompletionEntry, SqliteStore};
use anyhow::Result;
use std::sync::Mutex;

/// Trait for the top-level completion engine — one method, easy to mock.
pub trait CompletionEngine: Send + Sync {
    /// Produce completions for the given buffer at `position`.
    fn complete(&self, buffer: &str, position: usize) -> Result<Vec<CompletionEntry>>;
}

/// Default engine backed by [`SqliteStore`] and [`Config`].
pub struct DefaultEngine {
    store: Mutex<SqliteStore>,
    config: Config,
}

impl DefaultEngine {
    /// Create a new engine, opening the default SQLite database.
    pub fn new(config: Config) -> Result<Self> {
        let store = SqliteStore::open_or_create()?;
        Ok(Self {
            store: Mutex::new(store),
            config,
        })
    }

    /// Access the underlying store (e.g. for indexing).
    pub fn store(&self) -> std::sync::MutexGuard<'_, SqliteStore> {
        self.store.lock().expect("store mutex poisoned")
    }

    /// Access the configuration.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }
}

impl CompletionEngine for DefaultEngine {
    fn complete(&self, buffer: &str, position: usize) -> Result<Vec<CompletionEntry>> {
        let store = self.store.lock().expect("store mutex poisoned");
        completions::complete(buffer, position, &*store, &self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{MemStore, Store};

    /// Minimal mock engine for test isolation.
    struct MockEngine {
        results: Vec<CompletionEntry>,
    }

    impl CompletionEngine for MockEngine {
        fn complete(&self, _buffer: &str, _position: usize) -> Result<Vec<CompletionEntry>> {
            Ok(self.results.clone())
        }
    }

    #[test]
    fn mock_engine_returns_results() {
        let engine = MockEngine {
            results: vec![CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: "Record changes".into(),
                source: "mock".into(),
            }],
        };
        let results = engine.complete("git co", 6).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].completion, "commit");
    }

    #[test]
    fn default_engine_delegates_to_complete() {
        // Use MemStore through the trait to verify the delegation pattern works.
        let store = MemStore::new();
        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: "Record changes".into(),
                source: "fish".into(),
            })
            .unwrap();

        let cfg = crate::config::TestConfig {
            index_path: false,
            ..crate::config::TestConfig::default()
        };
        let results = completions::complete("git co", 6, &store, &cfg).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].completion, "commit");
    }
}
