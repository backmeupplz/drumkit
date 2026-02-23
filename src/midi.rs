use anyhow::{Context, Result};
use midir::MidiInput;
use std::fmt;
use std::sync::mpsc;

/// MIDI message types relevant to drum kits
#[derive(Debug, Clone, PartialEq)]
pub enum MidiMessage {
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    PolyAftertouch {
        channel: u8,
        note: u8,
        pressure: u8,
    },
    ControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },
    /// MIDI system real-time messages (clock, start, stop, etc.) — filtered from display
    SystemRealtime {
        status: u8,
    },
    Other {
        data: Vec<u8>,
    },
}

impl MidiMessage {
    /// Parse raw MIDI bytes into a MidiMessage
    pub fn parse(data: &[u8]) -> Self {
        if data.is_empty() {
            return MidiMessage::Other { data: data.to_vec() };
        }

        // System real-time messages (single byte, 0xF8-0xFF)
        if data.len() == 1 && data[0] >= 0xF8 {
            return MidiMessage::SystemRealtime { status: data[0] };
        }

        let status = data[0] & 0xF0;
        let channel = (data[0] & 0x0F) + 1; // 1-indexed for display

        match (status, data.len()) {
            (0x90, 3) if data[2] > 0 => MidiMessage::NoteOn {
                channel,
                note: data[1],
                velocity: data[2],
            },
            // Note-on with velocity 0 is equivalent to note-off
            (0x90, 3) => MidiMessage::NoteOff {
                channel,
                note: data[1],
                velocity: 0,
            },
            (0x80, 3) => MidiMessage::NoteOff {
                channel,
                note: data[1],
                velocity: data[2],
            },
            (0xA0, 3) => MidiMessage::PolyAftertouch {
                channel,
                note: data[1],
                pressure: data[2],
            },
            (0xB0, 3) => MidiMessage::ControlChange {
                channel,
                controller: data[1],
                value: data[2],
            },
            _ => MidiMessage::Other { data: data.to_vec() },
        }
    }
}

/// Map MIDI note numbers to General MIDI drum names
pub fn drum_name(note: u8) -> &'static str {
    match note {
        21 => "HH Splash",
        23 => "HH Half-Open",
        35 => "Bass Drum 2",
        36 => "Kick",
        37 => "Side Stick",
        38 => "Snare",
        39 => "Hand Clap",
        40 => "Snare Rim",
        41 => "Low Floor Tom",
        42 => "Closed Hi-Hat",
        43 => "High Floor Tom",
        44 => "Pedal Hi-Hat",
        45 => "Low Tom",
        46 => "Open Hi-Hat",
        47 => "Low-Mid Tom",
        48 => "Hi-Mid Tom",
        49 => "Crash 1",
        50 => "High Tom",
        51 => "Ride",
        52 => "Chinese Cymbal",
        53 => "Ride Bell",
        54 => "Tambourine",
        55 => "Splash Cymbal",
        56 => "Cowbell",
        57 => "Crash 2",
        58 => "Vibraslap",
        59 => "Ride 2",
        _ => "Unknown",
    }
}

