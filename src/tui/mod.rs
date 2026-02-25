mod event_loop;
pub(crate) mod input;
pub(crate) mod list_nav;
mod popups;
mod render;
mod render_popups;
pub(crate) mod widgets;

use anyhow::Result;
use arc_swap::ArcSwap;
use crossterm::{
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{widgets::ListState, Terminal};
use std::collections::HashMap;
use std::io::stdout;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

use crate::{audio, download, kit, mapping, midi, stderr};

/// Events fed into the TUI from various sources.
pub enum TuiEvent {
    Hit { note: u8, velocity: u8 },
    Choke { note: u8 },
    KitReloaded { note_keys: Vec<u8>, kit_path: PathBuf },
    KitReloadError(String),
    KitLoadComplete {
        result: Result<kit::Kit, String>,
        path: PathBuf,
        name: String,
    },
    MappingReloaded(mapping::NoteMapping, PathBuf),
    KitStoreFetched {
        result: Result<Vec<download::RemoteKit>, String>,
    },
    KitDownloadComplete {
        result: Result<std::path::PathBuf, String>,
        kit_name: String,
    },
}

/// Mode for the library directory popup.
pub enum DirPopupMode {
    Browse,
    AddKit,
    AddMapping,
}

/// Popup overlays in play mode.
pub enum Popup {
    Log { scroll: usize },
    KitPicker { kits: Vec<kit::DiscoveredKit>, list_state: ListState },
    AudioPicker { devices: Vec<audio::AudioDevice>, list_state: ListState },
    MidiPicker { devices: Vec<midi::MidiDevice>, list_state: ListState },
    LibraryDir {
        mode: DirPopupMode,
        selected: usize,
        input: String,
        cursor: usize,
        error: Option<String>,
    },
    Loading {
        kit_name: String,
        progress: Arc<AtomicUsize>,
        total: Arc<AtomicUsize>,
    },
    MappingPicker { mappings: Vec<mapping::NoteMapping>, list_state: ListState },
    DeleteMapping { name: String, path: PathBuf, },
    NoteRename { note: u8, input: String, cursor: usize },
    KitStoreFetching,
    KitStore { kits: Vec<download::RemoteKit>, rows: Vec<download::StoreRow>, list_state: ListState },
    KitDownloading {
        kit_name: String,
        progress: Arc<AtomicUsize>,
        total: Arc<AtomicUsize>,
    },
    KitStoreRepos {
        selected: usize,
        adding: bool,
        input: String,
        cursor: usize,
        error: Option<String>,
        confirm_delete: bool,
    },
}

/// Swappable resources owned by the TUI event loop during play mode.
pub struct PlayResources {
    pub stream: cpal::Stream,
    pub connection: midir::MidiInputConnection<()>,
    pub producer: Arc<Mutex<Option<rtrb::Producer<audio::AudioCommand>>>>,
    pub shared_notes: Arc<ArcSwap<HashMap<u8, Arc<kit::NoteGroup>>>>,
    pub kit_path: PathBuf,
    pub shared_kit_path: Arc<ArcSwap<PathBuf>>,
    pub suppress_reload: Arc<AtomicBool>,
    pub sample_rate: u32,
    pub channels: u16,
    pub audio_device_index: usize,
    pub midi_port_index: usize,
    pub tui_tx: mpsc::Sender<TuiEvent>,
    pub choke_fade: usize,
    pub aftertouch_fade: usize,
    pub watcher: notify::RecommendedWatcher,
    pub stderr_capture: Option<stderr::StderrCapture>,
    pub extra_kits_dirs: Vec<PathBuf>,
    pub extra_mapping_dirs: Vec<PathBuf>,
    pub shared_mapping: Arc<ArcSwap<mapping::NoteMapping>>,
    pub kit_repos: Vec<String>,
}

/// Visual state for a single pad in the grid.
pub(crate) struct PadState {
    pub(crate) note: u8,
    pub(crate) name: String,
    pub(crate) last_hit: Option<Instant>,
    pub(crate) last_velocity: u8,
}

/// A single entry in the scrolling hit log.
pub(crate) struct HitLogEntry {
    pub(crate) name: String,
    pub(crate) note: u8,
    pub(crate) velocity: u8,
}

pub(crate) const MAX_HIT_LOG: usize = 50;
pub(crate) const FLASH_DURATION_MS: u128 = 300;

/// Complete application state for the TUI.
pub struct AppState {
    pub kit_name: String,
    pub midi_device: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub mapping: Arc<mapping::NoteMapping>,
    pub(crate) pads: Vec<PadState>,
    pub(crate) pad_index: HashMap<u8, usize>,
    pub(crate) hit_log: Vec<HitLogEntry>,
    pub(crate) total_hits: u64,
    pub(crate) status_message: Option<(String, Instant)>,
    pub(crate) should_quit: bool,
    pub(crate) popup: Option<Popup>,
    pub(crate) log_lines: Vec<String>,
}

impl AppState {
    pub fn new(
        kit_name: String,
        midi_device: String,
        sample_rate: u32,
        channels: u16,
        note_keys: &[u8],
        initial_log: Vec<String>,
        note_mapping: Arc<mapping::NoteMapping>,
    ) -> Self {
        let mut pads = Vec::with_capacity(note_keys.len());
        let mut pad_index = HashMap::new();
        for (i, &note) in note_keys.iter().enumerate() {
            pads.push(PadState {
                note,
                name: note_mapping.drum_name(note).to_owned(),
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
            mapping: note_mapping,
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

    pub(crate) fn on_hit(&mut self, note: u8, velocity: u8) {
        self.total_hits += 1;
        if let Some(&idx) = self.pad_index.get(&note) {
            self.pads[idx].last_hit = Some(Instant::now());
            self.pads[idx].last_velocity = velocity;
        }
        self.hit_log.insert(
            0,
            HitLogEntry {
                name: self.mapping.drum_name(note).to_owned(),
                note,
                velocity,
            },
        );
        if self.hit_log.len() > MAX_HIT_LOG {
            self.hit_log.truncate(MAX_HIT_LOG);
        }
    }

    pub(crate) fn on_choke(&mut self, note: u8) {
        if let Some(&idx) = self.pad_index.get(&note) {
            self.pads[idx].last_hit = Some(Instant::now());
        }
    }

    pub(crate) fn rebuild_pads(&mut self, note_keys: &[u8]) {
        self.pads.clear();
        self.pad_index.clear();
        for (i, &note) in note_keys.iter().enumerate() {
            self.pads.push(PadState {
                note,
                name: self.mapping.drum_name(note).to_owned(),
                last_hit: None,
                last_velocity: 0,
            });
            self.pad_index.insert(note, i);
        }
    }

    pub(crate) fn update_hit_log_names(&mut self) {
        for entry in &mut self.hit_log {
            entry.name = self.mapping.drum_name(entry.note).to_owned();
        }
        for pad in &mut self.pads {
            pad.name = self.mapping.drum_name(pad.note).to_owned();
        }
    }

    pub(crate) fn set_status(&mut self, msg: String) {
        self.status_message = Some((msg, Instant::now()));
    }
}

/// The terminal type used by the TUI.
pub type Term = Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>;

/// Enter alternate screen and return the terminal. Call `restore_terminal` when done.
pub fn init_terminal() -> Result<Term> {
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
    Ok(terminal)
}

/// Restore the terminal to normal mode.
pub fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = stdout().execute(LeaveAlternateScreen);
}

/// Run the event loop on an already-initialized terminal. Restores terminal on exit.
pub fn run(
    mut terminal: Term,
    event_rx: mpsc::Receiver<TuiEvent>,
    mut state: AppState,
    mut resources: PlayResources,
) -> Result<()> {
    let result = event_loop::event_loop(&mut terminal, &event_rx, &mut state, &mut resources);

    restore_terminal();

    // Restore stderr
    if let Some(cap) = resources.stderr_capture.take() {
        cap.restore();
    }

    result
}
