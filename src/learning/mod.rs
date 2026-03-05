pub mod input;
pub mod metronome;
pub mod render;
pub mod scheduler;
pub mod scoring;

use ratatui::widgets::ListState;
use std::path::PathBuf;
use std::sync::mpsc;

use crate::lesson::{DiscoveredLesson, Lesson, Segment};
use scheduler::{SchedulerCommand, SchedulerEvent};
use scoring::{AttemptScore, AttemptTracker};

/// An entry in the file browser.
#[derive(Debug, Clone)]
pub struct BrowseEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// Phase of the learning mode state machine.
#[derive(Debug, Clone, PartialEq)]
pub enum LearningPhase {
    /// Selecting a lesson from the list.
    SelectLesson,
    /// Browsing files on disk.
    BrowseFiles,
    /// Listening to the reference playback of the current segment.
    Listening,
    /// Count-in before practicing.
    CountIn { current_beat: u8, total_beats: u8 },
    /// Actively practicing the segment — user plays along.
    Practicing,
    /// Reviewing the score after an attempt.
    ReviewScore,
    /// Exam mode: full song, no aids.
    Exam,
    /// Exam finished, showing results.
    ExamResult,
}

/// Full learning mode state.
pub struct LearningState {
    pub phase: LearningPhase,
    pub lesson: Option<Lesson>,
    pub current_segment: usize,
    pub current_bpm: f64,
    pub target_bpm: f64,
    pub metronome_enabled: bool,
    pub consecutive_passes: u32,
    pub consecutive_fails: u32,
    pub attempt_tracker: Option<AttemptTracker>,
    pub last_score: Option<AttemptScore>,
    pub available_lessons: Vec<DiscoveredLesson>,
    pub lesson_list_state: ListState,
    pub scheduler_tx: Option<mpsc::Sender<SchedulerCommand>>,
    /// Current playhead beat position (updated by scheduler events).
    pub playhead_beat: f64,
    /// Whether we're in a combined segment review.
    pub combined_segments: Vec<usize>,
    /// Completed segments (at target BPM, without metronome).
    pub completed_segments: Vec<usize>,
    /// Total attempts for current segment.
    pub total_attempts: u32,
    /// Exam score.
    pub exam_score: Option<AttemptScore>,
    /// File browser: current directory.
    pub browse_dir: PathBuf,
    /// File browser: entries in the current directory.
    pub browse_entries: Vec<BrowseEntry>,
    /// File browser: list selection state.
    pub browse_list_state: ListState,
    /// File browser: error message.
    pub browse_error: Option<String>,
}

impl LearningState {
    pub fn new(
        available_lessons: Vec<DiscoveredLesson>,
        scheduler_tx: Option<mpsc::Sender<SchedulerCommand>>,
    ) -> Self {
        let mut list_state = ListState::default();
        if !available_lessons.is_empty() {
            list_state.select(Some(0));
        }
        Self {
            phase: LearningPhase::SelectLesson,
            lesson: None,
            current_segment: 0,
            current_bpm: 60.0,
            target_bpm: 120.0,
            metronome_enabled: true,
            consecutive_passes: 0,
            consecutive_fails: 0,
            attempt_tracker: None,
            last_score: None,
            available_lessons,
            lesson_list_state: list_state,
            scheduler_tx,
            playhead_beat: 0.0,
            combined_segments: Vec::new(),
            completed_segments: Vec::new(),
            total_attempts: 0,
            exam_score: None,
            browse_dir: std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/")),
            browse_entries: Vec::new(),
            browse_list_state: ListState::default(),
            browse_error: None,
        }
    }

    /// Start learning a selected lesson.
    pub fn start_lesson(&mut self, lesson: Lesson) {
        let original_bpm = lesson.original_bpm;
        self.target_bpm = original_bpm;
        self.current_bpm = (original_bpm * 0.6).max(60.0);
        self.current_segment = 0;
        self.metronome_enabled = true;
        self.consecutive_passes = 0;
        self.consecutive_fails = 0;
        self.attempt_tracker = None;
        self.last_score = None;
        self.playhead_beat = 0.0;
        self.combined_segments = Vec::new();
        self.completed_segments = Vec::new();
        self.total_attempts = 0;
        self.exam_score = None;
        self.lesson = Some(lesson);
        self.phase = LearningPhase::Listening;
    }

