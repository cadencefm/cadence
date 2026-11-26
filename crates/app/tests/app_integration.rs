//! Consolidated integration test harness for `cadence-app` (pure controller & workflows;
//! links only `cadence-core` + `cadence-app` with zero audio/MIDI/GPU dependencies).

#[path = "app_integration/arrange_pattern_bank_workflows.rs"]
mod arrange_pattern_bank_workflows;
#[path = "app_integration/arrangement_workflow.rs"]
mod arrangement_workflow;
#[path = "app_integration/authoring_draft.rs"]
mod authoring_draft;
#[path = "app_integration/common.rs"]
mod common;
#[path = "app_integration/deck_workflow.rs"]
mod deck_workflow;
#[path = "app_integration/devices_audio_deck.rs"]
mod devices_audio_deck;
#[path = "app_integration/file_hub_and_assets.rs"]
mod file_hub_and_assets;
#[path = "app_integration/follow_workflow.rs"]
mod follow_workflow;
#[path = "app_integration/format.rs"]
mod format;
#[path = "app_integration/links_and_modulations.rs"]
mod links_and_modulations;
#[path = "app_integration/material_workflow.rs"]
mod material_workflow;
#[path = "app_integration/modal_authoring.rs"]
mod modal_authoring;
#[path = "app_integration/modeless_input.rs"]
mod modeless_input;
#[path = "app_integration/navigation_confirms_and_hints.rs"]
mod navigation_confirms_and_hints;
#[path = "app_integration/overlays_and_palette.rs"]
mod overlays_and_palette;
#[path = "app_integration/pattern_and_mixer_workflows.rs"]
mod pattern_and_mixer_workflows;
#[path = "app_integration/performance_identity.rs"]
mod performance_identity;
#[path = "app_integration/robustness.rs"]
mod robustness;
#[path = "app_integration/sample_catalog.rs"]
mod sample_catalog;
#[path = "app_integration/sample_deck_and_slicing.rs"]
mod sample_deck_and_slicing;
#[path = "app_integration/search_and_help.rs"]
mod search_and_help;
#[path = "app_integration/signal_graph.rs"]
mod signal_graph;
#[path = "app_integration/take_workflow.rs"]
mod take_workflow;
#[path = "app_integration/transport_workflow.rs"]
mod transport_workflow;
#[path = "app_integration/workflow_drafts.rs"]
mod workflow_drafts;
#[path = "app_integration/workflow_session.rs"]
mod workflow_session;
