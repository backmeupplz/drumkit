use anyhow::Result;
use arc_swap::ArcSwap;
use notify::Watcher;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use crate::{audio, kit, mapping, midi, settings, setup, stderr, tui};

pub fn cmd_play(kit: Option<PathBuf>, port: Option<usize>, device: Option<usize>, kits_dirs: Vec<PathBuf>) -> Result<()> {
    let saved = settings::load_settings();
    let mut all_kit_dirs = saved.extra_kit_dirs.clone();
    for d in &kits_dirs {
        if !all_kit_dirs.contains(d) {
            all_kit_dirs.push(d.clone());
        }
    }
    let extra_mapping_dirs = saved.extra_mapping_dirs.clone();

    if let (Some(kit_path), Some(port_idx), Some(dev_idx)) = (&kit, port, device) {
        let audio_devices = audio::list_output_devices().unwrap_or_default();
        let midi_devices = midi::list_devices().unwrap_or_default();
        let audio_name = audio_devices
            .iter()
            .find(|d| d.index == dev_idx)
            .map(|d| d.name.clone());
        let midi_name = midi_devices
            .iter()
            .find(|d| d.port_index == port_idx)
            .map(|d| d.name.clone());
        let _ = settings::save_settings(&settings::Settings {
            kit_path: Some(kit_path.clone()),
            audio_device: audio_name,
            midi_device: midi_name,
            extra_kit_dirs: all_kit_dirs.clone(),
            extra_mapping_dirs: extra_mapping_dirs.clone(),
            kit_repos: saved.kit_repos.clone(),
        });
        return cmd_play_direct(kit_path.clone(), port_idx, dev_idx, all_kit_dirs, extra_mapping_dirs, saved.kit_repos.clone());
    }

    match setup::run_setup(kit, device, port, &all_kit_dirs)? {
        setup::SetupResult::Selected {
            kit_path,
            audio_device,
            audio_device_name,
            midi_port,
            midi_device_name,
        } => {
            let saved = settings::load_settings();
            let _ = settings::save_settings(&settings::Settings {
                kit_path: Some(kit_path.clone()),
                audio_device: Some(audio_device_name),
                midi_device: Some(midi_device_name),
                extra_kit_dirs: all_kit_dirs.clone(),
                extra_mapping_dirs: extra_mapping_dirs.clone(),
                kit_repos: saved.kit_repos.clone(),
            });
            cmd_play_direct(kit_path, midi_port, audio_device, all_kit_dirs, extra_mapping_dirs, saved.kit_repos)
        }
        setup::SetupResult::Cancelled => Ok(()),
    }
}

fn cmd_play_direct(kit_path: PathBuf, port_index: usize, audio_device: usize, kits_dirs: Vec<PathBuf>, extra_mapping_dirs: Vec<PathBuf>, kit_repos: Vec<String>) -> Result<()> {
    let mut terminal = tui::init_terminal()?;
    let kit_name_display = kit_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "kit".to_string());

    let progress = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let total = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let load_path = kit_path.clone();
    let prog = Arc::clone(&progress);
    let tot = Arc::clone(&total);
    let (load_tx, load_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = kit::load_kit_with_progress(&load_path, &prog, &tot);
        let _ = load_tx.send(result);
    });

    // Draw progress bar while waiting
    let loaded_kit = loop {
        use ratatui::{
            style::{Color, Modifier, Style},
            text::{Line, Span},
            widgets::{Block, BorderType, Borders, Paragraph},
        };
        use std::sync::atomic::Ordering;

        let cur = progress.load(Ordering::Relaxed);
        let tot = total.load(Ordering::Relaxed);

        let kit_name = kit_name_display.clone();
        terminal.draw(move |frame| {
            let area = frame.area();
            let outer = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" drumkit ")
                .title_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
            let inner = outer.inner(area);
            frame.render_widget(outer, area);

            if inner.height < 3 || inner.width < 10 {
                return;
            }

            let progress_text = if tot == 0 {
                format!("  Loading \"{}\"...", kit_name)
            } else {
                format!("  Loading \"{}\"... {}/{}", kit_name, cur, tot)
            };

            let bar_w = (inner.width as usize).saturating_sub(4);
            let (filled, empty) = if tot == 0 || bar_w == 0 {
                (0, bar_w)
            } else {
                let f = (cur * bar_w) / tot;
                (f.min(bar_w), bar_w.saturating_sub(f))
            };

            let lines = vec![
                Line::from(Span::styled(
                    progress_text,
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        "\u{2588}".repeat(filled),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        "\u{2591}".repeat(empty),
                        Style::default().fg(Color::Rgb(50, 50, 60)),
                    ),
                ]),
            ];
            frame.render_widget(Paragraph::new(lines), inner);
        })?;

        match load_rx.try_recv() {
            Ok(result) => break result?,
            Err(mpsc::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(33));
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                tui::restore_terminal();
                anyhow::bail!("Kit loading thread panicked");
            }
        }
    };

    match cmd_play_direct_inner(terminal, kit_path, loaded_kit, port_index, audio_device, kits_dirs, extra_mapping_dirs, kit_repos) {
        Ok(()) => Ok(()),
        Err(e) => {
            tui::restore_terminal();
            Err(e)
        }
    }
}

