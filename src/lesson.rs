use anyhow::{Context, Result};
use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};
use std::path::{Path, PathBuf};

/// A single note event extracted from a MIDI file (channel 10 / drums).
#[derive(Debug, Clone)]
pub struct LessonNote {
    /// Absolute time in microseconds from the start of the file.
    pub time_us: u64,
    /// MIDI note number.
    pub note: u8,
    /// MIDI velocity (1-127).
    pub velocity: u8,
    /// Beat position (floating point beats from start).
    pub beat_position: f64,
}

/// A segment of a lesson (typically 1-2 bars).
#[derive(Debug, Clone)]
pub struct Segment {
    pub index: usize,
    pub start_beat: f64,
    pub end_beat: f64,
    pub notes: Vec<LessonNote>,
    pub bar_start: usize,
    pub bar_count: usize,
}

/// A tempo change event in the MIDI file.
#[derive(Debug, Clone)]
struct TempoChange {
    tick: u64,
    tempo_us_per_beat: u64,
}

/// A parsed lesson from a MIDI file.
#[derive(Debug, Clone)]
pub struct Lesson {
    pub name: String,
    pub path: PathBuf,
    pub ticks_per_beat: u16,
    pub beats_per_bar: u8,
    pub beat_unit: u8,
    pub original_bpm: f64,
    pub segments: Vec<Segment>,
    pub all_notes: Vec<LessonNote>,
    /// Unique MIDI note numbers used in this lesson (sorted).
    pub unique_notes: Vec<u8>,
}

/// A discovered lesson file on disk.
#[derive(Debug, Clone)]
pub struct DiscoveredLesson {
    pub name: String,
    pub path: PathBuf,
}

/// Discover lesson .mid files from the standard lessons directory.
pub fn discover_lessons(extra_dirs: &[PathBuf]) -> Vec<DiscoveredLesson> {
    let mut lessons = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();

    // Standard dir: ~/.local/share/drumkit/lessons/
    let data_home = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".local/share")
        });
    dirs.push(data_home.join("drumkit/lessons"));

    for d in extra_dirs {
        if !dirs.contains(d) {
            dirs.push(d.clone());
        }
    }

    for dir in &dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("mid") {
                    let name = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    lessons.push(DiscoveredLesson {
                        name,
                        path,
                    });
                }
            }
        }
    }

    lessons.sort_by(|a, b| a.name.cmp(&b.name));
    lessons
}

