mod events;
mod render;
mod state;

use anyhow::Result;
use crossterm::{
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{widgets::ListState, Terminal};
use std::io::stdout;
use std::path::PathBuf;

use crate::audio;
use crate::kit;
use crate::midi;
use crate::settings;
use crate::stderr::StderrCapture;

use state::{SetupState, SetupStep};

/// Result of the setup flow.
pub enum SetupResult {
    Selected {
        kit_path: PathBuf,
        audio_device: usize,
        audio_device_name: String,
        midi_port: usize,
        midi_device_name: String,
    },
    Cancelled,
}

/// Try to resolve all missing presets from saved settings without entering the TUI.
fn try_autofill(
    saved: &settings::Settings,
    preset_kit: &Option<PathBuf>,
    preset_audio: Option<usize>,
    preset_midi: Option<usize>,
    _extra_kits_dirs: &[PathBuf],
) -> Option<SetupResult> {
    let kit_path = if let Some(p) = preset_kit {
        p.clone()
    } else {
        let saved_path = saved.kit_path.as_ref()?;
        if !saved_path.is_dir() {
            return None;
        }
        saved_path.clone()
    };

    let capture = StderrCapture::start();

    let (audio_device, audio_device_name) = if let Some(idx) = preset_audio {
        (idx, format!("Device {}", idx))
    } else {
        let saved_name = saved.audio_device.as_ref()?;
        let devices = audio::list_output_devices().ok()?;
        let dev = devices.iter().find(|d| d.name == *saved_name)?;
        (dev.index, dev.name.clone())
    };

    let (midi_port, midi_device_name) = if let Some(idx) = preset_midi {
        (idx, format!("MIDI port {}", idx))
    } else {
        let saved_name = saved.midi_device.as_ref()?;
        let devices = midi::list_devices().ok()?;
        let dev = devices.iter().find(|d| d.name == *saved_name)?;
        (dev.port_index, dev.name.clone())
    };

    if let Some(cap) = capture {
        cap.restore();
    }

    Some(SetupResult::Selected {
        kit_path,
        audio_device,
        audio_device_name,
        midi_port,
        midi_device_name,
    })
}

/// Run the interactive setup TUI.
pub fn run_setup(
    preset_kit: Option<PathBuf>,
    preset_audio: Option<usize>,
    preset_midi: Option<usize>,
    extra_kits_dirs: &[PathBuf],
) -> Result<SetupResult> {
    if let (Some(kit_path), Some(audio_device), Some(midi_port)) =
        (&preset_kit, preset_audio, preset_midi)
    {
        return Ok(SetupResult::Selected {
            kit_path: kit_path.clone(),
            audio_device,
            audio_device_name: format!("Device {}", audio_device),
            midi_port,
            midi_device_name: format!("MIDI port {}", midi_port),
        });
    }

    let saved = settings::load_settings();

    if preset_kit.is_none() || preset_audio.is_none() || preset_midi.is_none() {
        let can_autofill = try_autofill(
            &saved,
            &preset_kit,
            preset_audio,
            preset_midi,
            extra_kits_dirs,
        );
        if let Some(result) = can_autofill {
            return Ok(result);
        }
    }

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

    terminal.draw(render::render_loading)?;

    let capture = StderrCapture::start();

    let kits = kit::discover_kits(extra_kits_dirs);
    let audio_devices = audio::list_output_devices()?;
    let midi_devices = midi::list_devices()?;

    let first_step = if preset_kit.is_none() {
        SetupStep::Kit
    } else if preset_audio.is_none() {
        SetupStep::AudioDevice
    } else {
        SetupStep::MidiPort
    };

    let mut kit_list_state = ListState::default();
    if !kits.is_empty() {
        let default_idx = if preset_kit.is_none() {
            saved.kit_path.as_ref().and_then(|saved_path| {
                kits.iter().position(|k| k.path == *saved_path)
            })
        } else {
            None
        };
        kit_list_state.select(Some(default_idx.unwrap_or(0)));
    }
    let mut audio_list_state = ListState::default();
    if !audio_devices.is_empty() {
        let default_idx = if preset_audio.is_none() {
            saved.audio_device.as_ref().and_then(|saved_name| {
                audio_devices.iter().position(|d| d.name == *saved_name)
            })
        } else {
            None
        };
        audio_list_state.select(Some(default_idx.unwrap_or(0)));
    }
    let mut midi_list_state = ListState::default();
    if !midi_devices.is_empty() {
        let default_idx = if preset_midi.is_none() {
            saved.midi_device.as_ref().and_then(|saved_name| {
                midi_devices.iter().position(|d| d.name == *saved_name)
            })
        } else {
            None
        };
        midi_list_state.select(Some(default_idx.unwrap_or(0)));
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

    let state_kit_repos = saved.kit_repos.clone();

    let mut setup_state = SetupState {
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
        saved,
        store_popup: None,
        extra_kits_dirs: extra_kits_dirs.to_vec(),
        kit_repos: state_kit_repos,
    };

    let result = events::setup_event_loop(&mut terminal, &mut setup_state, &capture, preset_audio, preset_midi);

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    if let Some(cap) = capture {
        cap.restore();
    }

    result
}
