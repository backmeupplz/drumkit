mod audio;
mod kit;
mod mapping;
mod midi;
mod sample;
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
    // If all three are provided, go straight to play
    if let (Some(kit_path), Some(port_idx), Some(dev_idx)) = (&kit, port, device) {
        return cmd_play_direct(kit_path.clone(), port_idx, dev_idx, kits_dirs);
    }

    // Run the interactive setup TUI for any missing values
    match setup::run_setup(kit, device, port, &kits_dirs)? {
        setup::SetupResult::Selected {
            kit_path,
            audio_device,
            midi_port,
        } => cmd_play_direct(kit_path, midi_port, audio_device, kits_dirs),
        setup::SetupResult::Cancelled => Ok(()),
    }
}

fn cmd_play_direct(kit_path: PathBuf, port_index: usize, audio_device: usize, kits_dirs: Vec<PathBuf>) -> Result<()> {
    // Start capturing stderr before any device enumeration (ALSA noise)
    let capture = stderr::StderrCapture::start();

    let loaded_kit = kit::load_kit(&kit_path)?;

    // Load default mapping (General MIDI)
    let shared_mapping = Arc::new(ArcSwap::from_pointee(mapping::default_mapping()));

    // Print summary before entering TUI (visible in normal screen buffer)
    kit::print_summary(&loaded_kit, &shared_mapping.load());

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

    // Spawn debounce thread for hot-reload
    let debounce_shared_notes = Arc::clone(&shared_notes);
    let debounce_kit_path = kit_path.clone();
    let debounce_tui_tx = tui_tx.clone();
    std::thread::spawn(move || {
        let debounce = Duration::from_millis(500);
        let mut last_event: Option<Instant> = None;

        loop {
            let timeout = match last_event {
                Some(t) => {
                    let elapsed = t.elapsed();
                    if elapsed >= debounce {
                        // Quiet period passed — reload now
                        match kit::load_kit(&debounce_kit_path) {
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
                                    let _ = debounce_tui_tx
                                        .send(tui::TuiEvent::KitReloaded { note_keys });
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

    // Drain initial stderr capture into log lines
    let mut initial_log = Vec::new();
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
        shared_mapping,
    };

    let result = tui::run(tui_rx, state, resources);

    result
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
