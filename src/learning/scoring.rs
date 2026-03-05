use std::time::Instant;

/// Tolerance for matching a user hit to an expected note (±50ms).
const HIT_TOLERANCE_MS: f64 = 50.0;

/// Result of matching a single user hit against expected notes.
#[derive(Debug, Clone)]
pub enum HitResult {
    /// Hit matched an expected note within tolerance.
    Correct {
        /// Deviation in ms (negative = early/rushing, positive = late/dragging).
        deviation_ms: f64,
    },
    /// Hit a drum that wasn't expected at this time.
    ExtraHit,
    /// Wrong drum: right time window but wrong note.
    WrongDrum,
}

/// Overall tendency of the player's timing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tendency {
    Rushing,
    OnTime,
    Dragging,
}

/// Score for a single attempt at a segment.
#[derive(Debug, Clone)]
pub struct AttemptScore {
    pub notes_hit: usize,
    pub notes_missed: usize,
    pub extra_hits: usize,
    pub accuracy_percent: f64,
    pub average_deviation_ms: f64,
    pub tendency: Tendency,
    pub passed: bool,
}

/// An expected note with its target time.
#[derive(Debug, Clone)]
pub struct ExpectedNote {
    /// Beat position within the segment (relative to segment start).
    pub beat_offset: f64,
    /// MIDI note number.
    pub note: u8,
    /// Whether this note has been matched by a user hit.
    pub matched: bool,
    /// Deviation if matched.
    pub deviation_ms: Option<f64>,
}

/// A recorded user hit.
#[derive(Debug, Clone)]
pub struct UserHit {
    /// Time of the hit relative to segment start.
    pub time_ms: f64,
    /// MIDI note number.
    pub note: u8,
    /// Result of matching.
    pub result: Option<HitResult>,
}

/// Tracks expected notes and user hits for a single attempt.
pub struct AttemptTracker {
    expected: Vec<ExpectedNote>,
    hits: Vec<UserHit>,
    segment_start: Option<Instant>,
    bpm: f64,
    pass_threshold: f64,
}

impl AttemptTracker {
    /// Create a new tracker for a segment.
    ///
    /// `beat_offsets_and_notes`: (beat_offset, note) pairs for expected notes.
    /// `bpm`: Current tempo for converting beats to time.
    pub fn new(beat_offsets_and_notes: Vec<(f64, u8)>, bpm: f64) -> Self {
        let expected = beat_offsets_and_notes
            .into_iter()
            .map(|(beat_offset, note)| ExpectedNote {
                beat_offset,
                note,
                matched: false,
                deviation_ms: None,
            })
            .collect();

        Self {
            expected,
            hits: Vec::new(),
            segment_start: None,
            bpm,
            pass_threshold: 80.0,
        }
    }

    /// Mark the start of the attempt (called when count-in ends).
    pub fn start(&mut self) {
        self.segment_start = Some(Instant::now());
    }

    /// Record a user hit. Returns the hit result.
    pub fn record_hit(&mut self, note: u8) -> HitResult {
        let time_ms = match self.segment_start {
            Some(start) => start.elapsed().as_secs_f64() * 1000.0,
            None => return HitResult::ExtraHit,
        };

        let result = self.match_hit(time_ms, note);

        self.hits.push(UserHit {
            time_ms,
            note,
            result: Some(result.clone()),
        });

        result
    }

