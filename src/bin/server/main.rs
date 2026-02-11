use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
};
use clap::Parser;
use digitalis::{Library, PlayRequest, PlaybackStatus, SeekRequest, Track, VolumeRequest};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{RwLock, mpsc};
use tower_http::cors::CorsLayer;
use tracing::{debug, error, info, warn};

#[derive(Parser, Debug)]
#[command(name = "music-server")]
#[command(about = "Music server with HTTP API")]
struct Args {
    #[arg(short, long, default_value = "~/Music")]
    music_dir: String,
    #[arg(short, long, default_value = "3000")]
    port: u16,
    #[arg(short, long, default_value = "0.0.0.0")]
    bind: String,
}

#[derive(Clone)]
struct AppState {
    library: Arc<RwLock<Library>>,
    audio_tx: mpsc::Sender<AudioCommand>,
    music_root: PathBuf,
}

#[derive(Debug)]
enum AudioCommand {
    Play { path: PathBuf, track: Option<Track> },
    Pause,
    Resume,
    Stop,
    Seek(u64),
    SetVolume(f32),
    GetStatus(tokio::sync::oneshot::Sender<PlaybackStatus>),
}

struct AudioThreadState {
    sink: Option<Sink>,
    _stream: OutputStream,
    _stream_handle: OutputStreamHandle,
    current_track: Option<Track>,
    start_time: Option<Instant>,
    pause_offset: Duration,
    volume: f32,
}

