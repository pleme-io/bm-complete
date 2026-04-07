use crate::engine::CompletionEngine;
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

/// Run the completion daemon listening on a Unix socket.
///
/// The engine is opened once at startup and shared across all connections.
pub async fn run(socket_path: &Path, engine: Arc<dyn CompletionEngine>) -> Result<()> {
    // Remove stale socket
    if socket_path.exists() {
        std::fs::remove_file(socket_path).context("failed to remove stale socket")?;
    }

    // Ensure parent directory exists
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(socket_path)
        .context(format!("failed to bind to {}", socket_path.display()))?;

    println!("bm-complete daemon listening on {}", socket_path.display());

    loop {
        let (stream, _) = listener.accept().await?;
        let engine = Arc::clone(&engine);

        // Handle each connection
        // Protocol: one JSON request per line, one JSON response per line
        // Request: {"buffer": "git co", "position": 6}
        // Response: [{"completion": "commit", "description": "..."}, ...]
        tokio::spawn(async move {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut line = String::new();

            while let Ok(n) = reader.read_line(&mut line).await {
                if n == 0 {
                    break;
                }

                let response = match handle_request(line.trim(), &*engine) {
                    Ok(resp) => resp,
                    Err(e) => format!("{{\"error\": \"{e}\"}}\n"),
                };

                if writer.write_all(response.as_bytes()).await.is_err() {
                    break;
                }
                line.clear();
            }
        });
    }
}

#[derive(serde::Deserialize)]
struct CompletionRequest {
    buffer: String,
    #[serde(default)]
    position: Option<usize>,
}

