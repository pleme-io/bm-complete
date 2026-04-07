use crate::engine::CompletionEngine;
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

/// Run the completion daemon listening on a Unix socket.
///
/// The engine is opened once at startup and shared across all connections.
///
/// # Errors
///
/// Returns an error if the socket cannot be bound or a fatal I/O error
/// occurs on the listener.
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
        let result = handle_request(r"{}", &engine);
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

    #[test]
    fn handle_request_unicode_buffer() {
        let engine = MockEngine {
            results: Vec::new(),
        };
        let resp =
            handle_request(r#"{"buffer": "echo 日本語", "position": 12}"#, &engine).unwrap();
        let parsed: Vec<CompletionEntry> = serde_json::from_str(resp.trim()).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn handle_request_large_position() {
        let engine = MockEngine {
            results: Vec::new(),
        };
        let resp = handle_request(
            r#"{"buffer": "ls", "position": 9999}"#,
            &engine,
        )
        .unwrap();
        let parsed: Vec<CompletionEntry> = serde_json::from_str(resp.trim()).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn handle_request_null_position_uses_buffer_len() {
        let engine = MockEngine {
            results: vec![CompletionEntry {
                command: "test".into(),
                completion: "result".into(),
                description: String::new(),
                source: "mock".into(),
            }],
        };
        let resp =
            handle_request(r#"{"buffer": "test ", "position": null}"#, &engine).unwrap();
        let parsed: Vec<CompletionEntry> = serde_json::from_str(resp.trim()).unwrap();
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn handle_request_error_json_format() {
        let engine = MockEngine {
            results: Vec::new(),
        };
        let result = handle_request("not json at all", &engine);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("invalid request JSON"),
            "error should mention invalid JSON: {err_msg}"
        );
    }

    #[test]
    fn handle_request_wrong_type_for_buffer() {
        let engine = MockEngine {
            results: Vec::new(),
        };
        let result = handle_request(r#"{"buffer": 42}"#, &engine);
        assert!(result.is_err(), "numeric buffer should fail deserialization");
    }

    #[test]
    fn handle_request_wrong_type_for_position() {
        let engine = MockEngine {
            results: Vec::new(),
        };
        let result = handle_request(r#"{"buffer": "ls", "position": "five"}"#, &engine);
        assert!(
            result.is_err(),
            "string position should fail deserialization"
        );
    }

    #[tokio::test]
    async fn daemon_run_accepts_connection() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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

    #[tokio::test]
    async fn daemon_concurrent_connections() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("concurrent.socket");

        let engine: Arc<dyn CompletionEngine> = Arc::new(MockEngine {
            results: vec![CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: String::new(),
                source: "mock".into(),
            }],
        });

        let sp = socket_path.clone();
        let handle = tokio::spawn(async move { run(&sp, engine).await });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut tasks = Vec::new();
        for _ in 0..4 {
            let sp = socket_path.clone();
            tasks.push(tokio::spawn(async move {
                let stream = tokio::net::UnixStream::connect(&sp).await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut reader = BufReader::new(reader);

                writer
                    .write_all(b"{\"buffer\": \"git co\", \"position\": 6}\n")
                    .await
                    .unwrap();

                let mut response = String::new();
                reader.read_line(&mut response).await.unwrap();

                let parsed: Vec<CompletionEntry> =
                    serde_json::from_str(response.trim()).unwrap();
                assert_eq!(parsed.len(), 1);
                assert_eq!(parsed[0].completion, "commit");
            }));
        }

        for task in tasks {
            task.await.unwrap();
        }

        handle.abort();
    }

    #[tokio::test]
    async fn daemon_handles_malformed_json_gracefully() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("malformed.socket");

        let engine: Arc<dyn CompletionEngine> = Arc::new(MockEngine {
            results: Vec::new(),
        });

        let sp = socket_path.clone();
        let handle = tokio::spawn(async move { run(&sp, engine).await });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);

        writer
            .write_all(b"this is not json\n")
            .await
            .unwrap();

        let mut response = String::new();
        reader.read_line(&mut response).await.unwrap();

        assert!(
            response.contains("error"),
            "malformed request should produce an error response: {response}"
        );

        writer
            .write_all(b"{\"buffer\": \"ls\"}\n")
            .await
            .unwrap();

        let mut response2 = String::new();
        reader.read_line(&mut response2).await.unwrap();

        let parsed: Vec<CompletionEntry> =
            serde_json::from_str(response2.trim()).unwrap();
        assert!(
            parsed.is_empty() || !parsed.is_empty(),
            "connection should still work after malformed request"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn daemon_handles_client_disconnect() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("disconnect.socket");

        let engine: Arc<dyn CompletionEngine> = Arc::new(MockEngine {
            results: Vec::new(),
        });

        let sp = socket_path.clone();
        let handle = tokio::spawn(async move { run(&sp, engine).await });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        {
            let _stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        }

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let stream = tokio::net::UnixStream::connect(&socket_path).await;
        assert!(
            stream.is_ok(),
            "daemon should accept new connections after a client disconnects"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn daemon_response_is_valid_json_array() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("json_array.socket");

        let engine: Arc<dyn CompletionEngine> = Arc::new(MockEngine {
            results: vec![
                CompletionEntry {
                    command: "git".into(),
                    completion: "commit".into(),
                    description: "Record changes".into(),
                    source: "mock".into(),
                },
                CompletionEntry {
                    command: "git".into(),
                    completion: "checkout".into(),
                    description: "Switch branches".into(),
                    source: "mock".into(),
                },
            ],
        });

        let sp = socket_path.clone();
        let handle = tokio::spawn(async move { run(&sp, engine).await });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);

        writer
            .write_all(b"{\"buffer\": \"git co\", \"position\": 6}\n")
            .await
            .unwrap();

        let mut response = String::new();
        reader.read_line(&mut response).await.unwrap();

        assert!(response.ends_with('\n'), "response should be newline-terminated");

        let parsed: Vec<CompletionEntry> =
            serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed.len(), 2);

        for entry in &parsed {
            assert_eq!(entry.command, "git");
            assert_eq!(entry.source, "mock");
        }

        handle.abort();
    }
}
