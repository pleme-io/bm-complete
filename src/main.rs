use bm_complete::{completions, config, daemon, engine, source, store};

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "bm-complete",
    about = "Shell completion daemon — fast, cached completions via Unix socket",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the completion daemon
    Daemon {
        /// Socket path
        #[arg(short, long, default_value = "/tmp/bm-complete.socket")]
        socket: PathBuf,
        /// Config file path
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    /// Query completions (for testing without the daemon)
    Complete {
        /// Command line buffer
        #[arg(short, long)]
        buffer: String,
        /// Cursor position in buffer
        #[arg(short, long)]
        position: Option<usize>,
    },
    /// Index completion sources (fish completions, man pages)
    Index {
        /// Source directory for fish completions
        #[arg(short, long)]
        fish_dir: Option<PathBuf>,
    },
    /// Show daemon status
    Status {
        /// Socket path
        #[arg(short, long, default_value = "/tmp/bm-complete.socket")]
        socket: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Daemon { socket, config } => {
            let cfg = config::load(config.as_deref())?;
            let eng = engine::DefaultEngine::new(cfg)?;
            let eng: Arc<dyn engine::CompletionEngine> = Arc::new(eng);
            daemon::run(&socket, eng).await
        }
        Command::Complete { buffer, position } => {
            let pos = position.unwrap_or(buffer.len());
            let cfg = config::Config::default();
            let st = store::SqliteStore::open_or_create()?;
            let paths = completions::FsPathProvider;
            let results = completions::complete(&buffer, pos, &st, &cfg, &paths)?;
            for r in results {
                println!("{}", serde_json::to_string(&r)?);
            }
            Ok(())
        }
        Command::Index { fish_dir } => {
            let st = store::SqliteStore::open_or_create()?;
            let dirs = if let Some(dir) = &fish_dir {
                vec![dir.clone()]
            } else {
                source::FishSource::default_dirs()
            };
            let fish = source::FishSource::new(dirs);
            let sources: Vec<&dyn source::CompletionSource> = vec![&fish];
            completions::index_sources(&st, &sources)?;
            println!("indexing complete");
            Ok(())
        }
        Command::Status { socket } => {
            if socket.exists() {
                println!("daemon socket exists: {}", socket.display());
                // TODO: send status query over socket
            } else {
                println!("daemon not running (no socket at {})", socket.display());
            }
            Ok(())
        }
    }
}