fn handle_request(json: &str, engine: &dyn CompletionEngine) -> Result<String> {
    let req: CompletionRequest =
        serde_json::from_str(json).context("invalid request JSON")?;

    let pos = req.position.unwrap_or(req.buffer.len());
    let results = engine.complete(&req.buffer, pos)?;

    let json = serde_json::to_string(&results)?;
    Ok(format!("{json}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::CompletionEntry;

    struct MockEngine {
        results: Vec<CompletionEntry>,
    }

    impl CompletionEngine for MockEngine {
        fn complete(&self, _buffer: &str, _position: usize) -> Result<Vec<CompletionEntry>> {
            Ok(self.results.clone())
        }
    }

    #[test]
    fn handle_request_valid_json() {
        let engine = MockEngine {
            results: vec![CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: "Record changes".into(),
                source: "mock".into(),
            }],
        };
        let resp =
            handle_request(r#"{"buffer": "git co", "position": 6}"#, &engine).unwrap();
        assert!(resp.contains("commit"));
    }

    #[test]
    fn handle_request_invalid_json() {
        let engine = MockEngine {
            results: Vec::new(),
        };
        let result = handle_request("not json", &engine);
        assert!(result.is_err());
    }

    #[test]
    fn handle_request_position_defaults_to_buffer_len() {
        let engine = MockEngine {
            results: Vec::new(),
        };
        let resp = handle_request(r#"{"buffer": "ls"}"#, &engine).unwrap();
        // Should return an empty array (valid JSON) — no panic
        assert!(resp.contains('['));
    }

    #[test]
    fn handle_request_returns_json_array() {
        let engine = MockEngine {
            results: vec![
                CompletionEntry {
                    command: "git".into(),
                    completion: "commit".into(),
                    description: "Record changes".into(),
                    source: "mock".into(),
                },
                CompletionEntry {
                    command: "git".into(),
                    completion: "config".into(),
                    description: "Get/set options".into(),
                    source: "mock".into(),
                },
            ],
        };
        let resp =
            handle_request(r#"{"buffer": "git co", "position": 6}"#, &engine).unwrap();
        let parsed: Vec<CompletionEntry> =
            serde_json::from_str(resp.trim()).expect("response should be valid JSON array");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].completion, "commit");
        assert_eq!(parsed[1].completion, "config");
    }

    #[test]
    fn handle_request_error_returns_empty() {
        let engine = MockEngine {
            results: Vec::new(),
        };
        // Malformed JSON — handle_request returns Err, which the caller
        // converts to an error JSON. Verify the Err path.
        let result = handle_request("{malformed", &engine);
        assert!(result.is_err(), "malformed JSON should produce an error");
    }

    #[test]
    fn handle_request_empty_buffer() {
        let engine = MockEngine {
            results: Vec::new(),
        };
        let resp = handle_request(r#"{"buffer": ""}"#, &engine).unwrap();
        let parsed: Vec<CompletionEntry> = serde_json::from_str(resp.trim()).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn handle_request_response_ends_with_newline() {
        let engine = MockEngine {
            results: Vec::new(),
        };
        let resp = handle_request(r#"{"buffer": "ls"}"#, &engine).unwrap();
        assert!(
            resp.ends_with('\n'),
            "response should end with newline for line-based protocol"
        );
    }

    #[test]
    fn handle_request_explicit_position() {
        let engine = MockEngine {
            results: vec![CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: String::new(),
                source: "mock".into(),
            }],
        };
        let resp = handle_request(
            r#"{"buffer": "git commit --amend", "position": 10}"#,
            &engine,
        )
        .unwrap();
        let parsed: Vec<CompletionEntry> = serde_json::from_str(resp.trim()).unwrap();
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn handle_request_empty_json_object() {
        let engine = MockEngine {
            results: Vec::new(),
        };
        let result = handle_request(r#"{}"#, &engine);
        assert!(
            result.is_err(),
            "JSON without 'buffer' field should error"
        );
    }

    #[test]
    fn handle_request_extra_fields_ignored() {
        let engine = MockEngine {
            results: Vec::new(),
        };
        let resp = handle_request(
            r#"{"buffer": "ls", "position": 2, "extra": true}"#,
            &engine,
        )
        .unwrap();
        assert!(resp.contains('['), "extra fields should be silently ignored");
    }

    #[test]
    fn handle_request_position_zero() {
        let engine = MockEngine {
            results: Vec::new(),
        };
        let resp =
            handle_request(r#"{"buffer": "git commit", "position": 0}"#, &engine).unwrap();
        let parsed: Vec<CompletionEntry> = serde_json::from_str(resp.trim()).unwrap();
        assert!(parsed.is_empty());
    }

    #[tokio::test]
    async fn daemon_run_accepts_connection() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("test.socket");

        let engine: Arc<dyn CompletionEngine> = Arc::new(MockEngine {
            results: vec![CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: "Record changes".into(),
                source: "mock".into(),
            }],
        });

        let sp = socket_path.clone();
        let handle = tokio::spawn(async move { run(&sp, engine).await });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        let (reader, mut writer) = stream.into_split();

        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        writer
            .write_all(b"{\"buffer\": \"git co\", \"position\": 6}\n")
            .await
            .unwrap();

        let mut reader = BufReader::new(reader);
        let mut response = String::new();
        reader.read_line(&mut response).await.unwrap();

        let parsed: Vec<CompletionEntry> =
            serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].completion, "commit");

        handle.abort();
    }

    #[tokio::test]
    async fn daemon_removes_stale_socket() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("stale.socket");

        std::fs::write(&socket_path, "stale").unwrap();
        assert!(socket_path.exists());

        let engine: Arc<dyn CompletionEngine> = Arc::new(MockEngine {
            results: Vec::new(),
        });

        let sp = socket_path.clone();
        let handle = tokio::spawn(async move { run(&sp, engine).await });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert!(
            socket_path.exists(),
            "daemon should create a new socket after removing stale one"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn daemon_multiple_requests_on_same_connection() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("multi.socket");

        let engine: Arc<dyn CompletionEngine> = Arc::new(MockEngine {
            results: vec![CompletionEntry {
                command: "test".into(),
                completion: "result".into(),
                description: String::new(),
                source: "mock".into(),
            }],
        });

        let sp = socket_path.clone();
        let handle = tokio::spawn(async move { run(&sp, engine).await });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);

        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        for _ in 0..3 {
            writer
                .write_all(b"{\"buffer\": \"test \"}\n")
                .await
                .unwrap();
            let mut response = String::new();
            reader.read_line(&mut response).await.unwrap();
            let parsed: Vec<CompletionEntry> =
                serde_json::from_str(response.trim()).unwrap();
            assert_eq!(parsed.len(), 1);
        }

        handle.abort();
    }
}
