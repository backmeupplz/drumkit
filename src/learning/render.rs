use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph},
    Frame,
};
use std::sync::Arc;

use super::{LearningPhase, LearningState};
use crate::mapping::NoteMapping;
use crate::learning::scoring::{Tendency, HitResult};

/// Render the full learning mode screen.
pub fn render_learning(frame: &mut Frame, area: Rect, state: &LearningState, mapping: &Arc<NoteMapping>) {
    match &state.phase {
        LearningPhase::SelectLesson => render_lesson_picker(frame, area, state),
        LearningPhase::BrowseFiles => render_file_browser(frame, area, state),
        _ => render_practice_screen(frame, area, state, mapping),
    }
}

/// Lesson picker screen.
fn render_lesson_picker(frame: &mut Frame, area: Rect, state: &LearningState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Select Lesson ")
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.available_lessons.is_empty() {
        let help = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No lessons found.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Place .mid files in:",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                format!("  {}", crate::lesson::lessons_dir().display()),
                Style::default().fg(Color::White),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("  Or press ", Style::default().fg(Color::DarkGray)),
                Span::styled("b", Style::default().fg(Color::Yellow)),
                Span::styled(" to browse for .mid files (e.g. ~/Downloads)", Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "  Press Esc to go back.",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        frame.render_widget(Paragraph::new(help), inner);

        // Still need to handle 'b' key even when list is empty
        let footer_area = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner)[1];
        let footer = Line::from(vec![
            Span::styled(" b", Style::default().fg(Color::Yellow)),
            Span::styled(" browse files  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::Red)),
            Span::styled(" back", Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(footer), footer_area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let items: Vec<ListItem> = state
        .available_lessons
        .iter()
        .map(|l| {
            ListItem::new(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(&l.name, Style::default().fg(Color::White)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let mut list_state = state.lesson_list_state.clone();
    frame.render_stateful_widget(list, chunks[0], &mut list_state);

    let footer = Line::from(vec![
        Span::styled(" Enter", Style::default().fg(Color::Green)),
        Span::styled(" select  ", Style::default().fg(Color::DarkGray)),
        Span::styled("b", Style::default().fg(Color::Yellow)),
        Span::styled(" browse files  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Esc", Style::default().fg(Color::Red)),
        Span::styled(" back", Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(footer), chunks[1]);
}

/// File browser screen.
fn render_file_browser(frame: &mut Frame, area: Rect, state: &LearningState) {
    let dir_display = state.browse_dir.display().to_string();
    let title = format!(" Browse: {} ", dir_display);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title)
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(if state.browse_error.is_some() { 2 } else { 1 }),
        ])
        .split(inner);

    if state.browse_entries.is_empty() {
        let msg = if let Some(ref err) = state.browse_error {
            err.as_str()
        } else {
            "No .mid files or directories found"
        };
        let help = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  {}", msg),
                Style::default().fg(Color::DarkGray),
            )),
        ];
        frame.render_widget(Paragraph::new(help), chunks[0]);
    } else {
        let items: Vec<ListItem> = state
            .browse_entries
            .iter()
            .map(|entry| {
                let (icon, color) = if entry.is_dir {
                    ("/", Color::Yellow)
                } else {
                    (" ", Color::White)
                };
                ListItem::new(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(icon, Style::default().fg(color)),
                    Span::styled(" ", Style::default()),
                    Span::styled(&entry.name, Style::default().fg(color)),
                ]))
            })
            .collect();

        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");

        let mut list_state = state.browse_list_state.clone();
        frame.render_stateful_widget(list, chunks[0], &mut list_state);
    }

    // Footer with controls and optional error
    let mut footer_lines = Vec::new();
    if let Some(ref err) = state.browse_error {
        footer_lines.push(Line::from(Span::styled(
            format!(" {}", err),
            Style::default().fg(Color::Red),
        )));
    }
    footer_lines.push(Line::from(vec![
        Span::styled(" Enter", Style::default().fg(Color::Green)),
        Span::styled(" open  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Backspace", Style::default().fg(Color::Yellow)),
        Span::styled(" parent  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Esc", Style::default().fg(Color::Red)),
        Span::styled(" back", Style::default().fg(Color::DarkGray)),
    ]));
    frame.render_widget(Paragraph::new(footer_lines), chunks[1]);
}

/// Main practice screen with beat grid, score, status.
fn render_practice_screen(frame: &mut Frame, area: Rect, state: &LearningState, mapping: &Arc<NoteMapping>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Header
            Constraint::Min(5),     // Beat grid
            Constraint::Length(3),  // Score / status
            Constraint::Length(1),  // Controls
        ])
        .split(area);

    render_practice_header(frame, chunks[0], state);
    render_beat_grid(frame, chunks[1], state, mapping);
    render_score_area(frame, chunks[2], state);
    render_practice_controls(frame, chunks[3], state);
}

/// Header: lesson name, BPM, segment info.
fn render_practice_header(frame: &mut Frame, area: Rect, state: &LearningState) {
    let lesson_name = state
        .lesson
        .as_ref()
        .map(|l| l.name.as_str())
        .unwrap_or("--");
    let total_segments = state
        .lesson
        .as_ref()
        .map(|l| l.segments.len())
        .unwrap_or(0);

    let phase_text = match &state.phase {
        LearningPhase::Listening => "Listening",
        LearningPhase::CountIn { current_beat, total_beats } => {
            // We'll format this inline
            &format!("Count-in: {}/{}", current_beat, total_beats)
        }
        LearningPhase::Practicing => "Playing",
        LearningPhase::ReviewScore => "Review",
        LearningPhase::Exam => "Exam",
        LearningPhase::ExamResult => "Exam Result",
        LearningPhase::SelectLesson | LearningPhase::BrowseFiles => "Select",
    };

    let bpm_text = if state.current_bpm < state.target_bpm {
        format!("{:.0} → {:.0}", state.current_bpm, state.target_bpm)
    } else {
        format!("{:.0}", state.current_bpm)
    };

    let metronome_indicator = if state.metronome_enabled { "♪" } else { " " };

    let line1 = Line::from(vec![
        Span::styled(" Learning: ", Style::default().fg(Color::DarkGray)),
        Span::styled(lesson_name, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled("BPM: ", Style::default().fg(Color::DarkGray)),
        Span::styled(&bpm_text, Style::default().fg(Color::Yellow)),
        Span::raw("  "),
        Span::styled(metronome_indicator, Style::default().fg(Color::Green)),
    ]);

    let seg_text = if state.combined_segments.is_empty() {
        format!(
            "Segment {}/{}",
            state.current_segment + 1,
            total_segments
        )
    } else {
        format!(
            "Combined {}-{}",
            state.combined_segments.first().map(|s| s + 1).unwrap_or(0),
            state.combined_segments.last().map(|s| s + 1).unwrap_or(0),
        )
    };

    let line2 = Line::from(vec![
        Span::styled(format!(" {}", seg_text), Style::default().fg(Color::DarkGray)),
        Span::raw("  "),
        Span::styled(phase_text, Style::default().fg(Color::White)),
        Span::raw("  "),
        Span::styled(
            format!("Attempts: {}", state.total_attempts),
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray));

    frame.render_widget(Paragraph::new(vec![line1, line2]).block(block), area);
}

/// Beat grid: shows expected notes and user hits on a time grid.
fn render_beat_grid(frame: &mut Frame, area: Rect, state: &LearningState, mapping: &Arc<NoteMapping>) {
    if area.height < 3 || area.width < 20 {
        return;
    }

    let lesson = match &state.lesson {
        Some(l) => l,
        None => return,
    };

    let notes = state.current_practice_notes();
    if notes.is_empty() {
        return;
    }

    // Get unique notes for this segment (sorted)
    let mut unique_notes: Vec<u8> = notes.iter().map(|n| n.note).collect();
    unique_notes.sort();
    unique_notes.dedup();

    let beats_per_bar = lesson.beats_per_bar as usize;
    let total_beats = state.current_total_beats();
    let segment_start_beat = notes.first().map(|n| n.beat_position).unwrap_or(0.0);

    // Layout: name column + beat columns
    let name_col_width = 4.min(area.width as usize / 4).max(2);
    let grid_width = (area.width as usize).saturating_sub(name_col_width + 1);
    let subdivisions = 2; // 8th note resolution
    let total_slots = (total_beats as usize) * subdivisions;
    let slot_width = if total_slots > 0 {
        (grid_width / total_slots).max(1)
    } else {
        1
    };

    // Split area: reference grid on top, user grid on bottom
    let available_rows = area.height as usize;
    let note_count = unique_notes.len();
    let ref_rows = (note_count + 2).min(available_rows / 2); // +2 for header and separator
    let user_rows = available_rows.saturating_sub(ref_rows);

    // Render reference header
    let mut lines: Vec<Line> = Vec::new();

    // Beat number header
    let mut header_spans = vec![Span::styled(
        format!("{:>width$}|", "", width = name_col_width),
        Style::default().fg(Color::DarkGray),
    )];
    for beat in 0..total_beats.ceil() as usize {
        let beat_label = format!("{}", (beat % beats_per_bar) + 1);
        let slot_str = format!("{:<width$}", beat_label, width = slot_width * subdivisions);
        let is_playhead = (state.playhead_beat - beat as f64).abs() < 0.5;
        let style = if is_playhead {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        header_spans.push(Span::styled(slot_str, style));
    }
    lines.push(Line::from(header_spans));

    // Reference note rows
    for &note in &unique_notes {
        let drum_name = mapping.drum_name(note);
        let short_name: String = drum_name.chars().take(name_col_width).collect();
        let mut row_spans = vec![Span::styled(
            format!("{:>width$}|", short_name, width = name_col_width),
            Style::default().fg(Color::Cyan),
        )];

        // Fill slots
        for slot in 0..total_slots.min(grid_width) {
            let slot_beat = slot as f64 / subdivisions as f64;
            let has_note = notes.iter().any(|n| {
                n.note == note
                    && ((n.beat_position - segment_start_beat) - slot_beat).abs() < (0.5 / subdivisions as f64)
            });

            let is_playhead = (state.playhead_beat - slot_beat).abs() < (0.5 / subdivisions as f64);

            let (ch, style) = if has_note && is_playhead {
                ("X", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
            } else if has_note {
                ("x", Style::default().fg(Color::White))
            } else if is_playhead {
                ("▼", Style::default().fg(Color::Green))
            } else {
                ("·", Style::default().fg(Color::Rgb(60, 60, 70)))
            };

            row_spans.push(Span::styled(
                format!("{:<width$}", ch, width = slot_width.max(1)),
                style,
            ));
        }

        lines.push(Line::from(row_spans));
    }

    // Separator
    if user_rows > 0 {
        lines.push(Line::from(Span::styled(
            format!("{} You {}", "─".repeat(name_col_width), "─".repeat(grid_width.saturating_sub(5))),
            Style::default().fg(Color::DarkGray),
        )));
    }

    // User hit rows
    if let Some(ref tracker) = state.attempt_tracker {
        let ms_per_beat = 60_000.0 / state.current_bpm;

        for &note in &unique_notes {
            if lines.len() >= area.height as usize {
                break;
            }

            let drum_name = mapping.drum_name(note);
            let short_name: String = drum_name.chars().take(name_col_width).collect();
            let mut row_spans = vec![Span::styled(
                format!("{:>width$}|", short_name, width = name_col_width),
                Style::default().fg(Color::Cyan),
            )];

            // Fill slots with user hits
            for slot in 0..total_slots.min(grid_width) {
                let slot_beat = slot as f64 / subdivisions as f64;
                let slot_time_ms = slot_beat * ms_per_beat;

                let user_hit = tracker.user_hits().iter().find(|h| {
                    h.note == note && (h.time_ms - slot_time_ms).abs() < (ms_per_beat / subdivisions as f64 * 0.5)
                });

                let (ch, style) = match user_hit {
                    Some(h) => match &h.result {
                        Some(HitResult::Correct { .. }) => {
                            ("x", Style::default().fg(Color::Green))
                        }
                        Some(HitResult::WrongDrum) => {
                            ("!", Style::default().fg(Color::Red))
                        }
                        Some(HitResult::ExtraHit) => {
                            ("*", Style::default().fg(Color::Yellow))
                        }
                        None => ("·", Style::default().fg(Color::Rgb(60, 60, 70))),
                    },
                    None => ("·", Style::default().fg(Color::Rgb(60, 60, 70))),
                };

                row_spans.push(Span::styled(
                    format!("{:<width$}", ch, width = slot_width.max(1)),
                    style,
                ));
            }

            lines.push(Line::from(row_spans));
        }
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Score and status area.
fn render_score_area(frame: &mut Frame, area: Rect, state: &LearningState) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    match &state.phase {
        LearningPhase::ReviewScore | LearningPhase::ExamResult => {
            let score = if state.phase == LearningPhase::ExamResult {
                state.exam_score.as_ref()
            } else {
                state.last_score.as_ref()
            };

            if let Some(score) = score {
                let accuracy_color = if score.accuracy_percent >= 80.0 {
                    Color::Green
                } else if score.accuracy_percent >= 60.0 {
                    Color::Yellow
                } else {
                    Color::Red
                };

                let tendency_text = match score.tendency {
                    Tendency::Rushing => "Rushing ◀",
                    Tendency::OnTime => "On Time ●",
                    Tendency::Dragging => "Dragging ▶",
                };

                let tendency_color = match score.tendency {
                    Tendency::OnTime => Color::Green,
                    _ => Color::Yellow,
                };

                let pass_text = if score.passed { "PASS ▲" } else { "FAIL ▼" };
                let pass_color = if score.passed { Color::Green } else { Color::Red };

                let line = Line::from(vec![
                    Span::styled(" Score: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{:.0}%", score.accuracy_percent),
                        Style::default().fg(accuracy_color).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        format!("{}/{} hit", score.notes_hit, score.notes_hit + score.notes_missed),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::raw("  "),
                    Span::styled(tendency_text, Style::default().fg(tendency_color)),
                    Span::raw("  "),
                    Span::styled(
                        format!("{:+.0}ms", score.average_deviation_ms),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::raw("  "),
                    Span::styled(pass_text, Style::default().fg(pass_color)),
                ]);

                frame.render_widget(Paragraph::new(line), inner);
            }
        }
        LearningPhase::CountIn { current_beat, total_beats } => {
            let line = Line::from(vec![
                Span::styled(" Count-in: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{}", current_beat),
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("/{}", total_beats),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);
            frame.render_widget(Paragraph::new(line), inner);
        }
        LearningPhase::Listening => {
            let line = Line::from(Span::styled(
                " Listening to reference...",
                Style::default().fg(Color::DarkGray),
            ));
            frame.render_widget(Paragraph::new(line), inner);
        }
        LearningPhase::Practicing | LearningPhase::Exam => {
            let label = if state.phase == LearningPhase::Exam {
                " Exam: Play along!"
            } else {
                " Play along!"
            };
            let line = Line::from(Span::styled(
                label,
                Style::default().fg(Color::Green),
            ));
            frame.render_widget(Paragraph::new(line), inner);
        }
        _ => {}
    }
}

/// Control hints at the bottom.
fn render_practice_controls(frame: &mut Frame, area: Rect, state: &LearningState) {
    let controls = match &state.phase {
        LearningPhase::ReviewScore => {
            vec![
                Span::styled(" Space", Style::default().fg(Color::Green)),
                Span::styled(" continue  ", Style::default().fg(Color::DarkGray)),
                Span::styled("r", Style::default().fg(Color::Yellow)),
                Span::styled(" retry  ", Style::default().fg(Color::DarkGray)),
                Span::styled("Esc", Style::default().fg(Color::Red)),
                Span::styled(" back", Style::default().fg(Color::DarkGray)),
            ]
        }
        LearningPhase::ExamResult => {
            vec![
                Span::styled(" Space", Style::default().fg(Color::Green)),
                Span::styled(" retry exam  ", Style::default().fg(Color::DarkGray)),
                Span::styled("Esc", Style::default().fg(Color::Red)),
                Span::styled(" back", Style::default().fg(Color::DarkGray)),
            ]
        }
        LearningPhase::Listening | LearningPhase::CountIn { .. } | LearningPhase::Practicing | LearningPhase::Exam => {
            vec![
                Span::styled(" Esc", Style::default().fg(Color::Red)),
                Span::styled(" stop", Style::default().fg(Color::DarkGray)),
            ]
        }
        _ => vec![
            Span::styled(" Esc", Style::default().fg(Color::Red)),
            Span::styled(" back", Style::default().fg(Color::DarkGray)),
        ],
    };

    frame.render_widget(Paragraph::new(Line::from(controls)), area);
}
