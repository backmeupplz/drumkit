use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::widgets::ListState;
use std::time::Duration;

use super::render::setup_ui;
use super::state::{SetupBgEvent, SetupState, SetupStep, SetupStorePopup};
use super::SetupResult;
use crate::tui::list_nav::{list_down, list_up};
use crate::{audio, download, kit, midi};
use crate::stderr::StderrCapture;

use std::sync::atomic::AtomicUsize;
use std::sync::{mpsc, Arc};

pub(super) fn setup_event_loop(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    state: &mut SetupState,
    capture: &Option<StderrCapture>,
    preset_audio: Option<usize>,
    preset_midi: Option<usize>,
) -> Result<SetupResult> {
    let (bg_tx, bg_rx) = mpsc::channel::<SetupBgEvent>();

    loop {
        // Drain captured stderr lines
        if let Some(cap) = capture {
            cap.drain_into(&mut state.log_lines);
        }

        // Drain background events
        while let Ok(ev) = bg_rx.try_recv() {
            match ev {
                SetupBgEvent::StoreFetched(result) => {
                    if !matches!(state.store_popup, Some(SetupStorePopup::Fetching)) {
                        continue;
                    }
                    match result {
                        Ok(kits) => {
                            let mut list_state = ListState::default();
                            if !kits.is_empty() {
                                list_state.select(Some(0));
                            }
                            state.store_popup = Some(SetupStorePopup::Browse { kits, list_state });
                        }
                        Err(e) => {
                            state.error_message = Some(format!("Kit store error: {}", e));
                            state.store_popup = None;
                        }
                    }
                }
                SetupBgEvent::StoreDownloaded { result, kit_name } => {
                    let is_downloading = matches!(
                        &state.store_popup,
                        Some(SetupStorePopup::Downloading { kit_name: n, .. }) if *n == kit_name
                    );
                    if !is_downloading {
                        continue;
                    }
                    match result {
                        Ok(_path) => {
                            state.error_message = Some(format!("Downloaded: {}", kit_name));
                            state.kits = kit::discover_kits(&state.extra_kits_dirs);
                            if !state.kits.is_empty() && state.kit_list_state.selected().is_none() {
                                state.kit_list_state.select(Some(0));
                            }
                            state.store_popup = Some(SetupStorePopup::Fetching);
                            let tx = bg_tx.clone();
                            let dirs = state.extra_kits_dirs.clone();
                            let repos = state.kit_repos.clone();
                            std::thread::spawn(move || {
                                let result = download::fetch_kit_list(&repos, &dirs)
                                    .map_err(|e| e.to_string());
                                let _ = tx.send(SetupBgEvent::StoreFetched(result));
                            });
                        }
                        Err(e) => {
                            state.error_message = Some(format!("Download failed: {}", e));
                            state.store_popup = None;
                        }
                    }
                }
            }
        }

        terminal.draw(|frame| setup_ui(frame, state))?;

        if state.should_quit {
            return Ok(SetupResult::Cancelled);
        }

        if state.done {
            let kit_path = state.selected_kit.as_ref().unwrap().1.clone();
            let (audio_device_name, audio_device) = {
                let a = state.selected_audio.as_ref().unwrap();
                (a.0.clone(), a.1)
            };
            let midi_port = state
                .midi_list_state
                .selected()
                .expect("MIDI port must be selected");
            let midi_device_name = state.midi_devices[midi_port].name.clone();
            return Ok(SetupResult::Selected {
                kit_path,
                audio_device,
                audio_device_name,
                midi_port,
                midi_device_name,
            });
        }

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                // Store popup handles its own keys
                if state.store_popup.is_some() {
                    handle_store_popup_key(state, &bg_tx, key.code);
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
                        state.log_scroll = state.log_lines.len().saturating_sub(1);
                    }
                    KeyCode::Char('s') if state.step == SetupStep::Kit => {
                        state.store_popup = Some(SetupStorePopup::Fetching);
                        let tx = bg_tx.clone();
                        let dirs = state.extra_kits_dirs.clone();
                        let repos = state.kit_repos.clone();
                        std::thread::spawn(move || {
                            let result = download::fetch_kit_list(&repos, &dirs)
                                .map_err(|e| e.to_string());
                            let _ = tx.send(SetupBgEvent::StoreFetched(result));
                        });
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

pub(super) fn handle_store_popup_key(
    state: &mut SetupState,
    bg_tx: &mpsc::Sender<SetupBgEvent>,
    key: KeyCode,
) {
    match &mut state.store_popup {
        Some(SetupStorePopup::Fetching) => match key {
            KeyCode::Esc | KeyCode::Char('s') => { state.store_popup = None; }
            KeyCode::Char('q') => { state.store_popup = None; state.should_quit = true; }
            _ => {}
        },
        Some(SetupStorePopup::Browse { kits, list_state }) => match key {
            KeyCode::Esc | KeyCode::Char('s') => { state.store_popup = None; }
            KeyCode::Char('q') => { state.store_popup = None; state.should_quit = true; }
            KeyCode::Up => list_up(list_state, kits.len()),
            KeyCode::Down => list_down(list_state, kits.len()),
            KeyCode::Enter => {
                if kits.is_empty() { return; }
                if let Some(idx) = list_state.selected() {
                    if kits[idx].installed { return; }
                    let kit_name = kits[idx].name.clone();
                    let kit_repo = kits[idx].repo.clone();
                    let progress = Arc::new(AtomicUsize::new(0));
                    let total = Arc::new(AtomicUsize::new(0));

                    let tx = bg_tx.clone();
                    let name_clone = kit_name.clone();
                    let prog = Arc::clone(&progress);
                    let tot = Arc::clone(&total);
                    std::thread::spawn(move || {
                        let result = download::download_kit(&kit_repo, &name_clone, &prog, &tot)
                            .map_err(|e| e.to_string());
                        let _ = tx.send(SetupBgEvent::StoreDownloaded {
                            result,
                            kit_name: name_clone,
                        });
                    });

                    state.store_popup = Some(SetupStorePopup::Downloading {
                        kit_name,
                        progress,
                        total,
                    });
                }
            }
            _ => {}
        },
        Some(SetupStorePopup::Downloading { .. }) => match key {
            KeyCode::Esc => { state.store_popup = None; }
            KeyCode::Char('q') => { state.store_popup = None; state.should_quit = true; }
            _ => {}
        },
        None => {}
    }
}

/// After rescanning audio devices, select the saved device or fall back to index 0.
fn rescan_audio(state: &mut SetupState) {
    if let Ok(devices) = audio::list_output_devices() {
        state.audio_devices = devices;
        if !state.audio_devices.is_empty() {
            let idx = state.saved.audio_device.as_ref().and_then(|name| {
                state.audio_devices.iter().position(|d| d.name == *name)
            });
            state.audio_list_state.select(Some(idx.unwrap_or(0)));
        } else {
            state.audio_list_state.select(None);
        }
    }
}

/// After rescanning MIDI devices, select the saved device or fall back to index 0.
fn rescan_midi(state: &mut SetupState) {
    if let Ok(devices) = midi::list_devices() {
        state.midi_devices = devices;
        if !state.midi_devices.is_empty() {
            let idx = state.saved.midi_device.as_ref().and_then(|name| {
                state.midi_devices.iter().position(|d| d.name == *name)
            });
            state.midi_list_state.select(Some(idx.unwrap_or(0)));
        } else {
            state.midi_list_state.select(None);
        }
    }
}

fn handle_back(state: &mut SetupState) {
    match state.step {
        SetupStep::Kit => {
            state.should_quit = true;
        }
        SetupStep::AudioDevice => {
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
            rescan_audio(state);
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
                        rescan_midi(state);
                        state.step = SetupStep::MidiPort;
                    }
                } else {
                    rescan_audio(state);
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
                    rescan_midi(state);
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

    if delta < 0 {
        list_up(list_state, len);
    } else {
        list_down(list_state, len);
    }
}
