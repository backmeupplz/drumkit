mod audio;
mod kit;
mod mapping;
mod midi;
mod sample;
mod settings;
mod setup;
mod stderr;
mod tui;

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use clap::{Parser, Subcommand};
use crossterm::style::{self, Stylize};
use notify::Watcher;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "drumkit")]
#[command(about = "Low-latency TUI MIDI drum sampler for electronic drum kits")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List available MIDI input devices
    Devices,
    /// Monitor MIDI input in real-time (hit pads to see messages)
    Monitor {
        /// MIDI port number from 'drumkit devices' (e.g. --port 1). Prompts if omitted.
        #[arg(short, long)]
        port: Option<usize>,
    },
    /// List available audio output devices
    AudioDevices,
    /// Play a WAV file through an audio output device
    TestSound {
        /// Path to a WAV file
        #[arg(short, long)]
        file: PathBuf,
        /// Audio device index from 'drumkit audio-devices'. Uses default if omitted.
        #[arg(short, long)]
        device: Option<usize>,
    },
    /// Trigger a sample from a MIDI note (hit a pad → hear a sound)
    TestTrigger {
        /// Path to a WAV file to play on trigger
        #[arg(short, long)]
        file: PathBuf,
        /// MIDI note number to trigger on (e.g. 36 for kick)
        #[arg(short, long)]
        note: u8,
        /// MIDI port number from 'drumkit devices'
        #[arg(short, long)]
        port: Option<usize>,
        /// Audio device index from 'drumkit audio-devices'. Uses default if omitted.
        #[arg(short, long)]
        device: Option<usize>,
    },
    /// Play a full drum kit — load samples from a folder, trigger by MIDI note.
    ///
    /// Kits are discovered from ~/.local/share/drumkit/kits/ and ./kits/.
    /// MIDI note mappings are loaded from ~/.local/share/drumkit/mappings/.
    Play {
        /// Path to kit directory containing WAV files (e.g. 36.wav, 38.wav).
        /// If omitted, an interactive picker lets you choose from discovered kits.
        #[arg(short, long)]
        kit: Option<PathBuf>,
        /// MIDI port number from 'drumkit devices'. Prompts if omitted.
        #[arg(short, long)]
        port: Option<usize>,
        /// Audio device index from 'drumkit audio-devices'. Uses default if omitted.
        #[arg(short, long)]
        device: Option<usize>,
        /// Extra directories to search for kits (can be repeated)
        #[arg(long = "kits-dir", value_name = "DIR")]
        kits_dirs: Vec<PathBuf>,
    },
}

fn cmd_devices() -> Result<()> {
    let devices = midi::list_devices()?;

    if devices.is_empty() {
        println!("No MIDI input devices found.");
        println!();
        println!("Tips:");
        println!("  - Connect your drum module via USB");
        println!("  - Check: amidi -l");
        println!("  - Check: aconnect -l");
        return Ok(());
    }

    println!("MIDI input devices:");
    println!();
    for device in &devices {
        println!("  [{}] {}", device.port_index, device.name);
    }
    println!();
    println!("Use: drumkit monitor --port <number>");
    println!("  e.g. drumkit monitor --port 1");

    Ok(())
}

