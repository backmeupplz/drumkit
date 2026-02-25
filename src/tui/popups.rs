use crossterm::event::KeyCode;
use ratatui::widgets::ListState;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use super::input::handle_text_input_key;
use super::list_nav::{index_down, index_up, list_down, list_down_skip, list_up, list_up_skip};
use super::{AppState, DirPopupMode, PlayResources, Popup, TuiEvent};
use crate::{audio, download, kit, mapping, midi, settings};

pub(super) fn handle_popup_key(state: &mut AppState, resources: &mut PlayResources, key: KeyCode) {
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
            KeyCode::Char('s') => {
                state.popup = Some(Popup::KitStoreFetching);
                let tx = resources.tui_tx.clone();
                let dirs = resources.extra_kits_dirs.clone();
                let repos = resources.kit_repos.clone();
                std::thread::spawn(move || {
                    let result = download::fetch_kit_list(&repos, &dirs)
                        .map_err(|e| e.to_string());
                    let _ = tx.send(TuiEvent::KitStoreFetched { result });
                });
            }
            KeyCode::Up => list_up(list_state, kits.len()),
            KeyCode::Down => list_down(list_state, kits.len()),
            KeyCode::Enter => {
                if kits.is_empty() { return; }
                if let Some(idx) = list_state.selected() {
                    let selected_path = kits[idx].path.clone();
                    let selected_name = kits[idx].name.clone();

                    let progress = Arc::new(AtomicUsize::new(0));
                    let total = Arc::new(AtomicUsize::new(0));

                    let tx = resources.tui_tx.clone();
                    let path_clone = selected_path.clone();
                    let name_clone = selected_name.clone();
                    let prog = Arc::clone(&progress);
                    let tot = Arc::clone(&total);
                    std::thread::spawn(move || {
                        let result = kit::load_kit_with_progress(&path_clone, &prog, &tot)
                            .map_err(|e| e.to_string());
                        let _ = tx.send(TuiEvent::KitLoadComplete {
                            result,
                            path: path_clone,
                            name: name_clone,
                        });
                    });

                    state.popup = Some(Popup::Loading {
                        kit_name: selected_name,
                        progress,
                        total,
                    });
                }
            }
            _ => {}
        },
        Popup::AudioPicker { devices, list_state } => match key {
            KeyCode::Char('a') | KeyCode::Esc => { state.popup = None; }
            KeyCode::Char('q') => { state.popup = None; state.should_quit = true; }
            KeyCode::Up => list_up(list_state, devices.len()),
            KeyCode::Down => list_down(list_state, devices.len()),
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
                            resources.stream = new_stream;
                            resources.audio_device_index = new_device_index;
                            {
                                let mut guard = resources.producer.lock().unwrap();
                                *guard = Some(new_producer);
                            }
                            state.set_status(format!("Audio: {}", new_device_name));
                            let mut s = settings::load_settings();
                            s.audio_device = Some(new_device_name);
                            let _ = settings::save_settings(&s);
                        }
                        Err(e) => {
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
                                Err(_) => {}
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
            KeyCode::Up => list_up(list_state, devices.len()),
            KeyCode::Down => list_down(list_state, devices.len()),
            KeyCode::Enter => {
                if devices.is_empty() { return; }
                if let Some(idx) = list_state.selected() {
                    let new_port_index = devices[idx].port_index;
                    let new_port_name = devices[idx].name.clone();

                    let callback = midi::build_midi_callback(
                        Arc::clone(&resources.producer),
                        Arc::clone(&resources.shared_notes),
                        Arc::clone(&resources.shared_mapping),
                        resources.tui_tx.clone(),
                        resources.choke_fade,
                        resources.aftertouch_fade,
                    );

                    match midi::connect_callback(new_port_index, callback) {
                        Ok(new_connection) => {
                            resources.connection = new_connection;
                            resources.midi_port_index = new_port_index;
                            state.midi_device = new_port_name.clone();
                            state.set_status(format!("MIDI: {}", new_port_name));
                            let mut s = settings::load_settings();
                            s.midi_device = Some(new_port_name);
                            let _ = settings::save_settings(&s);
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
        Popup::Loading { .. } => match key {
            KeyCode::Esc => { state.popup = None; }
            KeyCode::Char('q') => { state.popup = None; state.should_quit = true; }
            _ => {}
        },
        Popup::MappingPicker { mappings, list_state } => match key {
            KeyCode::Char('n') | KeyCode::Esc => { state.popup = None; }
            KeyCode::Char('q') => { state.popup = None; state.should_quit = true; }
            KeyCode::Up => list_up(list_state, mappings.len()),
            KeyCode::Down => list_down(list_state, mappings.len()),
            KeyCode::Enter => {
                if mappings.is_empty() { return; }
                if let Some(idx) = list_state.selected() {
                    let selected = mappings[idx].clone();
                    let new_mapping = Arc::new(selected);
                    resources.shared_mapping.store(Arc::clone(&new_mapping));
                    state.mapping = new_mapping;
                    state.update_hit_log_names();
                    state.set_status(format!("Mapping: {}", state.mapping.name));
                    state.popup = None;
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                if mappings.is_empty() { return; }
                if let Some(idx) = list_state.selected() {
                    if let mapping::MappingSource::UserFile(path) = &mappings[idx].source {
                        let name = mappings[idx].name.clone();
                        let path = path.clone();
                        state.popup = Some(Popup::DeleteMapping { name, path });
                    }
                }
            }
            _ => {}
        },
        Popup::DeleteMapping { name, path } => match key {
            KeyCode::Char('y') | KeyCode::Enter => {
                let deleted_name = name.clone();
                if std::fs::remove_file(&path).is_ok() {
                    state.set_status(format!("Deleted mapping: {}", deleted_name));
                } else {
                    state.set_status(format!("Failed to delete: {}", deleted_name));
                }
                let mappings = mapping::discover_all_mappings(&resources.extra_mapping_dirs);
                let mut list_state = ListState::default();
                if !mappings.is_empty() {
                    let sel = mappings.iter().position(|m| m.name == state.mapping.name).unwrap_or(0);
                    list_state.select(Some(sel));
                }
                state.popup = Some(Popup::MappingPicker { mappings, list_state });
            }
            _ => {
                let mappings = mapping::discover_all_mappings(&resources.extra_mapping_dirs);
                let mut list_state = ListState::default();
                if !mappings.is_empty() {
                    let sel = mappings.iter().position(|m| m.name == state.mapping.name).unwrap_or(0);
                    list_state.select(Some(sel));
                }
                state.popup = Some(Popup::MappingPicker { mappings, list_state });
            }
        },
        Popup::NoteRename { note, input, cursor } => match key {
            KeyCode::Esc => { state.popup = None; }
            KeyCode::Char('q') if input.is_empty() => { state.popup = None; state.should_quit = true; }
            KeyCode::Enter => {
                if !input.is_empty() {
                    let note_val = *note;
                    let name = input.clone();

                    let mut new_mapping = (*state.mapping).clone();
                    if matches!(new_mapping.source, mapping::MappingSource::BuiltIn | mapping::MappingSource::KitFile(_)) {
                        new_mapping.name = format!("{} (Custom)", new_mapping.name);
                        new_mapping.source = mapping::MappingSource::UserFile(
                            PathBuf::from("unsaved"),
                        );
                    }
                    new_mapping.set_note_name(note_val, name);

                    let _ = mapping::save_user_mapping(&new_mapping);

                    let new_mapping = Arc::new(new_mapping);
                    resources.shared_mapping.store(Arc::clone(&new_mapping));
                    state.mapping = new_mapping;
                    state.update_hit_log_names();
                    state.set_status(format!("Renamed note {}", note_val));
                    state.popup = None;
                }
            }
            other => { handle_text_input_key(input, cursor, None, other); }
        },
        Popup::LibraryDir { .. } => {
            handle_library_dir_key(state, resources, key);
        }
        Popup::KitStoreFetching => match key {
            KeyCode::Esc | KeyCode::Char('s') => { state.popup = None; }
            KeyCode::Char('q') => { state.popup = None; state.should_quit = true; }
            _ => {}
        },
        Popup::KitStore { kits, rows, list_state } => match key {
            KeyCode::Esc | KeyCode::Char('s') => { state.popup = None; }
            KeyCode::Char('q') => { state.popup = None; state.should_quit = true; }
            KeyCode::Char('r') => {
                state.popup = Some(Popup::KitStoreRepos {
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
                    if kits[idx].installed {
                        return;
                    }
                    let kit_name = kits[idx].name.clone();
                    let kit_repo = kits[idx].repo.clone();
                    let progress = Arc::new(AtomicUsize::new(0));
                    let total = Arc::new(AtomicUsize::new(0));

                    let tx = resources.tui_tx.clone();
                    let name_clone = kit_name.clone();
                    let prog = Arc::clone(&progress);
                    let tot = Arc::clone(&total);
                    std::thread::spawn(move || {
                        let result = download::download_kit(&kit_repo, &name_clone, &prog, &tot)
                            .map_err(|e| e.to_string());
                        let _ = tx.send(TuiEvent::KitDownloadComplete {
                            result,
                            kit_name: name_clone,
                        });
                    });

                    state.popup = Some(Popup::KitDownloading {
                        kit_name,
                        progress,
                        total,
                    });
                }
            }
            _ => {}
        },
        Popup::KitDownloading { .. } => match key {
            KeyCode::Esc => { state.popup = None; }
            KeyCode::Char('q') => { state.popup = None; state.should_quit = true; }
            _ => {}
        },
        Popup::KitStoreRepos { .. } => {
            handle_kit_store_repos_key(state, resources, key);
        },
    }
}

fn save_repo_settings(resources: &PlayResources) {
    let mut s = settings::load_settings();
    s.kit_repos = resources.kit_repos.clone();
    let _ = settings::save_settings(&s);
}

fn handle_kit_store_repos_key(state: &mut AppState, resources: &mut PlayResources, key: KeyCode) {
    let is_adding = matches!(
        &state.popup,
        Some(Popup::KitStoreRepos { adding: true, .. })
    );

    if is_adding {
        match key {
            KeyCode::Esc => {
                if let Some(Popup::KitStoreRepos { adding, input, cursor, error, .. }) = &mut state.popup {
                    *adding = false;
                    input.clear();
                    *cursor = 0;
                    *error = None;
                }
            }
            KeyCode::Enter => {
                let raw = if let Some(Popup::KitStoreRepos { input, .. }) = &state.popup {
                    input.clone()
                } else {
                    return;
                };
                match download::normalize_repo_input(&raw) {
                    None => {
                        if let Some(Popup::KitStoreRepos { error, .. }) = &mut state.popup {
                            *error = Some("Format: owner/repo".to_string());
                        }
                    }
                    Some(repo) if resources.kit_repos.contains(&repo) => {
                        if let Some(Popup::KitStoreRepos { error, .. }) = &mut state.popup {
                            *error = Some("Already added".to_string());
                        }
                    }
                    Some(repo) => {
                        resources.kit_repos.push(repo);
                        save_repo_settings(resources);
                        if let Some(Popup::KitStoreRepos { adding, input, cursor, error, .. }) = &mut state.popup {
                            *adding = false;
                            input.clear();
                            *cursor = 0;
                            *error = None;
                        }
                    }
                }
            }
            other => {
                if let Some(Popup::KitStoreRepos { input, cursor, error, .. }) = &mut state.popup {
                    handle_text_input_key(input, cursor, Some(error), other);
                }
            }
        }
    } else {
        // Check if we're in confirm_delete mode
        let is_confirming = matches!(
            &state.popup,
            Some(Popup::KitStoreRepos { confirm_delete: true, .. })
        );

        if is_confirming {
            match key {
                KeyCode::Char('d') | KeyCode::Delete => {
                    // Confirmed: actually delete
                    let sel = if let Some(Popup::KitStoreRepos { selected, .. }) = &state.popup {
                        *selected
                    } else {
                        return;
                    };
                    let len = resources.kit_repos.len();
                    if len > 0 && sel < len {
                        let removed = resources.kit_repos.remove(sel);
                        save_repo_settings(resources);
                        state.set_status(format!("Removed repo: {}", removed));
                        let new_len = resources.kit_repos.len();
                        if let Some(Popup::KitStoreRepos { selected, confirm_delete, .. }) = &mut state.popup {
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
                    if let Some(Popup::KitStoreRepos { confirm_delete, .. }) = &mut state.popup {
                        *confirm_delete = false;
                    }
                }
            }
            return;
        }

        match key {
            KeyCode::Esc => {
                state.popup = Some(Popup::KitStoreFetching);
                let tx = resources.tui_tx.clone();
                let dirs = resources.extra_kits_dirs.clone();
                let repos = resources.kit_repos.clone();
                std::thread::spawn(move || {
                    let result = download::fetch_kit_list(&repos, &dirs)
                        .map_err(|e| e.to_string());
                    let _ = tx.send(TuiEvent::KitStoreFetched { result });
                });
            }
            KeyCode::Char('q') => { state.popup = None; state.should_quit = true; }
            KeyCode::Char('a') => {
                if let Some(Popup::KitStoreRepos { adding, input, cursor, error, .. }) = &mut state.popup {
                    *adding = true;
                    input.clear();
                    *cursor = 0;
                    *error = None;
                }
            }
            KeyCode::Up => {
                let len = resources.kit_repos.len();
                if let Some(Popup::KitStoreRepos { selected, .. }) = &mut state.popup {
                    index_up(selected, len);
                }
            }
            KeyCode::Down => {
                let len = resources.kit_repos.len();
                if let Some(Popup::KitStoreRepos { selected, .. }) = &mut state.popup {
                    index_down(selected, len);
                }
            }
            KeyCode::Delete | KeyCode::Char('d') => {
                let sel = if let Some(Popup::KitStoreRepos { selected, .. }) = &state.popup {
                    *selected
                } else {
                    return;
                };
                if !resources.kit_repos.is_empty() && sel < resources.kit_repos.len() {
                    if let Some(Popup::KitStoreRepos { confirm_delete, .. }) = &mut state.popup {
                        *confirm_delete = true;
                    }
                }
            }
            _ => {}
        }
    }
}

/// Save the current extra kit/mapping dirs to settings.
fn save_dir_settings(resources: &PlayResources) {
    let mut s = settings::load_settings();
    s.extra_kit_dirs = resources.extra_kits_dirs.clone();
    s.extra_mapping_dirs = resources.extra_mapping_dirs.clone();
    let _ = settings::save_settings(&s);
}

fn handle_library_dir_key(state: &mut AppState, resources: &mut PlayResources, key: KeyCode) {
    let is_browse = matches!(
        &state.popup,
        Some(Popup::LibraryDir { mode: DirPopupMode::Browse, .. })
    );

    if is_browse {
        match key {
            KeyCode::Esc | KeyCode::Char('d') => { state.popup = None; }
            KeyCode::Char('q') => { state.popup = None; state.should_quit = true; }
            KeyCode::Char('a') => {
                if let Some(Popup::LibraryDir { mode, input, cursor, error, .. }) = &mut state.popup {
                    *mode = DirPopupMode::AddKit;
                    input.clear();
                    *cursor = 0;
                    *error = None;
                }
            }
            KeyCode::Char('A') => {
                if let Some(Popup::LibraryDir { mode, input, cursor, error, .. }) = &mut state.popup {
                    *mode = DirPopupMode::AddMapping;
                    input.clear();
                    *cursor = 0;
                    *error = None;
                }
            }
            KeyCode::Up => {
                let count = resources.extra_kits_dirs.len() + resources.extra_mapping_dirs.len();
                if let Some(Popup::LibraryDir { selected, .. }) = &mut state.popup {
                    index_up(selected, count);
                }
            }
            KeyCode::Down => {
                let count = resources.extra_kits_dirs.len() + resources.extra_mapping_dirs.len();
                if let Some(Popup::LibraryDir { selected, .. }) = &mut state.popup {
                    index_down(selected, count);
                }
            }
            KeyCode::Delete | KeyCode::Backspace => {
                let sel = if let Some(Popup::LibraryDir { selected, .. }) = &state.popup {
                    *selected
                } else {
                    return;
                };
                let kit_count = resources.extra_kits_dirs.len();
                let total = kit_count + resources.extra_mapping_dirs.len();
                if total > 0 && sel < total {
                    let status_msg = if sel < kit_count {
                        let removed = resources.extra_kits_dirs.remove(sel);
                        format!("Removed kit dir: {}", removed.display())
                    } else {
                        let mapping_idx = sel - kit_count;
                        let removed = resources.extra_mapping_dirs.remove(mapping_idx);
                        format!("Removed mapping dir: {}", removed.display())
                    };
                    save_dir_settings(resources);
                    state.set_status(status_msg);
                    let new_total = resources.extra_kits_dirs.len() + resources.extra_mapping_dirs.len();
                    if let Some(Popup::LibraryDir { selected, .. }) = &mut state.popup {
                        if new_total == 0 {
                            *selected = 0;
                        } else if *selected >= new_total {
                            *selected = new_total - 1;
                        }
                    }
                }
            }
            _ => {}
        }
    } else {
        // Add mode
        match key {
            KeyCode::Esc => {
                if let Some(Popup::LibraryDir { mode, input, cursor, error, .. }) = &mut state.popup {
                    *mode = DirPopupMode::Browse;
                    input.clear();
                    *cursor = 0;
                    *error = None;
                }
            }
            KeyCode::Enter => {
                let (path_str, is_add_kit) = if let Some(Popup::LibraryDir { mode, input, .. }) = &state.popup {
                    (input.clone(), matches!(mode, DirPopupMode::AddKit))
                } else {
                    return;
                };
                let path = PathBuf::from(&path_str);
                if path.is_dir() {
                    let status_msg = if is_add_kit {
                        if !resources.extra_kits_dirs.contains(&path) {
                            resources.extra_kits_dirs.push(path.clone());
                        }
                        format!("Added kit dir: {}", path.display())
                    } else {
                        if !resources.extra_mapping_dirs.contains(&path) {
                            resources.extra_mapping_dirs.push(path.clone());
                        }
                        format!("Added mapping dir: {}", path.display())
                    };
                    save_dir_settings(resources);
                    state.set_status(status_msg);
                    if let Some(Popup::LibraryDir { mode, input, cursor, error, .. }) = &mut state.popup {
                        *mode = DirPopupMode::Browse;
                        input.clear();
                        *cursor = 0;
                        *error = None;
                    }
                } else {
                    if let Some(Popup::LibraryDir { error, .. }) = &mut state.popup {
                        *error = Some(format!("Not a directory: {}", path_str));
                    }
                }
            }
            other => {
                if let Some(Popup::LibraryDir { input, cursor, error, .. }) = &mut state.popup {
                    handle_text_input_key(input, cursor, Some(error), other);
                }
            }
        }
    }
}
