use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::widgets::ListState;
use std::time::Duration;

use super::render::setup_ui;
use super::state::{SetupBgEvent, SetupState, SetupStep, SetupStorePopup};
use super::SetupResult;
use crate::tui::input::handle_text_input_key;
use crate::tui::list_nav::{first_selectable, index_down, index_up, list_down, list_down_skip, list_up, list_up_skip};
use crate::{audio, download, kit, midi, settings};
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

    // Auto-open Kit Store when no kits are installed (first-run experience)
    if state.step == SetupStep::Kit && state.kits.is_empty() {
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
                            let rows = download::build_store_rows(&kits);
                            let mut list_state = ListState::default();
                            let first = first_selectable(rows.len(), |i| matches!(rows[i], download::StoreRow::Kit(_)));
                            list_state.select(first);
                            state.store_popup = Some(SetupStorePopup::Browse { kits, rows, list_state });
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
        Some(SetupStorePopup::Browse { kits, rows, list_state }) => match key {
            KeyCode::Esc | KeyCode::Char('s') => { state.store_popup = None; }
            KeyCode::Char('q') => { state.store_popup = None; state.should_quit = true; }
            KeyCode::Char('r') => {
                state.store_popup = Some(SetupStorePopup::Repos {
                    selected: 0,
                    adding: false,
                    input: String::new(),
                    cursor: 0,
                    error: None,
                    confirm_delete: false,
                });
            }
            KeyCode::Up => list_up_skip(list_state, rows.len(), |i| matches!(rows[i], download::StoreRow::Kit(_))),
            KeyCode::Down => list_down_skip(list_state, rows.len(), |i| matches!(rows[i], download::StoreRow::Kit(_))),
            KeyCode::Enter => {
                if kits.is_empty() { return; }
                if let Some(sel) = list_state.selected()
                    && let download::StoreRow::Kit(idx) = rows[sel]
                {
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
        Some(SetupStorePopup::Repos { .. }) => {
            handle_repos_popup_key(state, bg_tx, key);
        }
        None => {}
    }
}

fn save_setup_repo_settings(state: &SetupState) {
    let mut s = settings::load_settings();
    s.kit_repos = state.kit_repos.clone();
    let _ = settings::save_settings(&s);
}

fn handle_repos_popup_key(
    state: &mut SetupState,
    bg_tx: &mpsc::Sender<SetupBgEvent>,
    key: KeyCode,
) {
    let is_adding = matches!(
        &state.store_popup,
        Some(SetupStorePopup::Repos { adding: true, .. })
    );

    if is_adding {
        match key {
            KeyCode::Esc => {
                if let Some(SetupStorePopup::Repos { adding, input, cursor, error, .. }) = &mut state.store_popup {
                    *adding = false;
                    input.clear();
                    *cursor = 0;
                    *error = None;
                }
            }
            KeyCode::Enter => {
                let raw = if let Some(SetupStorePopup::Repos { input, .. }) = &state.store_popup {
                    input.clone()
                } else {
                    return;
                };
                match download::normalize_repo_input(&raw) {
                    None => {
                        if let Some(SetupStorePopup::Repos { error, .. }) = &mut state.store_popup {
                            *error = Some("Format: owner/repo".to_string());
                        }
                    }
                    Some(repo) if state.kit_repos.contains(&repo) => {
                        if let Some(SetupStorePopup::Repos { error, .. }) = &mut state.store_popup {
                            *error = Some("Already added".to_string());
                        }
                    }
                    Some(repo) => {
                        state.kit_repos.push(repo);
                        save_setup_repo_settings(state);
                        if let Some(SetupStorePopup::Repos { adding, input, cursor, error, .. }) = &mut state.store_popup {
                            *adding = false;
                            input.clear();
                            *cursor = 0;
                            *error = None;
                        }
                    }
                }
            }
            other => {
                if let Some(SetupStorePopup::Repos { input, cursor, error, .. }) = &mut state.store_popup {
                    handle_text_input_key(input, cursor, Some(error), other);
                }
            }
        }
    } else {
        // Check if we're in confirm_delete mode
        let is_confirming = matches!(
            &state.store_popup,
            Some(SetupStorePopup::Repos { confirm_delete: true, .. })
        );

        if is_confirming {
            match key {
                KeyCode::Char('d') | KeyCode::Delete => {
                    // Confirmed: actually delete
                    let sel = if let Some(SetupStorePopup::Repos { selected, .. }) = &state.store_popup {
                        *selected
                    } else {
                        return;
                    };
                    let len = state.kit_repos.len();
                    if len > 0 && sel < len {
                        state.kit_repos.remove(sel);
                        save_setup_repo_settings(state);
                        let new_len = state.kit_repos.len();
                        if let Some(SetupStorePopup::Repos { selected, confirm_delete, .. }) = &mut state.store_popup {
                            *confirm_delete = false;
                            if new_len == 0 {
                                *selected = 0;
                            } else if *selected >= new_len {
                                *selected = new_len - 1;
                            }
                        }
                    }
                }
                _ => {
                    // Cancel confirmation
                    if let Some(SetupStorePopup::Repos { confirm_delete, .. }) = &mut state.store_popup {
                        *confirm_delete = false;
                    }
                }
            }
            return;
        }

        match key {
            KeyCode::Esc => {
                // Go back to store: re-fetch kit list
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
            KeyCode::Char('q') => { state.store_popup = None; state.should_quit = true; }
            KeyCode::Char('a') => {
                if let Some(SetupStorePopup::Repos { adding, input, cursor, error, .. }) = &mut state.store_popup {
                    *adding = true;
                    input.clear();
                    *cursor = 0;
                    *error = None;
                }
            }
            KeyCode::Up => {
                let len = state.kit_repos.len();
                if let Some(SetupStorePopup::Repos { selected, .. }) = &mut state.store_popup {
                    index_up(selected, len);
                }
            }
            KeyCode::Down => {
                let len = state.kit_repos.len();
                if let Some(SetupStorePopup::Repos { selected, .. }) = &mut state.store_popup {
                    index_down(selected, len);
                }
            }
            KeyCode::Delete | KeyCode::Char('d') => {
                let sel = if let Some(SetupStorePopup::Repos { selected, .. }) = &state.store_popup {
                    *selected
                } else {
                    return;
                };
                if !state.kit_repos.is_empty() && sel < state.kit_repos.len() {
                    if let Some(SetupStorePopup::Repos { confirm_delete, .. }) = &mut state.store_popup {
                        *confirm_delete = true;
                    }
                }
            }
            _ => {}
        }
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
