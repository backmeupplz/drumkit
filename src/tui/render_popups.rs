use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::{AppState, Popup};
use crate::{audio, kit, mapping, midi};

pub(super) fn render_popup(frame: &mut Frame, area: Rect, popup: &Popup, state: &AppState, extra_dirs: &[PathBuf]) {
    match popup {
        Popup::Log { scroll } => render_log_popup(frame, area, state, *scroll),
        Popup::KitPicker { kits, list_state } => render_kit_popup(frame, area, kits, list_state),
        Popup::AudioPicker { devices, list_state } => render_audio_popup(frame, area, devices, list_state),
        Popup::MidiPicker { devices, list_state } => render_midi_popup(frame, area, devices, list_state),
        Popup::LibraryDir { input, cursor, error } => render_library_dir_popup(frame, area, input, *cursor, error.as_deref(), extra_dirs),
        Popup::Loading { kit_name, progress, total } => render_loading_popup(frame, area, kit_name, progress, total),
        Popup::MappingPicker { mappings, list_state } => render_mapping_popup(frame, area, mappings, list_state, state),
        Popup::DeleteMapping { name, .. } => render_delete_mapping_popup(frame, area, name),
        Popup::NoteRename { note, input, cursor } => render_note_rename_popup(frame, area, *note, input, *cursor),
    }
}

fn popup_area(area: Rect) -> Rect {
    let popup_w = (area.width * 4 / 5).max(30).min(area.width);
    let popup_h = (area.height * 4 / 5).max(6).min(area.height);
    let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    Rect::new(x, y, popup_w, popup_h)
}

