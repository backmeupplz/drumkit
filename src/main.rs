mod audio;
mod commands;
mod download;
mod kit;
mod mapping;
mod midi;
mod play;
mod sample;
mod settings;
mod setup;
mod stderr;
mod tui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

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

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Devices => commands::cmd_devices(),
        Commands::Monitor { port } => commands::cmd_monitor(port),
        Commands::AudioDevices => commands::cmd_audio_devices(),
        Commands::TestSound { file, device } => commands::cmd_test_sound(file, device),
        Commands::TestTrigger { file, note, port, device } => {
            commands::cmd_test_trigger(file, note, port, device)
        }
        Commands::Play { kit, port, device, kits_dirs } => play::cmd_play(kit, port, device, kits_dirs),
    }
}