    /// Try to match a hit against expected notes.
    fn match_hit(&mut self, time_ms: f64, note: u8) -> HitResult {
        let ms_per_beat = 60_000.0 / self.bpm;

        let mut best_idx: Option<usize> = None;
        let mut best_deviation = f64::MAX;

        for (i, expected) in self.expected.iter().enumerate() {
            if expected.matched {
                continue;
            }
            if expected.note != note {
                continue;
            }

            let expected_time_ms = expected.beat_offset * ms_per_beat;
            let deviation = time_ms - expected_time_ms;

            if deviation.abs() <= HIT_TOLERANCE_MS && deviation.abs() < best_deviation.abs() {
                best_deviation = deviation;
                best_idx = Some(i);
            }
        }

        if let Some(idx) = best_idx {
            self.expected[idx].matched = true;
            self.expected[idx].deviation_ms = Some(best_deviation);
            return HitResult::Correct {
                deviation_ms: best_deviation,
            };
        }

        // Check if the timing is right but wrong drum
        for expected in &self.expected {
            if expected.matched {
                continue;
            }
            let expected_time_ms = expected.beat_offset * ms_per_beat;
            let deviation = (time_ms - expected_time_ms).abs();
            if deviation <= HIT_TOLERANCE_MS {
                return HitResult::WrongDrum;
            }
        }

        HitResult::ExtraHit
    }

    /// Compute the final score for this attempt.
    pub fn score(&self) -> AttemptScore {
        let total_expected = self.expected.len();
        let notes_hit = self.expected.iter().filter(|n| n.matched).count();
        let notes_missed = total_expected - notes_hit;
        let extra_hits = self
            .hits
            .iter()
            .filter(|h| matches!(h.result, Some(HitResult::ExtraHit | HitResult::WrongDrum)))
            .count();

        let accuracy_percent = if total_expected == 0 {
            100.0
        } else {
            (notes_hit as f64 / total_expected as f64) * 100.0
        };

        // Average deviation of correct hits
        let deviations: Vec<f64> = self
            .expected
            .iter()
            .filter_map(|n| n.deviation_ms)
            .collect();

        let average_deviation_ms = if deviations.is_empty() {
            0.0
        } else {
            deviations.iter().sum::<f64>() / deviations.len() as f64
        };

        let tendency = if average_deviation_ms < -10.0 {
            Tendency::Rushing
        } else if average_deviation_ms > 10.0 {
            Tendency::Dragging
        } else {
            Tendency::OnTime
        };

        let passed = accuracy_percent >= self.pass_threshold;

        AttemptScore {
            notes_hit,
            notes_missed,
            extra_hits,
            accuracy_percent,
            average_deviation_ms,
            tendency,
            passed,
        }
    }

    /// Get the expected notes (for rendering the beat grid).
    pub fn expected_notes(&self) -> &[ExpectedNote] {
        &self.expected
    }

    /// Get user hits (for rendering the user input overlay).
    pub fn user_hits(&self) -> &[UserHit] {
        &self.hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_segment_scores_100() {
        let tracker = AttemptTracker::new(vec![], 120.0);
        let score = tracker.score();
        assert!((score.accuracy_percent - 100.0).abs() < f64::EPSILON);
        assert!(score.passed);
    }

    #[test]
    fn all_missed_scores_zero() {
        let tracker = AttemptTracker::new(
            vec![(0.0, 36), (1.0, 38), (2.0, 36), (3.0, 38)],
            120.0,
        );
        let score = tracker.score();
        assert!((score.accuracy_percent - 0.0).abs() < f64::EPSILON);
        assert!(!score.passed);
        assert_eq!(score.notes_missed, 4);
    }

    #[test]
    fn tendency_detection() {
        // Test rushing tendency
        let score = AttemptScore {
            notes_hit: 4,
            notes_missed: 0,
            extra_hits: 0,
            accuracy_percent: 100.0,
            average_deviation_ms: -15.0,
            tendency: Tendency::Rushing,
            passed: true,
        };
        assert_eq!(score.tendency, Tendency::Rushing);

        // Test dragging tendency
        let score2 = AttemptScore {
            notes_hit: 4,
            notes_missed: 0,
            extra_hits: 0,
            accuracy_percent: 100.0,
            average_deviation_ms: 15.0,
            tendency: Tendency::Dragging,
            passed: true,
        };
        assert_eq!(score2.tendency, Tendency::Dragging);
    }
}
