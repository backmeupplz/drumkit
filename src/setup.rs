use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use std::io::{stdout, BufRead, BufReader};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use crate::audio;
use crate::midi;

/// A kit directory discovered on disk.
pub struct DiscoveredKit {
    pub name: String,
    pub path: PathBuf,
    pub wav_count: usize,
}

/// Which step of the setup flow we're on.
#[derive(Clone, Copy, PartialEq)]
enum SetupStep {
    Kit,
    AudioDevice,
    MidiPort,
}

/// Result of the setup flow.
pub enum SetupResult {
    Selected {
        kit_path: PathBuf,
        audio_device: usize,
        midi_port: usize,
    },
    Cancelled,
}

/// Internal state for the setup TUI.
struct SetupState {
    step: SetupStep,
    kits: Vec<DiscoveredKit>,
    audio_devices: Vec<audio::AudioDevice>,
    midi_devices: Vec<midi::MidiDevice>,
    kit_list_state: ListState,
    audio_list_state: ListState,
    midi_list_state: ListState,
    selected_kit: Option<(String, PathBuf)>,
    selected_audio: Option<(String, usize)>,
    error_message: Option<String>,
    should_quit: bool,
    done: bool,
    log_lines: Vec<String>,
    show_log: bool,
    log_scroll: usize,
}

/// Redirect stderr (fd 2) to a pipe. Returns the receiver for captured lines
/// and the saved fd to restore later.
pub struct StderrCapture {
    saved_fd: i32,
    rx: mpsc::Receiver<String>,
}

impl StderrCapture {
    pub fn start() -> Option<Self> {
        let mut fds = [0i32; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return None;
        }
        let read_fd = fds[0];
        let write_fd = fds[1];

        let stderr_fd = std::io::stderr().as_raw_fd();
        let saved_fd = unsafe { libc::dup(stderr_fd) };
        if saved_fd < 0 {
            unsafe {
                libc::close(read_fd);
                libc::close(write_fd);
            }
            return None;
        }

        // Point stderr at the write end of the pipe
        unsafe { libc::dup2(write_fd, stderr_fd) };
        unsafe { libc::close(write_fd) };

        let (tx, rx) = mpsc::channel();

        // Background thread reads lines from the pipe
        std::thread::spawn(move || {
            let file = unsafe { std::fs::File::from_raw_fd(read_fd) };
            let reader = BufReader::new(file);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Some(Self { saved_fd, rx })
    }

    /// Drain any new lines into the provided vec.
    pub fn drain_into(&self, log: &mut Vec<String>) {
        while let Ok(line) = self.rx.try_recv() {
            log.push(line);
        }
    }

    /// Restore the original stderr fd.
    pub fn restore(self) {
        let stderr_fd = std::io::stderr().as_raw_fd();
        unsafe {
            libc::dup2(self.saved_fd, stderr_fd);
            libc::close(self.saved_fd);
        }
    }
}

/// Return the built-in search directories (for display purposes).
pub fn default_search_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("./kits")];
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(xdg).join("drumkit/kits"));
    } else if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/drumkit/kits"));
    }
    dirs
}

/// Scan standard locations for kit directories containing .wav files.
pub fn discover_kits(extra_dirs: &[PathBuf]) -> Vec<DiscoveredKit> {
    let mut kits = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();

    let mut search_dirs = vec![PathBuf::from("./kits")];

    // XDG_DATA_HOME or ~/.local/share
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        search_dirs.push(PathBuf::from(xdg).join("drumkit/kits"));
    } else if let Ok(home) = std::env::var("HOME") {
        search_dirs.push(PathBuf::from(home).join(".local/share/drumkit/kits"));
    }

    // Append user-supplied extra directories
    search_dirs.extend_from_slice(extra_dirs);

    for search_dir in &search_dirs {
        let entries = match std::fs::read_dir(search_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let canonical = match path.canonicalize() {
                Ok(c) => c,
                Err(_) => continue,
            };

            if !seen_paths.insert(canonical) {
                continue;
            }

            let wav_count = std::fs::read_dir(&path)
                .map(|rd| {
                    rd.filter_map(|e| e.ok())
                        .filter(|e| {
                            e.path()
                                .extension()
                                .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
                        })
                        .count()
                })
                .unwrap_or(0);

            if wav_count == 0 {
                continue;
            }

            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "unnamed".to_string());

            kits.push(DiscoveredKit {
                name,
                path,
                wav_count,
            });
        }
    }

    kits.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    kits
}

