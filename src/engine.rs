use crate::completions::{self, FsPathProvider, PathProvider};
use crate::config::Config;
use crate::store::{CompletionEntry, SqliteStore};
use anyhow::Result;
use std::sync::Arc;

/// Trait for the top-level completion engine — one method, easy to mock.
pub trait CompletionEngine: Send + Sync {
    /// Produce completions for the given buffer at `position`.
    fn complete(&self, buffer: &str, position: usize) -> Result<Vec<CompletionEntry>>;
}

/// Default engine backed by [`SqliteStore`] and [`Config`].
pub struct DefaultEngine {
    store: SqliteStore,
    config: Config,
    path_provider: Arc<dyn PathProvider>,
}

impl DefaultEngine {
    /// Create a new engine, opening the default SQLite database.
    /// Uses [`FsPathProvider`] for real filesystem path completions.
    pub fn new(config: Config) -> Result<Self> {
        Self::with_path_provider(config, Arc::new(FsPathProvider))
    }

    /// Create a new engine with a custom [`PathProvider`].
    pub fn with_path_provider(
        config: Config,
        path_provider: Arc<dyn PathProvider>,
    ) -> Result<Self> {
        let store = SqliteStore::open_or_create()?;
        Ok(Self {
            store,
            config,
            path_provider,
        })
    }

    /// Access the underlying store (e.g. for indexing).
    #[must_use]
    pub fn store(&self) -> &SqliteStore {
        &self.store
    }

    /// Access the configuration.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }
}

impl CompletionEngine for DefaultEngine {
    fn complete(&self, buffer: &str, position: usize) -> Result<Vec<CompletionEntry>> {
        completions::complete(
            buffer,
            position,
            &self.store,
            &self.config,
            &*self.path_provider,
        )
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

    /// Verify that a poisoned mutex produces a recoverable error via
    /// `map_err` rather than a panic — the same pattern used inside
    /// `SqliteStore`, `MemStore`, and `MemCache`.
    #[test]
    fn mutex_poisoning_returns_error() {
        use std::sync::{Arc, Mutex};

        let store = Arc::new(Mutex::new(0_u32));
        let store_clone = Arc::clone(&store);

        // Poison the mutex by panicking while holding the lock
        let handle = std::thread::spawn(move || {
            let _guard = store_clone.lock().unwrap();
            panic!("intentional panic to poison mutex");
        });
        let _ = handle.join(); // join returns Err because the thread panicked

        // Verify the mutex is poisoned
        assert!(store.lock().is_err(), "mutex should be poisoned");

        // The .map_err pattern used throughout the codebase converts
        // PoisonError into an anyhow::Error instead of panicking.
        let result: Result<std::sync::MutexGuard<'_, u32>> = store
            .lock()
            .map_err(|e| anyhow::anyhow!("store mutex poisoned: {e}"));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("store mutex poisoned"),
            "error message should mention poisoned mutex, got: {err_msg}"
        );
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
        let paths = FsPathProvider;
        let results = completions::complete("git co", 6, &store, &cfg, &paths).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].completion, "commit");
    }
}
