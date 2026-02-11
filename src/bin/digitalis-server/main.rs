use clap::Parser;
use digitalis::{Library, Track};
use std::{net::SocketAddr, path::PathBuf};
use tracing::{debug, info};

mod routes;

#[derive(Parser, Debug)]
#[command(name = "digitalis-server")]
#[command(about = "Music server with HTTP API")]
struct Args {
    #[arg(short, long, default_value_os_t = dirs::audio_dir().unwrap())]
    music_dir: PathBuf,
    #[arg(short, long, default_value = "0.0.0.0:3000")]
    bind: SocketAddr,
}

fn scan_library(music_root: &PathBuf) -> Library {
    info!("Scanning music directory: {}", music_root.display());

    // music_root is already canonicalized in main(), so we can use it directly
    let mut tracks = Vec::new();
    let mut file_count = 0;
    let mut supported_count = 0;

    for entry in walkdir::WalkDir::new(music_root)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.is_file() {
            file_count += 1;
            if let Some(ext) = path.extension() {
                let ext = ext.to_string_lossy().to_lowercase();
                if ["mp3", "flac", "ogg", "wav", "m4a", "aac"].contains(&ext.as_str()) {
                    supported_count += 1;
                    if let Some(track) = Track::from_path(&path.to_path_buf(), music_root) {
                        debug!("Found track: {} | Path: {}", track.title, track.path);
                        tracks.push(track);
                    } else {
                        debug!(
                            "Skipping file (could not parse metadata): {}",
                            path.display()
                        );
                    }
                }
            }
        }
    }

    info!(
        "Scanned {} files ({} supported), found {} valid tracks",
        file_count,
        supported_count,
        tracks.len()
    );
    Library { tracks }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "music_server=info,tower_http=debug".into()),
        )
        .init();

    let args = Args::parse();

    let music_root = args.music_dir;

    if !music_root.exists() {
        anyhow::bail!("Music directory does not exist: {}", music_root.display());
    }

    let music_root = music_root
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("Failed to canonicalize music directory: {}", e))?;

    info!("Starting music server");
    info!("Music directory: {}", music_root.display());
    info!("Listening on {}:{}", args.bind, args.bind.port());

    let library = scan_library(&music_root);

    let state = routes::AppState::new(library, music_root);

    let app = routes::setup_router(state);

    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    info!("Server ready");

    axum::serve(listener, app).await?;

    Ok(())
}
