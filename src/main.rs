mod audio;
mod kit;
mod midi;
mod sample;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use crossterm::style::{self, Stylize};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

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
    /// Play a full drum kit — load samples from a folder, trigger by MIDI note
    Play {
        /// Path to kit directory containing WAV files (e.g. 36.wav, 38.wav)
        #[arg(short, long)]
        kit: PathBuf,
        /// MIDI port number from 'drumkit devices'. Prompts if omitted.
        #[arg(short, long)]
        port: Option<usize>,
        /// Audio device index from 'drumkit audio-devices'. Uses default if omitted.
        #[arg(short, long)]
        device: Option<usize>,
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
    let data = sample::load_wav(&file)?;
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
    let data = sample::load_wav(&file)?;
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
                });
            }
        }
    })?;

    let drum = midi::drum_name(note);
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

fn cmd_play(kit_path: PathBuf, port: Option<usize>, device: Option<usize>) -> Result<()> {
    let device = resolve_audio_device(device)?;
    let loaded_kit = kit::load_kit(&kit_path)?;

    // Set up rtrb ring buffer for MIDI→audio communication
    let (mut producer, consumer) = rtrb::RingBuffer::new(64);

    // Start persistent audio output stream
    let _stream = audio::run_output_stream(device, consumer, loaded_kit.sample_rate, loaded_kit.channels)?;

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

    let kit_notes = loaded_kit.notes.clone();

    // Connect MIDI with a raw callback — no allocation in the hot path
    let _connection = midi::connect_callback(port_index, move |_timestamp, data| {
        if data.len() == 3 {
            let status = data[0] & 0xF0;
            let note = data[1];
            let velocity = data[2];

            // Note-on with velocity > 0
            if status == 0x90 && velocity > 0 {
                if let Some(group) = kit_notes.get(&note) {
                    if let Some(samples) = group.select(velocity) {
                        let gain = velocity as f32 / 127.0;
                        let _ = producer.push(audio::AudioCommand::Trigger {
                            samples: Arc::clone(samples),
                            gain,
                        });
                    }
                }
            }
        }
    })?;

    println!();
    println!(
        "{}",
        format!("Listening on: {}", devices[port_index].name)
            .with(style::Color::Green)
    );
    println!(
        "{}",
        "Hit your pads! Press Ctrl+C to quit."
            .with(style::Color::DarkGrey)
    );

    // Park the main thread until Ctrl+C
    std::thread::park();

    Ok(())
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
        Commands::Play { kit, port, device } => cmd_play(kit, port, device),
    }
}