fn cmd_play_direct_inner(
    terminal: tui::Term,
    kit_path: PathBuf,
    loaded_kit: kit::Kit,
    port_index: usize,
    audio_device: usize,
    kits_dirs: Vec<PathBuf>,
    extra_mapping_dirs: Vec<PathBuf>,
    kit_repos: Vec<String>,
) -> Result<()> {
    let capture = stderr::StderrCapture::start();

    let initial_mapping = mapping::load_kit_mapping(&kit_path)
        .unwrap_or_else(mapping::default_mapping);
    let shared_mapping = Arc::new(ArcSwap::from_pointee(initial_mapping));

    let kit_summary = kit::summary_lines(&loaded_kit, &shared_mapping.load());

    let choke_fade = (loaded_kit.sample_rate as f64 * 0.068) as usize;
    let aftertouch_fade = (loaded_kit.sample_rate as f64 * 0.085) as usize;

    let (producer, consumer) = rtrb::RingBuffer::new(128);
    let shared_producer = Arc::new(Mutex::new(Some(producer)));

    let stream = audio::run_output_stream(Some(audio_device), consumer, loaded_kit.sample_rate, loaded_kit.channels)?;

    let midi_device_name = midi::list_devices()?
        .into_iter()
        .find(|d| d.port_index == port_index)
        .map(|d| d.name)
        .unwrap_or_else(|| format!("MIDI port {}", port_index));

    let shared_notes = Arc::new(ArcSwap::from_pointee(loaded_kit.notes));

    let (tui_tx, tui_rx) = mpsc::channel::<tui::TuiEvent>();

    let connection = midi::connect_callback(
        port_index,
        midi::build_midi_callback(
            Arc::clone(&shared_producer),
            Arc::clone(&shared_notes),
            Arc::clone(&shared_mapping),
            tui_tx.clone(),
            choke_fade,
            aftertouch_fade,
        ),
    )?;

    let (watch_tx, watch_rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        if let Ok(_event) = res {
            let _ = watch_tx.send(());
        }
    })?;
    watcher.watch(kit_path.as_ref(), notify::RecursiveMode::NonRecursive)?;

    let stream_sample_rate = loaded_kit.sample_rate;
    let stream_channels = loaded_kit.channels;

    let shared_kit_path = Arc::new(ArcSwap::from_pointee(kit_path.clone()));
    let suppress_reload = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let debounce_shared_notes = Arc::clone(&shared_notes);
    let debounce_shared_mapping = Arc::clone(&shared_mapping);
    let debounce_kit_path = Arc::clone(&shared_kit_path);
    let debounce_suppress = Arc::clone(&suppress_reload);
    let debounce_tui_tx = tui_tx.clone();
    std::thread::spawn(move || {
        let debounce = Duration::from_millis(500);
        let mut last_event: Option<Instant> = None;

        loop {
            let timeout = match last_event {
                Some(t) => {
                    let elapsed = t.elapsed();
                    if elapsed >= debounce {
                        if debounce_suppress.swap(false, std::sync::atomic::Ordering::Relaxed) {
                            last_event = None;
                            continue;
                        }
                        let current_path = debounce_kit_path.load();
                        match kit::load_kit(&current_path) {
                            Ok(new_kit) => {
                                if new_kit.sample_rate != stream_sample_rate {
                                    let _ = debounce_tui_tx.send(tui::TuiEvent::KitReloadError(
                                        format!(
                                            "sample rate mismatch (expected {} Hz, got {} Hz)",
                                            stream_sample_rate, new_kit.sample_rate
                                        ),
                                    ));
                                } else if new_kit.channels != stream_channels {
                                    let _ = debounce_tui_tx.send(tui::TuiEvent::KitReloadError(
                                        format!(
                                            "channel count mismatch (expected {}, got {})",
                                            stream_channels, new_kit.channels
                                        ),
                                    ));
                                } else {
                                    let note_keys = kit::note_keys(&new_kit.notes);
                                    debounce_shared_notes.store(Arc::new(new_kit.notes));
                                    if let Some(new_mapping) = mapping::load_kit_mapping(&current_path) {
                                        debounce_shared_mapping.store(Arc::new(new_mapping.clone()));
                                        let _ = debounce_tui_tx.send(tui::TuiEvent::MappingReloaded(new_mapping, current_path.to_path_buf()));
                                    }
                                    let _ = debounce_tui_tx
                                        .send(tui::TuiEvent::KitReloaded { note_keys, kit_path: (*current_path).to_path_buf() });
                                }
                            }
                            Err(e) => {
                                let _ = debounce_tui_tx
                                    .send(tui::TuiEvent::KitReloadError(format!("{}", e)));
                            }
                        }
                        last_event = None;
                        continue;
                    }
                    debounce - elapsed
                }
                None => Duration::from_secs(3600),
            };

            match watch_rx.recv_timeout(timeout) {
                Ok(()) => {
                    while watch_rx.try_recv().is_ok() {}
                    last_event = Some(Instant::now());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    let mut initial_log = kit_summary;
    if let Some(ref cap) = capture {
        cap.drain_into(&mut initial_log);
    }

    let note_keys = kit::note_keys(&shared_notes.load());
    let current_mapping = Arc::clone(&shared_mapping.load());
    let state = tui::AppState::new(
        loaded_kit.name,
        midi_device_name,
        stream_sample_rate,
        stream_channels,
        &note_keys,
        initial_log,
        current_mapping,
    );

    let resources = tui::PlayResources {
        stream,
        connection,
        producer: shared_producer,
        shared_notes,
        kit_path,
        shared_kit_path,
        suppress_reload,
        sample_rate: stream_sample_rate,
        channels: stream_channels,
        audio_device_index: audio_device,
        midi_port_index: port_index,
        tui_tx: tui_tx.clone(),
        choke_fade,
        aftertouch_fade,
        watcher,
        stderr_capture: capture,
        extra_kits_dirs: kits_dirs,
        extra_mapping_dirs,
        shared_mapping,
        kit_repos,
    };

    tui::run(terminal, tui_rx, state, resources)
}
