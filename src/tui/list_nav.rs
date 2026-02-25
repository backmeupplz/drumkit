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

/// Move a `ListState` selection up, skipping rows where `is_selectable` returns false.
pub(crate) fn list_up_skip(list_state: &mut ListState, len: usize, is_selectable: impl Fn(usize) -> bool) {
    if len == 0 {
        return;
    }
    let cur = list_state.selected().unwrap_or(0);
    let mut next = if cur == 0 { len - 1 } else { cur - 1 };
    // Walk up to find the next selectable row, wrapping at most once
    for _ in 0..len {
        if is_selectable(next) {
            list_state.select(Some(next));
            return;
        }
        next = if next == 0 { len - 1 } else { next - 1 };
    }
}

/// Move a `ListState` selection down, skipping rows where `is_selectable` returns false.
pub(crate) fn list_down_skip(list_state: &mut ListState, len: usize, is_selectable: impl Fn(usize) -> bool) {
    if len == 0 {
        return;
    }
    let cur = list_state.selected().unwrap_or(0);
    let mut next = if cur >= len - 1 { 0 } else { cur + 1 };
    for _ in 0..len {
        if is_selectable(next) {
            list_state.select(Some(next));
            return;
        }
        next = if next >= len - 1 { 0 } else { next + 1 };
    }
}

/// Find the first selectable index, or `None` if no rows are selectable.
pub(crate) fn first_selectable(len: usize, is_selectable: impl Fn(usize) -> bool) -> Option<usize> {
    (0..len).find(|&i| is_selectable(i))
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
