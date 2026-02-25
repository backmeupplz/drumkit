use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Frame,
};

/// Centered popup taking ~80% of the given area (min 30×6).
pub(crate) fn popup_area_percent(area: Rect) -> Rect {
    let popup_w = (area.width * 4 / 5).max(30).min(area.width);
    let popup_h = (area.height * 4 / 5).max(6).min(area.height);
    let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    Rect::new(x, y, popup_w, popup_h)
}

/// Centered popup with fixed dimensions, clamped to fit.
pub(crate) fn popup_area_fixed(area: Rect, w: u16, h: u16) -> Rect {
    let popup_w = w.min(area.width.saturating_sub(2));
    let popup_h = h.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    Rect::new(x, y, popup_w, popup_h)
}

/// Standard popup block: all borders, rounded, colored title in bold.
pub(crate) fn styled_block(title: &str, color: Color) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(title)
        .title_style(Style::default().fg(color).add_modifier(Modifier::BOLD))
}

/// Split an inner area into (content, 1-line footer).
pub(crate) fn content_footer_split(inner: Rect) -> (Rect, Rect) {
    let content = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1));
    let footer = Rect::new(
        inner.x,
        inner.y + inner.height.saturating_sub(1),
        inner.width,
        1,
    );
    (content, footer)
}

/// Render a single-line text input with a visible block cursor.
pub(crate) fn render_text_input(
    frame: &mut Frame,
    area: Rect,
    input: &str,
    cursor: usize,
    color: Color,
) {
    let w = area.width as usize;
    let display_input = if input.len() + 3 > w {
        let start = cursor.saturating_sub(w.saturating_sub(4));
        &input[start..]
    } else {
        input
    };
    let cursor_in_display = cursor.min(display_input.len());

    let before = &display_input[..cursor_in_display];
    let cursor_char = display_input
        .get(cursor_in_display..cursor_in_display + 1)
        .unwrap_or(" ");
    let after = if cursor_in_display + 1 <= display_input.len() {
        &display_input[cursor_in_display + 1..]
    } else {
        ""
    };

    let line = Line::from(vec![
        Span::styled(" > ", Style::default().fg(color)),
        Span::raw(before),
        Span::styled(
            cursor_char,
            Style::default().fg(Color::Black).bg(Color::White),
        ),
        Span::raw(after),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// Render a progress popup with name, status text, progress bar, and optional footer.
pub(crate) fn render_progress_popup(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    name: &str,
    status_text: &str,
    cur: usize,
    tot: usize,
    color: Color,
    footer: &str,
) {
    let popup = popup_area_fixed(area, 36, 7);
    frame.render_widget(Clear, popup);

    let block = styled_block(title, color);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 3 || inner.width < 10 {
        return;
    }

    let name_w = inner.width as usize;
    let display_name = if name.len() > name_w {
        &name[..name_w]
    } else {
        name
    };

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
            format!(" {}", status_text),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "\u{2588}".repeat(filled),
                Style::default().fg(color),
            ),
            Span::styled(
                "\u{2591}".repeat(empty),
                Style::default().fg(Color::Rgb(50, 50, 60)),
            ),
        ]),
    ];

    if inner.height >= 4 && !footer.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(" {}", footer),
            Style::default().fg(Color::DarkGray),
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Render a single-line DarkGray footer hint.
pub(crate) fn render_footer_hint(frame: &mut Frame, area: Rect, text: &str) {
    let footer = Paragraph::new(Line::from(Span::styled(
        text,
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(footer, area);
}