impl fmt::Display for MidiMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MidiMessage::NoteOn { channel, note, velocity } => {
                let name = drum_name(*note);
                let bar_len = (*velocity as usize) * 40 / 127;
                let bar: String = "█".repeat(bar_len);
                write!(
                    f,
                    "NOTE ON   ch:{ch:<2} note:{note:<3} vel:{vel:<3} {name:<16} {bar}",
                    ch = channel,
                    note = note,
                    vel = velocity,
                    name = name,
                    bar = bar,
                )
            }
            MidiMessage::NoteOff { channel, note, velocity } => {
                let name = drum_name(*note);
                write!(
                    f,
                    "NOTE OFF  ch:{ch:<2} note:{note:<3} vel:{vel:<3} {name}",
                    ch = channel,
                    note = note,
                    vel = velocity,
                    name = name,
                )
            }
            MidiMessage::PolyAftertouch { channel, note, pressure } => {
                let name = drum_name(*note);
                write!(
                    f,
                    "CHOKE     ch:{ch:<2} note:{note:<3} prs:{prs:<3} {name}",
                    ch = channel,
                    note = note,
                    prs = pressure,
                    name = name,
                )
            }
            MidiMessage::ControlChange { channel, controller, value } => {
                let cc_name = if *controller == 4 { "HH Pedal" } else { "" };
                write!(
                    f,
                    "CC        ch:{ch:<2} cc:{cc:<3}  val:{val:<3} {name}",
                    ch = channel,
                    cc = controller,
                    val = value,
                    name = cc_name,
                )
            }
            MidiMessage::SystemRealtime { status } => {
                let name = match status {
                    0xF8 => "Clock",
                    0xFA => "Start",
                    0xFB => "Continue",
                    0xFC => "Stop",
                    0xFE => "Active Sensing",
                    0xFF => "Reset",
                    _ => "Unknown",
                };
                write!(f, "SYSTEM    {name}")
            }
            MidiMessage::Other { data } => {
                write!(f, "OTHER     {:02X?}", data)
            }
        }
    }
}

/// A MIDI input device descriptor
#[derive(Debug, Clone)]
pub struct MidiDevice {
    pub name: String,
    pub port_index: usize,
}

/// List available MIDI input devices
pub fn list_devices() -> Result<Vec<MidiDevice>> {
    let midi_in = MidiInput::new("drumkit-list")
        .context("Failed to create MIDI input")?;

    let ports = midi_in.ports();
    let mut devices = Vec::new();

    for (i, port) in ports.iter().enumerate() {
        let name = midi_in.port_name(port).unwrap_or_else(|_| format!("Unknown port {}", i));
        devices.push(MidiDevice {
            name,
            port_index: i,
        });
    }

    Ok(devices)
}

/// Timestamped MIDI message from the callback
pub struct TimestampedMessage {
    pub _timestamp_us: u64,
    pub message: MidiMessage,
}

