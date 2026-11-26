//! Sample-clock unified Eurorack modular Rack synthesis & Hermite stereo Sampler
//! execution, live polyphonic/mono Jam notes (`NoteOn`/`NoteOff`), lock-free sample
//! bank, sub-step frame scheduling, native CPAL output, silent transport, and
//! deterministic WAV export.

mod engine;
mod input;
pub mod render;
mod runtime;
mod scheduler;
mod shared;
mod voice_pool;
mod voices;

pub use render::{
    finalize_recording_wav, render_bounce_wav, render_measure_report,
    render_rack_activity_snapshot, render_schedule_f32, render_schedule_measure,
    render_schedule_pcm, render_schedule_report, render_stems_with_samples_report,
    render_wav_with_samples, render_wav_with_samples_report, write_stereo_wav_16bit,
    write_stereo_wav_24bit, RecordedWavStats, RenderChannelReport, RenderEffectReport,
    RenderReport, RenderSignalReport, StemExportSummary,
};
pub use runtime::{AudioDeviceChange, AudioRuntime, AudioTapHandle};
pub use shared::{build_sample_bank, AUDIO_CAPTURE_CAPACITY, AUDIO_TAP_CAPACITY};

use anyhow::{bail, ensure, Context, Result};
use cadence_core::assets::load_project_samples;
use cadence_core::mixer::{ChannelRef, MAX_CHANNELS};
use cadence_core::music::EndRule;
use cadence_core::runtime::{AudioSnapshot, ChannelMeter, SampleBank, TransportIntent};
use cadence_core::sampler::{SampleData, SampleLibrary};
use cadence_core::score::{ScoreWindow, MAX_INSTRUMENTS};

#[cfg(test)]
mod tests;