/// Run the interactive setup TUI.
///
/// Presets skip corresponding steps. If all three are provided, returns immediately.
pub fn run_setup(
    preset_kit: Option<PathBuf>,
    preset_audio: Option<usize>,
    preset_midi: Option<usize>,
    extra_kits_dirs: &[PathBuf],
) -> Result<SetupResult> {
    // If everything is preset, skip the TUI entirely
    if let (Some(kit_path), Some(audio_device), Some(midi_port)) =
        (&preset_kit, preset_audio, preset_midi)
    {
        return Ok(SetupResult::Selected {
            kit_path: kit_path.clone(),
            audio_device,
            midi_port,
        });
    }

    // Start capturing stderr before device enumeration
    let capture = StderrCapture::start();

    let kits = discover_kits(extra_kits_dirs);
    let audio_devices = audio::list_output_devices()?;
    let midi_devices = midi::list_devices()?;

    // Determine the starting step
    let first_step = if preset_kit.is_none() {
        SetupStep::Kit
    } else if preset_audio.is_none() {
        SetupStep::AudioDevice
    } else {
        SetupStep::MidiPort
    };

    let mut kit_list_state = ListState::default();
    if !kits.is_empty() {
        kit_list_state.select(Some(0));
    }
    let mut audio_list_state = ListState::default();
    if !audio_devices.is_empty() {
        audio_list_state.select(Some(0));
    }
    let mut midi_list_state = ListState::default();
    if !midi_devices.is_empty() {
        midi_list_state.select(Some(0));
    }

    let selected_kit = preset_kit.map(|p| {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "kit".to_string());
        (name, p)
    });

    let selected_audio = preset_audio.map(|idx| {
        let name = audio_devices
            .iter()
            .find(|d| d.index == idx)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| format!("Device {}", idx));
        (name, idx)
    });

    let mut log_lines = Vec::new();
    if let Some(ref cap) = capture {
        cap.drain_into(&mut log_lines);
    }

    let mut state = SetupState {
        step: first_step,
        kits,
        audio_devices,
        midi_devices,
        kit_list_state,
        audio_list_state,
        midi_list_state,
        selected_kit,
        selected_audio,
        error_message: None,
        should_quit: false,
        done: false,
        log_lines,
        show_log: false,
        log_scroll: 0,
    };

    // Set up terminal
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

    let result = setup_event_loop(&mut terminal, &mut state, &capture, preset_audio, preset_midi);

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    // Restore stderr before returning
    if let Some(cap) = capture {
        cap.restore();
    }

    result
}

fn setup_event_loop(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    state: &mut SetupState,
    capture: &Option<StderrCapture>,
    preset_audio: Option<usize>,
    preset_midi: Option<usize>,
) -> Result<SetupResult> {
    loop {
        // Drain captured stderr lines
        if let Some(cap) = capture {
            cap.drain_into(&mut state.log_lines);
        }

        terminal.draw(|frame| setup_ui(frame, state))?;

        if state.should_quit {
            return Ok(SetupResult::Cancelled);
        }

        if state.done {
            let kit_path = state.selected_kit.as_ref().unwrap().1.clone();
            let audio_device = state.selected_audio.as_ref().unwrap().1;
            let midi_port = state
                .midi_list_state
                .selected()
                .expect("MIDI port must be selected");
            return Ok(SetupResult::Selected {
                kit_path,
                audio_device,
                midi_port,
            });
        }

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                // Log popup handles its own keys
                if state.show_log {
                    match key.code {
                        KeyCode::Char('l') | KeyCode::Esc => {
                            state.show_log = false;
                        }
                        KeyCode::Char('q') => {
                            state.show_log = false;
                            state.should_quit = true;
                        }
                        KeyCode::Up => {
                            state.log_scroll = state.log_scroll.saturating_sub(1);
                        }
                        KeyCode::Down => {
                            if state.log_scroll + 1 < state.log_lines.len() {
                                state.log_scroll += 1;
                            }
                        }
                        KeyCode::Home => {
                            state.log_scroll = 0;
                        }
                        KeyCode::End => {
                            state.log_scroll = state.log_lines.len().saturating_sub(1);
                        }
                        _ => {}
                    }
                    continue;
                }

                state.error_message = None;

                match key.code {
                    KeyCode::Char('q') => {
                        state.should_quit = true;
                    }
                    KeyCode::Char('l') => {
                        state.show_log = true;
                        // Jump to end of log
                        state.log_scroll = state.log_lines.len().saturating_sub(1);
                    }
                    KeyCode::Esc => {
                        handle_back(state);
                    }
                    KeyCode::Up => {
                        move_selection(state, -1);
                    }
                    KeyCode::Down => {
                        move_selection(state, 1);
                    }
                    KeyCode::Enter => {
                        handle_enter(state, preset_audio, preset_midi);
                    }
                    _ => {}
                }
            }
        }
    }
}

