use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use digitalis::{Library, PlayRequest, PlaybackStatus, Track, VolumeRequest};
use ratatui::{
    Frame, Terminal,
    backend::{Backend, CrosstermBackend},
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use std::{
    io, net::SocketAddr, time::{Duration, Instant}
};

#[derive(Parser, Debug)]
#[command(name = "digitalis-client")]
#[command(about = "Music player TUI client")]
struct Args {
    #[arg(short, long, default_value = "127.0.0.1:3000")]
    server: SocketAddr,
}

#[derive(Debug, Clone)]
struct App {
    server: SocketAddr,
    library: Option<Library>,
    playback_status: PlaybackStatus,

    artists: Vec<String>,
    albums: Vec<String>,
    tracks: Vec<Track>,

    selected_artist: usize,
    selected_album: usize,
    selected_track: usize,

    active_panel: Panel,

    loading: bool,
    error_message: Option<String>,
    last_update: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Panel {
    Artists,
    Albums,
    Tracks,
}

impl App {
    fn new(server: SocketAddr) -> Self {
        Self {
            server,
            library: None,
            playback_status: PlaybackStatus::default(),
            artists: Vec::new(),
            albums: Vec::new(),
            tracks: Vec::new(),
            selected_artist: 0,
            selected_album: 0,
            selected_track: 0,
            active_panel: Panel::Artists,
            loading: true,
            error_message: None,
            last_update: Instant::now(),
        }
    }

    async fn fetch_library(&mut self, client: &reqwest::Client) -> anyhow::Result<()> {
        let url = format!("{}/api/library", self.server);
        let library = client.get(&url).send().await?.json::<Library>().await?;
        self.library = Some(library);
        self.fetch_artists(client).await?;
        Ok(())
    }

    async fn fetch_artists(&mut self, client: &reqwest::Client) -> anyhow::Result<()> {
        let url = format!("{}/api/library/artists", self.server);
        self.artists = client.get(&url).send().await?.json::<Vec<String>>().await?;
        if !self.artists.is_empty() {
            self.fetch_albums(client).await?;
        }
        Ok(())
    }

    async fn fetch_albums(&mut self, client: &reqwest::Client) -> anyhow::Result<()> {
        if self.selected_artist >= self.artists.len() {
            return Ok(());
        }
        let artist = &self.artists[self.selected_artist];
        let url = format!(
            "{}/api/library/artists/{}/albums",
            self.server,
            urlencoding::encode(artist)
        );
        self.albums = client.get(&url).send().await?.json::<Vec<String>>().await?;
        if !self.albums.is_empty() {
            self.fetch_tracks(client).await?;
        }
        Ok(())
    }

    async fn fetch_tracks(&mut self, client: &reqwest::Client) -> anyhow::Result<()> {
        if self.selected_artist >= self.artists.len() || self.selected_album >= self.albums.len() {
            return Ok(());
        }
        let artist = &self.artists[self.selected_artist];
        let album = &self.albums[self.selected_album];
        let url = format!(
            "{}/api/library/artists/{}/{}",
            self.server,
            urlencoding::encode(artist),
            urlencoding::encode(album)
        );
        self.tracks = client.get(&url).send().await?.json::<Vec<Track>>().await?;
        Ok(())
    }

    async fn fetch_status(&mut self, client: &reqwest::Client) -> anyhow::Result<()> {
        let url = format!("{}/api/status", self.server);
        self.playback_status = client
            .get(&url)
            .send()
            .await?
            .json::<PlaybackStatus>()
            .await?;
        Ok(())
    }

    async fn play_track(&self, client: &reqwest::Client, track: &Track) -> anyhow::Result<()> {
        let url = format!("{}/api/play", self.server);
        let request = PlayRequest {
            path: track.path.clone(),
        };
        client.post(&url).json(&request).send().await?;
        Ok(())
    }

    async fn pause(&self, client: &reqwest::Client) -> anyhow::Result<()> {
        let url = format!("{}/api/pause", self.server);
        client.post(&url).send().await?;
        Ok(())
    }

    async fn resume(&self, client: &reqwest::Client) -> anyhow::Result<()> {
        let url = format!("{}/api/resume", self.server);
        client.post(&url).send().await?;
        Ok(())
    }

    async fn stop(&self, client: &reqwest::Client) -> anyhow::Result<()> {
        let url = format!("{}/api/stop", self.server);
        client.post(&url).send().await?;
        Ok(())
    }

    async fn set_volume(&self, client: &reqwest::Client, volume: f32) -> anyhow::Result<()> {
        let url = format!("{}/api/volume", self.server);
        let request = VolumeRequest {
            volume: volume.clamp(0.0, 1.0),
        };
        client.post(&url).json(&request).send().await?;
        Ok(())
    }

    fn next_panel(&mut self) {
        self.active_panel = match self.active_panel {
            Panel::Artists => Panel::Albums,
            Panel::Albums => Panel::Tracks,
            Panel::Tracks => Panel::Tracks,
        };
    }

    fn prev_panel(&mut self) {
        self.active_panel = match self.active_panel {
            Panel::Artists => Panel::Artists,
            Panel::Albums => Panel::Artists,
            Panel::Tracks => Panel::Albums,
        };
    }

    fn next_item(&mut self) {
        match self.active_panel {
            Panel::Artists => {
                if !self.artists.is_empty() {
                    self.selected_artist = (self.selected_artist + 1) % self.artists.len();
                }
            }
            Panel::Albums => {
                if !self.albums.is_empty() {
                    self.selected_album = (self.selected_album + 1) % self.albums.len();
                }
            }
            Panel::Tracks => {
                if !self.tracks.is_empty() {
                    self.selected_track = (self.selected_track + 1) % self.tracks.len();
                }
            }
        }
    }

    fn prev_item(&mut self) {
        match self.active_panel {
            Panel::Artists => {
                if !self.artists.is_empty() {
                    self.selected_artist = self.selected_artist.saturating_sub(1);
                }
            }
            Panel::Albums => {
                if !self.albums.is_empty() {
                    self.selected_album = self.selected_album.saturating_sub(1);
                }
            }
            Panel::Tracks => {
                if !self.tracks.is_empty() {
                    self.selected_track = self.selected_track.saturating_sub(1);
                }
            }
        }
    }
}

fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(8),
        ])
        .split(f.area());

    // Header
    let header = Paragraph::new(format!("Music Player - Connected to {}", app.server))
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(header, chunks[0]);

    // Browser area
    let browser_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(33),
            Constraint::Percentage(34),
        ])
        .split(chunks[1]);

    // Artists list
    let artists_items: Vec<ListItem> = app
        .artists
        .iter()
        .enumerate()
        .map(|(i, artist)| {
            let style = if i == app.selected_artist {
                if app.active_panel == Panel::Artists {
                    Style::default().bg(Color::Blue).fg(Color::White)
                } else {
                    Style::default().bg(Color::DarkGray).fg(Color::White)
                }
            } else {
                Style::default()
            };
            ListItem::new(artist.as_str()).style(style)
        })
        .collect();

    let artists_list = List::new(artists_items)
        .block(
            Block::default()
                .title("Artists")
                .borders(Borders::ALL)
                .border_style(if app.active_panel == Panel::Artists {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                }),
        )
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));
    f.render_widget(artists_list, browser_chunks[0]);

    // Albums list
    let albums_items: Vec<ListItem> = app
        .albums
        .iter()
        .enumerate()
        .map(|(i, album)| {
            let style = if i == app.selected_album {
                if app.active_panel == Panel::Albums {
                    Style::default().bg(Color::Blue).fg(Color::White)
                } else {
                    Style::default().bg(Color::DarkGray).fg(Color::White)
                }
            } else {
                Style::default()
            };
            ListItem::new(album.as_str()).style(style)
        })
        .collect();

    let albums_list = List::new(albums_items)
        .block(
            Block::default()
                .title("Albums")
                .borders(Borders::ALL)
                .border_style(if app.active_panel == Panel::Albums {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                }),
        )
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));
    f.render_widget(albums_list, browser_chunks[1]);

    // Tracks list
    let tracks_items: Vec<ListItem> = app
        .tracks
        .iter()
        .enumerate()
        .map(|(i, track)| {
            let style = if i == app.selected_track {
                if app.active_panel == Panel::Tracks {
                    Style::default().bg(Color::Blue).fg(Color::White)
                } else {
                    Style::default().bg(Color::DarkGray).fg(Color::White)
                }
            } else {
                Style::default()
            };
            ListItem::new(track.title.as_str()).style(style)
        })
        .collect();

    let tracks_list = List::new(tracks_items)
        .block(
            Block::default()
                .title("Tracks")
                .borders(Borders::ALL)
                .border_style(if app.active_panel == Panel::Tracks {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                }),
        )
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));
    f.render_widget(tracks_list, browser_chunks[2]);

    // Now playing area
    let now_playing_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(chunks[2]);

    let mut now_playing_text = vec![];

    if let Some(ref track) = app.playback_status.track {
        now_playing_text.push(Line::from(vec![
            Span::styled("Now Playing: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{} - {}", track.artist, track.album)),
        ]));
        now_playing_text.push(Line::from(vec![
            Span::styled("Track: ", Style::default().fg(Color::Yellow)),
            Span::raw(&track.title),
        ]));
    } else {
        now_playing_text.push(Line::from("Nothing playing"));
    }

    now_playing_text.push(Line::from(""));

    let position_secs = app.playback_status.position_ms / 1000;
    let position_str = format!("{:02}:{:02}", position_secs / 60, position_secs % 60);

    let status_icon = if app.playback_status.playing {
        "▶"
    } else {
        "⏸"
    };

    now_playing_text.push(Line::from(format!("{} {}", status_icon, position_str)));

    let volume_bar = "█".repeat((app.playback_status.volume * 10.0) as usize)
        + &"░".repeat(10 - (app.playback_status.volume * 10.0) as usize);
    now_playing_text.push(Line::from(format!("Volume: [{}]", volume_bar)));

    let now_playing = Paragraph::new(now_playing_text)
        .block(Block::default().title("Now Playing").borders(Borders::ALL))
        .wrap(Wrap { trim: true });
    f.render_widget(now_playing, now_playing_chunks[0]);

    // Controls
    let controls_text = Text::from(vec![
        Line::from("Controls:"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Tab/← →", Style::default().fg(Color::Green)),
            Span::raw(" Switch panel"),
        ]),
        Line::from(vec![
            Span::styled("↑ ↓", Style::default().fg(Color::Green)),
            Span::raw(" Navigate"),
        ]),
        Line::from(vec![
            Span::styled("Enter", Style::default().fg(Color::Green)),
            Span::raw(" Play track / Select"),
        ]),
        Line::from(vec![
            Span::styled("Space", Style::default().fg(Color::Green)),
            Span::raw(" Pause/Resume"),
        ]),
        Line::from(vec![
            Span::styled("S", Style::default().fg(Color::Green)),
            Span::raw(" Stop  "),
            Span::styled("Q", Style::default().fg(Color::Green)),
            Span::raw(" Quit"),
        ]),
    ]);

    let controls = Paragraph::new(controls_text)
        .block(Block::default().title("Controls").borders(Borders::ALL));
    f.render_widget(controls, now_playing_chunks[1]);

    // Error message overlay
    if let Some(ref error) = app.error_message {
        let error_text = Paragraph::new(error.as_str())
            .style(Style::default().fg(Color::Red))
            .block(Block::default().title("Error").borders(Borders::ALL));
        let area = centered_rect(60, 20, f.area());
        f.render_widget(Clear, area);
        f.render_widget(error_text, area);
    }

    // Loading indicator
    if app.loading {
        let loading_text = Paragraph::new("Loading...")
            .style(Style::default().fg(Color::Yellow))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        let area = centered_rect(30, 10, f.area());
        f.render_widget(Clear, area);
        f.render_widget(loading_text, area);
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

async fn run_app<B: Backend>(terminal: &mut Terminal<B>, mut app: App) -> io::Result<()> {
    let client = reqwest::Client::new();

    // Initial data load
    if let Err(e) = app.fetch_library(&client).await {
        app.error_message = Some(format!("Failed to load library: {}", e));
    }
    app.loading = false;

    let mut last_tick = Instant::now();
    let tick_rate = Duration::from_millis(250);
    let status_update_rate = Duration::from_secs(1);
    let mut last_status_update = Instant::now();

    loop {
        terminal.draw(|f| draw(f, &app))?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(()),
                        KeyCode::Tab | KeyCode::Right => app.next_panel(),
                        KeyCode::Left => app.prev_panel(),
                        KeyCode::Down => app.next_item(),
                        KeyCode::Up => app.prev_item(),
                        KeyCode::Enter => {
                            if app.active_panel == Panel::Artists {
                                if let Err(e) = app.fetch_albums(&client).await {
                                    app.error_message = Some(format!("Error: {}", e));
                                }
                            } else if app.active_panel == Panel::Albums {
                                if let Err(e) = app.fetch_tracks(&client).await {
                                    app.error_message = Some(format!("Error: {}", e));
                                }
                            } else if app.active_panel == Panel::Tracks {
                                if let Some(track) = app.tracks.get(app.selected_track) {
                                    if let Err(e) = app.play_track(&client, track).await {
                                        app.error_message = Some(format!("Error: {}", e));
                                    }
                                }
                            }
                        }
                        KeyCode::Char(' ') => {
                            if app.playback_status.playing {
                                if let Err(e) = app.pause(&client).await {
                                    app.error_message = Some(format!("Error: {}", e));
                                }
                            } else {
                                if let Err(e) = app.resume(&client).await {
                                    app.error_message = Some(format!("Error: {}", e));
                                }
                            }
                        }
                        KeyCode::Char('s') | KeyCode::Char('S') => {
                            if let Err(e) = app.stop(&client).await {
                                app.error_message = Some(format!("Error: {}", e));
                            }
                        }
                        KeyCode::Char('+') | KeyCode::Char('=') => {
                            let new_volume = (app.playback_status.volume + 0.1).min(1.0);
                            if let Err(e) = app.set_volume(&client, new_volume).await {
                                app.error_message = Some(format!("Error: {}", e));
                            }
                        }
                        KeyCode::Char('-') => {
                            let new_volume = (app.playback_status.volume - 0.1).max(0.0);
                            if let Err(e) = app.set_volume(&client, new_volume).await {
                                app.error_message = Some(format!("Error: {}", e));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }

        // Update status periodically
        if last_status_update.elapsed() >= status_update_rate {
            if let Err(e) = app.fetch_status(&client).await {
                app.error_message = Some(format!("Connection error: {}", e));
            } else {
                app.error_message = None;
            }
            last_status_update = Instant::now();
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let app = App::new(args.server);

    let res = run_app(&mut terminal, app).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error: {:?}", err);
    }

    Ok(())
}
