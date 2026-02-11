use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use digitalis::Library;
use ratatui::{
    Frame, Terminal,
    backend::{Backend, CrosstermBackend},
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};
use std::{
    io,
    net::SocketAddr,
    time::{Duration, Instant},
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
    selected_track: usize,
    loading: bool,
    error_message: Option<String>,
}

impl App {
    fn new(server: SocketAddr) -> Self {
        Self {
            server,
            library: None,
            selected_track: 0,
            loading: true,
            error_message: None,
        }
    }

    async fn fetch_library(&mut self, client: &reqwest::Client) -> anyhow::Result<()> {
        let url = format!("http://{}/api/library", self.server);
        dbg!(&url);
        let library = client.get(url).send().await?.json::<Library>().await?;
        self.library = Some(library);
        Ok(())
    }

    fn next_item(&mut self) {
        if let Some(ref library) = self.library {
            let tracks = &library.tracks;
            if !tracks.is_empty() {
                self.selected_track = (self.selected_track + 1) % tracks.len();
            }
        }
    }

    fn prev_item(&mut self) {
        if let Some(ref library) = self.library {
            let tracks = &library.tracks;
            if !tracks.is_empty() {
                self.selected_track = self.selected_track.saturating_sub(1);
            }
        }
    }
}

fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
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

    // Tracks list
    let tracks_items: Vec<ListItem> = match app.library {
        None => Vec::new(),
        Some(ref library) => library
            .tracks
            .iter()
            .enumerate()
            .map(|(i, track)| {
                let style = if i == app.selected_track {
                    Style::default().bg(Color::Blue).fg(Color::White)
                } else {
                    Style::default()
                };
                ListItem::new(track.title.as_str()).style(style)
            })
            .collect(),
    };

    let tracks_list = List::new(tracks_items)
        .block(
            Block::default()
                .title("Tracks")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));
    f.render_widget(tracks_list, chunks[1]);

    // Now playing area
    let now_playing_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(chunks[2]);

    // Controls
    let controls_text = Text::from(vec![
        Line::from("Controls:"),
        Line::from(""),
        Line::from(vec![
            Span::styled("↑ ↓", Style::default().fg(Color::Green)),
            Span::raw(" Navigate"),
        ]),
        Line::from(vec![
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

    let last_tick = Instant::now();
    let tick_rate = Duration::from_millis(250);
    let _status_update_rate = Duration::from_secs(1);
    let _last_status_update = Instant::now();

    loop {
        terminal
            .draw(|f| draw(f, &app))
            .map_err(|_| io::Error::new(io::ErrorKind::AddrNotAvailable, ""))?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Down => app.next_item(),
                        KeyCode::Up => app.prev_item(),
                        KeyCode::Enter => {}
                        _ => {}
                    }
                }
            }
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