fn handle_back(state: &mut SetupState) {
    match state.step {
        SetupStep::Kit => {
            state.should_quit = true;
        }
        SetupStep::AudioDevice => {
            // Can only go back to Kit step if kits were discovered (not preset)
            if !state.kits.is_empty() {
                state.step = SetupStep::Kit;
                state.selected_kit = None;
            } else {
                state.should_quit = true;
            }
        }
        SetupStep::MidiPort => {
            state.step = SetupStep::AudioDevice;
            state.selected_audio = None;
            // Rescan audio devices
            if let Ok(devices) = audio::list_output_devices() {
                state.audio_devices = devices;
                if !state.audio_devices.is_empty() {
                    state.audio_list_state.select(Some(0));
                } else {
                    state.audio_list_state.select(None);
                }
            }
        }
    }
}

fn handle_enter(state: &mut SetupState, preset_audio: Option<usize>, preset_midi: Option<usize>) {
    match state.step {
        SetupStep::Kit => {
            if state.kits.is_empty() {
                return;
            }
            if let Some(idx) = state.kit_list_state.selected() {
                let kit = &state.kits[idx];
                state.selected_kit = Some((kit.name.clone(), kit.path.clone()));

                if let Some(audio_idx) = preset_audio {
                    // Audio is preset, skip to MIDI
                    let name = state
                        .audio_devices
                        .iter()
                        .find(|d| d.index == audio_idx)
                        .map(|d| d.name.clone())
                        .unwrap_or_else(|| format!("Device {}", audio_idx));
                    state.selected_audio = Some((name, audio_idx));

                    if preset_midi.is_some() {
                        state.done = true;
                    } else {
                        // Rescan MIDI devices
                        if let Ok(devices) = midi::list_devices() {
                            state.midi_devices = devices;
                            if !state.midi_devices.is_empty() {
                                state.midi_list_state.select(Some(0));
                            } else {
                                state.midi_list_state.select(None);
                            }
                        }
                        state.step = SetupStep::MidiPort;
                    }
                } else {
                    // Rescan audio devices at step transition
                    if let Ok(devices) = audio::list_output_devices() {
                        state.audio_devices = devices;
                        if !state.audio_devices.is_empty() {
                            state.audio_list_state.select(Some(0));
                        } else {
                            state.audio_list_state.select(None);
                        }
                    }
                    state.step = SetupStep::AudioDevice;
                }
            }
        }
        SetupStep::AudioDevice => {
            if state.audio_devices.is_empty() {
                return;
            }
            if let Some(idx) = state.audio_list_state.selected() {
                let dev = &state.audio_devices[idx];
                state.selected_audio = Some((dev.name.clone(), dev.index));

                if preset_midi.is_some() {
                    state.done = true;
                } else {
                    // Rescan MIDI devices at step transition
                    if let Ok(devices) = midi::list_devices() {
                        state.midi_devices = devices;
                        if !state.midi_devices.is_empty() {
                            state.midi_list_state.select(Some(0));
                        } else {
                            state.midi_list_state.select(None);
                        }
                    }
                    state.step = SetupStep::MidiPort;
                }
            }
        }
        SetupStep::MidiPort => {
            if state.midi_devices.is_empty() {
                return;
            }
            if state.midi_list_state.selected().is_some() {
                state.done = true;
            }
        }
    }
}

fn move_selection(state: &mut SetupState, delta: i32) {
    let (list_state, len) = match state.step {
        SetupStep::Kit => (&mut state.kit_list_state, state.kits.len()),
        SetupStep::AudioDevice => (&mut state.audio_list_state, state.audio_devices.len()),
        SetupStep::MidiPort => (&mut state.midi_list_state, state.midi_devices.len()),
    };

    if len == 0 {
        return;
    }

    let current = list_state.selected().unwrap_or(0);
    let next = if delta < 0 {
        if current == 0 {
            len - 1
        } else {
            current - 1
        }
    } else {
        if current >= len - 1 {
            0
        } else {
            current + 1
        }
    };
    list_state.select(Some(next));
}

