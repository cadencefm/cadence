//! Keyboard input state and routing.
use super::*;

mod alt;
mod decks;
mod forms;
mod insert;
mod jam;
mod keyboard;
mod manipulate;
mod normal;
mod parse;
mod prefixes;
mod route;

pub use parse::{normalize_shifted_key, parse_key_sequence};

/// What the keyboard is doing: the mode, a pending count, held notes and the
/// performance focus. Session state, never saved or undone.
pub(super) struct Input {
    pub(super) mode: Mode,
    /// A count typed before a command, such as the 4 in `4l`.
    pub(super) count: Option<usize>,
    /// Keys sounding notes: the key, its instrument and MIDI note.
    pub(super) held: Vec<(char, Id, u8)>,
    /// MIDI notes sounding: the note and its instrument.
    pub(super) midi_held: Vec<(u8, Id)>,
    /// The octave of the bottom-row notes in Insert and Jam.
    pub(super) octave: i8,
    /// The lane whose layer is open after a lane key: the next key chooses
    /// a part or toggles the lane.
    pub(super) lane_layer: Option<Id>,
    /// The lane named by the last lane key. Naming a lane never moves the
    /// selection.
    pub(super) focused_lane: Option<Id>,
    /// Which bank of ten lanes and sections the performance keys address.
    pub(super) bank: usize,
    /// Mode to return to when a transient modal/palette/prefix closes (`Jam`, `Tutorial`, etc.).
    pub(super) resume_mode: Option<Mode>,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            mode: Mode::Normal,
            count: None,
            held: Vec::new(),
            midi_held: Vec::new(),
            octave: 4,
            lane_layer: None,
            focused_lane: None,
            bank: 0,
            resume_mode: None,
        }
    }
}
