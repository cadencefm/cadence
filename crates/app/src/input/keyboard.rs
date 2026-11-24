//! The note layout shared by Insert and Jam: one octave plus the next C on
//! the bottom two rows, white keys on `z` to `,` and black keys on
//! `s d g h j`. Shift accents a note; Caps Lock therefore only accents.

/// Bottom-row keys in semitone order from C.
const NOTE_KEYS: &str = "zsxdcvgbhnjm,";

/// The velocity of a note played in Jam.
pub(crate) const VELOCITY: f32 = 0.85;
/// The velocity of an accented note, played or written with Shift.
pub(crate) const ACCENT_VELOCITY: f32 = 1.0;

/// A note key's semitone above C and whether it is accented.
pub(crate) fn note_key(c: char) -> Option<(i16, bool)> {
    let semitone = NOTE_KEYS
        .chars()
        .position(|k| k == c.to_ascii_lowercase())?;
    Some((semitone as i16, c.is_ascii_uppercase()))
}

/// `<` and `>` move the note octave down and up.
pub(crate) fn octave_step(c: char) -> Option<i8> {
    match c {
        '<' => Some(-1),
        '>' => Some(1),
        _ => None,
    }
}

pub(crate) fn shift_octave(octave: i8, step: i8) -> i8 {
    (octave + step).clamp(0, 8)
}

/// The MIDI note for a semitone in the given octave, where octave 4 starts at
/// middle C.
pub(crate) fn midi_note(octave: i8, semitone: i16) -> u8 {
    ((octave as i16 + 1) * 12 + semitone).clamp(0, 127) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_octave_plus_the_next_c_with_shift_for_accents() {
        assert_eq!(note_key('z'), Some((0, false)));
        assert_eq!(note_key('s'), Some((1, false)));
        assert_eq!(note_key(','), Some((12, false)));
        assert_eq!(note_key('Z'), Some((0, true)));
        assert_eq!(note_key('M'), Some((11, true)));
        // The q row, digits, and surrounding punctuation are not notes.
        for c in ['q', '2', '9', '0', 'l', '.', ';', '/', '<'] {
            assert_eq!(note_key(c), None, "{c}");
        }
        assert_eq!(midi_note(4, 0), 60);
        assert_eq!(shift_octave(8, 1), 8);
    }
}
