use crossterm::event::KeyCode;

use super::{LearningPhase, LearningState};
use crate::lesson;
use crate::tui::list_nav;

/// Handle a key press in learning mode.
/// Returns `true` if learning mode should exit (return to normal play mode).
pub fn handle_learning_key(state: &mut LearningState, key: KeyCode) -> bool {
    match &state.phase {
        LearningPhase::SelectLesson => handle_select_lesson_key(state, key),
        LearningPhase::BrowseFiles => handle_browse_key(state, key),
        LearningPhase::ReviewScore => handle_review_key(state, key),
        LearningPhase::ExamResult => handle_exam_result_key(state, key),
        LearningPhase::Listening
        | LearningPhase::CountIn { .. }
        | LearningPhase::Practicing
        | LearningPhase::Exam => {
            // During active phases, only Esc to stop and go back
            if key == KeyCode::Esc {
                state.stop_playback();
                state.phase = LearningPhase::SelectLesson;
            }
            false
        }
    }
}

fn handle_select_lesson_key(state: &mut LearningState, key: KeyCode) -> bool {
    match key {
        KeyCode::Esc => {
            // Exit learning mode entirely
            return true;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            list_nav::list_up(&mut state.lesson_list_state, state.available_lessons.len());
        }
        KeyCode::Down | KeyCode::Char('j') => {
            list_nav::list_down(&mut state.lesson_list_state, state.available_lessons.len());
        }
        KeyCode::Char('b') | KeyCode::Char('o') => {
            // Open file browser
            state.open_browser();
        }
        KeyCode::Enter => {
            if let Some(selected) = state.lesson_list_state.selected() {
                if let Some(discovered) = state.available_lessons.get(selected) {
                    match lesson::parse_lesson(&discovered.path) {
                        Ok(lesson) => {
                            state.start_lesson(lesson);
                            state.start_segment_playback();
                        }
                        Err(_e) => {
                            // Could show error, but for now just stay on picker
                        }
                    }
                }
            }
        }
        _ => {}
    }
    false
}

fn handle_browse_key(state: &mut LearningState, key: KeyCode) -> bool {
    match key {
        KeyCode::Esc => {
            state.phase = LearningPhase::SelectLesson;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            list_nav::list_up(&mut state.browse_list_state, state.browse_entries.len());
        }
        KeyCode::Down | KeyCode::Char('j') => {
            list_nav::list_down(&mut state.browse_list_state, state.browse_entries.len());
        }
        KeyCode::Backspace | KeyCode::Left => {
            // Go to parent directory
            state.browse_parent();
        }
        KeyCode::Enter | KeyCode::Right => {
            if let Some(selected) = state.browse_list_state.selected() {
                if let Some(entry) = state.browse_entries.get(selected).cloned() {
                    if entry.is_dir {
                        state.browse_enter_dir(entry.path);
                    } else {
                        // Try to parse and start the lesson
                        match lesson::parse_lesson(&entry.path) {
                            Ok(parsed_lesson) => {
                                state.start_lesson(parsed_lesson);
                                state.start_segment_playback();
                            }
                            Err(e) => {
                                state.browse_error = Some(format!("Failed to parse: {}", e));
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
    false
}

fn handle_review_key(state: &mut LearningState, key: KeyCode) -> bool {
    match key {
        KeyCode::Char(' ') | KeyCode::Enter => {
            // Advance: apply adaptive logic
            let _msg = state.advance();
            // If we moved to Listening or Exam, start playback
            match state.phase {
                LearningPhase::Listening => {
                    state.start_segment_playback();
                }
                LearningPhase::Exam => {
                    state.start_exam();
                }
                _ => {}
            }
        }
        KeyCode::Char('r') => {
            // Retry current segment at same BPM
            state.phase = LearningPhase::Listening;
            state.start_segment_playback();
        }
        KeyCode::Esc => {
            state.stop_playback();
            state.phase = LearningPhase::SelectLesson;
        }
        _ => {}
    }
    false
}

fn handle_exam_result_key(state: &mut LearningState, key: KeyCode) -> bool {
    match key {
        KeyCode::Char(' ') | KeyCode::Enter => {
            // Retry exam
            state.start_exam();
        }
        KeyCode::Esc => {
            state.stop_playback();
            state.phase = LearningPhase::SelectLesson;
        }
        _ => {}
    }
    false
}
