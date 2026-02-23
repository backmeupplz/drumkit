mod audio;
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

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Devices => cmd_devices(),
        Commands::Monitor { port } => cmd_monitor(port),
        Commands::AudioDevices => cmd_audio_devices(),
        Commands::TestSound { file, device } => cmd_test_sound(file, device),
    }
}
