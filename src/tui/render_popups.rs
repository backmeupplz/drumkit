use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::widgets::{
    content_footer_split, popup_area_fixed, popup_area_percent, render_footer_hint,
    render_progress_popup, render_text_input, styled_block,
};
use super::{AppState, DirPopupMode, Popup};
use crate::{audio, download, kit, mapping, midi};

pub(super) fn render_popup(frame: &mut Frame, area: Rect, popup: &Popup, state: &AppState, extra_kit_dirs: &[PathBuf], extra_mapping_dirs: &[PathBuf], kit_repos: &[String]) {
    match popup {
        Popup::Log { scroll } => render_log_popup(frame, area, state, *scroll),
        Popup::KitPicker { kits, list_state } => render_kit_popup(frame, area, kits, list_state),
        Popup::AudioPicker { devices, list_state } => render_audio_popup(frame, area, devices, list_state),
        Popup::MidiPicker { devices, list_state } => render_midi_popup(frame, area, devices, list_state),
        Popup::LibraryDir { mode, selected, input, cursor, error } => render_library_dir_popup(frame, area, mode, *selected, input, *cursor, error.as_deref(), extra_kit_dirs, extra_mapping_dirs),
        Popup::Loading { kit_name, progress, total } => render_loading_popup(frame, area, kit_name, progress, total),
        Popup::MappingPicker { mappings, list_state } => render_mapping_popup(frame, area, mappings, list_state, state),
        Popup::DeleteMapping { name, .. } => render_delete_mapping_popup(frame, area, name),
        Popup::NoteRename { note, input, cursor } => render_note_rename_popup(frame, area, *note, input, *cursor),
        Popup::KitStoreFetching => render_kit_store_fetching(frame, area),
        Popup::KitStore { kits, list_state } => render_kit_store(frame, area, kits, list_state),
        Popup::KitDownloading { kit_name, progress, total } => render_kit_downloading(frame, area, kit_name, progress, total),
        Popup::KitStoreRepos { selected, adding, input, cursor, error } => render_kit_store_repos(frame, area, kit_repos, *selected, *adding, input, *cursor, error.as_deref()),
    }
}

fn render_log_popup(frame: &mut Frame, area: Rect, state: &AppState, scroll: usize) {
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
        let start = scroll.min(state.log_lines.len().saturating_sub(visible));

        let lines: Vec<Line> = state.log_lines[start..]
            .iter()
            .take(visible)
            .map(|l| Line::from(Span::styled(format!(" {}", l), Style::default().fg(Color::DarkGray))))
            .collect();

        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, content_area);
    }

    render_footer_hint(frame, footer_area, " \u{2191}\u{2193} scroll  Esc/l close  q quit");
}

fn render_kit_popup(frame: &mut Frame, area: Rect, kits: &[kit::DiscoveredKit], list_state: &ListState) {
    let popup = popup_area_percent(area);
    frame.render_widget(Clear, popup);

    let block = styled_block(" Select Kit ", Color::Cyan);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let (content_area, footer_area) = content_footer_split(inner);

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

    render_footer_hint(frame, footer_area, " \u{2191}\u{2193} navigate  Enter select  Esc/k close  q quit");
}

fn render_audio_popup(frame: &mut Frame, area: Rect, devices: &[audio::AudioDevice], list_state: &ListState) {
    let popup = popup_area_percent(area);
    frame.render_widget(Clear, popup);

    let block = styled_block(" Select Audio Device ", Color::Green);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let (content_area, footer_area) = content_footer_split(inner);

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

    render_footer_hint(frame, footer_area, " \u{2191}\u{2193} navigate  Enter select  Esc/a close  q quit");
}

fn render_midi_popup(frame: &mut Frame, area: Rect, devices: &[midi::MidiDevice], list_state: &ListState) {
    let popup = popup_area_percent(area);
    frame.render_widget(Clear, popup);

    let block = styled_block(" Select MIDI Input ", Color::Magenta);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let (content_area, footer_area) = content_footer_split(inner);

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

    render_footer_hint(frame, footer_area, " \u{2191}\u{2193} navigate  Enter select  Esc/m close  q quit");
}

fn render_mapping_popup(
    frame: &mut Frame,
    area: Rect,
    mappings: &[mapping::NoteMapping],
    list_state: &ListState,
    state: &AppState,
) {
    let popup = popup_area_percent(area);
    frame.render_widget(Clear, popup);

    let block = styled_block(" Select Mapping ", Color::Yellow);
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
    render_footer_hint(frame, path_area, &format!(" User: {}", mappings_dir.display()));
    render_footer_hint(frame, footer_area, " \u{2191}\u{2193} navigate  Enter select  d delete  Esc/n close  q quit");
}

