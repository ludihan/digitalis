use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub path: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackStatus {
    pub playing: bool,
    pub track: Option<Track>,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub volume: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeekRequest {
    pub position_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeRequest {
    pub volume: f32,
}

impl Track {
    pub fn from_path(full_path: &PathBuf, music_root: &PathBuf) -> Option<Self> {
        // Normalize paths to ensure consistent handling
        let full_path = full_path
            .canonicalize()
            .unwrap_or_else(|_| full_path.clone());
        let music_root = music_root
            .canonicalize()
            .unwrap_or_else(|_| music_root.clone());

        let relative_path = full_path.strip_prefix(&music_root).ok()?;

        // Normalize the path separators for cross-platform compatibility
        // and remove any leading separator
        let path_str = relative_path
            .to_str()?
            .trim_start_matches(['/', '\\'])
            .replace('\\', "/")
            .to_string();

        let components: Vec<_> = relative_path.iter().collect();
        if components.len() < 3 {
            return None;
        }

        let filename = components[components.len() - 1].to_str()?;
        let title = filename
            .rsplit_once('.')
            .map(|(name, _)| name)
            .unwrap_or(filename)
            .to_string();

        Some(Track {
            path: path_str,
            title,
        })
    }
}

impl Default for PlaybackStatus {
    fn default() -> Self {
        Self {
            playing: false,
            track: None,
            position_ms: 0,
            duration_ms: None,
            volume: 1.0,
        }
    }
}
