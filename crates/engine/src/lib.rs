//! Real-time Eurorack modular DSP synthesis, sampler voices, mixer engine, audio output, WAV export, and MIDI.
pub mod audio;
pub mod midi;
pub mod mix_engine;
pub mod modular_voice;

#[cfg(test)]
pub(crate) mod test_fixtures;