impl AudioThreadState {
    fn new() -> anyhow::Result<Self> {
        let (stream, stream_handle) = OutputStream::try_default()?;
        let sink = Sink::try_new(&stream_handle)?;
        sink.set_volume(1.0);

        Ok(Self {
            sink: Some(sink),
            _stream: stream,
            _stream_handle: stream_handle,
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
        self.sink.as_ref().map(|s| !s.is_paused()).unwrap_or(false)
    }

    fn handle_command(&mut self, cmd: AudioCommand) {
        match cmd {
            AudioCommand::Play { path, track } => match std::fs::File::open(&path) {
                Ok(file) => match Decoder::new(std::io::BufReader::new(file)) {
                    Ok(source) => {
                        if let Some(ref sink) = self.sink {
                            sink.stop();
                            sink.append(source);
                            sink.play();
                            self.current_track = track;
                            self.start_time = Some(Instant::now());
                            self.pause_offset = Duration::ZERO;
                            info!("Started playing: {}", path.display());
                        }
                    }
                    Err(e) => {
                        error!("Failed to decode audio: {}", e);
                    }
                },
                Err(e) => {
                    error!("Failed to open file: {}", e);
                }
            },
            AudioCommand::Pause => {
                if let Some(ref sink) = self.sink {
                    if self.is_playing() {
                        sink.pause();
                        if let Some(start) = self.start_time {
                            self.pause_offset += start.elapsed();
                        }
                        self.start_time = None;
                        info!("Playback paused");
                    }
                }
            }
            AudioCommand::Resume => {
                if let Some(ref sink) = self.sink {
                    sink.play();
                    self.start_time = Some(Instant::now());
                    info!("Playback resumed");
                }
            }
            AudioCommand::Stop => {
                if let Some(ref sink) = self.sink {
                    sink.stop();
                    self.current_track = None;
                    self.start_time = None;
                    self.pause_offset = Duration::ZERO;
                    info!("Playback stopped");
                }
            }
            AudioCommand::Seek(_position_ms) => {
                warn!("Seek not yet implemented - requires rodio sink seek support");
            }
            AudioCommand::SetVolume(vol) => {
                if let Some(ref sink) = self.sink {
                    let volume = vol.clamp(0.0, 1.0);
                    sink.set_volume(volume);
                    self.volume = volume;
                    info!("Volume set to {}", volume);
                }
            }
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
                        debug!(
                            "Found track: {} | Path: {} | Artist: {} | Album: {}",
                            track.title, track.path, track.artist, track.album
                        );
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

    tracks.sort_by(|a, b| {
        a.artist
            .cmp(&b.artist)
            .then(a.album.cmp(&b.album))
            .then(a.title.cmp(&b.title))
    });

    info!(
        "Scanned {} files ({} supported), found {} valid tracks",
        file_count,
        supported_count,
        tracks.len()
    );
    Library { tracks }
}

async fn get_library(State(state): State<AppState>) -> Json<Library> {
    debug!("GET /api/library");
    let library = state.library.read().await;
    Json(library.clone())
}

async fn get_artists(State(state): State<AppState>) -> Json<Vec<String>> {
    debug!("GET /api/library/artists");
    let library = state.library.read().await;
    let mut artists: Vec<String> = library
        .tracks
        .iter()
        .map(|t| t.artist.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    artists.sort();
    Json(artists)
}

async fn get_albums(
    Path(artist): Path<String>,
    State(state): State<AppState>,
) -> Json<Vec<String>> {
    debug!("GET /api/library/artists/{}/albums", artist);
    let library = state.library.read().await;
    let mut albums: Vec<String> = library
        .tracks
        .iter()
        .filter(|t| t.artist == artist)
        .map(|t| t.album.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    albums.sort();
    Json(albums)
}

async fn get_tracks(
    Path((artist, album)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Json<Vec<Track>> {
    debug!("GET /api/library/artists/{}/{}", artist, album);
    let library = state.library.read().await;
    let mut tracks: Vec<Track> = library
        .tracks
        .iter()
        .filter(|t| t.artist == artist && t.album == album)
        .cloned()
        .collect();
    tracks.sort_by(|a, b| a.title.cmp(&b.title));
    Json(tracks)
}

async fn play(State(state): State<AppState>, Json(request): Json<PlayRequest>) -> StatusCode {
    info!("POST /api/play - request.path: {}", request.path);
    debug!("Music root: {}", state.music_root.display());

    // Parse the relative path from the request and join with music root
    let relative_path = PathBuf::from(&request.path);
    debug!("Relative path: {:?}", relative_path);

    // Join with music root - PathBuf::join handles relative paths correctly
    let full_path = state.music_root.join(&relative_path);
    debug!("Full path: {}", full_path.display());

    // Verify the file exists
    let canonical_full_path = match full_path.canonicalize() {
        Ok(path) => path,
        Err(e) => {
            warn!(
                "Track not found or cannot access: {} - Error: {}",
                full_path.display(),
                e
            );
            return StatusCode::NOT_FOUND;
        }
    };
    debug!("Canonicalized full path: {}", canonical_full_path.display());

    // Verify the resolved path is still within the music directory
    if !canonical_full_path.starts_with(&state.music_root) {
        warn!(
            "Path traversal attempt detected: {}",
            canonical_full_path.display()
        );
        return StatusCode::FORBIDDEN;
    }

    // Look up track in library
    let track = state
        .library
        .read()
        .await
        .tracks
        .iter()
        .find(|t| t.path == request.path)
        .cloned();

    info!(
        "Playing: {} (track found: {})",
        canonical_full_path.display(),
        track.is_some()
    );

    let cmd = AudioCommand::Play {
        path: canonical_full_path,
        track,
    };

    match state.audio_tx.send(cmd).await {
        Ok(_) => StatusCode::OK,
        Err(e) => {
            error!("Failed to send play command: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

async fn pause(State(state): State<AppState>) -> StatusCode {
    info!("POST /api/pause");
    match state.audio_tx.send(AudioCommand::Pause).await {
        Ok(_) => StatusCode::OK,
        Err(e) => {
            error!("Failed to send pause command: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

async fn resume(State(state): State<AppState>) -> StatusCode {
    info!("POST /api/resume");
    match state.audio_tx.send(AudioCommand::Resume).await {
        Ok(_) => StatusCode::OK,
        Err(e) => {
            error!("Failed to send resume command: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

async fn stop(State(state): State<AppState>) -> StatusCode {
    info!("POST /api/stop");
    match state.audio_tx.send(AudioCommand::Stop).await {
        Ok(_) => StatusCode::OK,
        Err(e) => {
            error!("Failed to send stop command: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

async fn seek(State(state): State<AppState>, Json(request): Json<SeekRequest>) -> StatusCode {
    info!("POST /api/seek - {}ms", request.position_ms);
    match state
        .audio_tx
        .send(AudioCommand::Seek(request.position_ms))
        .await
    {
        Ok(_) => StatusCode::NOT_IMPLEMENTED,
        Err(e) => {
            error!("Failed to send seek command: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

async fn set_volume(
    State(state): State<AppState>,
    Json(request): Json<VolumeRequest>,
) -> StatusCode {
    info!("POST /api/volume - {}", request.volume);
    match state
        .audio_tx
        .send(AudioCommand::SetVolume(request.volume))
        .await
    {
        Ok(_) => StatusCode::OK,
        Err(e) => {
            error!("Failed to send volume command: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

async fn get_status(State(state): State<AppState>) -> Json<PlaybackStatus> {
    debug!("GET /api/status");
    let (tx, rx) = tokio::sync::oneshot::channel();

    match state.audio_tx.send(AudioCommand::GetStatus(tx)).await {
        Ok(_) => match rx.await {
            Ok(status) => Json(status),
            Err(e) => {
                error!("Failed to receive status: {}", e);
                Json(PlaybackStatus {
                    playing: false,
                    track: None,
                    position_ms: 0,
                    duration_ms: None,
                    volume: 1.0,
                })
            }
        },
        Err(e) => {
            error!("Failed to send get_status command: {}", e);
            Json(PlaybackStatus {
                playing: false,
                track: None,
                position_ms: 0,
                duration_ms: None,
                volume: 1.0,
            })
        }
    }
}

async fn health_check() -> &'static str {
    "OK"
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

    let music_root = if args.music_dir.starts_with("~/") {
        dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
            .join(&args.music_dir[2..])
    } else {
        PathBuf::from(&args.music_dir)
    };

    if !music_root.exists() {
        anyhow::bail!("Music directory does not exist: {}", music_root.display());
    }

    // Canonicalize the music root path for consistent path handling
    let music_root = music_root
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("Failed to canonicalize music directory: {}", e))?;

    info!("Starting music server");
    info!("Music directory: {}", music_root.display());
    info!("Listening on {}:{}", args.bind, args.port);

    let library = scan_library(&music_root);
    let library = Arc::new(RwLock::new(library));

    let audio_tx = spawn_audio_thread()?;

    let state = AppState {
        library: library.clone(),
        audio_tx,
        music_root: music_root.clone(),
    };

    let app = Router::new()
        .route("/health", get(health_check))
        .route("/api/library", get(get_library))
        .route("/api/library/artists", get(get_artists))
        .route("/api/library/artists/{artist}/albums", get(get_albums))
        .route("/api/library/artists/{artist}/{album}", get(get_tracks))
        .route("/api/play", post(play))
        .route("/api/pause", post(pause))
        .route("/api/resume", post(resume))
        .route("/api/stop", post(stop))
        .route("/api/seek", post(seek))
        .route("/api/volume", post(set_volume))
        .route("/api/status", get(get_status))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("{}:{}", args.bind, args.port)).await?;
    info!("Server ready");

    axum::serve(listener, app).await?;

    Ok(())
}
