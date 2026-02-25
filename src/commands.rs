use anyhow::{Context, Result};
use crossterm::style::{self, Stylize};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

use crate::{audio, mapping, midi, sample};

pub fn cmd_devices() -> Result<()> {
    let devices = midi::list_devices()?;

    if devices.is_empty() {
        println!("No MIDI input devices found.");
        println!();
        println!("Tips:");
        println!("  - Connect your drum module via USB");
        #[cfg(target_os = "linux")]
        {
            println!("  - Check: amidi -l");
            println!("  - Check: aconnect -l");
        }
        #[cfg(target_os = "macos")]
        {
            println!("  - Open Audio MIDI Setup.app to verify your device is recognized");
        }
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

pub fn select_port(devices: &[midi::MidiDevice]) -> Result<usize> {
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

pub fn select_audio_device(devices: &[audio::AudioDevice]) -> Result<usize> {
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
pub fn resolve_audio_device(device: Option<usize>) -> Result<Option<usize>> {
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

pub fn cmd_monitor(port: Option<usize>) -> Result<()> {
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
    let _connection = midi::connect(port_index, tx)?;

    loop {
        match rx.recv() {
            Ok(msg) => {
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

pub fn cmd_audio_devices() -> Result<()> {
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

pub fn cmd_test_sound(file: PathBuf, device: Option<usize>) -> Result<()> {
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

pub fn cmd_test_trigger(file: PathBuf, note: u8, port: Option<usize>, device: Option<usize>) -> Result<()> {
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

    let (mut producer, consumer) = rtrb::RingBuffer::new(64);

    let _stream = audio::run_output_stream(device, consumer, data.sample_rate, data.channels)?;

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

    let _connection = midi::connect_callback(port_index, move |_timestamp, data| {
        if data.len() == 3 {
            let status = data[0] & 0xF0;
            let msg_note = data[1];
            let velocity = data[2];

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

    std::thread::park();

    Ok(())
}
