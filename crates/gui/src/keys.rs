//! Exact `winit 0.30` -> `cadence_app::keys::KeyEvent` translation.
//!
//! Preserves native `Press` / `Repeat` / `Release` events for sustained Jam notes
//! and translates macOS/Linux/Windows `Alt` chords cleanly.
use cadence_app::keys::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use winit::event::{ElementState, KeyEvent as WinitKeyEvent};
use winit::keyboard::{Key, KeyCode as PhysicalCode, ModifiersState, NamedKey, PhysicalKey};

pub fn from_winit(event: &WinitKeyEvent, mods: ModifiersState) -> Option<KeyEvent> {
    let mut modifiers = KeyModifiers::NONE;
    if mods.shift_key() {
        modifiers |= KeyModifiers::SHIFT;
    }
    if mods.control_key() {
        modifiers |= KeyModifiers::CONTROL;
    }
    if mods.alt_key() {
        modifiers |= KeyModifiers::ALT;
    }

    let kind = match event.state {
        ElementState::Pressed if event.repeat => KeyEventKind::Repeat,
        ElementState::Pressed => KeyEventKind::Press,
        ElementState::Released => KeyEventKind::Release,
    };

    let code = match &event.logical_key {
        Key::Named(named) => match named {
            NamedKey::Enter => {
                if mods.control_key() && event.physical_key == PhysicalKey::Code(PhysicalCode::KeyM)
                {
                    physical_fallback(event.physical_key, mods.shift_key())?
                } else {
                    KeyCode::Enter
                }
            }
            NamedKey::Escape => KeyCode::Esc,
            NamedKey::Backspace => {
                if mods.control_key() && event.physical_key == PhysicalKey::Code(PhysicalCode::KeyH)
                {
                    physical_fallback(event.physical_key, mods.shift_key())?
                } else {
                    KeyCode::Backspace
                }
            }
            NamedKey::Tab => {
                if mods.control_key() && event.physical_key == PhysicalKey::Code(PhysicalCode::KeyI)
                {
                    physical_fallback(event.physical_key, mods.shift_key())?
                } else if mods.shift_key() {
                    KeyCode::BackTab
                } else {
                    KeyCode::Tab
                }
            }
            NamedKey::Space => KeyCode::Char(' '),
            NamedKey::ArrowUp => KeyCode::Up,
            NamedKey::ArrowDown => KeyCode::Down,
            NamedKey::ArrowLeft => KeyCode::Left,
            NamedKey::ArrowRight => KeyCode::Right,
            NamedKey::Home => KeyCode::Home,
            NamedKey::End => KeyCode::End,
            NamedKey::PageUp => KeyCode::PageUp,
            NamedKey::PageDown => KeyCode::PageDown,
            _ => return None,
        },
        Key::Character(text) => {
            let ch = text.chars().next()?;
            // On macOS/international layouts when Alt or Ctrl is held, prefer the
            // uncomposed ASCII physical key if logical_key turned into a non-ASCII
            // or ASCII control character (e.g. Ctrl-- -> 0x1F).
            if (mods.alt_key() || mods.control_key()) && (!ch.is_ascii() || ch.is_ascii_control()) {
                physical_fallback(event.physical_key, mods.shift_key())?
            } else {
                KeyCode::Char(ch)
            }
        }
        Key::Dead(_) => physical_fallback(event.physical_key, mods.shift_key())?,
        Key::Unidentified(_) => return None,
    };

    // Map native macOS Cmd+= / Cmd+- / Cmd+0 zoom chords cleanly onto Cadence's
    // Ctrl-zoom bindings while leaving all OS Cmd shortcuts (Cmd+Q/H/M/etc.) untouched.
    if mods.super_key() && !mods.alt_key() && !mods.control_key() {
        if matches!(code, KeyCode::Char('=' | '+' | '-' | '0')) {
            modifiers |= KeyModifiers::CONTROL;
        } else {
            return None;
        }
    }

    Some(KeyEvent::new_with_kind(code, modifiers, kind))
}

fn physical_fallback(physical: PhysicalKey, shift: bool) -> Option<KeyCode> {
    let PhysicalKey::Code(code) = physical else {
        return None;
    };
    let c = match code {
        PhysicalCode::KeyA => 'a',
        PhysicalCode::KeyB => 'b',
        PhysicalCode::KeyC => 'c',
        PhysicalCode::KeyD => 'd',
        PhysicalCode::KeyE => 'e',
        PhysicalCode::KeyF => 'f',
        PhysicalCode::KeyG => 'g',
        PhysicalCode::KeyH => 'h',
        PhysicalCode::KeyI => 'i',
        PhysicalCode::KeyJ => 'j',
        PhysicalCode::KeyK => 'k',
        PhysicalCode::KeyL => 'l',
        PhysicalCode::KeyM => 'm',
        PhysicalCode::KeyN => 'n',
        PhysicalCode::KeyO => 'o',
        PhysicalCode::KeyP => 'p',
        PhysicalCode::KeyQ => 'q',
        PhysicalCode::KeyR => 'r',
        PhysicalCode::KeyS => 's',
        PhysicalCode::KeyT => 't',
        PhysicalCode::KeyU => 'u',
        PhysicalCode::KeyV => 'v',
        PhysicalCode::KeyW => 'w',
        PhysicalCode::KeyX => 'x',
        PhysicalCode::KeyY => 'y',
        PhysicalCode::KeyZ => 'z',
        PhysicalCode::Digit0 => '0',
        PhysicalCode::Digit1 => '1',
        PhysicalCode::Digit2 => '2',
        PhysicalCode::Digit3 => '3',
        PhysicalCode::Digit4 => '4',
        PhysicalCode::Digit5 => '5',
        PhysicalCode::Digit6 => '6',
        PhysicalCode::Digit7 => '7',
        PhysicalCode::Digit8 => '8',
        PhysicalCode::Digit9 => '9',
        PhysicalCode::Equal => '=',
        PhysicalCode::Minus => '-',
        PhysicalCode::Comma => ',',
        PhysicalCode::Period => '.',
        PhysicalCode::Slash => '/',
        PhysicalCode::BracketLeft => '[',
        PhysicalCode::BracketRight => ']',
        PhysicalCode::Backslash => '\\',
        _ => return None,
    };
    let c = if shift && c.is_ascii_lowercase() {
        c.to_ascii_uppercase()
    } else {
        c
    };
    Some(KeyCode::Char(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_fallback_preserves_shift_and_control_punctuation() {
        assert_eq!(
            physical_fallback(PhysicalKey::Code(PhysicalCode::KeyM), false),
            Some(KeyCode::Char('m'))
        );
        assert_eq!(
            physical_fallback(PhysicalKey::Code(PhysicalCode::KeyI), true),
            Some(KeyCode::Char('I'))
        );
        assert_eq!(
            physical_fallback(PhysicalKey::Code(PhysicalCode::KeyH), false),
            Some(KeyCode::Char('h'))
        );
        assert_eq!(
            physical_fallback(PhysicalKey::Code(PhysicalCode::KeyH), true),
            Some(KeyCode::Char('H'))
        );
        assert_eq!(
            physical_fallback(PhysicalKey::Code(PhysicalCode::Comma), false),
            Some(KeyCode::Char(','))
        );
        assert_eq!(
            physical_fallback(PhysicalKey::Code(PhysicalCode::Slash), true),
            Some(KeyCode::Char('/'))
        );
    }
}