fn render_delete_mapping_popup(frame: &mut Frame, area: Rect, name: &str) {
    let popup = popup_area_fixed(area, 50, 7);
    frame.render_widget(Clear, popup);

    let block = styled_block(" Delete Mapping ", Color::Red);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 3 || inner.width < 10 {
        return;
    }

    let (content_area, footer_area) = content_footer_split(inner);

    let msg = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  Delete \"{}\"?", name),
            Style::default().fg(Color::White),
        )),
    ]);
    frame.render_widget(msg, content_area);

    render_footer_hint(frame, footer_area, " y/Enter confirm  any other key cancel");
}

fn render_note_rename_popup(
    frame: &mut Frame,
    area: Rect,
    note: u8,
    input: &str,
    cursor: usize,
) {
    let popup = popup_area_fixed(area, 40, 7);
    frame.render_widget(Clear, popup);

    let title = format!(" Rename Note {} ", note);
    let block = styled_block(&title, Color::Yellow);
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
    frame.render_widget(label, Rect::new(inner.x, inner.y, inner.width, 1));

    // Input line with cursor
    render_text_input(
        frame,
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
        input,
        cursor,
        Color::Yellow,
    );

    // Footer
    if inner.height >= 3 {
        let footer_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);
        render_footer_hint(frame, footer_area, " Enter save  Esc cancel");
    }
}

fn render_loading_popup(
    frame: &mut Frame,
    area: Rect,
    kit_name: &str,
    progress: &Arc<AtomicUsize>,
    total: &Arc<AtomicUsize>,
) {
    let cur = progress.load(Ordering::Relaxed);
    let tot = total.load(Ordering::Relaxed);
    let status = if tot == 0 {
        "Scanning...".to_string()
    } else {
        format!("Loading... {}/{} files", cur, tot)
    };
    render_progress_popup(
        frame, area, " Loading Kit ", kit_name, &status, cur, tot,
        Color::Cyan, "Esc cancel  q quit",
    );
}

fn render_library_dir_popup(
    frame: &mut Frame,
    area: Rect,
    mode: &DirPopupMode,
    selected: usize,
    input: &str,
    cursor: usize,
    error: Option<&str>,
    extra_kit_dirs: &[PathBuf],
    extra_mapping_dirs: &[PathBuf],
) {
    let popup = popup_area_percent(area);
    frame.render_widget(Clear, popup);

    let title = match mode {
        DirPopupMode::Browse => " Library Directories ",
        DirPopupMode::AddKit => " Add Kit Directory ",
        DirPopupMode::AddMapping => " Add Mapping Directory ",
    };

    let block = styled_block(title, Color::Yellow);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 4 || inner.width < 10 {
        return;
    }

    match mode {
        DirPopupMode::Browse => {
            render_browse_mode(frame, inner, selected, extra_kit_dirs, extra_mapping_dirs);
        }
        DirPopupMode::AddKit | DirPopupMode::AddMapping => {
            render_add_mode(frame, inner, input, cursor, error);
        }
    }
}

fn render_browse_mode(
    frame: &mut Frame,
    inner: Rect,
    selected: usize,
    extra_kit_dirs: &[PathBuf],
    extra_mapping_dirs: &[PathBuf],
) {
    let (content_area, footer_area) = content_footer_split(inner);

    let builtin_kit = kit::default_search_dirs();
    let builtin_mapping_dir = mapping::user_mappings_dir();

    let mut lines: Vec<Line> = Vec::new();

    // Kit directories section
    lines.push(Line::from(Span::styled(
        " Kit directories:",
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
    )));
    for d in &builtin_kit {
        lines.push(Line::from(Span::styled(
            format!("   {}", d.display()),
            Style::default().fg(Color::DarkGray),
        )));
    }

    // User-added kit dirs (selectable)
    let mut user_idx = 0;
    for d in extra_kit_dirs {
        let is_selected = user_idx == selected;
        let prefix = if is_selected { " \u{25b8} " } else { "   " };
        let style = if is_selected {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };
        lines.push(Line::from(Span::styled(
            format!("{}{}", prefix, d.display()),
            style,
        )));
        user_idx += 1;
    }

    lines.push(Line::from(""));

    // Mapping directories section
    lines.push(Line::from(Span::styled(
        " Mapping directories:",
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!("   {}", builtin_mapping_dir.display()),
        Style::default().fg(Color::DarkGray),
    )));

    // User-added mapping dirs (selectable, continuing from kit user_idx)
    for d in extra_mapping_dirs {
        let is_selected = user_idx == selected;
        let prefix = if is_selected { " \u{25b8} " } else { "   " };
        let style = if is_selected {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };
        lines.push(Line::from(Span::styled(
            format!("{}{}", prefix, d.display()),
            style,
        )));
        user_idx += 1;
    }

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, content_area);

    render_footer_hint(frame, footer_area, " a add kit dir  A add mapping dir  Del remove  Esc close  q quit");
}

