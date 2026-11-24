//! Cancellable authoring and object-creation drafts (`Draft`, `WorkflowDraft`) and modal commit handlers.
mod commit;
pub mod form;
pub mod pattern;
mod pattern_view;
pub mod plan;
pub mod workflow;
mod workflow_view;

pub use form::{
    parse_duration, parse_position, DraftCandidate, DraftField, DraftFieldView, DraftInput,
    DraftOutcome, DraftSubmission, DraftView,
};
pub use pattern::Draft;
pub use plan::{
    ChannelChoice, InstrumentChoice, InstrumentSpec, LaneChoice, PartChoice, PartSpec,
    PatternChoice, PlacementSpec, SoundChoice, WorkflowContext, WorkflowKind, WorkflowOutcome,
    WorkflowPlan,
};
pub use workflow::WorkflowDraft;
pub(crate) use workflow_view::format_modulation_target;
