use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use notify::Watcher;
use ratatui::{widgets::ListState, Terminal};
use std::io;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use super::{popups, render, AppState, PlayResources, Popup, TuiEvent};
use crate::{audio, kit, mapping, midi};

pub(super) fn event_loop(
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

        let extra_dirs = &resources.extra_kits_dirs;
        terminal.draw(|frame| render::ui(frame, state, extra_dirs))?;

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
                    popups::handle_popup_key(state, resources, key.code);
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
                        let kits = kit::discover_kits(&resources.extra_kits_dirs);
                        let mut list_state = ListState::default();
                        if !kits.is_empty() {
                            list_state.select(Some(0));
                        }
                        state.popup = Some(Popup::KitPicker { kits, list_state });
                    }
                    KeyCode::Char('d') => {
                        state.popup = Some(Popup::LibraryDir {
                            input: String::new(),
                            cursor: 0,
                            error: None,
                        });
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
                    KeyCode::Char('n') => {
                        let mappings = mapping::discover_all_mappings();
                        let mut list_state = ListState::default();
                        if !mappings.is_empty() {
                            // Pre-select current mapping
                            let sel = mappings.iter().position(|m| m.name == state.mapping.name).unwrap_or(0);
                            list_state.select(Some(sel));
                        }
                        state.popup = Some(Popup::MappingPicker { mappings, list_state });
                    }
                    KeyCode::Char('r') => {
                        // Find the most recent "Unknown" note in hit log
                        if let Some(entry) = state.hit_log.iter().find(|e| e.name == "Unknown") {
                            state.popup = Some(Popup::NoteRename {
                                note: entry.note,
                                input: String::new(),
                                cursor: 0,
                            });
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
                TuiEvent::KitLoadComplete { result, path, name } => {
                    // Only process if we're still showing the Loading popup for this kit
                    let is_loading = matches!(
                        &state.popup,
                        Some(Popup::Loading { kit_name, .. }) if *kit_name == name
                    );
                    if !is_loading {
                        continue; // Load was cancelled
                    }
                    match result {
                        Ok(new_kit) => {
                            // Rebuild audio stream if sample rate or channels changed
                            let need_stream_rebuild = new_kit.sample_rate != resources.sample_rate
                                || new_kit.channels != resources.channels;

                            if need_stream_rebuild {
                                // Pause audio: take the producer so MIDI callback drops events
                                {
                                    let mut guard = resources.producer.lock().unwrap();
                                    *guard = None;
                                }

                                let (new_producer, new_consumer) = rtrb::RingBuffer::new(128);

                                match audio::run_output_stream(
                                    Some(resources.audio_device_index),
                                    new_consumer,
                                    new_kit.sample_rate,
                                    new_kit.channels,
                                ) {
                                    Ok(new_stream) => {
                                        resources.stream = new_stream;
                                        resources.sample_rate = new_kit.sample_rate;
                                        resources.channels = new_kit.channels;
                                        {
                                            let mut guard = resources.producer.lock().unwrap();
                                            *guard = Some(new_producer);
                                        }
                                    }
                                    Err(e) => {
                                        // Try to restore old stream
                                        let (restore_producer, restore_consumer) = rtrb::RingBuffer::new(128);
                                        if let Ok(restored) = audio::run_output_stream(
                                            Some(resources.audio_device_index),
                                            restore_consumer,
                                            resources.sample_rate,
                                            resources.channels,
                                        ) {
                                            resources.stream = restored;
                                            let mut guard = resources.producer.lock().unwrap();
                                            *guard = Some(restore_producer);
                                        }
                                        state.set_status(format!(
                                            "Stream rebuild failed: {}",
                                            e
                                        ));
                                        state.popup = None;
                                        continue;
                                    }
                                }
                            }

                            let note_keys = kit::note_keys(&new_kit.notes);
                            resources.shared_notes.store(Arc::new(new_kit.notes));
                            let _ = resources.watcher.unwatch(&resources.kit_path);
                            let _ = resources.watcher.watch(path.as_ref(), notify::RecursiveMode::NonRecursive);
                            resources.kit_path = path;
                            state.kit_name = name;
                            state.sample_rate = resources.sample_rate;
                            state.channels = resources.channels;
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
        }
    }
}
