//! Retained musical material and its uses: the authoring document.
//!
//! Definitions have no track ownership. Compilation creates a private, dense
//! performance score; persistence and editors retain these source relationships.

mod codec;
mod compile;
mod demo;
mod eval;
mod eval_events;
mod live;
mod mutate;
mod set;
mod types;
mod validate;

#[cfg(test)]
mod tests;

use compile::ScoreBuilder;
pub use eval::eval_value_points;
use types::MAX_DERIVED_OFFSET;

pub use crate::macros::{MacroBinding, MacroControl, MAX_MACRO_BINDINGS, MAX_PROJECT_MACROS};
pub use crate::mixer::LiveFlags;
pub use crate::music::STEPS_PER_BAR;
pub use codec::{parse_document_json, write_document_json};
pub use live::{
    Audition, LiveSegment, Override, AUDITION_INSTRUMENT_ID, LIVE_RING_BARS, LIVE_RING_STEPS,
};
pub use set::{
    edit_label, Boundary, EditTiming, LanePart, LiveTiming, MAX_LANE_PARTS, MAX_LATE_TOLERANCE,
    MAX_PARTS_ON_LANES,
};
pub use types::{
    ArpMode, Consumer, Edit, EventOrigin, Exception, Id, InstrumentInstance, Lane, MixerGraph,
    Modulation, ModulationTarget, ParameterLink, Part, Pattern, PatternKind, PatternType,
    PatternValue, PhaseMode, Placement, PlacementSample, PreviewStep, Project, Section, ValuePoint,
    ValueUnit, MAX_BARS, MAX_DEPTH, MAX_GLOBAL_MODULATIONS, MAX_INSTRUMENTS, MAX_LANES, MAX_LINKS,
    MAX_LITERAL_STEPS, MAX_PARTS, MAX_PATTERNS, MAX_PLACEMENTS, MAX_SECTIONS,
    MAX_TRANSACTION_EDITS, VERSION,
};
