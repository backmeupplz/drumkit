use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use crate::audio::AudioCommand;
use crate::lesson::LessonNote;

/// Commands sent to the scheduler thread.
pub enum SchedulerCommand {
    /// Play a segment: metronome clicks + optional reference notes.
    PlaySegment {
        notes: Vec<LessonNote>,
        bpm: f64,
        beats_per_bar: u8,
        total_beats: f64,
        play_metronome: bool,
        play_reference: bool,
        count_in_bars: u8,
    },
    /// Stop current playback.
    Stop,
    /// Shut down the scheduler thread.
    Shutdown,
}

/// Events sent from the scheduler back to the TUI.
#[derive(Debug)]
pub enum SchedulerEvent {
    /// A beat tick occurred (for playhead animation).
    BeatTick { beat: f64 },
    /// Count-in beat.
    CountInBeat { beat: u8, total: u8 },
    /// Segment playback finished (ready for scoring).
    SegmentComplete,
}

/// Scheduler state for a playing segment.
struct PlaybackState {
    notes: Vec<LessonNote>,
    bpm: f64,
    beats_per_bar: u8,
    total_beats: f64,
    play_metronome: bool,
    play_reference: bool,
    count_in_bars: u8,
    downbeat_click: Arc<Vec<f32>>,
    offbeat_click: Arc<Vec<f32>>,
    /// Pre-loaded kit samples for reference playback, keyed by note number.
    kit_samples: std::collections::HashMap<u8, Arc<Vec<f32>>>,
}

/// Spawn the scheduler thread. Returns the command sender.
///
/// The scheduler uses the existing audio ring buffer to trigger metronome clicks
/// and reference notes with precise timing.
pub fn spawn_scheduler(
    producer: Arc<Mutex<Option<rtrb::Producer<AudioCommand>>>>,
    event_tx: mpsc::Sender<SchedulerEvent>,
    downbeat_click: Arc<Vec<f32>>,
    offbeat_click: Arc<Vec<f32>>,
) -> mpsc::Sender<SchedulerCommand> {
    let (cmd_tx, cmd_rx) = mpsc::channel();

    std::thread::Builder::new()
        .name("drumkit-scheduler".into())
        .spawn(move || {
            scheduler_loop(cmd_rx, producer, event_tx, downbeat_click, offbeat_click);
        })
        .expect("Failed to spawn scheduler thread");

    cmd_tx
}

fn scheduler_loop(
    cmd_rx: mpsc::Receiver<SchedulerCommand>,
    producer: Arc<Mutex<Option<rtrb::Producer<AudioCommand>>>>,
    event_tx: mpsc::Sender<SchedulerEvent>,
    downbeat_click: Arc<Vec<f32>>,
    offbeat_click: Arc<Vec<f32>>,
) {
    loop {
        // Wait for next command
        let cmd = match cmd_rx.recv() {
            Ok(cmd) => cmd,
            Err(_) => return, // channel closed
        };

        match cmd {
            SchedulerCommand::Shutdown => return,
            SchedulerCommand::Stop => continue,
            SchedulerCommand::PlaySegment {
                notes,
                bpm,
                beats_per_bar,
                total_beats,
                play_metronome,
                play_reference,
                count_in_bars,
            } => {
                let state = PlaybackState {
                    notes,
                    bpm,
                    beats_per_bar,
                    total_beats,
                    play_metronome,
                    play_reference,
                    count_in_bars,
                    downbeat_click: Arc::clone(&downbeat_click),
                    offbeat_click: Arc::clone(&offbeat_click),
                    kit_samples: std::collections::HashMap::new(),
                };
                play_segment(&state, &producer, &event_tx, &cmd_rx);
            }
        }
    }
}