fn render_add_mode(
    frame: &mut Frame,
    inner: Rect,
    input: &str,
    cursor: usize,
    error: Option<&str>,
) {
    // Label
    let label = Paragraph::new(Line::from(Span::styled(
        " Enter directory path:",
        Style::default().fg(Color::White),
    )));
    frame.render_widget(label, Rect::new(inner.x, inner.y, inner.width, 1));

    // Input line with visible cursor
    render_text_input(
        frame,
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
        input,
        cursor,
        Color::Yellow,
    );

    // Error line
    if let Some(err) = error {
        let err_area = Rect::new(inner.x, inner.y + 2, inner.width, 1);
        let err_line = Paragraph::new(Line::from(Span::styled(
            format!(" {}", err),
            Style::default().fg(Color::Red),
        )));
        frame.render_widget(err_line, err_area);
    }

    // Footer
    let footer_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);
    render_footer_hint(frame, footer_area, " Enter add  Esc back");
}

fn render_kit_store_fetching(frame: &mut Frame, area: Rect) {
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

fn render_kit_store(
    frame: &mut Frame,
    area: Rect,
    kits: &[download::RemoteKit],
    list_state: &ListState,
) {
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
        let show_repo = kits.iter().map(|k| &k.repo).collect::<std::collections::HashSet<_>>().len() > 1;
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
                if show_repo {
                    spans.push(Span::styled(
                        format!("  [{}]", kit.repo),
                        Style::default().fg(Color::Rgb(100, 100, 120)),
                    ));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();

        let list = List::new(items)
            .highlight_symbol("\u{25b8} ")
            .highlight_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

        let mut ls = list_state.clone();
        frame.render_stateful_widget(list, content_area, &mut ls);
    }

    render_footer_hint(frame, footer_area, " \u{2191}\u{2193} navigate  Enter download  r repos  Esc/s close  q quit");
}

fn render_kit_store_repos(
    frame: &mut Frame,
    area: Rect,
    kit_repos: &[String],
    selected: usize,
    adding: bool,
    input: &str,
    cursor: usize,
    error: Option<&str>,
) {
    let popup = popup_area_percent(area);
    frame.render_widget(Clear, popup);

    let block = styled_block(" Kit Repos ", Color::Yellow);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 3 || inner.width < 10 {
        return;
    }

    if adding {
        // Add mode: show input field
        let label = Paragraph::new(Line::from(Span::styled(
            " Enter repo (owner/repo):",
            Style::default().fg(Color::White),
        )));
        frame.render_widget(label, Rect::new(inner.x, inner.y, inner.width, 1));

        render_text_input(
            frame,
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
            input,
            cursor,
            Color::Yellow,
        );

        if let Some(err) = error {
            let err_area = Rect::new(inner.x, inner.y + 2, inner.width, 1);
            let err_line = Paragraph::new(Line::from(Span::styled(
                format!(" {}", err),
                Style::default().fg(Color::Red),
            )));
            frame.render_widget(err_line, err_area);
        }

        let footer_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);
        render_footer_hint(frame, footer_area, " Enter add  Esc back");
    } else {
        // Browse mode: list repos
        let (content_area, footer_area) = content_footer_split(inner);

        if kit_repos.is_empty() {
            let msg = Paragraph::new(Line::from(Span::styled(
                " No repos configured. Press a to add one.",
                Style::default().fg(Color::DarkGray),
            )));
            frame.render_widget(msg, content_area);
        } else {
            let mut lines: Vec<Line> = Vec::new();
            for (i, repo) in kit_repos.iter().enumerate() {
                let is_selected = i == selected;
                let prefix = if is_selected { " \u{25b8} " } else { "   " };
                let style = if is_selected {
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                lines.push(Line::from(Span::styled(
                    format!("{}{}", prefix, repo),
                    style,
                )));
            }
            frame.render_widget(Paragraph::new(lines), content_area);
        }

        render_footer_hint(frame, footer_area, " a add  d/Del remove  Esc back  q quit");
    }
}

fn render_kit_downloading(
    frame: &mut Frame,
    area: Rect,
    kit_name: &str,
    progress: &Arc<AtomicUsize>,
    total: &Arc<AtomicUsize>,
) {
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
