//! Consolidated host, CLI, control-socket, and app+engine integration harness for `cadence`.

mod common;

#[path = "integration/asset_readiness.rs"]
mod asset_readiness;
#[path = "integration/audio_recording.rs"]
mod audio_recording;
#[path = "integration/cli.rs"]
mod cli;
#[path = "integration/ctl_bridge.rs"]
mod ctl_bridge;
#[path = "integration/dsp_eq_limiter_fm.rs"]
mod dsp_eq_limiter_fm;
#[path = "integration/external_presets.rs"]
mod external_presets;
#[path = "integration/live_staging_midi_pan.rs"]
mod live_staging_midi_pan;
#[path = "integration/live_workflow.rs"]
mod live_workflow;
#[path = "integration/macros.rs"]
mod macros;
#[path = "integration/modular_rack.rs"]
mod modular_rack;
#[path = "integration/rack_zones_bounce.rs"]
mod rack_zones_bounce;
#[path = "integration/zone_params_and_routing.rs"]
mod zone_params_and_routing;