/// Play a segment with metronome and/or reference, checking for stop commands.
fn play_segment(
    state: &PlaybackState,
    producer: &Arc<Mutex<Option<rtrb::Producer<AudioCommand>>>>,
    event_tx: &mpsc::Sender<SchedulerEvent>,
    cmd_rx: &mpsc::Receiver<SchedulerCommand>,
) {
    let beat_duration = Duration::from_secs_f64(60.0 / state.bpm);
    let count_in_beats = state.count_in_bars as f64 * state.beats_per_bar as f64;
    let total_count_in = count_in_beats as u8;

    // --- Count-in phase ---
    if state.count_in_bars > 0 && state.play_metronome {
        let start = Instant::now();
        for beat_idx in 0..total_count_in {
            let target = start + beat_duration * beat_idx as u32;
            if !sleep_until_or_stop(target, cmd_rx) {
                return;
            }

            // Trigger metronome click
            let is_downbeat = (beat_idx % state.beats_per_bar) == 0;
            let click = if is_downbeat {
                &state.downbeat_click
            } else {
                &state.offbeat_click
            };
            trigger_audio(producer, Arc::clone(click), 0.6, 0);

            let _ = event_tx.send(SchedulerEvent::CountInBeat {
                beat: beat_idx + 1,
                total: total_count_in,
            });
        }
    }

    // --- Segment playback phase ---
    let segment_start = Instant::now();
    let total_beats_int = state.total_beats.ceil() as u32;

    // Pre-compute beat events and note events into a single timeline
    let mut next_beat: u32 = 0;
    let mut next_note_idx: usize = 0;

    // Sort notes by beat position (should already be sorted)
    let mut sorted_notes = state.notes.clone();
    sorted_notes.sort_by(|a, b| a.beat_position.partial_cmp(&b.beat_position).unwrap());

    loop {
        // Determine next event time
        let next_beat_time = if next_beat < total_beats_int {
            Some(segment_start + beat_duration * next_beat)
        } else {
            None
        };

        let next_note_time = if next_note_idx < sorted_notes.len() {
            let note = &sorted_notes[next_note_idx];
            // Note beat position is relative to the segment start
            let note_offset = Duration::from_secs_f64(
                (note.beat_position - state.notes.first().map(|n| n.beat_position).unwrap_or(0.0))
                    * 60.0
                    / state.bpm,
            );
            Some(segment_start + note_offset)
        } else {
            None
        };

        // If both done, we're finished
        if next_beat_time.is_none() && next_note_time.is_none() {
            break;
        }

        // Pick whichever comes first
        let (do_beat, do_note) = match (next_beat_time, next_note_time) {
            (Some(bt), Some(nt)) => {
                if bt <= nt {
                    (true, nt.duration_since(segment_start) <= bt.duration_since(segment_start) + Duration::from_millis(1))
                } else {
                    (bt.duration_since(segment_start) <= nt.duration_since(segment_start) + Duration::from_millis(1), true)
                }
            }
            (Some(_), None) => (true, false),
            (None, Some(_)) => (false, true),
            (None, None) => break,
        };

        let target = match (next_beat_time, next_note_time) {
            (Some(bt), Some(nt)) => bt.min(nt),
            (Some(bt), None) => bt,
            (None, Some(nt)) => nt,
            _ => break,
        };

        if !sleep_until_or_stop(target, cmd_rx) {
            return;
        }

        if do_beat && next_beat < total_beats_int {
            if state.play_metronome {
                let is_downbeat = (next_beat % state.beats_per_bar as u32) == 0;
                let click = if is_downbeat {
                    &state.downbeat_click
                } else {
                    &state.offbeat_click
                };
                trigger_audio(producer, Arc::clone(click), 0.5, 0);
            }

            let _ = event_tx.send(SchedulerEvent::BeatTick {
                beat: next_beat as f64,
            });
            next_beat += 1;
        }

        if do_note && next_note_idx < sorted_notes.len() {
            if state.play_reference {
                let note = &sorted_notes[next_note_idx];
                // Reference playback uses the kit samples if available
                if let Some(sample) = state.kit_samples.get(&note.note) {
                    let gain = note.velocity as f32 / 127.0;
                    trigger_audio(producer, Arc::clone(sample), gain, note.note);
                }
            }
            next_note_idx += 1;
        }
    }

    // Wait for the final beat duration to pass
    let end_target = segment_start + beat_duration * total_beats_int;
    if !sleep_until_or_stop(end_target, cmd_rx) {
        return;
    }

    let _ = event_tx.send(SchedulerEvent::SegmentComplete);
}

/// Push an audio trigger through the shared producer.
fn trigger_audio(
    producer: &Arc<Mutex<Option<rtrb::Producer<AudioCommand>>>>,
    samples: Arc<Vec<f32>>,
    gain: f32,
    note: u8,
) {
    if let Ok(mut guard) = producer.lock() {
        if let Some(ref mut prod) = *guard {
            // Best-effort: don't block if ring buffer is full
            let _ = prod.push(AudioCommand::Trigger {
                samples,
                gain,
                note,
            });
        }
    }
}

/// Sleep until target time, checking for stop commands. Returns false if stopped.
fn sleep_until_or_stop(target: Instant, cmd_rx: &mpsc::Receiver<SchedulerCommand>) -> bool {
    loop {
        let now = Instant::now();
        if now >= target {
            return true;
        }

        let remaining = target - now;

        // If more than 5ms away, sleep in chunks and check for commands
        if remaining > Duration::from_millis(5) {
            let sleep_time = remaining - Duration::from_millis(2);
            match cmd_rx.recv_timeout(sleep_time.min(Duration::from_millis(50))) {
                Ok(SchedulerCommand::Stop | SchedulerCommand::Shutdown) => return false,
                Ok(_) => {} // ignore other commands during playback
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return false,
            }
        } else {
            // Spin-wait for sub-ms accuracy
            std::hint::spin_loop();
        }
    }
}

