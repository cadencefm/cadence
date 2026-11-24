//! Key names and key-script sequences.
use super::*;

pub fn normalize_shifted_key(mut key: KeyEvent) -> KeyEvent {
    if key.modifiers.contains(KeyModifiers::SHIFT)
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        if let KeyCode::Char(c) = key.code {
            let mapped = match c {
                '/' => Some('?'),
                '=' => Some('+'),
                ',' => Some('<'),
                '.' => Some('>'),
                ';' => Some(':'),
                '[' => Some('{'),
                ']' => Some('}'),
                '`' => Some('~'),
                '1' => Some('!'),
                '4' => Some('$'),
                c if c.is_ascii_lowercase() => Some(c.to_ascii_uppercase()),
                _ => None,
            };
            if let Some(ch) = mapped {
                key.code = KeyCode::Char(ch);
            }
        }
    }
    key
}

/// Parse a compact key sequence like `"<Tab>llFo:bpm 124<Enter>?"` into `KeyEvent`s.
pub fn parse_key_sequence(input: &str) -> Vec<KeyEvent> {
    let mut events = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            let mut token = String::new();
            let mut closed = false;
            for ch in chars.by_ref() {
                if ch == '>' {
                    closed = true;
                    break;
                }
                token.push(ch);
            }
            if !closed {
                events.push(KeyEvent::new(KeyCode::Char('<'), KeyModifiers::NONE));
                for ch in token.chars() {
                    events.push(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
                }
                break;
            }
            let lower = token.to_ascii_lowercase();
            let event = match lower.as_str() {
                "tab" => KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
                "s-tab" | "shift-tab" | "backtab" => {
                    KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)
                }
                "c-home" | "ctrl-home" => KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL),
                "home" => KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
                "end" => KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
                "esc" | "escape" => KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                "enter" | "cr" | "return" => KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                "s-enter" | "shift-enter" | "s-cr" | "s-return" => {
                    KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)
                }
                "bs" | "backspace" => KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
                "m-bs" | "m-backspace" => KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT),
                "space" => KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
                "up" => KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
                "down" => KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                "left" => KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
                "right" => KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
                "m-up" => KeyEvent::new(KeyCode::Up, KeyModifiers::ALT),
                "m-down" => KeyEvent::new(KeyCode::Down, KeyModifiers::ALT),
                "m-left" => KeyEvent::new(KeyCode::Left, KeyModifiers::ALT),
                "m-right" => KeyEvent::new(KeyCode::Right, KeyModifiers::ALT),
                "m-s-up" | "m-shift-up" => {
                    KeyEvent::new(KeyCode::Up, KeyModifiers::ALT | KeyModifiers::SHIFT)
                }
                "m-s-down" | "m-shift-down" => {
                    KeyEvent::new(KeyCode::Down, KeyModifiers::ALT | KeyModifiers::SHIFT)
                }
                "m-s-left" | "m-shift-left" => {
                    KeyEvent::new(KeyCode::Left, KeyModifiers::ALT | KeyModifiers::SHIFT)
                }
                "m-s-right" | "m-shift-right" => {
                    KeyEvent::new(KeyCode::Right, KeyModifiers::ALT | KeyModifiers::SHIFT)
                }
                "pageup" | "page-up" | "pgup" => KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
                "pagedown" | "page-down" | "pgdown" => {
                    KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE)
                }
                "lt" => KeyEvent::new(KeyCode::Char('<'), KeyModifiers::NONE),
                "gt" => KeyEvent::new(KeyCode::Char('>'), KeyModifiers::NONE),
                // Alt with a named key: <m-enter>.
                "m-enter" | "m-cr" | "m-return" => KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT),
                // Ctrl+Shift+<char>: <c-s-p>, <c-shift-p>, <c-s-,>, <c-s-/>
                other
                    if (other.starts_with("c-s-") && other.chars().count() == 5)
                        || (other.starts_with("c-shift-") && other.chars().count() == 9) =>
                {
                    let ch = token.chars().last().unwrap();
                    let ch = if ch.is_ascii_lowercase() {
                        ch.to_ascii_uppercase()
                    } else {
                        ch
                    };
                    KeyEvent::new(
                        KeyCode::Char(ch),
                        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                    )
                }
                // Ctrl+<char>: preserves shift on uppercase (<c-P>) or shifted symbols (<c-?>, <c-<>).
                other if other.starts_with("c-") && other.chars().count() == 3 => {
                    let ch = token.chars().nth(2).unwrap();
                    let mods = if ch.is_ascii_uppercase() || matches!(ch, '?' | '<') {
                        KeyModifiers::CONTROL | KeyModifiers::SHIFT
                    } else {
                        KeyModifiers::CONTROL
                    };
                    KeyEvent::new(KeyCode::Char(ch), mods)
                }
                // Alt+Shift+<char>: <m-s-h>, <m-shift-h>
                other
                    if (other.starts_with("m-s-") && other.chars().count() == 5)
                        || (other.starts_with("m-shift-") && other.chars().count() == 9) =>
                {
                    let ch = token.chars().last().unwrap().to_ascii_uppercase();
                    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::ALT | KeyModifiers::SHIFT)
                }
                // Alt keeps its case: <m-X> is Alt+Shift+x.
                other if other.starts_with("m-") && other.chars().count() == 3 => {
                    let ch = token.chars().nth(2).unwrap();
                    let mods = if ch.is_ascii_uppercase() {
                        KeyModifiers::ALT | KeyModifiers::SHIFT
                    } else {
                        KeyModifiers::ALT
                    };
                    KeyEvent::new(KeyCode::Char(ch), mods)
                }
                _ => continue,
            };
            events.push(event);
        } else {
            events.push(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ctrl_shift_alt_arrows_and_alt_backspace_chords() {
        let seq = parse_key_sequence(
            "<c-p><c-s-p><c-P><c-,><c-s-,><c-?><m-bs><m-left><m-s-right><m-H><s-enter>",
        );
        assert_eq!(
            seq,
            vec![
                KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
                KeyEvent::new(
                    KeyCode::Char('P'),
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT
                ),
                KeyEvent::new(
                    KeyCode::Char('P'),
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT
                ),
                KeyEvent::new(KeyCode::Char(','), KeyModifiers::CONTROL),
                KeyEvent::new(
                    KeyCode::Char(','),
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT
                ),
                KeyEvent::new(
                    KeyCode::Char('?'),
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT
                ),
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT),
                KeyEvent::new(KeyCode::Left, KeyModifiers::ALT),
                KeyEvent::new(KeyCode::Right, KeyModifiers::ALT | KeyModifiers::SHIFT),
                KeyEvent::new(KeyCode::Char('H'), KeyModifiers::ALT | KeyModifiers::SHIFT),
                KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
            ]
        );
    }
}
