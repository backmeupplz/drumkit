use ratatui::widgets::ListState;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use crate::audio;
use crate::download;
use crate::kit;
use crate::midi;
use crate::settings;

/// Which step of the setup flow we're on.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum SetupStep {
    Kit,
    AudioDevice,
    MidiPort,
}

/// Store popup overlay states for the setup flow.
pub(super) enum SetupStorePopup {
    Fetching,
    Browse { kits: Vec<download::RemoteKit>, list_state: ListState },
    Downloading {
        kit_name: String,
        progress: Arc<AtomicUsize>,
        total: Arc<AtomicUsize>,
    },
}

/// Events from background threads in the setup flow.
pub(super) enum SetupBgEvent {
    StoreFetched(Result<Vec<download::RemoteKit>, String>),
    StoreDownloaded { result: Result<PathBuf, String>, kit_name: String },
}

/// Internal state for the setup TUI.
pub(super) struct SetupState {
    pub step: SetupStep,
    pub kits: Vec<kit::DiscoveredKit>,
    pub audio_devices: Vec<audio::AudioDevice>,
    pub midi_devices: Vec<midi::MidiDevice>,
    pub kit_list_state: ListState,
    pub audio_list_state: ListState,
    pub midi_list_state: ListState,
    pub selected_kit: Option<(String, PathBuf)>,
    pub selected_audio: Option<(String, usize)>,
    pub error_message: Option<String>,
    pub should_quit: bool,
    pub done: bool,
    pub log_lines: Vec<String>,
    pub show_log: bool,
    pub log_scroll: usize,
    pub saved: settings::Settings,
    pub store_popup: Option<SetupStorePopup>,
    pub extra_kits_dirs: Vec<PathBuf>,
    pub kit_repos: Vec<String>,
}