/// Connect to a MIDI input port and send parsed messages to the channel.
/// Returns the connection handle (dropping it disconnects).
pub fn connect(
    port_index: usize,
    tx: mpsc::Sender<TimestampedMessage>,
) -> Result<midir::MidiInputConnection<()>> {
    let midi_in = MidiInput::new("drumkit-monitor")
        .context("Failed to create MIDI input")?;

    let ports = midi_in.ports();
    let port = ports
        .get(port_index)
        .context(format!("MIDI port index {} not found", port_index))?;

    let port_name = midi_in.port_name(port).unwrap_or_else(|_| "Unknown".to_string());

    let connection = midi_in
        .connect(
            port,
            "drumkit-in",
            move |timestamp_us, data, _| {
                let message = MidiMessage::parse(data);
                let _ = tx.send(TimestampedMessage {
                    _timestamp_us: timestamp_us,
                    message,
                });
            },
            (),
        )
        .map_err(|e| anyhow::anyhow!("Failed to connect to MIDI port {}: {}", port_name, e))?;

    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_note_on() {
        let msg = MidiMessage::parse(&[0x99, 38, 100]);
        assert_eq!(
            msg,
            MidiMessage::NoteOn {
                channel: 10,
                note: 38,
                velocity: 100,
            }
        );
    }

    #[test]
    fn parse_note_on_velocity_zero_is_note_off() {
        let msg = MidiMessage::parse(&[0x99, 38, 0]);
        assert_eq!(
            msg,
            MidiMessage::NoteOff {
                channel: 10,
                note: 38,
                velocity: 0,
            }
        );
    }

    #[test]
    fn parse_note_off() {
        let msg = MidiMessage::parse(&[0x89, 42, 64]);
        assert_eq!(
            msg,
            MidiMessage::NoteOff {
                channel: 10,
                note: 42,
                velocity: 64,
            }
        );
    }

    #[test]
    fn parse_poly_aftertouch() {
        let msg = MidiMessage::parse(&[0xA9, 49, 127]);
        assert_eq!(
            msg,
            MidiMessage::PolyAftertouch {
                channel: 10,
                note: 49,
                pressure: 127,
            }
        );
    }

    #[test]
    fn parse_control_change() {
        let msg = MidiMessage::parse(&[0xB9, 4, 65]);
        assert_eq!(
            msg,
            MidiMessage::ControlChange {
                channel: 10,
                controller: 4,
                value: 65,
            }
        );
    }

    #[test]
    fn parse_empty_data() {
        let msg = MidiMessage::parse(&[]);
        assert_eq!(msg, MidiMessage::Other { data: vec![] });
    }

    #[test]
    fn parse_short_data() {
        let msg = MidiMessage::parse(&[0x90]);
        assert_eq!(msg, MidiMessage::Other { data: vec![0x90] });
    }

    #[test]
    fn parse_system_realtime_clock() {
        let msg = MidiMessage::parse(&[0xF8]);
        assert_eq!(msg, MidiMessage::SystemRealtime { status: 0xF8 });
    }

    #[test]
    fn parse_system_realtime_active_sensing() {
        let msg = MidiMessage::parse(&[0xFE]);
        assert_eq!(msg, MidiMessage::SystemRealtime { status: 0xFE });
    }

    #[test]
    fn parse_unknown_status() {
        let msg = MidiMessage::parse(&[0xF0, 0x7E, 0xF7]);
        assert_eq!(
            msg,
            MidiMessage::Other {
                data: vec![0xF0, 0x7E, 0xF7],
            }
        );
    }

    #[test]
    fn drum_name_known_notes() {
        assert_eq!(drum_name(36), "Kick");
        assert_eq!(drum_name(38), "Snare");
        assert_eq!(drum_name(42), "Closed Hi-Hat");
        assert_eq!(drum_name(46), "Open Hi-Hat");
        assert_eq!(drum_name(49), "Crash 1");
        assert_eq!(drum_name(51), "Ride");
    }

    #[test]
    fn drum_name_alesis_nonstandard() {
        assert_eq!(drum_name(21), "HH Splash");
        assert_eq!(drum_name(23), "HH Half-Open");
    }

    #[test]
    fn drum_name_unknown() {
        assert_eq!(drum_name(0), "Unknown");
        assert_eq!(drum_name(127), "Unknown");
    }

    #[test]
    fn display_note_on() {
        let msg = MidiMessage::NoteOn {
            channel: 10,
            note: 38,
            velocity: 100,
        };
        let s = format!("{}", msg);
        assert!(s.contains("NOTE ON"));
        assert!(s.contains("38"));
        assert!(s.contains("100"));
        assert!(s.contains("Snare"));
    }

    #[test]
    fn display_choke() {
        let msg = MidiMessage::PolyAftertouch {
            channel: 10,
            note: 49,
            pressure: 127,
        };
        let s = format!("{}", msg);
        assert!(s.contains("CHOKE"));
        assert!(s.contains("Crash 1"));
    }

    #[test]
    fn display_cc_hihat() {
        let msg = MidiMessage::ControlChange {
            channel: 10,
            controller: 4,
            value: 65,
        };
        let s = format!("{}", msg);
        assert!(s.contains("CC"));
        assert!(s.contains("HH Pedal"));
    }

    #[test]
    fn all_channels_parse_correctly() {
        for ch in 0..16u8 {
            let msg = MidiMessage::parse(&[0x90 | ch, 60, 100]);
            assert_eq!(
                msg,
                MidiMessage::NoteOn {
                    channel: ch + 1,
                    note: 60,
                    velocity: 100,
                }
            );
        }
    }
}
