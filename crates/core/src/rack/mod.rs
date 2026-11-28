//! Unified Eurorack-style Modular Rack instrument domain model, 12 orthogonal
//! DSP-backed modules, parameter metadata, topological DAG compiler, and bare
//! neutral engine templates.

pub mod compile;
pub mod modules;
pub mod params;
pub mod templates;
pub mod types;

#[cfg(test)]
mod tests;

pub use crate::dsp::env::EnvMode;
pub use crate::dsp::osc::{ideal_waveform, lfo_shape_sample};
pub use crate::mixer::DriveMode;
pub use crate::sampler::SampleLoopMode;

pub use compile::{
    compile_zone_graph, CompiledCable, CompiledOp, CompiledZoneGraph, PATCHBAY_COLUMNS,
};
pub use modules::{
    cable_amount_range, cable_amount_unit, cable_default_amount, cable_signal_level,
    denormalize_cable_amount, format_cable_amount, normalize_cable_amount, nudge_cable_amount,
    parse_cable_amount, AeroModule, DriveModule, EnvModule, FilterModule, LfoModule, LoFiModule,
    ModuleKind, ModuleKindTag, NoiseModule, OutModule, ResonatorModule, SamplerModule,
    SamplerParams, VcaModule, VcoModule, OUT_MAX_GAIN_DB, OUT_MIN_GAIN_DB,
};
pub use params::{ModuleParam, RackParam, ZoneParam, LFO_DIVISIONS, LFO_DIVISION_LABELS};
pub use types::{
    drum_midi, module_sample_key, AeroMode, BodyGeometry, FilterMode, InPort, LfoShape, NoiseColor,
    OutPort, PanelPin, PatchCable, PitchTracking, RackModule, RackPatch, RackTemplate, RackZone,
    ResonatorMode, TriggerMode, Unit, UpdatePolicy, VoiceMode, VoiceSettings, Waveform,
    MAX_AEROS_PER_ZONE, MAX_CABLES_PER_ZONE, MAX_CHOKE_GROUPS, MAX_MODULES_PER_ZONE,
    MAX_PANEL_PINS, MAX_RESONATORS_PER_ZONE, MAX_ZONES,
};
