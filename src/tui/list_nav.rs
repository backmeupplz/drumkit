use ratatui::widgets::ListState;

/// Move a `ListState` selection up with wrapping.
pub(crate) fn list_up(list_state: &mut ListState, len: usize) {
    if len == 0 {
        return;
    }
    let cur = list_state.selected().unwrap_or(0);
    list_state.select(Some(if cur == 0 { len - 1 } else { cur - 1 }));
}

/// Move a `ListState` selection down with wrapping.
pub(crate) fn list_down(list_state: &mut ListState, len: usize) {
    if len == 0 {
        return;
    }
    let cur = list_state.selected().unwrap_or(0);
    list_state.select(Some(if cur >= len - 1 { 0 } else { cur + 1 }));
}

/// Move a plain `usize` index up with wrapping.
pub(crate) fn index_up(selected: &mut usize, len: usize) {
    if len == 0 {
        return;
    }
    *selected = if *selected == 0 { len - 1 } else { *selected - 1 };
}

/// Move a plain `usize` index down with wrapping.
pub(crate) fn index_down(selected: &mut usize, len: usize) {
    if len == 0 {
        return;
    }
    *selected = if *selected >= len - 1 { 0 } else { *selected + 1 };
}