/// Parse a MIDI file into a Lesson.
pub fn parse_lesson(path: &Path) -> Result<Lesson> {
    let data = std::fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let smf = Smf::parse(&data).with_context(|| format!("Failed to parse MIDI: {}", path.display()))?;

    let ticks_per_beat = match smf.header.timing {
        Timing::Metrical(tpb) => tpb.as_int(),
        Timing::Timecode(..) => anyhow::bail!("SMPTE timecode MIDI files are not supported"),
    };

    // Collect tempo changes and time signature from all tracks
    let mut tempo_changes: Vec<TempoChange> = Vec::new();
    let mut beats_per_bar: u8 = 4;
    let mut beat_unit: u8 = 4;
    let mut drum_events: Vec<(u64, u8, u8)> = Vec::new(); // (tick, note, velocity)

    for track in &smf.tracks {
        let mut abs_tick: u64 = 0;
        for event in track {
            abs_tick += event.delta.as_int() as u64;
            match event.kind {
                TrackEventKind::Meta(MetaMessage::Tempo(t)) => {
                    tempo_changes.push(TempoChange {
                        tick: abs_tick,
                        tempo_us_per_beat: t.as_int() as u64,
                    });
                }
                TrackEventKind::Meta(MetaMessage::TimeSignature(num, denom, _, _)) => {
                    beats_per_bar = num;
                    beat_unit = 1 << denom;
                }
                TrackEventKind::Midi { channel, message } => {
                    // Channel 10 (index 9) is drums in General MIDI
                    if channel.as_int() == 9 {
                        if let MidiMessage::NoteOn { key, vel } = message {
                            if vel.as_int() > 0 {
                                drum_events.push((abs_tick, key.as_int(), vel.as_int()));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Default tempo if none found
    if tempo_changes.is_empty() {
        tempo_changes.push(TempoChange {
            tick: 0,
            tempo_us_per_beat: 500_000, // 120 BPM
        });
    }
    tempo_changes.sort_by_key(|t| t.tick);

    let original_bpm = 60_000_000.0 / tempo_changes[0].tempo_us_per_beat as f64;

    // Sort drum events by tick
    drum_events.sort_by_key(|&(tick, _, _)| tick);

    if drum_events.is_empty() {
        anyhow::bail!("No drum notes found on channel 10 in {}", path.display());
    }

    // Convert ticks to microseconds and beat positions
    let all_notes: Vec<LessonNote> = drum_events
        .iter()
        .map(|&(tick, note, velocity)| {
            let time_us = tick_to_us(tick, &tempo_changes, ticks_per_beat);
            let beat_position = tick as f64 / ticks_per_beat as f64;
            LessonNote {
                time_us,
                note,
                velocity,
                beat_position,
            }
        })
        .collect();

    // Unique notes
    let mut unique_notes: Vec<u8> = all_notes.iter().map(|n| n.note).collect();
    unique_notes.sort();
    unique_notes.dedup();

    // Segment by bars (each segment = 2 bars for short lessons, 1 bar for long)
    let total_beats = all_notes.last().map(|n| n.beat_position).unwrap_or(0.0);
    let total_bars = (total_beats / beats_per_bar as f64).ceil() as usize;
    let bars_per_segment = if total_bars <= 4 { 1 } else { 2 };

    let mut segments = Vec::new();
    let num_segments = (total_bars + bars_per_segment - 1) / bars_per_segment;

    for seg_idx in 0..num_segments {
        let bar_start = seg_idx * bars_per_segment;
        let bar_end = ((seg_idx + 1) * bars_per_segment).min(total_bars);
        let start_beat = (bar_start as f64) * beats_per_bar as f64;
        let end_beat = (bar_end as f64) * beats_per_bar as f64;

        let notes: Vec<LessonNote> = all_notes
            .iter()
            .filter(|n| n.beat_position >= start_beat && n.beat_position < end_beat)
            .cloned()
            .collect();

        if !notes.is_empty() {
            segments.push(Segment {
                index: segments.len(),
                start_beat,
                end_beat,
                notes,
                bar_start,
                bar_count: bar_end - bar_start,
            });
        }
    }

    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_string());

    Ok(Lesson {
        name,
        path: path.to_path_buf(),
        ticks_per_beat,
        beats_per_bar,
        beat_unit,
        original_bpm,
        segments,
        all_notes,
        unique_notes,
    })
}

/// Convert a tick position to microseconds using the tempo map.
fn tick_to_us(tick: u64, tempo_changes: &[TempoChange], ticks_per_beat: u16) -> u64 {
    let mut us: u64 = 0;
    let mut last_tick: u64 = 0;
    let mut current_tempo: u64 = 500_000; // default 120 BPM

    for tc in tempo_changes {
        if tc.tick >= tick {
            break;
        }
        let delta_ticks = tc.tick - last_tick;
        us += delta_ticks * current_tempo / ticks_per_beat as u64;
        last_tick = tc.tick;
        current_tempo = tc.tempo_us_per_beat;
    }

    let remaining = tick - last_tick;
    us += remaining * current_tempo / ticks_per_beat as u64;
    us
}

/// Standard lessons directory path.
pub fn lessons_dir() -> PathBuf {
    let data_home = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".local/share")
        });
    data_home.join("drumkit/lessons")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_to_us_default_tempo() {
        let tempo_changes = vec![TempoChange {
            tick: 0,
            tempo_us_per_beat: 500_000,
        }];
        // At 120 BPM, 480 ticks per beat: 1 beat = 500,000 us
        assert_eq!(tick_to_us(480, &tempo_changes, 480), 500_000);
        assert_eq!(tick_to_us(960, &tempo_changes, 480), 1_000_000);
    }

    #[test]
    fn tick_to_us_tempo_change() {
        let tempo_changes = vec![
            TempoChange {
                tick: 0,
                tempo_us_per_beat: 500_000, // 120 BPM
            },
            TempoChange {
                tick: 480,
                tempo_us_per_beat: 1_000_000, // 60 BPM
            },
        ];
        // First beat at 120 BPM: 500,000 us
        // Second beat at 60 BPM: 1,000,000 us
        assert_eq!(tick_to_us(960, &tempo_changes, 480), 1_500_000);
    }

    #[test]
    fn discover_lessons_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let lessons = discover_lessons(&[dir.path().to_path_buf()]);
        assert!(lessons.is_empty());
    }
}
