use axum::{Router, extract::State, response::Json, routing::get};
use digitalis::Library;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tracing::debug;

#[derive(Clone)]
pub struct AppState {
    library: Arc<RwLock<Library>>,
    music_root: PathBuf,
}

impl AppState {
    pub fn new(library: Library, music_root: PathBuf) -> Self {
        AppState {
            library: Arc::new(RwLock::new(library)),
            music_root: music_root.clone(),
        }
    }
}

async fn get_library(State(state): State<AppState>) -> Json<Library> {
    debug!("GET /api/library");
    let library = state.library.read().await;
    Json(library.clone())
}

async fn health_check() -> &'static str {
    "OK"
}

pub fn setup_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/api/library", get(get_library))
        .layer(CorsLayer::permissive())
        .with_state(state)
}