fn select_port(devices: &[midi::MidiDevice]) -> Result<usize> {
    println!("MIDI input devices:");
    println!();
    for device in devices {
        println!("  [{}] {}", device.port_index, device.name);
    }
    println!();
    print!("Select port: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let port: usize = input
        .trim()
        .parse()
        .context("Invalid port number")?;

    if port >= devices.len() {
        anyhow::bail!("Port index {} out of range (0-{})", port, devices.len() - 1);
    }

    Ok(port)
}

fn select_audio_device(devices: &[audio::AudioDevice]) -> Result<usize> {
    println!("Audio output devices:");
    println!();
    for device in devices {
        println!("  [{}] {}", device.index, device.name);
    }
    println!();
    print!("Select audio device: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let idx: usize = input
        .trim()
        .parse()
        .context("Invalid device number")?;

    if idx >= devices.len() {
        anyhow::bail!("Device index {} out of range (0-{})", idx, devices.len() - 1);
    }

    Ok(idx)
}

/// Resolve audio device index: use provided value, or prompt if omitted.
fn resolve_audio_device(device: Option<usize>) -> Result<Option<usize>> {
    match device {
        Some(d) => Ok(Some(d)),
        None => {
            let devices = audio::list_output_devices()?;
            if devices.is_empty() {
                anyhow::bail!("No audio output devices found.");
            }
            let idx = select_audio_device(&devices)?;
            Ok(Some(idx))
        }
    }
}

fn cmd_monitor(port: Option<usize>) -> Result<()> {
    let devices = midi::list_devices()?;

    if devices.is_empty() {
        println!("No MIDI input devices found.");
        println!();
        println!("Connect your drum module via USB and try again.");
        return Ok(());
    }

    let port_index = match port {
        Some(p) => {
            if p >= devices.len() {
                anyhow::bail!(
                    "Port index {} out of range (0-{})",
                    p,
                    devices.len() - 1
                );
            }
            p
        }
        None => select_port(&devices)?,
    };

    let device_name = &devices[port_index].name;
    println!();
    println!(
        "{}",
        format!("Connected to: {}", device_name)
            .with(style::Color::Green)
    );
    println!(
        "{}",
        "Hit your pads! Press Ctrl+C to quit."
            .with(style::Color::DarkGrey)
    );
    println!();

    let (tx, rx) = mpsc::channel();
    // _connection must stay alive — dropping it disconnects MIDI
    let _connection = midi::connect(port_index, tx)?;

    loop {
        match rx.recv() {
            Ok(msg) => {
                // Silently skip MIDI clock/timing messages
                if matches!(msg.message, midi::MidiMessage::SystemRealtime { .. }) {
                    continue;
                }
                let line = format!("{}", msg.message);
                match msg.message {
                    midi::MidiMessage::NoteOn { .. } => {
                        println!("{}", line.with(style::Color::Cyan));
                    }
                    midi::MidiMessage::NoteOff { .. } => {
                        println!("{}", line.with(style::Color::DarkGrey));
                    }
                    midi::MidiMessage::PolyAftertouch { .. } => {
                        println!("{}", line.with(style::Color::Yellow));
                    }
                    midi::MidiMessage::ControlChange { .. } => {
                        println!("{}", line.with(style::Color::Magenta));
                    }
                    _ => {
                        println!("{}", line.with(style::Color::DarkGrey));
                    }
                }
            }
            Err(_) => {
                println!("MIDI connection closed.");
                break;
            }
        }
    }

    Ok(())
}

fn cmd_audio_devices() -> Result<()> {
    let devices = audio::list_output_devices()?;

    if devices.is_empty() {
        println!("No audio output devices found.");
        return Ok(());
    }

    println!("Audio output devices:");
    println!();
    for device in &devices {
        println!("  [{}] {}", device.index, device.name);
    }
    println!();
    println!("Use: drumkit test-sound --file <path> --device <number>");

    Ok(())
}

fn cmd_test_sound(file: PathBuf, device: Option<usize>) -> Result<()> {
    let device = resolve_audio_device(device)?;
    let data = sample::load_audio(&file)?;
    println!(
        "Loaded: {} ({} Hz, {} ch, {:.2}s)",
        file.display(),
        data.sample_rate,
        data.channels,
        data.samples.len() as f64 / (data.sample_rate as f64 * data.channels as f64),
    );

    let samples = Arc::new(data.samples);
    audio::play_sample(device, samples, data.sample_rate, data.channels)?;

    println!("Done.");
    Ok(())
}

fn cmd_test_trigger(file: PathBuf, note: u8, port: Option<usize>, device: Option<usize>) -> Result<()> {
    let device = resolve_audio_device(device)?;
    let data = sample::load_audio(&file)?;
    let duration_s = data.samples.len() as f64 / (data.sample_rate as f64 * data.channels as f64);
    println!(
        "Loaded: {} ({} Hz, {} ch, {:.2}s)",
        file.display(),
        data.sample_rate,
        data.channels,
        duration_s,
    );

    let samples = Arc::new(data.samples);

    // Set up rtrb ring buffer for MIDI→audio communication
    let (mut producer, consumer) = rtrb::RingBuffer::new(64);

    // Start persistent audio output stream
    let _stream = audio::run_output_stream(device, consumer, data.sample_rate, data.channels)?;

    // Resolve MIDI port
    let devices = midi::list_devices()?;
    if devices.is_empty() {
        anyhow::bail!("No MIDI input devices found. Connect your drum module via USB.");
    }

    let port_index = match port {
        Some(p) => {
            if p >= devices.len() {
                anyhow::bail!(
                    "Port index {} out of range (0-{})",
                    p,
                    devices.len() - 1
                );
            }
            p
        }
        None => select_port(&devices)?,
    };

    let target_note = note;
    let trigger_samples = Arc::clone(&samples);

    // Connect MIDI with a raw callback — no allocation in the hot path
    let _connection = midi::connect_callback(port_index, move |_timestamp, data| {
        if data.len() == 3 {
            let status = data[0] & 0xF0;
            let msg_note = data[1];
            let velocity = data[2];

            // Note-on with velocity > 0 matching our target note
            if status == 0x90 && velocity > 0 && msg_note == target_note {
                let gain = velocity as f32 / 127.0;
                let _ = producer.push(audio::AudioCommand::Trigger {
                    samples: Arc::clone(&trigger_samples),
                    gain,
                    note: target_note,
                });
            }
        }
    })?;

    let gm_mapping = mapping::default_mapping();
    let drum = gm_mapping.drum_name(note);
    println!();
    println!(
        "{}",
        format!("Listening on: {}", devices[port_index].name)
            .with(style::Color::Green)
    );
    println!(
        "{}",
        format!("Trigger note: {} ({})", note, drum)
            .with(style::Color::Cyan)
    );
    println!(
        "{}",
        "Hit the pad! Press Ctrl+C to quit."
            .with(style::Color::DarkGrey)
    );

    // Park the main thread until Ctrl+C
    std::thread::park();

    Ok(())
}

fn cmd_play(kit: Option<PathBuf>, port: Option<usize>, device: Option<usize>, kits_dirs: Vec<PathBuf>) -> Result<()> {
    // Load persisted settings and merge extra directories from settings + CLI
    let saved = settings::load_settings();
    let mut all_kit_dirs = saved.extra_kit_dirs.clone();
    for d in &kits_dirs {
        if !all_kit_dirs.contains(d) {
            all_kit_dirs.push(d.clone());
        }
    }
    let extra_mapping_dirs = saved.extra_mapping_dirs.clone();

    // If all three are provided, go straight to play
    if let (Some(kit_path), Some(port_idx), Some(dev_idx)) = (&kit, port, device) {
        // Save explicit CLI choices so they're remembered next time
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
        });
        return cmd_play_direct(kit_path.clone(), port_idx, dev_idx, all_kit_dirs, extra_mapping_dirs);
    }

    // Run the interactive setup TUI for any missing values
    match setup::run_setup(kit, device, port, &all_kit_dirs)? {
        setup::SetupResult::Selected {
            kit_path,
            audio_device,
            audio_device_name,
            midi_port,
            midi_device_name,
        } => {
            // Save chosen settings for next launch (names already known from setup)
            let _ = settings::save_settings(&settings::Settings {
                kit_path: Some(kit_path.clone()),
                audio_device: Some(audio_device_name),
                midi_device: Some(midi_device_name),
                extra_kit_dirs: all_kit_dirs.clone(),
                extra_mapping_dirs: extra_mapping_dirs.clone(),
            });
            cmd_play_direct(kit_path, midi_port, audio_device, all_kit_dirs, extra_mapping_dirs)
        }
        setup::SetupResult::Cancelled => Ok(()),
    }
}

