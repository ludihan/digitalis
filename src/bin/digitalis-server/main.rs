use clap::Parser;
use digitalis::{Library, PlaybackStatus, Track};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use std::{
    net::SocketAddr,
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use routes::AudioCommand;

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

    let state = routes::AppState::new(
        library,
        audio_tx,
        music_root,
    );

    let app = routes::setup_router(state);

    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    info!("Server ready");

    axum::serve(listener, app).await?;

    Ok(())
}
