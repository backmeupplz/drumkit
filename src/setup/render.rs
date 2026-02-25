use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};
use std::sync::atomic::Ordering;

use super::state::{SetupState, SetupStep, SetupStorePopup};
use crate::tui::widgets::{
    content_footer_split, popup_area_fixed, popup_area_percent, render_footer_hint,
    render_progress_popup, styled_block,
};
use crate::download;

pub(super) fn render_loading(frame: &mut Frame) {
    let area = frame.area();

    let outer = styled_block(" drumkit setup ", Color::DarkGray);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let msg = Paragraph::new(Line::from(Span::styled(
        "  Discovering kits and devices...",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(msg, inner);
}

pub(super) fn setup_ui(frame: &mut Frame, state: &SetupState) {
    let area = frame.area();

    if area.width < 30 || area.height < 8 {
        let msg = Paragraph::new("Terminal too small\nNeed at least 30x8\nq: quit")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(msg, area);
        return;
    }

    let outer = styled_block(" drumkit setup ", Color::DarkGray);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // breadcrumb
            Constraint::Length(1), // title
            Constraint::Min(1),   // list
            Constraint::Length(1), // footer
        ])
        .split(inner);

    render_breadcrumb(frame, chunks[0], state);
    render_step_title(frame, chunks[1], state);
    render_list(frame, chunks[2], state);
    render_setup_footer(frame, chunks[3], state);

    // Log popup overlay
    if state.show_log {
        render_log_popup(frame, area, state);
    }

    // Store popup overlay
    if let Some(ref store) = state.store_popup {
        render_store_popup(frame, area, store);
    }
}

fn render_breadcrumb(frame: &mut Frame, area: Rect, state: &SetupState) {
    if area.width < 10 || area.height == 0 {
        return;
    }

    let mut spans = Vec::new();
    spans.push(Span::raw("  "));

    // Kit step
    let kit_style = match state.step {
        SetupStep::Kit => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        _ => Style::default().fg(Color::Green),
    };
    match &state.selected_kit {
        Some((name, _)) if state.step != SetupStep::Kit => {
            spans.push(Span::styled(format!("Kit: {}", name), kit_style));
        }
        _ => {
            spans.push(Span::styled("Kit", kit_style));
        }
    }

    spans.push(Span::styled(" \u{25b8} ", Style::default().fg(Color::DarkGray)));

    // Audio step
    let audio_style = match state.step {
        SetupStep::AudioDevice => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        SetupStep::MidiPort => Style::default().fg(Color::Green),
        _ => Style::default().fg(Color::DarkGray),
    };
    match &state.selected_audio {
        Some((name, _)) if state.step == SetupStep::MidiPort => {
            let display = if name.len() > 20 {
                format!("Audio: {}...", &name[..17])
            } else {
                format!("Audio: {}", name)
            };
            spans.push(Span::styled(display, audio_style));
        }
        _ => {
            spans.push(Span::styled("Audio Device", audio_style));
        }
    }

    spans.push(Span::styled(" \u{25b8} ", Style::default().fg(Color::DarkGray)));

    // MIDI step
    let midi_style = match state.step {
        SetupStep::MidiPort => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        _ => Style::default().fg(Color::DarkGray),
    };
    spans.push(Span::styled("MIDI", midi_style));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_step_title(frame: &mut Frame, area: Rect, state: &SetupState) {
    if area.height == 0 {
        return;
    }

    let title = match state.step {
        SetupStep::Kit => "  Select Kit",
        SetupStep::AudioDevice => "  Select Audio Device",
        SetupStep::MidiPort => "  Select MIDI Input",
    };

    let paragraph = Paragraph::new(Line::from(Span::styled(
        title,
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    frame.render_widget(paragraph, area);
}

fn render_list(frame: &mut Frame, area: Rect, state: &SetupState) {
    if area.height == 0 || area.width < 5 {
        return;
    }

    let padded = Rect::new(
        area.x + 2,
        area.y,
        area.width.saturating_sub(4),
        area.height,
    );

    match state.step {
        SetupStep::Kit => render_kit_list(frame, padded, state),
        SetupStep::AudioDevice => render_audio_list(frame, padded, state),
        SetupStep::MidiPort => render_midi_list(frame, padded, state),
    }

    if let Some(ref msg) = state.error_message {
        let err_area = Rect::new(
            area.x + 2,
            area.y + area.height.saturating_sub(1),
            area.width.saturating_sub(4),
            1,
        );
        let err = Paragraph::new(Line::from(Span::styled(
            msg.as_str(),
            Style::default().fg(Color::Red),
        )));
        frame.render_widget(err, err_area);
    }
}

fn render_kit_list(frame: &mut Frame, area: Rect, state: &SetupState) {
    if state.kits.is_empty() {
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No kits found.",
                Style::default().fg(Color::Yellow),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Press s to open the Kit Store and download kits.",
                Style::default().fg(Color::Cyan),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Or place WAV files in a subdirectory of:",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  ./kits/<kit-name>/",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  ~/.local/share/drumkit/kits/<kit-name>/",
                Style::default().fg(Color::DarkGray),
            )),
        ]);
        frame.render_widget(msg, area);
        return;
    }

    let items: Vec<ListItem> = state
        .kits
        .iter()
        .map(|kit| {
            ListItem::new(Line::from(vec![
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
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let mut list_state = state.kit_list_state.clone();
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_audio_list(frame: &mut Frame, area: Rect, state: &SetupState) {
    if state.audio_devices.is_empty() {
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No audio output devices found.",
                Style::default().fg(Color::Yellow),
            )),
        ]);
        frame.render_widget(msg, area);
        return;
    }

    let items: Vec<ListItem> = state
        .audio_devices
        .iter()
        .map(|dev| ListItem::new(Line::from(Span::raw(&dev.name))))
        .collect();

    let list = List::new(items)
        .highlight_symbol("\u{25b8} ")
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let mut list_state = state.audio_list_state.clone();
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_midi_list(frame: &mut Frame, area: Rect, state: &SetupState) {
    if state.midi_devices.is_empty() {
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No MIDI input devices found.",
                Style::default().fg(Color::Yellow),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Connect your drum module via USB and press Enter to rescan.",
                Style::default().fg(Color::DarkGray),
            )),
        ]);
        frame.render_widget(msg, area);
        return;
    }

    let items: Vec<ListItem> = state
        .midi_devices
        .iter()
        .map(|dev| ListItem::new(Line::from(Span::raw(&dev.name))))
        .collect();

    let list = List::new(items)
        .highlight_symbol("\u{25b8} ")
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let mut list_state = state.midi_list_state.clone();
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_log_popup(frame: &mut Frame, area: Rect, state: &SetupState) {
    let popup = popup_area_percent(area);
    frame.render_widget(Clear, popup);

    let title = format!(" Log ({} lines) ", state.log_lines.len());
    let block = styled_block(&title, Color::Yellow);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let (content_area, footer_area) = content_footer_split(inner);

    if state.log_lines.is_empty() {
        let msg = Paragraph::new(Line::from(Span::styled(
            " No log messages.",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(msg, content_area);
    } else {
        let visible = content_area.height as usize;
        let start = state.log_scroll.min(state.log_lines.len().saturating_sub(visible));

        let lines: Vec<Line> = state.log_lines[start..]
            .iter()
            .take(visible)
            .map(|l| {
                Line::from(Span::styled(
                    format!(" {}", l),
                    Style::default().fg(Color::DarkGray),
                ))
            })
            .collect();

        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, content_area);
    }

    render_footer_hint(frame, footer_area, " \u{2191}\u{2193} scroll  Esc/l close  q quit");
}

fn render_setup_footer(frame: &mut Frame, area: Rect, state: &SetupState) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let can_go_back = match state.step {
        SetupStep::Kit => false,
        SetupStep::AudioDevice => !state.kits.is_empty(),
        SetupStep::MidiPort => true,
    };

    let has_items = match state.step {
        SetupStep::Kit => !state.kits.is_empty(),
        SetupStep::AudioDevice => !state.audio_devices.is_empty(),
        SetupStep::MidiPort => !state.midi_devices.is_empty(),
    };

    let mut hints = vec![Span::styled("  ", Style::default())];

    if has_items {
        hints.push(Span::styled(
            "\u{2191}\u{2193} navigate",
            Style::default().fg(Color::DarkGray),
        ));
        hints.push(Span::raw("  "));
        hints.push(Span::styled(
            "Enter select",
            Style::default().fg(Color::DarkGray),
        ));
    }

    if can_go_back {
        hints.push(Span::raw("  "));
        hints.push(Span::styled(
            "Esc back",
            Style::default().fg(Color::DarkGray),
        ));
    }

    if state.step == SetupStep::Kit {
        hints.push(Span::raw("  "));
        hints.push(Span::styled(
            "s store",
            Style::default().fg(Color::DarkGray),
        ));
    }

    hints.push(Span::raw("  "));
    hints.push(Span::styled(
        "l log",
        Style::default().fg(Color::DarkGray),
    ));
    hints.push(Span::raw("  "));
    hints.push(Span::styled(
        "q quit",
        Style::default().fg(Color::DarkGray),
    ));

    frame.render_widget(Paragraph::new(Line::from(hints)), area);
}

fn render_store_popup(frame: &mut Frame, area: Rect, store: &SetupStorePopup) {
    match store {
        SetupStorePopup::Fetching => {
            let popup = popup_area_fixed(area, 40, 5);
            frame.render_widget(Clear, popup);

            let block = styled_block(" Kit Store ", Color::Cyan);
            let inner = block.inner(popup);
            frame.render_widget(block, popup);

            let lines = vec![
                Line::from(Span::styled(
                    " Fetching kit list...",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    " Esc cancel  q quit",
                    Style::default().fg(Color::DarkGray),
                )),
            ];
            frame.render_widget(Paragraph::new(lines), inner);
        }
        SetupStorePopup::Browse { kits, list_state } => {
            let popup = popup_area_percent(area);
            frame.render_widget(Clear, popup);

            let block = styled_block(" Kit Store ", Color::Cyan);
            let inner = block.inner(popup);
            frame.render_widget(block, popup);

            if inner.height < 2 || inner.width < 4 {
                return;
            }

            let (content_area, footer_area) = content_footer_split(inner);

            if kits.is_empty() {
                let msg = Paragraph::new(Line::from(Span::styled(
                    " No kits available.",
                    Style::default().fg(Color::Yellow),
                )));
                frame.render_widget(msg, content_area);
            } else {
                let items: Vec<ListItem> = kits
                    .iter()
                    .map(|kit| {
                        let mut spans = vec![Span::raw(" ")];
                        if kit.installed {
                            spans.push(Span::styled(
                                "\u{2713} ",
                                Style::default().fg(Color::Green),
                            ));
                        } else {
                            spans.push(Span::raw("  "));
                        }
                        spans.push(Span::raw(&kit.name));
                        spans.push(Span::styled(
                            format!(
                                "  ({} files, {})",
                                kit.file_count,
                                download::format_size(kit.total_bytes),
                            ),
                            Style::default().fg(Color::DarkGray),
                        ));
                        ListItem::new(Line::from(spans))
                    })
                    .collect();

                let list = List::new(items)
                    .highlight_symbol("\u{25b8} ")
                    .highlight_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

                let mut ls = list_state.clone();
                frame.render_stateful_widget(list, content_area, &mut ls);
            }

            render_footer_hint(frame, footer_area, " \u{2191}\u{2193} navigate  Enter download  Esc/s close  q quit");
        }
        SetupStorePopup::Downloading { kit_name, progress, total } => {
            let cur = progress.load(Ordering::Relaxed);
            let tot = total.load(Ordering::Relaxed);
            let status = if tot == 0 {
                "Preparing...".to_string()
            } else {
                format!("Downloading... {}/{} files", cur, tot)
            };
            render_progress_popup(
                frame, area, " Downloading Kit ", kit_name, &status, cur, tot,
                Color::Cyan, "Esc cancel  q quit",
            );
        }
    }
}