    /// Get the current segment being practiced.
    pub fn current_segment_data(&self) -> Option<&Segment> {
        let lesson = self.lesson.as_ref()?;
        lesson.segments.get(self.current_segment)
    }

    /// Get all notes for the current practice range (single segment or combined).
    pub fn current_practice_notes(&self) -> Vec<crate::lesson::LessonNote> {
        let lesson = match &self.lesson {
            Some(l) => l,
            None => return Vec::new(),
        };

        if self.combined_segments.is_empty() {
            // Single segment
            match lesson.segments.get(self.current_segment) {
                Some(seg) => seg.notes.clone(),
                None => Vec::new(),
            }
        } else {
            // Combined segments
            let mut notes = Vec::new();
            for &idx in &self.combined_segments {
                if let Some(seg) = lesson.segments.get(idx) {
                    notes.extend(seg.notes.iter().cloned());
                }
            }
            notes.sort_by(|a, b| a.beat_position.partial_cmp(&b.beat_position).unwrap());
            notes
        }
    }

    /// Get the total beats for the current practice range.
    pub fn current_total_beats(&self) -> f64 {
        let lesson = match &self.lesson {
            Some(l) => l,
            None => return 0.0,
        };

        if self.combined_segments.is_empty() {
            match lesson.segments.get(self.current_segment) {
                Some(seg) => seg.end_beat - seg.start_beat,
                None => 0.0,
            }
        } else {
            let first = self.combined_segments.first().and_then(|&i| lesson.segments.get(i));
            let last = self.combined_segments.last().and_then(|&i| lesson.segments.get(i));
            match (first, last) {
                (Some(f), Some(l)) => l.end_beat - f.start_beat,
                _ => 0.0,
            }
        }
    }

    /// Start playing the current segment (reference + count-in + practice).
    pub fn start_segment_playback(&mut self) {
        let notes = self.current_practice_notes();
        let total_beats = self.current_total_beats();
        let lesson = match &self.lesson {
            Some(l) => l,
            None => return,
        };

        if let Some(ref tx) = self.scheduler_tx {
            let is_listening = self.phase == LearningPhase::Listening;
            let _ = tx.send(SchedulerCommand::PlaySegment {
                notes,
                bpm: if is_listening { self.target_bpm } else { self.current_bpm },
                beats_per_bar: lesson.beats_per_bar,
                total_beats,
                play_metronome: !is_listening && self.metronome_enabled,
                play_reference: is_listening,
                count_in_bars: if is_listening { 0 } else { 1 },
            });
        }
    }

    /// Begin a practice attempt for the current segment.
    pub fn start_practice(&mut self) {
        let notes = self.current_practice_notes();
        let segment_start_beat = notes.first().map(|n| n.beat_position).unwrap_or(0.0);

        let beat_offsets_and_notes: Vec<(f64, u8)> = notes
            .iter()
            .map(|n| (n.beat_position - segment_start_beat, n.note))
            .collect();

        let mut tracker = AttemptTracker::new(beat_offsets_and_notes, self.current_bpm);
        tracker.start();
        self.attempt_tracker = Some(tracker);
        self.total_attempts += 1;
        self.phase = LearningPhase::Practicing;
    }

    /// Record a MIDI hit during practice.
    pub fn record_hit(&mut self, note: u8) -> Option<scoring::HitResult> {
        if self.phase != LearningPhase::Practicing && self.phase != LearningPhase::Exam {
            return None;
        }
        self.attempt_tracker.as_mut().map(|t| t.record_hit(note))
    }

    /// Finish the current attempt and compute score.
    pub fn finish_attempt(&mut self) {
        if let Some(ref tracker) = self.attempt_tracker {
            let score = tracker.score();
            self.last_score = Some(score);
        }
        self.phase = LearningPhase::ReviewScore;
    }

