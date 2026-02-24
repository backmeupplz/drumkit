use anyhow::Result;
use arc_swap::ArcSwap;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use notify::Watcher;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use std::collections::HashMap;
use std::io::{self, stdout};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use crate::{audio, kit, midi, setup};

/// Events fed into the TUI from various sources.
pub enum TuiEvent {
    Hit { note: u8, velocity: u8 },
    Choke { note: u8 },
    KitReloaded { note_keys: Vec<u8> },
    KitReloadError(String),
}

/// Popup overlays in play mode.
pub enum Popup {
    Log { scroll: usize },
    KitPicker { kits: Vec<setup::DiscoveredKit>, list_state: ListState },
    AudioPicker { devices: Vec<audio::AudioDevice>, list_state: ListState },
    MidiPicker { devices: Vec<midi::MidiDevice>, list_state: ListState },
}

/// Swappable resources owned by the TUI event loop during play mode.
pub struct PlayResources {
    pub stream: cpal::Stream,
    pub connection: midir::MidiInputConnection<()>,
    pub producer: Arc<Mutex<Option<rtrb::Producer<audio::AudioCommand>>>>,
    pub shared_notes: Arc<ArcSwap<HashMap<u8, Arc<kit::NoteGroup>>>>,
    pub kit_path: PathBuf,
    pub sample_rate: u32,
    pub channels: u16,
    pub audio_device_index: usize,
    pub midi_port_index: usize,
    pub tui_tx: mpsc::Sender<TuiEvent>,
    pub choke_fade: usize,
    pub aftertouch_fade: usize,
    pub watcher: notify::RecommendedWatcher,
    pub stderr_capture: Option<setup::StderrCapture>,
}

/// Visual state for a single pad in the grid.
struct PadState {
    note: u8,
    name: &'static str,
    last_hit: Option<Instant>,
    last_velocity: u8,
}

/// A single entry in the scrolling hit log.
struct HitLogEntry {
    name: &'static str,
    note: u8,
    velocity: u8,
}

const MAX_HIT_LOG: usize = 50;
const FLASH_DURATION_MS: u128 = 300;

/// Complete application state for the TUI.
pub struct AppState {
    pub kit_name: String,
    pub midi_device: String,
    pub sample_rate: u32,
    pub channels: u16,
    pads: Vec<PadState>,
    pad_index: HashMap<u8, usize>,
    hit_log: Vec<HitLogEntry>,
    total_hits: u64,
    status_message: Option<(String, Instant)>,
    should_quit: bool,
    popup: Option<Popup>,
    log_lines: Vec<String>,
}

impl AppState {
    pub fn new(
        kit_name: String,
        midi_device: String,
        sample_rate: u32,
        channels: u16,
        note_keys: &[u8],
        initial_log: Vec<String>,
    ) -> Self {
        let mut pads = Vec::with_capacity(note_keys.len());
        let mut pad_index = HashMap::new();
        for (i, &note) in note_keys.iter().enumerate() {
            pads.push(PadState {
                note,
                name: midi::drum_name(note),
                last_hit: None,
                last_velocity: 0,
            });
            pad_index.insert(note, i);
        }
        Self {
            kit_name,
            midi_device,
            sample_rate,
            channels,
            pads,
            pad_index,
            hit_log: Vec::new(),
            total_hits: 0,
            status_message: None,
            should_quit: false,
            popup: None,
            log_lines: initial_log,
        }
    }

    fn on_hit(&mut self, note: u8, velocity: u8) {
        self.total_hits += 1;
        if let Some(&idx) = self.pad_index.get(&note) {
            self.pads[idx].last_hit = Some(Instant::now());
            self.pads[idx].last_velocity = velocity;
        }
        self.hit_log.insert(
            0,
            HitLogEntry {
                name: midi::drum_name(note),
                note,
                velocity,
            },
        );
        if self.hit_log.len() > MAX_HIT_LOG {
            self.hit_log.truncate(MAX_HIT_LOG);
        }
    }

    fn on_choke(&mut self, note: u8) {
        if let Some(&idx) = self.pad_index.get(&note) {
            self.pads[idx].last_hit = Some(Instant::now());
        }
    }

    fn rebuild_pads(&mut self, note_keys: &[u8]) {
        self.pads.clear();
        self.pad_index.clear();
        for (i, &note) in note_keys.iter().enumerate() {
            self.pads.push(PadState {
                note,
                name: midi::drum_name(note),
                last_hit: None,
                last_velocity: 0,
            });
            self.pad_index.insert(note, i);
        }
    }

