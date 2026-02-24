use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::{render_popups, AppState, PadState, FLASH_DURATION_MS};

pub(super) fn ui(frame: &mut Frame, state: &AppState, extra_kit_dirs: &[PathBuf], extra_mapping_dirs: &[PathBuf]) {
    let area = frame.area();

    if area.height < 3 || area.width < 15 {
        let msg = Paragraph::new("Terminal too small\nq: quit")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(msg, area);
        return;
    }

    let header_height = if area.height < 8 { 1 } else { 3 };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, chunks[0], state);
    render_body(frame, chunks[1], state);
    render_footer(frame, chunks[2], state);

    // Popup overlay
    if let Some(ref popup) = state.popup {
        render_popups::render_popup(frame, area, popup, state, extra_kit_dirs, extra_mapping_dirs);
    }
}

fn render_header(frame: &mut Frame, area: Rect, state: &AppState) {
    if area.height == 0 {
        return;
    }

    if area.height == 1 {
        let line = Line::from(vec![
            Span::styled(
                " drumkit ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(&state.kit_name, Style::default().fg(Color::White)),
            Span::raw("  "),
            Span::styled(
                format!("Hits: {}", state.total_hits),
                Style::default().fg(Color::Yellow),
            ),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    let line1 = Line::from(vec![
        Span::styled(" Kit: ", Style::default().fg(Color::DarkGray)),
        Span::styled(&state.kit_name, Style::default().fg(Color::Cyan)),
        Span::raw("   "),
        Span::styled("MIDI: ", Style::default().fg(Color::DarkGray)),
        Span::styled(&state.midi_device, Style::default().fg(Color::Green)),
    ]);
    let line2 = Line::from(vec![
        Span::styled(
            format!(" {} Hz / {} ch", state.sample_rate, state.channels),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("    "),
        Span::styled("Hits: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            state.total_hits.to_string(),
            Style::default().fg(Color::Yellow),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" drumkit ")
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let paragraph = Paragraph::new(vec![line1, line2]).block(block);
    frame.render_widget(paragraph, area);
}

fn render_body(frame: &mut Frame, area: Rect, state: &AppState) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    if area.width < 50 {
        // Narrow: stack vertically
        let pad_rows = if area.height < 6 {
            0
        } else {
            (area.height / 2).max(3)
        };
        if pad_rows == 0 {
            render_hit_log(frame, area, state);
        } else {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(pad_rows), Constraint::Min(1)])
                .split(area);
            render_pad_grid(frame, chunks[0], state);
            render_hit_log(frame, chunks[1], state);
        }
    } else {
        // Wide: horizontal split with visible separator
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(area);

        // Left pane: pad grid on top, shortcuts at bottom
        let left_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(chunks[0]);
        render_pad_grid(frame, left_chunks[0], state);
        render_shortcuts(frame, left_chunks[1]);

        render_hit_log(frame, chunks[1], state);
    }
}

fn pad_flash(pad: &PadState, now: Instant) -> (f32, u8) {
    match pad.last_hit {
        Some(hit_time) => {
            let elapsed_ms = now.duration_since(hit_time).as_millis();
            if elapsed_ms < FLASH_DURATION_MS {
                let intensity = (1.0 - elapsed_ms as f64 / FLASH_DURATION_MS as f64) as f32;
                let vel_factor = pad.last_velocity as f32 / 127.0;
                (intensity * vel_factor, pad.last_velocity)
            } else {
                (0.0, 0)
            }
        }
        None => (0.0, 0),
    }
}

fn flash_styles(green: u8) -> (Color, Color, Color) {
    let border = if green > 0 {
        Color::Rgb(0, green, 0)
    } else {
        Color::Rgb(60, 60, 70)
    };
    let text = if green > 100 {
        Color::Black
    } else if green > 0 {
        Color::White
    } else {
        Color::Rgb(160, 160, 180)
    };
    let bg = if green > 30 {
        Color::Rgb(0, green / 3, 0)
    } else {
        Color::Reset
    };
    (border, text, bg)
}

fn render_pad_grid(frame: &mut Frame, area: Rect, state: &AppState) {
    if state.pads.is_empty() || area.width < 2 || area.height < 1 {
        return;
    }

    let now = Instant::now();

    // Very narrow: compact text list
    if area.width < 18 {
        render_pad_list_compact(frame, area, state, now);
        return;
    }

    // Each pad needs: border(2) + name(up to 14) = 16 min width
    // and border(2) + 2 content lines = 4 height
    let pad_height: u16 = 4;

    // Find the widest column count where each pad is >= 17 wide
    let min_pad_w: u16 = 17;
    let max_pad_w: u16 = 24;
    let cols = ((area.width / min_pad_w) as usize).max(1);
    let pad_width = (area.width / cols as u16).min(max_pad_w);

    // Grid width for horizontal centering
    let grid_w = cols as u16 * pad_width;

    // Top-align vertically, center horizontally
    let offset_y = area.y;
    let offset_x = area.x + (area.width.saturating_sub(grid_w)) / 2;

    for (i, pad) in state.pads.iter().enumerate() {
        let col = i % cols;
        let row = i / cols;

        let x = offset_x + (col as u16) * pad_width;
        let y = offset_y + (row as u16) * pad_height;

        if x + pad_width > area.x + area.width || y + pad_height > area.y + area.height {
            continue;
        }

        let pad_area = Rect::new(x, y, pad_width, pad_height);
        let (flash, velocity) = pad_flash(pad, now);
        let green = (flash * 255.0) as u8;
        let (border_color, text_color, bg) = flash_styles(green);

        let inner_w = (pad_width as usize).saturating_sub(2);
        let name_display = if pad.name.len() > inner_w {
            &pad.name[..inner_w]
        } else {
            &pad.name
        };

        let note_line = if velocity > 0 {
            format!("{:>3} v{}", pad.note, velocity)
        } else {
            format!("{:>3}", pad.note)
        };

        let pad_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color));

        let content = vec![
            Line::from(Span::styled(
                name_display.to_string(),
                Style::default()
                    .fg(text_color)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                note_line,
                Style::default().fg(if green > 0 {
                    text_color
                } else {
                    Color::DarkGray
                }).bg(bg),
            )),
        ];

        let pad_text = Paragraph::new(content).block(pad_block);
        frame.render_widget(pad_text, pad_area);
    }
}

/// Compact pad list for very narrow terminals — one pad per line, no borders.
fn render_pad_list_compact(frame: &mut Frame, area: Rect, state: &AppState, now: Instant) {
    let max_lines = area.height as usize;
    let w = area.width as usize;

    let mut lines = Vec::with_capacity(max_lines);
    for pad in state.pads.iter().take(max_lines) {
        let (flash, velocity) = pad_flash(pad, now);
        let green = (flash * 255.0) as u8;
        let (_, text_color, bg) = flash_styles(green);

        let label = if velocity > 0 && w >= 12 {
            format!("{:>3} {} v{}", pad.note, pad.name, velocity)
        } else {
            format!("{:>3} {}", pad.note, pad.name)
        };

        let display = if label.len() > w {
            &label[..w]
        } else {
            &label
        };

        lines.push(Line::from(Span::styled(
            display.to_string(),
            Style::default().fg(text_color).bg(bg),
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn render_hit_log(frame: &mut Frame, area: Rect, state: &AppState) {
    if area.width < 4 || area.height < 1 {
        return;
    }

    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Hit Log ")
        .title_style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .title_alignment(Alignment::Left);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 4 || inner.height < 1 {
        return;
    }

    if state.hit_log.is_empty() {
        let hint = Paragraph::new(Line::from(Span::styled(
            " Hit a pad...",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(hint, inner);
        return;
    }

    let w = inner.width as usize;
    let max_lines = inner.height as usize;

    // Layout tiers:
    //   wide (>=40): name(16) + " " + note(3) + " " + bar(var) + " " + vel(3) = 25 + bar
    //   medium (>=20): name(10) + " " + note(3) + " " + vel(3) = 18
    //   narrow (>=10): name(var) + " " + vel(3)
    let show_bar = w >= 28;
    let show_note = w >= 20;

    let fixed = if show_bar { 9 } else if show_note { 8 } else { 4 }; // spaces + note + vel
    let name_w = w.saturating_sub(fixed).min(16).max(3);

    let bar_max = if show_bar {
        w.saturating_sub(name_w + 9).min(12).max(3)
    } else {
        0
    };

    let mut lines = Vec::with_capacity(max_lines);
    for entry in state.hit_log.iter().take(max_lines) {
        let name = if entry.name.len() > name_w {
            &entry.name[..name_w]
        } else {
            &entry.name
        };

        let mut spans = vec![
            Span::raw(" "),
            Span::styled(
                format!("{:<w$}", name, w = name_w),
                Style::default().fg(Color::Cyan),
            ),
        ];

        if show_note {
            spans.push(Span::styled(
                format!(" {:>3}", entry.note),
                Style::default().fg(Color::DarkGray),
            ));
        }

        if show_bar && bar_max > 0 {
            let filled = ((entry.velocity as usize) * bar_max + 63) / 127; // round up
            let empty = bar_max - filled;
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                "\u{2588}".repeat(filled),
                Style::default().fg(Color::Green),
            ));
            spans.push(Span::styled(
                "\u{2591}".repeat(empty),
                Style::default().fg(Color::Rgb(50, 50, 60)),
            ));
        }

        spans.push(Span::styled(
            format!(" {:>3}", entry.velocity),
            Style::default().fg(Color::White),
        ));

        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_shortcuts(frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let hints = " l log  k kit  n mapping  r rename  d dirs  a audio  m midi  q quit";
    let hint_style = Style::default().fg(Color::DarkGray);
    let line = Line::from(Span::styled(hints, hint_style));
    frame.render_widget(Paragraph::new(line), area);
}

fn render_footer(frame: &mut Frame, area: Rect, state: &AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let w = area.width as usize;

    let (status_text, status_style) = match &state.status_message {
        Some((msg, time)) if time.elapsed() < Duration::from_secs(3) => (
            format!(" \u{2713} {}", msg),
            Style::default().fg(Color::Green),
        ),
        _ => (String::new(), Style::default()),
    };

    let truncated: String = status_text.chars().take(w).collect();
    let line = Line::from(Span::styled(truncated, status_style));
    frame.render_widget(Paragraph::new(line), area);
}