    /// Apply adaptive logic after reviewing score. Returns the next action description.
    pub fn advance(&mut self) -> String {
        let score = match &self.last_score {
            Some(s) => s.clone(),
            None => {
                self.phase = LearningPhase::Listening;
                return "Restarting segment".to_string();
            }
        };

        if score.passed {
            self.consecutive_passes += 1;
            self.consecutive_fails = 0;

            // Check if we should increase BPM
            if self.consecutive_passes >= 3 {
                self.consecutive_passes = 0;
                let bpm_step = if self.current_bpm < self.target_bpm * 0.8 {
                    10.0
                } else {
                    5.0
                };
                self.current_bpm = (self.current_bpm + bpm_step).min(self.target_bpm);

                if self.current_bpm >= self.target_bpm {
                    if self.metronome_enabled {
                        // At target BPM: drop metronome
                        self.metronome_enabled = false;
                        self.consecutive_passes = 0;
                        self.phase = LearningPhase::Listening;
                        return format!("Target BPM reached! Metronome off. BPM: {:.0}", self.current_bpm);
                    } else {
                        // Segment complete! Advance.
                        return self.advance_segment();
                    }
                }

                self.phase = LearningPhase::Listening;
                return format!("BPM +{:.0} → {:.0}", bpm_step, self.current_bpm);
            }
        } else {
            // Failed
            self.consecutive_fails += 1;
            self.consecutive_passes = 0;

            if self.consecutive_fails >= 3 {
                self.consecutive_fails = 0;
                self.current_bpm = (self.current_bpm - 5.0).max(40.0);
                self.phase = LearningPhase::Listening;
                return format!("BPM -5 → {:.0}", self.current_bpm);
            }
        }

        self.phase = LearningPhase::Listening;
        "Retrying segment".to_string()
    }

    /// Advance to the next segment or start combining.
    fn advance_segment(&mut self) -> String {
        let lesson = match &self.lesson {
            Some(l) => l,
            None => return "No lesson".to_string(),
        };

        self.completed_segments.push(self.current_segment);

        // Check if all segments are done
        if self.completed_segments.len() >= lesson.segments.len() {
            // Start exam
            self.phase = LearningPhase::Exam;
            return "All segments complete! Starting exam...".to_string();
        }

        // Check if we should combine segments
        if self.completed_segments.len() >= 2 && self.combined_segments.is_empty() {
            // Start combining: first two segments
            self.combined_segments = self.completed_segments.clone();
            self.current_bpm = (self.target_bpm * 0.6).max(60.0);
            self.metronome_enabled = true;
            self.consecutive_passes = 0;
            self.consecutive_fails = 0;
            self.phase = LearningPhase::Listening;
            return format!(
                "Combining segments 1-{}",
                self.completed_segments.len()
            );
        }

        // Move to next segment
        self.current_segment += 1;
        self.current_bpm = (self.target_bpm * 0.6).max(60.0);
        self.metronome_enabled = true;
        self.consecutive_passes = 0;
        self.consecutive_fails = 0;
        self.combined_segments.clear();
        self.total_attempts = 0;
        self.phase = LearningPhase::Listening;
        format!(
            "Segment {}/{}",
            self.current_segment + 1,
            lesson.segments.len()
        )
    }

    /// Start exam mode: full song at original BPM, no metronome.
    pub fn start_exam(&mut self) {
        let lesson = match &self.lesson {
            Some(l) => l,
            None => return,
        };

        self.current_bpm = self.target_bpm;
        self.metronome_enabled = false;
        self.combined_segments = (0..lesson.segments.len()).collect();
        self.phase = LearningPhase::Exam;

        // Build tracker for the full song
        let notes = self.current_practice_notes();
        let segment_start_beat = notes.first().map(|n| n.beat_position).unwrap_or(0.0);
        let beat_offsets_and_notes: Vec<(f64, u8)> = notes
            .iter()
            .map(|n| (n.beat_position - segment_start_beat, n.note))
            .collect();
        let mut tracker = AttemptTracker::new(beat_offsets_and_notes, self.current_bpm);
        tracker.start();
        self.attempt_tracker = Some(tracker);

        // Start playback
        if let Some(ref tx) = self.scheduler_tx {
            let all_notes = self.current_practice_notes();
            let total_beats = self.current_total_beats();
            let _ = tx.send(SchedulerCommand::PlaySegment {
                notes: all_notes,
                bpm: self.current_bpm,
                beats_per_bar: lesson.beats_per_bar,
                total_beats,
                play_metronome: false,
                play_reference: false,
                count_in_bars: 1,
            });
        }
    }

