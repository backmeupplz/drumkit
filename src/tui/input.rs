use crossterm::event::KeyCode;

/// Byte offset of the `idx`-th char (or `s.len()` past the end).
fn byte_at(s: &str, idx: usize) -> usize {
    s.char_indices().nth(idx).map_or(s.len(), |(b, _)| b)
}

/// Handle common text-input key events (Char, Backspace, Delete, Left, Right, Home, End).
/// `cursor` is a char index, not a byte offset.
/// Clears `error` on any edit. Returns `true` if the key was handled.
pub(crate) fn handle_text_input_key(
    input: &mut String,
    cursor: &mut usize,
    error: Option<&mut Option<String>>,
    key: KeyCode,
) -> bool {
    let len = input.chars().count();
    *cursor = (*cursor).min(len);
    match key {
        KeyCode::Char(c) => {
            input.insert(byte_at(input, *cursor), c);
            *cursor += 1;
            if let Some(err) = error {
                *err = None;
            }
            true
        }
        KeyCode::Backspace => {
            if *cursor > 0 {
                *cursor -= 1;
                input.remove(byte_at(input, *cursor));
                if let Some(err) = error {
                    *err = None;
                }
            }
            true
        }
        KeyCode::Delete => {
            if *cursor < len {
                input.remove(byte_at(input, *cursor));
                if let Some(err) = error {
                    *err = None;
                }
            }
            true
        }
        KeyCode::Left => {
            *cursor = cursor.saturating_sub(1);
            true
        }
        KeyCode::Right => {
            if *cursor < len {
                *cursor += 1;
            }
            true
        }
        KeyCode::Home => {
            *cursor = 0;
            true
        }
        KeyCode::End => {
            *cursor = len;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multibyte_editing_does_not_panic() {
        let mut s = String::new();
        let mut c = 0;
        for k in [
            KeyCode::Char('é'),
            KeyCode::Char('🥁'),
            KeyCode::Left,
            KeyCode::Char('x'),
            KeyCode::Delete,
            KeyCode::Home,
            KeyCode::Delete,
            KeyCode::End,
            KeyCode::Backspace,
        ] {
            handle_text_input_key(&mut s, &mut c, None, k);
        }
        assert_eq!(s, "");
        handle_text_input_key(&mut s, &mut c, None, KeyCode::Char('ж'));
        assert_eq!((s.as_str(), c), ("ж", 1));
    }
}