    fn set_status(&mut self, msg: String) {
        self.status_message = Some((msg, Instant::now()));
    }
}

/// Enter alternate screen, run the event loop, and restore terminal on exit.
pub fn run(event_rx: mpsc::Receiver<TuiEvent>, mut state: AppState, mut resources: PlayResources) -> Result<()> {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = stdout().execute(LeaveAlternateScreen);
        let _ = disable_raw_mode();
        original_hook(panic_info);
    }));

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;

    let backend = ratatui::backend::CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = event_loop(&mut terminal, &event_rx, &mut state, &mut resources);

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    // Restore stderr
    if let Some(cap) = resources.stderr_capture.take() {
        cap.restore();
    }

    result
}

fn event_loop(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
    event_rx: &mpsc::Receiver<TuiEvent>,
    state: &mut AppState,
    resources: &mut PlayResources,
) -> Result<()> {
    loop {
        // Drain captured stderr lines
        if let Some(ref cap) = resources.stderr_capture {
            cap.drain_into(&mut state.log_lines);
        }

        terminal.draw(|frame| ui(frame, state))?;

        if state.should_quit {
            return Ok(());
        }

        if event::poll(Duration::from_millis(33))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                // Route keys to popup handler if a popup is open
                if state.popup.is_some() {
                    handle_popup_key(state, resources, key.code);
                    continue;
                }

                // Main key handler
                match key.code {
                    KeyCode::Char('q') => {
                        state.should_quit = true;
                    }
                    KeyCode::Char('l') => {
                        let scroll = state.log_lines.len().saturating_sub(1);
                        state.popup = Some(Popup::Log { scroll });
                    }
                    KeyCode::Char('k') => {
                        let kits = setup::discover_kits();
                        let mut list_state = ListState::default();
                        if !kits.is_empty() {
                            list_state.select(Some(0));
                        }
                        state.popup = Some(Popup::KitPicker { kits, list_state });
                    }
                    KeyCode::Char('a') => {
                        if let Ok(devices) = audio::list_output_devices() {
                            let mut list_state = ListState::default();
                            if !devices.is_empty() {
                                // Pre-select current device
                                let sel = devices.iter().position(|d| d.index == resources.audio_device_index).unwrap_or(0);
                                list_state.select(Some(sel));
                            }
                            state.popup = Some(Popup::AudioPicker { devices, list_state });
                        }
                    }
                    KeyCode::Char('m') => {
                        if let Ok(devices) = midi::list_devices() {
                            let mut list_state = ListState::default();
                            if !devices.is_empty() {
                                let sel = devices.iter().position(|d| d.port_index == resources.midi_port_index).unwrap_or(0);
                                list_state.select(Some(sel));
                            }
                            state.popup = Some(Popup::MidiPicker { devices, list_state });
                        }
                    }
                    _ => {}
                }
            }
        }

        while let Ok(ev) = event_rx.try_recv() {
            match ev {
                TuiEvent::Hit { note, velocity } => state.on_hit(note, velocity),
                TuiEvent::Choke { note } => state.on_choke(note),
                TuiEvent::KitReloaded { note_keys } => {
                    state.rebuild_pads(&note_keys);
                    state.set_status("Kit reloaded".to_string());
                }
                TuiEvent::KitReloadError(msg) => {
                    state.set_status(format!("Reload error: {}", msg));
                }
            }
        }
    }
}

