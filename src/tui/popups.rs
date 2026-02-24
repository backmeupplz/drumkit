use crossterm::event::KeyCode;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use super::{AppState, PlayResources, Popup, TuiEvent};
use crate::{audio, kit, mapping, midi};

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

                    let progress = Arc::new(AtomicUsize::new(0));
                    let total = Arc::new(AtomicUsize::new(0));

                    // Spawn background thread
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
        Popup::Loading { .. } => match key {
            KeyCode::Esc => { state.popup = None; }
            KeyCode::Char('q') => { state.popup = None; state.should_quit = true; }
            _ => {}
        },
        Popup::MappingPicker { mappings, list_state } => match key {
            KeyCode::Char('n') | KeyCode::Esc => { state.popup = None; }
            KeyCode::Char('q') => { state.popup = None; state.should_quit = true; }
            KeyCode::Up => {
                if !mappings.is_empty() {
                    let cur = list_state.selected().unwrap_or(0);
                    list_state.select(Some(if cur == 0 { mappings.len() - 1 } else { cur - 1 }));
                }
            }
            KeyCode::Down => {
                if !mappings.is_empty() {
                    let cur = list_state.selected().unwrap_or(0);
                    list_state.select(Some(if cur >= mappings.len() - 1 { 0 } else { cur + 1 }));
                }
            }
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
            _ => {}
        },
        Popup::NoteRename { note, input, cursor } => match key {
            KeyCode::Esc => { state.popup = None; }
            KeyCode::Char('q') if input.is_empty() => { state.popup = None; state.should_quit = true; }
            KeyCode::Enter => {
                if !input.is_empty() {
                    let note_val = *note;
                    let name = input.clone();

                    // Clone to a user mapping if current is built-in or kit-bundled
                    let mut new_mapping = (*state.mapping).clone();
                    if matches!(new_mapping.source, mapping::MappingSource::BuiltIn | mapping::MappingSource::KitFile(_)) {
                        new_mapping.name = format!("{} (Custom)", new_mapping.name);
                        new_mapping.source = mapping::MappingSource::UserFile(
                            PathBuf::from("unsaved"),
                        );
                    }
                    new_mapping.set_note_name(note_val, name);

                    // Try to save user mapping
                    let _ = mapping::save_user_mapping(&new_mapping);

                    let new_mapping = Arc::new(new_mapping);
                    resources.shared_mapping.store(Arc::clone(&new_mapping));
                    state.mapping = new_mapping;
                    state.update_hit_log_names();
                    state.set_status(format!("Renamed note {}", note_val));
                    state.popup = None;
                }
            }
            KeyCode::Char(c) => {
                input.insert(*cursor, c);
                *cursor += 1;
            }
            KeyCode::Backspace => {
                if *cursor > 0 {
                    *cursor -= 1;
                    input.remove(*cursor);
                }
            }
            KeyCode::Delete => {
                if *cursor < input.len() {
                    input.remove(*cursor);
                }
            }
            KeyCode::Left => {
                *cursor = cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                if *cursor < input.len() {
                    *cursor += 1;
                }
            }
            KeyCode::Home => { *cursor = 0; }
            KeyCode::End => { *cursor = input.len(); }
            _ => {}
        },
        Popup::LibraryDir { input, cursor, error } => match key {
            KeyCode::Esc => { state.popup = None; }
            KeyCode::Char('q') if input.is_empty() => { state.popup = None; state.should_quit = true; }
            KeyCode::Enter => {
                let path = PathBuf::from(input.as_str());
                if path.is_dir() {
                    resources.extra_kits_dirs.push(path);
                    state.set_status("Directory added".to_string());
                    state.popup = None;
                } else {
                    *error = Some(format!("Not a directory: {}", input));
                }
            }
            KeyCode::Char(c) => {
                input.insert(*cursor, c);
                *cursor += 1;
                *error = None;
            }
            KeyCode::Backspace => {
                if *cursor > 0 {
                    *cursor -= 1;
                    input.remove(*cursor);
                    *error = None;
                }
            }
            KeyCode::Delete => {
                if *cursor < input.len() {
                    input.remove(*cursor);
                    *error = None;
                }
            }
            KeyCode::Left => {
                *cursor = cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                if *cursor < input.len() {
                    *cursor += 1;
                }
            }
            KeyCode::Home => { *cursor = 0; }
            KeyCode::End => { *cursor = input.len(); }
            _ => {}
        },
    }
}