fn render_log_popup(frame: &mut Frame, area: Rect, state: &AppState, scroll: usize) {
    let popup = popup_area(area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow))
        .title(format!(" Log ({} lines) ", state.log_lines.len()))
        .title_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let content_area = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1));
    let footer_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);

    if state.log_lines.is_empty() {
        let msg = Paragraph::new(Line::from(Span::styled(
            " No log messages.",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(msg, content_area);
    } else {
        let visible = content_area.height as usize;
        let start = scroll.min(state.log_lines.len().saturating_sub(visible));

        let lines: Vec<Line> = state.log_lines[start..]
            .iter()
            .take(visible)
            .map(|l| Line::from(Span::styled(format!(" {}", l), Style::default().fg(Color::DarkGray))))
            .collect();

        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, content_area);
    }

    let footer = Paragraph::new(Line::from(Span::styled(
        " \u{2191}\u{2193} scroll  Esc/l close  q quit",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(footer, footer_area);
}

fn render_kit_popup(frame: &mut Frame, area: Rect, kits: &[kit::DiscoveredKit], list_state: &ListState) {
    let popup = popup_area(area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Select Kit ")
        .title_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let content_area = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1));
    let footer_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);

    if kits.is_empty() {
        let msg = Paragraph::new(Line::from(Span::styled(
            " No kits found.",
            Style::default().fg(Color::Yellow),
        )));
        frame.render_widget(msg, content_area);
    } else {
        let items: Vec<ListItem> = kits
            .iter()
            .map(|kit| {
                ListItem::new(Line::from(vec![
                    Span::raw(" "),
                    Span::raw(&kit.name),
                    Span::styled(
                        format!("  ({} samples)", kit.wav_count),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]))
            })
            .collect();

        let list = List::new(items)
            .highlight_symbol("\u{25b8} ")
            .highlight_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

        let mut ls = list_state.clone();
        frame.render_stateful_widget(list, content_area, &mut ls);
    }

    let footer = Paragraph::new(Line::from(Span::styled(
        " \u{2191}\u{2193} navigate  Enter select  Esc/k close  q quit",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(footer, footer_area);
}

fn render_audio_popup(frame: &mut Frame, area: Rect, devices: &[audio::AudioDevice], list_state: &ListState) {
    let popup = popup_area(area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Green))
        .title(" Select Audio Device ")
        .title_style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let content_area = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1));
    let footer_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);

    if devices.is_empty() {
        let msg = Paragraph::new(Line::from(Span::styled(
            " No audio devices found.",
            Style::default().fg(Color::Yellow),
        )));
        frame.render_widget(msg, content_area);
    } else {
        let items: Vec<ListItem> = devices
            .iter()
            .map(|dev| ListItem::new(Line::from(vec![Span::raw(" "), Span::raw(&dev.name)])))
            .collect();

        let list = List::new(items)
            .highlight_symbol("\u{25b8} ")
            .highlight_style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD));

        let mut ls = list_state.clone();
        frame.render_stateful_widget(list, content_area, &mut ls);
    }

    let footer = Paragraph::new(Line::from(Span::styled(
        " \u{2191}\u{2193} navigate  Enter select  Esc/a close  q quit",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(footer, footer_area);
}

fn render_midi_popup(frame: &mut Frame, area: Rect, devices: &[midi::MidiDevice], list_state: &ListState) {
    let popup = popup_area(area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Magenta))
        .title(" Select MIDI Input ")
        .title_style(Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let content_area = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1));
    let footer_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);

    if devices.is_empty() {
        let msg = Paragraph::new(Line::from(Span::styled(
            " No MIDI devices found.",
            Style::default().fg(Color::Yellow),
        )));
        frame.render_widget(msg, content_area);
    } else {
        let items: Vec<ListItem> = devices
            .iter()
            .map(|dev| ListItem::new(Line::from(vec![Span::raw(" "), Span::raw(&dev.name)])))
            .collect();

        let list = List::new(items)
            .highlight_symbol("\u{25b8} ")
            .highlight_style(Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD));

        let mut ls = list_state.clone();
        frame.render_stateful_widget(list, content_area, &mut ls);
    }

    let footer = Paragraph::new(Line::from(Span::styled(
        " \u{2191}\u{2193} navigate  Enter select  Esc/m close  q quit",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(footer, footer_area);
}

fn render_mapping_popup(
    frame: &mut Frame,
    area: Rect,
    mappings: &[mapping::NoteMapping],
    list_state: &ListState,
    state: &AppState,
) {
    let popup = popup_area(area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow))
        .title(" Select Mapping ")
        .title_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 3 || inner.width < 4 {
        return;
    }

    let content_area = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(2));
    let path_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(2), inner.width, 1);
    let footer_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);

    if mappings.is_empty() {
        let msg = Paragraph::new(Line::from(Span::styled(
            " No mappings found.",
            Style::default().fg(Color::Yellow),
        )));
        frame.render_widget(msg, content_area);
    } else {
        let items: Vec<ListItem> = mappings
            .iter()
            .map(|m| {
                let source_tag = match &m.source {
                    mapping::MappingSource::BuiltIn => " (built-in)",
                    mapping::MappingSource::UserFile(_) => " (user)",
                    mapping::MappingSource::KitFile(_) => " (kit)",
                };
                let current = if m.name == state.mapping.name { " *" } else { "" };
                ListItem::new(Line::from(vec![
                    Span::raw(" "),
                    Span::raw(&m.name),
                    Span::styled(source_tag, Style::default().fg(Color::DarkGray)),
                    Span::styled(current, Style::default().fg(Color::Green)),
                ]))
            })
            .collect();

        let list = List::new(items)
            .highlight_symbol("\u{25b8} ")
            .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

        let mut ls = list_state.clone();
        frame.render_stateful_widget(list, content_area, &mut ls);
    }

    let mappings_dir = mapping::user_mappings_dir();
    let path_hint = Paragraph::new(Line::from(Span::styled(
        format!(" User: {}", mappings_dir.display()),
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(path_hint, path_area);

    let footer = Paragraph::new(Line::from(Span::styled(
        " \u{2191}\u{2193} navigate  Enter select  d delete  Esc/n close  q quit",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(footer, footer_area);
}

fn render_delete_mapping_popup(frame: &mut Frame, area: Rect, name: &str) {
    let popup_w: u16 = 50.min(area.width.saturating_sub(2));
    let popup_h: u16 = 7.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup = Rect::new(x, y, popup_w, popup_h);

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Red))
        .title(" Delete Mapping ")
        .title_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 3 || inner.width < 10 {
        return;
    }

    let msg = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  Delete \"{}\"?", name),
            Style::default().fg(Color::White),
        )),
    ]);
    frame.render_widget(msg, Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1)));

    let footer = Paragraph::new(Line::from(Span::styled(
        " y/Enter confirm  any other key cancel",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(footer, Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1));
}

fn render_note_rename_popup(
    frame: &mut Frame,
    area: Rect,
    note: u8,
    input: &str,
    cursor: usize,
) {
    let popup_w: u16 = 40.min(area.width.saturating_sub(2));
    let popup_h: u16 = 7.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup = Rect::new(x, y, popup_w, popup_h);

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow))
        .title(format!(" Rename Note {} ", note))
        .title_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 3 || inner.width < 10 {
        return;
    }

    // Label
    let label = Paragraph::new(Line::from(Span::styled(
        " Enter name for this note:",
        Style::default().fg(Color::White),
    )));
    let label_area = Rect::new(inner.x, inner.y, inner.width, 1);
    frame.render_widget(label, label_area);

    // Input line with cursor
    let input_area = Rect::new(inner.x, inner.y + 1, inner.width, 1);
    let w = inner.width as usize;
    let display_input = if input.len() + 3 > w {
        let start = cursor.saturating_sub(w.saturating_sub(4));
        &input[start..]
    } else {
        input
    };
    let cursor_in_display = cursor.min(display_input.len());

    let before = &display_input[..cursor_in_display];
    let cursor_char = display_input.get(cursor_in_display..cursor_in_display + 1).unwrap_or(" ");
    let after = if cursor_in_display + 1 <= display_input.len() {
        &display_input[cursor_in_display + 1..]
    } else {
        ""
    };

    let input_line = Line::from(vec![
        Span::styled(" > ", Style::default().fg(Color::Yellow)),
        Span::raw(before),
        Span::styled(
            cursor_char,
            Style::default().fg(Color::Black).bg(Color::White),
        ),
        Span::raw(after),
    ]);
    frame.render_widget(Paragraph::new(input_line), input_area);

    // Footer
    if inner.height >= 3 {
        let footer_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);
        let footer = Paragraph::new(Line::from(Span::styled(
            " Enter save  Esc cancel",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(footer, footer_area);
    }
}

fn render_loading_popup(
    frame: &mut Frame,
    area: Rect,
    kit_name: &str,
    progress: &Arc<AtomicUsize>,
    total: &Arc<AtomicUsize>,
) {
    let popup_w: u16 = 36.min(area.width.saturating_sub(2));
    let popup_h: u16 = 7.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup = Rect::new(x, y, popup_w, popup_h);

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Loading Kit ")
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 3 || inner.width < 10 {
        return;
    }

    let cur = progress.load(Ordering::Relaxed);
    let tot = total.load(Ordering::Relaxed);

    // Line 1: kit name
    let name_w = inner.width as usize;
    let display_name = if kit_name.len() > name_w {
        &kit_name[..name_w]
    } else {
        kit_name
    };

    // Line 2: progress text
    let progress_text = if tot == 0 {
        " Scanning...".to_string()
    } else {
        format!(" Loading... {}/{} files", cur, tot)
    };

    // Line 3: progress bar
    let bar_w = (inner.width as usize).saturating_sub(2);
    let (filled, empty) = if tot == 0 || bar_w == 0 {
        (0, bar_w)
    } else {
        let f = (cur * bar_w) / tot;
        (f.min(bar_w), bar_w.saturating_sub(f))
    };

    let mut lines = vec![
        Line::from(Span::styled(
            format!(" {}", display_name),
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            progress_text,
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "\u{2588}".repeat(filled),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(
                "\u{2591}".repeat(empty),
                Style::default().fg(Color::Rgb(50, 50, 60)),
            ),
        ]),
    ];

    // Line 4: footer hints (if space permits)
    if inner.height >= 4 {
        lines.push(Line::from(Span::styled(
            " Esc cancel  q quit",
            Style::default().fg(Color::DarkGray),
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_library_dir_popup(
    frame: &mut Frame,
    area: Rect,
    input: &str,
    cursor: usize,
    error: Option<&str>,
    extra_dirs: &[PathBuf],
) {
    let popup = popup_area(area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow))
        .title(" Library Directories ")
        .title_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 4 || inner.width < 10 {
        return;
    }

    // Reserve lines: footer(1) + input(1) + error(1 if present) + separator(1)
    let error_lines: u16 = if error.is_some() { 1 } else { 0 };
    let bottom_reserved = 1 + 1 + error_lines + 1; // footer + input + error + separator
    let dir_list_height = inner.height.saturating_sub(bottom_reserved);

    let dir_list_area = Rect::new(inner.x, inner.y, inner.width, dir_list_height);
    let separator_area = Rect::new(inner.x, inner.y + dir_list_height, inner.width, 1);
    let input_area = Rect::new(inner.x, inner.y + dir_list_height + 1, inner.width, 1);
    let error_area = if error.is_some() {
        Rect::new(inner.x, inner.y + dir_list_height + 2, inner.width, 1)
    } else {
        Rect::default()
    };
    let footer_area = Rect::new(
        inner.x,
        inner.y + inner.height.saturating_sub(1),
        inner.width,
        1,
    );

    // Show current search directories
    let builtin = kit::default_search_dirs();
    let mut dir_lines: Vec<Line> = Vec::new();
    dir_lines.push(Line::from(Span::styled(
        " Kit directories:",
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
    )));
    for d in &builtin {
        dir_lines.push(Line::from(Span::styled(
            format!("   {}", d.display()),
            Style::default().fg(Color::DarkGray),
        )));
    }
    for d in extra_dirs {
        dir_lines.push(Line::from(Span::styled(
            format!("   {}", d.display()),
            Style::default().fg(Color::Green),
        )));
    }
    dir_lines.push(Line::from(Span::styled(
        " Mappings directory:",
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
    )));
    dir_lines.push(Line::from(Span::styled(
        format!("   {}", mapping::user_mappings_dir().display()),
        Style::default().fg(Color::DarkGray),
    )));

    let dir_paragraph = Paragraph::new(dir_lines).wrap(Wrap { trim: false });
    frame.render_widget(dir_paragraph, dir_list_area);

    // Separator
    let sep = Paragraph::new(Line::from(Span::styled(
        " Add directory:",
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
    )));
    frame.render_widget(sep, separator_area);

    // Input line with visible cursor
    let w = inner.width as usize;
    let display_input = if input.len() + 3 > w {
        // Scroll input so cursor is visible
        let start = cursor.saturating_sub(w.saturating_sub(4));
        &input[start..]
    } else {
        input
    };
    let cursor_in_display = cursor.min(display_input.len());

    let before = &display_input[..cursor_in_display];
    let cursor_char = display_input.get(cursor_in_display..cursor_in_display + 1).unwrap_or(" ");
    let after = if cursor_in_display + 1 <= display_input.len() {
        &display_input[cursor_in_display + 1..]
    } else {
        ""
    };

    let input_line = Line::from(vec![
        Span::styled(" > ", Style::default().fg(Color::Yellow)),
        Span::raw(before),
        Span::styled(
            cursor_char,
            Style::default().fg(Color::Black).bg(Color::White),
        ),
        Span::raw(after),
    ]);
    frame.render_widget(Paragraph::new(input_line), input_area);

    // Error line
    if let Some(err) = error {
        let err_line = Paragraph::new(Line::from(Span::styled(
            format!(" {}", err),
            Style::default().fg(Color::Red),
        )));
        frame.render_widget(err_line, error_area);
    }

    // Footer
    let footer = Paragraph::new(Line::from(Span::styled(
        " Enter add  Esc cancel",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(footer, footer_area);
}
