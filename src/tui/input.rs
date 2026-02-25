use crossterm::event::KeyCode;

/// Handle common text-input key events (Char, Backspace, Delete, Left, Right, Home, End).
/// Clears `error` on any edit. Returns `true` if the key was handled.
pub(crate) fn handle_text_input_key(
    input: &mut String,
    cursor: &mut usize,
    error: Option<&mut Option<String>>,
    key: KeyCode,
) -> bool {
    match key {
        KeyCode::Char(c) => {
            input.insert(*cursor, c);
            *cursor += 1;
            if let Some(err) = error {
                *err = None;
            }
            true
        }
        KeyCode::Backspace => {
            if *cursor > 0 {
                *cursor -= 1;
                input.remove(*cursor);
                if let Some(err) = error {
                    *err = None;
                }
            }
            true
        }
        KeyCode::Delete => {
            if *cursor < input.len() {
                input.remove(*cursor);
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
            if *cursor < input.len() {
                *cursor += 1;
            }
            true
        }
        KeyCode::Home => {
            *cursor = 0;
            true
        }
        KeyCode::End => {
            *cursor = input.len();
            true
        }
        _ => false,
    }
}
