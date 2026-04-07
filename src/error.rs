//! Typed error variants for bm-complete.
//!
//! Internal errors are represented as [`BmError`] with specific variants
//! rather than ad-hoc `anyhow::anyhow!(...)` strings.  Because `BmError`
//! implements `std::error::Error` (via `thiserror`), it converts
//! automatically into `anyhow::Error` through `?`.

/// Typed error enum covering all failure modes in bm-complete.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BmError {
    /// A `Mutex` was poisoned (a thread panicked while holding the lock).
    #[error("{context} mutex poisoned: {source}")]
    MutexPoisoned {
        context: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// SQLite / rusqlite failure.
    #[error(transparent)]
    Database(#[from] rusqlite::Error),

    /// Filesystem I/O failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// JSON serialization / deserialization failure.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// Configuration loading / parsing failure.
    #[error("configuration error: {0}")]
    Config(String),
}

impl BmError {
    /// Construct a [`BmError::MutexPoisoned`] from a `PoisonError`.
    ///
    /// The generic `T` is erased — we only care about the error message.
    pub fn mutex_poisoned<T>(
        context: &'static str,
        err: std::sync::PoisonError<T>,
    ) -> Self {
        Self::MutexPoisoned {
            context,
            source: Box::new(StrError(err.to_string())),
        }
    }
}

/// Tiny wrapper so we can box a `PoisonError` message as `dyn Error`.
#[derive(Debug)]
struct StrError(String);

impl std::fmt::Display for StrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StrError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn mutex_poisoned_error_message() {
        let m = Arc::new(Mutex::new(0u32));
        let m2 = Arc::clone(&m);
        let h = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("intentional");
        });
        let _ = h.join();

        let err = m.lock().map_err(|e| BmError::mutex_poisoned("TestStore", e));
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("TestStore"), "got: {msg}");
        assert!(msg.contains("mutex poisoned"), "got: {msg}");
    }

    #[test]
    fn io_error_converts() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let bm: BmError = io_err.into();
        assert!(bm.to_string().contains("gone"));
    }

    #[test]
    fn config_error_message() {
        let err = BmError::Config("bad yaml".into());
        assert_eq!(err.to_string(), "configuration error: bad yaml");
    }

    #[test]
    fn bm_error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<BmError>();
    }

    #[test]
    fn bm_error_into_anyhow() {
        let err = BmError::Config("test".into());
        let anyhow_err: anyhow::Error = err.into();
        assert!(anyhow_err.to_string().contains("test"));
    }
}