fn setup_ui(frame: &mut Frame, state: &SetupState) {
    let area = frame.area();

    if area.width < 30 || area.height < 8 {
        let msg = Paragraph::new("Terminal too small\nNeed at least 30x8\nq: quit")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(msg, area);
        return;
    }

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" drumkit setup ")
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // breadcrumb
            Constraint::Length(1), // title
            Constraint::Min(1),   // list
            Constraint::Length(1), // footer
        ])
        .split(inner);

    render_breadcrumb(frame, chunks[0], state);
    render_step_title(frame, chunks[1], state);
    render_list(frame, chunks[2], state);
    render_setup_footer(frame, chunks[3], state);

    // Log popup overlay
    if state.show_log {
        render_log_popup(frame, area, state);
    }
}

fn render_breadcrumb(frame: &mut Frame, area: Rect, state: &SetupState) {
    if area.width < 10 || area.height == 0 {
        return;
    }

    let mut spans = Vec::new();
    spans.push(Span::raw("  "));

    // Kit step
    let kit_style = match state.step {
        SetupStep::Kit => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        _ => Style::default().fg(Color::Green),
    };
    match &state.selected_kit {
        Some((name, _)) if state.step != SetupStep::Kit => {
            spans.push(Span::styled(format!("Kit: {}", name), kit_style));
        }
        _ => {
            spans.push(Span::styled("Kit", kit_style));
        }
    }

    spans.push(Span::styled(" \u{25b8} ", Style::default().fg(Color::DarkGray)));

    // Audio step
    let audio_style = match state.step {
        SetupStep::AudioDevice => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        SetupStep::MidiPort => Style::default().fg(Color::Green),
        _ => Style::default().fg(Color::DarkGray),
    };
    match &state.selected_audio {
        Some((name, _)) if state.step == SetupStep::MidiPort => {
            let display = if name.len() > 20 {
                format!("Audio: {}...", &name[..17])
            } else {
                format!("Audio: {}", name)
            };
            spans.push(Span::styled(display, audio_style));
        }
        _ => {
            spans.push(Span::styled("Audio Device", audio_style));
        }
    }

    spans.push(Span::styled(" \u{25b8} ", Style::default().fg(Color::DarkGray)));

    // MIDI step
    let midi_style = match state.step {
        SetupStep::MidiPort => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        _ => Style::default().fg(Color::DarkGray),
    };
    spans.push(Span::styled("MIDI", midi_style));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_step_title(frame: &mut Frame, area: Rect, state: &SetupState) {
    if area.height == 0 {
        return;
    }

    let title = match state.step {
        SetupStep::Kit => "  Select Kit",
        SetupStep::AudioDevice => "  Select Audio Device",
        SetupStep::MidiPort => "  Select MIDI Input",
    };

    let paragraph = Paragraph::new(Line::from(Span::styled(
        title,
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    frame.render_widget(paragraph, area);
}

fn render_list(frame: &mut Frame, area: Rect, state: &SetupState) {
    if area.height == 0 || area.width < 5 {
        return;
    }

    // Add padding
    let padded = Rect::new(
        area.x + 2,
        area.y,
        area.width.saturating_sub(4),
        area.height,
    );

    match state.step {
        SetupStep::Kit => render_kit_list(frame, padded, state),
        SetupStep::AudioDevice => render_audio_list(frame, padded, state),
        SetupStep::MidiPort => render_midi_list(frame, padded, state),
    }

    // Show error if any
    if let Some(ref msg) = state.error_message {
        let err_area = Rect::new(
            area.x + 2,
            area.y + area.height.saturating_sub(1),
            area.width.saturating_sub(4),
            1,
        );
        let err = Paragraph::new(Line::from(Span::styled(
            msg.as_str(),
            Style::default().fg(Color::Red),
        )));
        frame.render_widget(err, err_area);
    }
}

fn render_kit_list(frame: &mut Frame, area: Rect, state: &SetupState) {
    if state.kits.is_empty() {
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No kits found.",
                Style::default().fg(Color::Yellow),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Place WAV files in a subdirectory of:",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  ./kits/<kit-name>/",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  ~/.local/share/drumkit/kits/<kit-name>/",
                Style::default().fg(Color::DarkGray),
            )),
        ]);
        frame.render_widget(msg, area);
        return;
    }

    let items: Vec<ListItem> = state
        .kits
        .iter()
        .map(|kit| {
            ListItem::new(Line::from(vec![
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
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let mut list_state = state.kit_list_state.clone();
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_audio_list(frame: &mut Frame, area: Rect, state: &SetupState) {
    if state.audio_devices.is_empty() {
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No audio output devices found.",
                Style::default().fg(Color::Yellow),
            )),
        ]);
        frame.render_widget(msg, area);
        return;
    }

    let items: Vec<ListItem> = state
        .audio_devices
        .iter()
        .map(|dev| ListItem::new(Line::from(Span::raw(&dev.name))))
        .collect();

    let list = List::new(items)
        .highlight_symbol("\u{25b8} ")
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let mut list_state = state.audio_list_state.clone();
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_midi_list(frame: &mut Frame, area: Rect, state: &SetupState) {
    if state.midi_devices.is_empty() {
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No MIDI input devices found.",
                Style::default().fg(Color::Yellow),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Connect your drum module via USB and press Enter to rescan.",
                Style::default().fg(Color::DarkGray),
            )),
        ]);
        frame.render_widget(msg, area);
        return;
    }

    let items: Vec<ListItem> = state
        .midi_devices
        .iter()
        .map(|dev| ListItem::new(Line::from(Span::raw(&dev.name))))
        .collect();

    let list = List::new(items)
        .highlight_symbol("\u{25b8} ")
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let mut list_state = state.midi_list_state.clone();
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_log_popup(frame: &mut Frame, area: Rect, state: &SetupState) {
    // Center the popup, taking ~80% of the screen
    let popup_w = (area.width * 4 / 5).max(30).min(area.width);
    let popup_h = (area.height * 4 / 5).max(6).min(area.height);
    let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(x, y, popup_w, popup_h);

    // Clear the area behind the popup
    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow))
        .title(format!(" Log ({} lines) ", state.log_lines.len()))
        .title_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    // Reserve last line for footer
    let content_area = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1));
    let footer_area = Rect::new(
        inner.x,
        inner.y + inner.height.saturating_sub(1),
        inner.width,
        1,
    );

    if state.log_lines.is_empty() {
        let msg = Paragraph::new(Line::from(Span::styled(
            " No log messages.",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(msg, content_area);
    } else {
        let visible = content_area.height as usize;
        let start = state.log_scroll.min(state.log_lines.len().saturating_sub(visible));

        let lines: Vec<Line> = state.log_lines[start..]
            .iter()
            .take(visible)
            .map(|l| {
                Line::from(Span::styled(
                    format!(" {}", l),
                    Style::default().fg(Color::DarkGray),
                ))
            })
            .collect();

        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, content_area);
    }

    let footer = Paragraph::new(Line::from(vec![
        Span::styled(
            " \u{2191}\u{2193} scroll  Esc/l close  q quit",
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    frame.render_widget(footer, footer_area);
}

fn render_setup_footer(frame: &mut Frame, area: Rect, state: &SetupState) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let can_go_back = match state.step {
        SetupStep::Kit => false,
        SetupStep::AudioDevice => !state.kits.is_empty(),
        SetupStep::MidiPort => true,
    };

    let has_items = match state.step {
        SetupStep::Kit => !state.kits.is_empty(),
        SetupStep::AudioDevice => !state.audio_devices.is_empty(),
        SetupStep::MidiPort => !state.midi_devices.is_empty(),
    };

    let mut hints = vec![Span::styled("  ", Style::default())];

    if has_items {
        hints.push(Span::styled(
            "\u{2191}\u{2193} navigate",
            Style::default().fg(Color::DarkGray),
        ));
        hints.push(Span::raw("  "));
        hints.push(Span::styled(
            "Enter select",
            Style::default().fg(Color::DarkGray),
        ));
    }

    if can_go_back {
        hints.push(Span::raw("  "));
        hints.push(Span::styled(
            "Esc back",
            Style::default().fg(Color::DarkGray),
        ));
    }

    hints.push(Span::raw("  "));
    hints.push(Span::styled(
        "l log",
        Style::default().fg(Color::DarkGray),
    ));
    hints.push(Span::raw("  "));
    hints.push(Span::styled(
        "q quit",
        Style::default().fg(Color::DarkGray),
    ));

    frame.render_widget(Paragraph::new(Line::from(hints)), area);
}
