use crate::completions;
use crate::config::Config;
use crate::store::CompletionStore;
use anyhow::{Context, Result};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

/// Run the completion daemon listening on a Unix socket
pub async fn run(socket_path: &Path, cfg: Config) -> Result<()> {
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

    let store = CompletionStore::open_or_create()?;

    loop {
        let (stream, _) = listener.accept().await?;
        let cfg = cfg.clone();

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

                let response = match handle_request(&line.trim(), &cfg) {
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

fn handle_request(json: &str, cfg: &Config) -> Result<String> {
    let req: CompletionRequest =
        serde_json::from_str(json).context("invalid request JSON")?;

    let pos = req.position.unwrap_or(req.buffer.len());
    let store = CompletionStore::open_or_create()?;
    let results = completions::complete(&req.buffer, pos, &store, cfg)?;

    let json = serde_json::to_string(&results)?;
    Ok(format!("{json}\n"))
}
