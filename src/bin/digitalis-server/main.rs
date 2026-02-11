use clap::Parser;
use digitalis::{Library, PlaybackStatus, Track};
use rodio::Decoder;
use routes::AudioCommand;
use std::{
    net::SocketAddr,
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

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

struct AudioThreadState {
    current_track: Option<Track>,
    start_time: Option<Instant>,
    pause_offset: Duration,
    volume: f32,
}

impl AudioThreadState {
    fn new() -> anyhow::Result<Self> {
        Ok(Self {
            current_track: None,
            start_time: None,
            pause_offset: Duration::ZERO,
            volume: 1.0,
        })
    }

    fn position(&self) -> u64 {
        if let Some(start) = self.start_time {
            let elapsed = start.elapsed() + self.pause_offset;
            elapsed.as_millis() as u64
        } else {
            self.pause_offset.as_millis() as u64
        }
    }

    fn is_playing(&self) -> bool {
        true //self.sink.as_ref().map(|s| !s.is_paused()).unwrap_or(false)
    }

    fn handle_command(&mut self, cmd: AudioCommand) {
        match cmd {
            AudioCommand::Play { path, track } => match std::fs::File::open(&path) {
                Ok(file) => match Decoder::new(std::io::BufReader::new(file)) {
                    Ok(source) => {}
                    Err(e) => {
                        error!("Failed to decode audio: {}", e);
                    }
                },
                Err(e) => {
                    error!("Failed to open file: {}", e);
                }
            },
            AudioCommand::Pause => {}
            AudioCommand::Resume => {}
            AudioCommand::Stop => {}
            AudioCommand::Seek(_position_ms) => {
                warn!("Seek not yet implemented - requires rodio sink seek support");
            }
            AudioCommand::SetVolume(vol) => {}
            AudioCommand::GetStatus(tx) => {
                let status = PlaybackStatus {
                    playing: self.is_playing(),
                    track: self.current_track.clone(),
                    position_ms: self.position(),
                    duration_ms: None,
                    volume: self.volume,
                };
                let _ = tx.send(status);
            }
        }
    }
}

fn spawn_audio_thread() -> anyhow::Result<mpsc::Sender<AudioCommand>> {
    let (tx, mut rx) = mpsc::channel::<AudioCommand>(32);

    std::thread::spawn(move || {
        let mut state = match AudioThreadState::new() {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to initialize audio: {}", e);
                return;
            }
        };

        while let Some(cmd) = rx.blocking_recv() {
            state.handle_command(cmd);
        }
    });

    Ok(tx)
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

    let audio_tx = spawn_audio_thread()?;

    let state = routes::AppState::new(library, audio_tx, music_root);

    let app = routes::setup_router(state);

    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    info!("Server ready");

    axum::serve(listener, app).await?;

    Ok(())
}