    /// Finish exam and show results.
    pub fn finish_exam(&mut self) {
        if let Some(ref tracker) = self.attempt_tracker {
            self.exam_score = Some(tracker.score());
        }
        self.phase = LearningPhase::ExamResult;
    }

    /// Handle a scheduler event.
    pub fn handle_scheduler_event(&mut self, event: SchedulerEvent) {
        match event {
            SchedulerEvent::BeatTick { beat } => {
                self.playhead_beat = beat;
            }
            SchedulerEvent::CountInBeat { beat, total } => {
                self.phase = LearningPhase::CountIn {
                    current_beat: beat,
                    total_beats: total,
                };
            }
            SchedulerEvent::SegmentComplete => {
                match self.phase {
                    LearningPhase::Listening => {
                        // Listening done, start practice with count-in
                        self.start_segment_playback();
                        self.start_practice();
                    }
                    LearningPhase::Practicing => {
                        self.finish_attempt();
                    }
                    LearningPhase::Exam => {
                        self.finish_exam();
                    }
                    _ => {}
                }
            }
        }
    }

    /// Open the file browser at the given directory (defaults to ~/Downloads or ~).
    pub fn open_browser(&mut self) {
        let downloads = std::env::var("HOME")
            .map(|h| PathBuf::from(h).join("Downloads"))
            .unwrap_or_else(|_| PathBuf::from("/"));
        if downloads.is_dir() {
            self.browse_dir = downloads;
        }
        self.scan_browse_dir();
        self.phase = LearningPhase::BrowseFiles;
    }

    /// Scan the current browse directory and populate entries.
    pub fn scan_browse_dir(&mut self) {
        self.browse_entries.clear();
        self.browse_error = None;

        let entries = match std::fs::read_dir(&self.browse_dir) {
            Ok(e) => e,
            Err(e) => {
                self.browse_error = Some(format!("Cannot read directory: {}", e));
                return;
            }
        };

        let mut dirs: Vec<BrowseEntry> = Vec::new();
        let mut files: Vec<BrowseEntry> = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry
                .file_name()
                .to_string_lossy()
                .into_owned();

            // Skip hidden files
            if name.starts_with('.') {
                continue;
            }

            if path.is_dir() {
                dirs.push(BrowseEntry {
                    name,
                    path,
                    is_dir: true,
                });
            } else if path.extension().and_then(|e| e.to_str()) == Some("mid")
                || path.extension().and_then(|e| e.to_str()) == Some("midi")
            {
                files.push(BrowseEntry {
                    name,
                    path,
                    is_dir: false,
                });
            }
        }

        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        self.browse_entries = dirs;
        self.browse_entries.extend(files);

        self.browse_list_state = ListState::default();
        if !self.browse_entries.is_empty() {
            self.browse_list_state.select(Some(0));
        }
    }

    /// Navigate into a directory in the file browser.
    pub fn browse_enter_dir(&mut self, path: PathBuf) {
        self.browse_dir = path;
        self.scan_browse_dir();
    }

    /// Navigate to the parent directory in the file browser.
    pub fn browse_parent(&mut self) {
        if let Some(parent) = self.browse_dir.parent() {
            self.browse_dir = parent.to_path_buf();
            self.scan_browse_dir();
        }
    }

    /// Stop scheduler playback.
    pub fn stop_playback(&mut self) {
        if let Some(ref tx) = self.scheduler_tx {
            let _ = tx.send(SchedulerCommand::Stop);
        }
    }
}