fn cmd_play_direct(kit_path: PathBuf, port_index: usize, audio_device: usize, kits_dirs: Vec<PathBuf>, extra_mapping_dirs: Vec<PathBuf>) -> Result<()> {
    // Enter TUI immediately so the user sees a loading screen with progress
    let mut terminal = tui::init_terminal()?;
    let kit_name_display = kit_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "kit".to_string());

    // Load kit on a background thread with progress tracking
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

    match cmd_play_direct_inner(terminal, kit_path, loaded_kit, port_index, audio_device, kits_dirs, extra_mapping_dirs) {
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
) -> Result<()> {
    // Start capturing stderr before any device enumeration (ALSA noise)
    let capture = stderr::StderrCapture::start();

    // Load kit-specific mapping if available, otherwise General MIDI
    let initial_mapping = mapping::load_kit_mapping(&kit_path)
        .unwrap_or_else(mapping::default_mapping);
    let shared_mapping = Arc::new(ArcSwap::from_pointee(initial_mapping));

    // Build kit summary for the TUI log viewer
    let kit_summary = kit::summary_lines(&loaded_kit, &shared_mapping.load());

    // Pre-compute fade durations in frames
    let choke_fade = (loaded_kit.sample_rate as f64 * 0.068) as usize; // 68ms hi-hat choke
    let aftertouch_fade = (loaded_kit.sample_rate as f64 * 0.085) as usize; // 85ms cymbal grab

    // Set up rtrb ring buffer for MIDI→audio communication (128 to handle choke bursts)
    let (producer, consumer) = rtrb::RingBuffer::new(128);

    // Wrap producer in Arc<Mutex<Option>> so it can be swapped during audio device switches
    let shared_producer = Arc::new(Mutex::new(Some(producer)));

    // Start persistent audio output stream
    let stream = audio::run_output_stream(Some(audio_device), consumer, loaded_kit.sample_rate, loaded_kit.channels)?;

    // Look up MIDI device name for TUI display
    let midi_device_name = midi::list_devices()?
        .into_iter()
        .find(|d| d.port_index == port_index)
        .map(|d| d.name)
        .unwrap_or_else(|| format!("MIDI port {}", port_index));

    // Wrap kit notes in ArcSwap for lock-free hot-reload
    let shared_notes = Arc::new(ArcSwap::from_pointee(loaded_kit.notes));

    // Unified TUI event channel
    let (tui_tx, tui_rx) = mpsc::channel::<tui::TuiEvent>();

    // Connect MIDI using the shared producer
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

    // Set up filesystem watcher for hot-reload
    let (watch_tx, watch_rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        if let Ok(_event) = res {
            let _ = watch_tx.send(());
        }
    })?;
    watcher.watch(kit_path.as_ref(), notify::RecursiveMode::NonRecursive)?;

    let stream_sample_rate = loaded_kit.sample_rate;
    let stream_channels = loaded_kit.channels;

    // Shared kit path so the debounce thread always reads the current kit
    let shared_kit_path = Arc::new(ArcSwap::from_pointee(kit_path.clone()));
    // Flag to suppress spurious reloads after kit switch
    let suppress_reload = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Spawn debounce thread for hot-reload
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
                        // Check suppress flag (set during kit switch to avoid spurious reload)
                        if debounce_suppress.swap(false, std::sync::atomic::Ordering::Relaxed) {
                            last_event = None;
                            continue;
                        }
                        // Quiet period passed — reload now
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
                                    // Reload kit mapping if mapping.toml changed
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

    // Build initial log: kit summary + any captured stderr
    let mut initial_log = kit_summary;
    if let Some(ref cap) = capture {
        cap.drain_into(&mut initial_log);
    }

    // Extract stderr receiver from capture (keep saved_fd for restore after TUI)
    let stderr_rx = capture.as_ref().map(|_| {
        // We pass the whole capture's rx through PlayResources;
        // the TUI will drain it each tick
    });
    let _ = stderr_rx; // placeholder — we pass capture directly

    // Build TUI state and run
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
    };

    tui::run(terminal, tui_rx, state, resources)
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Devices => cmd_devices(),
        Commands::Monitor { port } => cmd_monitor(port),
        Commands::AudioDevices => cmd_audio_devices(),
        Commands::TestSound { file, device } => cmd_test_sound(file, device),
        Commands::TestTrigger { file, note, port, device } => {
            cmd_test_trigger(file, note, port, device)
        }
        Commands::Play { kit, port, device, kits_dirs } => cmd_play(kit, port, device, kits_dirs),
    }
}