fn handle_popup_key(state: &mut AppState, resources: &mut PlayResources, key: KeyCode) {
    let popup = state.popup.as_mut().unwrap();
    match popup {
        Popup::Log { scroll } => match key {
            KeyCode::Char('l') | KeyCode::Esc => { state.popup = None; }
            KeyCode::Char('q') => { state.popup = None; state.should_quit = true; }
            KeyCode::Up => { *scroll = scroll.saturating_sub(1); }
            KeyCode::Down => {
                if *scroll + 1 < state.log_lines.len() { *scroll += 1; }
            }
            KeyCode::Home => { *scroll = 0; }
            KeyCode::End => { *scroll = state.log_lines.len().saturating_sub(1); }
            _ => {}
        },
        Popup::KitPicker { kits, list_state } => match key {
            KeyCode::Char('k') | KeyCode::Esc => { state.popup = None; }
            KeyCode::Char('q') => { state.popup = None; state.should_quit = true; }
            KeyCode::Up => {
                if !kits.is_empty() {
                    let cur = list_state.selected().unwrap_or(0);
                    list_state.select(Some(if cur == 0 { kits.len() - 1 } else { cur - 1 }));
                }
            }
            KeyCode::Down => {
                if !kits.is_empty() {
                    let cur = list_state.selected().unwrap_or(0);
                    list_state.select(Some(if cur >= kits.len() - 1 { 0 } else { cur + 1 }));
                }
            }
            KeyCode::Enter => {
                if kits.is_empty() { return; }
                if let Some(idx) = list_state.selected() {
                    let selected_path = kits[idx].path.clone();
                    let selected_name = kits[idx].name.clone();
                    // Try to load the kit
                    match kit::load_kit(&selected_path) {
                        Ok(new_kit) => {
                            if new_kit.sample_rate != resources.sample_rate {
                                state.set_status(format!(
                                    "Kit rejected: sample rate {} Hz != stream {} Hz",
                                    new_kit.sample_rate, resources.sample_rate
                                ));
                                state.popup = None;
                                return;
                            }
                            if new_kit.channels != resources.channels {
                                state.set_status(format!(
                                    "Kit rejected: {} ch != stream {} ch",
                                    new_kit.channels, resources.channels
                                ));
                                state.popup = None;
                                return;
                            }
                            // Swap notes
                            let note_keys = kit::note_keys(&new_kit.notes);
                            resources.shared_notes.store(Arc::new(new_kit.notes));
                            // Update watcher
                            let _ = resources.watcher.unwatch(&resources.kit_path);
                            let _ = resources.watcher.watch(selected_path.as_ref(), notify::RecursiveMode::NonRecursive);
                            resources.kit_path = selected_path;
                            // Update state
                            state.kit_name = selected_name;
                            state.rebuild_pads(&note_keys);
                            state.set_status("Kit loaded".to_string());
                        }
                        Err(e) => {
                            state.set_status(format!("Kit load error: {}", e));
                        }
                    }
                    state.popup = None;
                }
            }
            _ => {}
        },
        Popup::AudioPicker { devices, list_state } => match key {
            KeyCode::Char('a') | KeyCode::Esc => { state.popup = None; }
            KeyCode::Char('q') => { state.popup = None; state.should_quit = true; }
            KeyCode::Up => {
                if !devices.is_empty() {
                    let cur = list_state.selected().unwrap_or(0);
                    list_state.select(Some(if cur == 0 { devices.len() - 1 } else { cur - 1 }));
                }
            }
            KeyCode::Down => {
                if !devices.is_empty() {
                    let cur = list_state.selected().unwrap_or(0);
                    list_state.select(Some(if cur >= devices.len() - 1 { 0 } else { cur + 1 }));
                }
            }
            KeyCode::Enter => {
                if devices.is_empty() { return; }
                if let Some(idx) = list_state.selected() {
                    let new_device_index = devices[idx].index;
                    let new_device_name = devices[idx].name.clone();

                    // 1. Lock producer, take it (MIDI callback silently drops events)
                    {
                        let mut guard = resources.producer.lock().unwrap();
                        *guard = None;
                    }

                    // 2. Create new ring buffer
                    let (new_producer, new_consumer) = rtrb::RingBuffer::new(128);

                    // 3. Try to create new stream
                    match audio::run_output_stream(
                        Some(new_device_index),
                        new_consumer,
                        resources.sample_rate,
                        resources.channels,
                    ) {
                        Ok(new_stream) => {
                            // Drop old stream by replacing it
                            resources.stream = new_stream;
                            resources.audio_device_index = new_device_index;
                            // Put new producer into mutex
                            {
                                let mut guard = resources.producer.lock().unwrap();
                                *guard = Some(new_producer);
                            }
                            state.set_status(format!("Audio: {}", new_device_name));
                        }
                        Err(e) => {
                            // Try to restore old device
                            let (restore_producer, restore_consumer) = rtrb::RingBuffer::new(128);
                            match audio::run_output_stream(
                                Some(resources.audio_device_index),
                                restore_consumer,
                                resources.sample_rate,
                                resources.channels,
                            ) {
                                Ok(restored_stream) => {
                                    resources.stream = restored_stream;
                                    let mut guard = resources.producer.lock().unwrap();
                                    *guard = Some(restore_producer);
                                }
                                Err(_) => {
                                    // Can't restore — audio is dead
                                }
                            }
                            state.set_status(format!("Audio switch failed: {}", e));
                        }
                    }
                    state.popup = None;
                }
            }
            _ => {}
        },
        Popup::MidiPicker { devices, list_state } => match key {
            KeyCode::Char('m') | KeyCode::Esc => { state.popup = None; }
            KeyCode::Char('q') => { state.popup = None; state.should_quit = true; }
            KeyCode::Up => {
                if !devices.is_empty() {
                    let cur = list_state.selected().unwrap_or(0);
                    list_state.select(Some(if cur == 0 { devices.len() - 1 } else { cur - 1 }));
                }
            }
            KeyCode::Down => {
                if !devices.is_empty() {
                    let cur = list_state.selected().unwrap_or(0);
                    list_state.select(Some(if cur >= devices.len() - 1 { 0 } else { cur + 1 }));
                }
            }
            KeyCode::Enter => {
                if devices.is_empty() { return; }
                if let Some(idx) = list_state.selected() {
                    let new_port_index = devices[idx].port_index;
                    let new_port_name = devices[idx].name.clone();

                    // Build new callback with same shared producer and notes
                    let callback = crate::build_midi_callback(
                        Arc::clone(&resources.producer),
                        Arc::clone(&resources.shared_notes),
                        resources.tui_tx.clone(),
                        resources.choke_fade,
                        resources.aftertouch_fade,
                    );

                    match midi::connect_callback(new_port_index, callback) {
                        Ok(new_connection) => {
                            // Drop old connection by replacing
                            resources.connection = new_connection;
                            resources.midi_port_index = new_port_index;
                            state.midi_device = new_port_name.clone();
                            state.set_status(format!("MIDI: {}", new_port_name));
                        }
                        Err(e) => {
                            state.set_status(format!("MIDI switch failed: {}", e));
                        }
                    }
                    state.popup = None;
                }
            }
            _ => {}
        },
    }
}

