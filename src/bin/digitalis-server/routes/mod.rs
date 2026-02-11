use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
};
use digitalis::{Library, PlayRequest, PlaybackStatus, SeekRequest, Track, VolumeRequest};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tower_http::cors::CorsLayer;
use tracing::{debug, error, info, warn};

#[derive(Clone)]
pub struct AppState {
    library: Arc<RwLock<Library>>,
    audio_tx: mpsc::Sender<AudioCommand>,
    music_root: PathBuf,
}

impl AppState {
    pub fn new(library: Library, audio_tx: mpsc::Sender<AudioCommand>, music_root: PathBuf) -> Self {
        AppState {
            library: Arc::new(RwLock::new(library)),
            audio_tx,
            music_root: music_root.clone(),
        }
    }
}

#[derive(Debug)]
pub enum AudioCommand {
    Play { path: PathBuf, track: Option<Track> },
    Pause,
    Resume,
    Stop,
    Seek(u64),
    SetVolume(f32),
    GetStatus(tokio::sync::oneshot::Sender<PlaybackStatus>),
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

pub fn setup_router(state: AppState) -> Router {
    Router::new()
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
        .with_state(state)
}