fn ui(frame: &mut Frame, state: &AppState) {
    let area = frame.area();

    if area.height < 3 || area.width < 15 {
        let msg = Paragraph::new("Terminal too small\nq: quit")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(msg, area);
        return;
    }

    let header_height = if area.height < 8 { 1 } else { 3 };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, chunks[0], state);
    render_body(frame, chunks[1], state);
    render_footer(frame, chunks[2], state);

    // Popup overlay
    if let Some(ref popup) = state.popup {
        render_popup(frame, area, popup, state);
    }
}

fn render_header(frame: &mut Frame, area: Rect, state: &AppState) {
    if area.height == 0 {
        return;
    }

    if area.height == 1 {
        let line = Line::from(vec![
            Span::styled(
                " drumkit ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(&state.kit_name, Style::default().fg(Color::White)),
            Span::raw("  "),
            Span::styled(
                format!("Hits: {}", state.total_hits),
                Style::default().fg(Color::Yellow),
            ),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    let line1 = Line::from(vec![
        Span::styled(" Kit: ", Style::default().fg(Color::DarkGray)),
        Span::styled(&state.kit_name, Style::default().fg(Color::Cyan)),
        Span::raw("   "),
        Span::styled("MIDI: ", Style::default().fg(Color::DarkGray)),
        Span::styled(&state.midi_device, Style::default().fg(Color::Green)),
    ]);
    let line2 = Line::from(vec![
        Span::styled(
            format!(" {} Hz / {} ch", state.sample_rate, state.channels),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("    "),
        Span::styled("Hits: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            state.total_hits.to_string(),
            Style::default().fg(Color::Yellow),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" drumkit ")
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let paragraph = Paragraph::new(vec![line1, line2]).block(block);
    frame.render_widget(paragraph, area);
}

fn render_body(frame: &mut Frame, area: Rect, state: &AppState) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    if area.width < 50 {
        // Narrow: stack vertically
        let pad_rows = if area.height < 6 {
            0
        } else {
            (area.height / 2).max(3)
        };
        if pad_rows == 0 {
            render_hit_log(frame, area, state);
        } else {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(pad_rows), Constraint::Min(1)])
                .split(area);
            render_pad_grid(frame, chunks[0], state);
            render_hit_log(frame, chunks[1], state);
        }
    } else {
        // Wide: horizontal split with visible separator
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(area);
        render_pad_grid(frame, chunks[0], state);
        render_hit_log(frame, chunks[1], state);
    }
}

fn pad_flash(pad: &PadState, now: Instant) -> (f32, u8) {
    match pad.last_hit {
        Some(hit_time) => {
            let elapsed_ms = now.duration_since(hit_time).as_millis();
            if elapsed_ms < FLASH_DURATION_MS {
                let intensity = (1.0 - elapsed_ms as f64 / FLASH_DURATION_MS as f64) as f32;
                let vel_factor = pad.last_velocity as f32 / 127.0;
                (intensity * vel_factor, pad.last_velocity)
            } else {
                (0.0, 0)
            }
        }
        None => (0.0, 0),
    }
}

fn flash_styles(green: u8) -> (Color, Color, Color) {
    let border = if green > 0 {
        Color::Rgb(0, green, 0)
    } else {
        Color::Rgb(60, 60, 70)
    };
    let text = if green > 100 {
        Color::Black
    } else if green > 0 {
        Color::White
    } else {
        Color::Rgb(160, 160, 180)
    };
    let bg = if green > 30 {
        Color::Rgb(0, green / 3, 0)
    } else {
        Color::Reset
    };
    (border, text, bg)
}

fn render_pad_grid(frame: &mut Frame, area: Rect, state: &AppState) {
    if state.pads.is_empty() || area.width < 2 || area.height < 1 {
        return;
    }

    let now = Instant::now();

    // Very narrow: compact text list
    if area.width < 18 {
        render_pad_list_compact(frame, area, state, now);
        return;
    }

    // Each pad needs: border(2) + name(up to 14) = 16 min width
    // and border(2) + 2 content lines = 4 height
    let pad_height: u16 = 4;

    // Find the widest column count where each pad is >= 17 wide
    let min_pad_w: u16 = 17;
    let max_pad_w: u16 = 24;
    let cols = ((area.width / min_pad_w) as usize).max(1);
    let pad_width = (area.width / cols as u16).min(max_pad_w);

    // Grid dimensions
    let total_rows = (state.pads.len() + cols - 1) / cols;
    let grid_h = total_rows as u16 * pad_height;
    let grid_w = cols as u16 * pad_width;

    // Center the grid vertically and horizontally
    let offset_y = area.y + (area.height.saturating_sub(grid_h)) / 2;
    let offset_x = area.x + (area.width.saturating_sub(grid_w)) / 2;

    for (i, pad) in state.pads.iter().enumerate() {
        let col = i % cols;
        let row = i / cols;

        let x = offset_x + (col as u16) * pad_width;
        let y = offset_y + (row as u16) * pad_height;

        if x + pad_width > area.x + area.width || y + pad_height > area.y + area.height {
            continue;
        }

        let pad_area = Rect::new(x, y, pad_width, pad_height);
        let (flash, velocity) = pad_flash(pad, now);
        let green = (flash * 255.0) as u8;
        let (border_color, text_color, bg) = flash_styles(green);

        let inner_w = (pad_width as usize).saturating_sub(2);
        let name_display = if pad.name.len() > inner_w {
            &pad.name[..inner_w]
        } else {
            pad.name
        };

        let note_line = if velocity > 0 {
            format!("{:>3} v{}", pad.note, velocity)
        } else {
            format!("{:>3}", pad.note)
        };

        let pad_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color));

        let content = vec![
            Line::from(Span::styled(
                name_display.to_string(),
                Style::default()
                    .fg(text_color)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                note_line,
                Style::default().fg(if green > 0 {
                    text_color
                } else {
                    Color::DarkGray
                }).bg(bg),
            )),
        ];

        let pad_text = Paragraph::new(content).block(pad_block);
        frame.render_widget(pad_text, pad_area);
    }
}

/// Compact pad list for very narrow terminals — one pad per line, no borders.
fn render_pad_list_compact(frame: &mut Frame, area: Rect, state: &AppState, now: Instant) {
    let max_lines = area.height as usize;
    let w = area.width as usize;

    let mut lines = Vec::with_capacity(max_lines);
    for pad in state.pads.iter().take(max_lines) {
        let (flash, velocity) = pad_flash(pad, now);
        let green = (flash * 255.0) as u8;
        let (_, text_color, bg) = flash_styles(green);

        let label = if velocity > 0 && w >= 12 {
            format!("{:>3} {} v{}", pad.note, pad.name, velocity)
        } else {
            format!("{:>3} {}", pad.note, pad.name)
        };

        let display = if label.len() > w {
            &label[..w]
        } else {
            &label
        };

        lines.push(Line::from(Span::styled(
            display.to_string(),
            Style::default().fg(text_color).bg(bg),
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn render_hit_log(frame: &mut Frame, area: Rect, state: &AppState) {
    if area.width < 4 || area.height < 1 {
        return;
    }

    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Hit Log ")
        .title_style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .title_alignment(Alignment::Left);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 4 || inner.height < 1 {
        return;
    }

    if state.hit_log.is_empty() {
        let hint = Paragraph::new(Line::from(Span::styled(
            " Hit a pad...",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(hint, inner);
        return;
    }

    let w = inner.width as usize;
    let max_lines = inner.height as usize;

    // Layout tiers:
    //   wide (>=40): name(16) + " " + note(3) + " " + bar(var) + " " + vel(3) = 25 + bar
    //   medium (>=20): name(10) + " " + note(3) + " " + vel(3) = 18
    //   narrow (>=10): name(var) + " " + vel(3)
    let show_bar = w >= 28;
    let show_note = w >= 20;

    let fixed = if show_bar { 9 } else if show_note { 8 } else { 4 }; // spaces + note + vel
    let name_w = w.saturating_sub(fixed).min(16).max(3);

    let bar_max = if show_bar {
        w.saturating_sub(name_w + 9).min(12).max(3)
    } else {
        0
    };

    let mut lines = Vec::with_capacity(max_lines);
    for entry in state.hit_log.iter().take(max_lines) {
        let name = if entry.name.len() > name_w {
            &entry.name[..name_w]
        } else {
            entry.name
        };

        let mut spans = vec![
            Span::raw(" "),
            Span::styled(
                format!("{:<w$}", name, w = name_w),
                Style::default().fg(Color::Cyan),
            ),
        ];

        if show_note {
            spans.push(Span::styled(
                format!(" {:>3}", entry.note),
                Style::default().fg(Color::DarkGray),
            ));
        }

        if show_bar && bar_max > 0 {
            let filled = ((entry.velocity as usize) * bar_max + 63) / 127; // round up
            let empty = bar_max - filled;
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                "\u{2588}".repeat(filled),
                Style::default().fg(Color::Green),
            ));
            spans.push(Span::styled(
                "\u{2591}".repeat(empty),
                Style::default().fg(Color::Rgb(50, 50, 60)),
            ));
        }

        spans.push(Span::styled(
            format!(" {:>3}", entry.velocity),
            Style::default().fg(Color::White),
        ));

        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_footer(frame: &mut Frame, area: Rect, state: &AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let w = area.width as usize;

    let (status_text, status_style) = match &state.status_message {
        Some((msg, time)) if time.elapsed() < Duration::from_secs(3) => (
            format!(" \u{2713} {}", msg),
            Style::default().fg(Color::Green),
        ),
        _ => (String::new(), Style::default()),
    };

    let hints = "l log  k kit  a audio  m midi  q quit ";
    let hint_style = Style::default().fg(Color::DarkGray);

    if w < hints.len() + 2 {
        // Narrow: just show minimal hints
        let line = Line::from(Span::styled("q quit ", hint_style));
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    let available_for_status = w.saturating_sub(hints.len());
    // Truncate status to available space so it never overflows into hints
    let truncated: String = status_text.chars().take(available_for_status).collect();
    let padded_status = format!("{:<width$}", truncated, width = available_for_status);

    let line = Line::from(vec![
        Span::styled(padded_status, status_style),
        Span::styled(hints, hint_style),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

fn popup_area(area: Rect) -> Rect {
    let popup_w = (area.width * 4 / 5).max(30).min(area.width);
    let popup_h = (area.height * 4 / 5).max(6).min(area.height);
    let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    Rect::new(x, y, popup_w, popup_h)
}

fn render_popup(frame: &mut Frame, area: Rect, popup: &Popup, state: &AppState) {
    match popup {
        Popup::Log { scroll } => render_log_popup(frame, area, state, *scroll),
        Popup::KitPicker { kits, list_state } => render_kit_popup(frame, area, kits, list_state),
        Popup::AudioPicker { devices, list_state } => render_audio_popup(frame, area, devices, list_state),
        Popup::MidiPicker { devices, list_state } => render_midi_popup(frame, area, devices, list_state),
    }
}

fn render_log_popup(frame: &mut Frame, area: Rect, state: &AppState, scroll: usize) {
    let popup = popup_area(area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow))
        .title(format!(" Log ({} lines) ", state.log_lines.len()))
        .title_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let content_area = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1));
    let footer_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);

    if state.log_lines.is_empty() {
        let msg = Paragraph::new(Line::from(Span::styled(
            " No log messages.",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(msg, content_area);
    } else {
        let visible = content_area.height as usize;
        let start = scroll.min(state.log_lines.len().saturating_sub(visible));

        let lines: Vec<Line> = state.log_lines[start..]
            .iter()
            .take(visible)
            .map(|l| Line::from(Span::styled(format!(" {}", l), Style::default().fg(Color::DarkGray))))
            .collect();

        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, content_area);
    }

    let footer = Paragraph::new(Line::from(Span::styled(
        " \u{2191}\u{2193} scroll  Esc/l close  q quit",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(footer, footer_area);
}

fn render_kit_popup(frame: &mut Frame, area: Rect, kits: &[setup::DiscoveredKit], list_state: &ListState) {
    let popup = popup_area(area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Select Kit ")
        .title_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let content_area = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1));
    let footer_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);

    if kits.is_empty() {
        let msg = Paragraph::new(Line::from(Span::styled(
            " No kits found.",
            Style::default().fg(Color::Yellow),
        )));
        frame.render_widget(msg, content_area);
    } else {
        let items: Vec<ListItem> = kits
            .iter()
            .map(|kit| {
                ListItem::new(Line::from(vec![
                    Span::raw(" "),
                    Span::raw(&kit.name),
                    Span::styled(
                        format!("  ({} samples)", kit.wav_count),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]))
            })
            .collect();

        let list = List::new(items)
            .highlight_symbol("\u{25b8} ")
            .highlight_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

        let mut ls = list_state.clone();
        frame.render_stateful_widget(list, content_area, &mut ls);
    }

    let footer = Paragraph::new(Line::from(Span::styled(
        " \u{2191}\u{2193} navigate  Enter select  Esc/k close  q quit",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(footer, footer_area);
}

fn render_audio_popup(frame: &mut Frame, area: Rect, devices: &[audio::AudioDevice], list_state: &ListState) {
    let popup = popup_area(area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Green))
        .title(" Select Audio Device ")
        .title_style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let content_area = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1));
    let footer_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);

    if devices.is_empty() {
        let msg = Paragraph::new(Line::from(Span::styled(
            " No audio devices found.",
            Style::default().fg(Color::Yellow),
        )));
        frame.render_widget(msg, content_area);
    } else {
        let items: Vec<ListItem> = devices
            .iter()
            .map(|dev| ListItem::new(Line::from(vec![Span::raw(" "), Span::raw(&dev.name)])))
            .collect();

        let list = List::new(items)
            .highlight_symbol("\u{25b8} ")
            .highlight_style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD));

        let mut ls = list_state.clone();
        frame.render_stateful_widget(list, content_area, &mut ls);
    }

    let footer = Paragraph::new(Line::from(Span::styled(
        " \u{2191}\u{2193} navigate  Enter select  Esc/a close  q quit",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(footer, footer_area);
}

fn render_midi_popup(frame: &mut Frame, area: Rect, devices: &[midi::MidiDevice], list_state: &ListState) {
    let popup = popup_area(area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Magenta))
        .title(" Select MIDI Input ")
        .title_style(Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let content_area = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1));
    let footer_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);

    if devices.is_empty() {
        let msg = Paragraph::new(Line::from(Span::styled(
            " No MIDI devices found.",
            Style::default().fg(Color::Yellow),
        )));
        frame.render_widget(msg, content_area);
    } else {
        let items: Vec<ListItem> = devices
            .iter()
            .map(|dev| ListItem::new(Line::from(vec![Span::raw(" "), Span::raw(&dev.name)])))
            .collect();

        let list = List::new(items)
            .highlight_symbol("\u{25b8} ")
            .highlight_style(Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD));

        let mut ls = list_state.clone();
        frame.render_stateful_widget(list, content_area, &mut ls);
    }

    let footer = Paragraph::new(Line::from(Span::styled(
        " \u{2191}\u{2193} navigate  Enter select  Esc/m close  q quit",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(footer, footer_area);
}
