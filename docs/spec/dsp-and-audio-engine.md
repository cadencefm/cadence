# Pure DSP Library, Modular Rack Engine, Studio Effects & Real-Time Audio Contract

This specification defines Cadence's shared pure DSP core (`crates/core/src/dsp/`), the unified Eurorack Modular Rack instrument architecture (`RackPatch`, 11 orthogonal modules, `CompiledZoneGraph`, and `ModularVoice`), all 8 studio effects, nested bus routing with Plugin Delay Compensation (PDC), the unclipped 32-bit float signal chain, pan and fader headroom laws, and the lock-free real-time execution contract (`crates/engine/src/`).

> **Related specifications:** [Material & Timing](material-and-timing.md) · [Decks & Interaction](decks-and-interaction.md) · [Protocols & Persistence](protocols.md) · [Verification](../verification.md)

---

## 1. `cadence-core::dsp` — Single Mathematical Source of Truth

All synthesis, filtering, waveshaping, bit-crushing/decimation, dynamics, spatial DSP, pan laws, fader tapers, and decibel math live in one pure, zero-allocation, zero-I/O library: `crates/core/src/dsp/`. Both `cadence-engine` (real-time and offline audio) and `cadence-app::view` (GPU visualizer curves) call the exact same functions, eliminating DSP-vs-UI drift and duplicate math by construction.

| Module | Real-Time Processors | Shared Analytical / UI & Math SPOTs |
| --- | --- | --- |
| `math.rs` | `Smoother` (1-pole exponential `f64`) | `db_to_lin`/`db_to_lin_f64`, `lin_to_db`/`lin_to_db_f64` (`DB_FLOOR_F32 = -120.0`, `LIN_FLOOR_F32 = 1e-6`), `log_to_norm`, `norm_to_log`, `midi_to_hz`, `sanitize_sample`/`f64`/`frame`, `flush_denormal_f32`/`f64` (`1e-24`), `sanitize_sample_rate` (`1_000..=384_000 Hz`), `one_pole_coeff_ms`/`decay_ms`, `glide_coeff_ms` |
| `gain.rs` | `apply_strip_gain`, `apply_fader_gain`, `apply_pan_balance` | `pan_mono_equal_power`/`f64`, `pan_stereo_balance`/`f64`, `norm_to_channel_gain`, `channel_gain_to_norm`, `nudge_channel_gain_db`, `format_db_value`, `format_gain_db` |
| `noise.rs` | `NoiseGen` (deterministic xorshift + Weyl seeding), `PinkFilter` (sample-rate-invariant Paul Kellet pink filter) | `GOLDEN_RATIO_32`, `SECONDARY_WEYL_SALT_32` |
| `osc.rs` | `oscillator_sample`, `osc_b_fm`, `osc_b_fm_2x`, `step_vco_bank_fm_2x` | `poly_blep`, `poly_blamp`, `phase_wrap`, `ideal_waveform`, `unison_offsets`, `unison_detune_ratio`, `unison_gain_comp`, `fm_depth_from_amount`, `MAX_UNISON = 7` |
| `env.rs` | `AdsrState`, `HoldDecayEnv`, `ModularEnv` (`EnvMode::{Adsr, HoldDecay}`, pre-delay + multi-burst), `RhythmicEnv` | `eval_adsr_curve`, `eval_modular_env_curve` |
| `biquad.rs` | `StereoBiquad`, `Eq`, `TptOnePoleLpf`, `TptOnePoleHpf`, `DcBlocker` | `BiquadCoeffs::{highpass, low_shelf, peaking, high_shelf}`, `BiquadCoeffs::magnitude_db`, `eq_response_db` |
| `filter.rs` | `Svf`, `AcidLadder`, `RhythmicFilter` | `svf_transfer`, `acid_ladder_transfer`, `one_pole_lpf_transfer`, `filter_transfer`, `filter_magnitude`, `filter_magnitude_db`, `filter_mix_magnitude_db`, `acid_accent_amount`, `acid_filter_decay_ms`, `filter_velocity_mod` |
| `saturate.rs` | `Drive` (`Tape`/`HardClip`/`Fold`, 2× `Halfband2x` OS) | `soft_clip_drive`/`f64`, `drive_shape_mode`/`f64`, `drive_shape_mode_asym`/`f64`, `asymmetric_drive_shape`/`f64`, `drive_transfer_sample`, `soft_guard`/`f64` |
| `lofi.rs` | `LoFiProcessor` (stereo bit-depth quantizer + phase-accumulating clock decimator) | `lofi_quantize`/`f64`, `lofi_transfer_sample` (`LOFI_MIN_BITS = 2.0`, `LOFI_MAX_BITS = 24.0`, `LOFI_BYPASS_BITS = 23.95`, `LOFI_MIN_RATE_HZ = 500.0`, `LOFI_MAX_RATE_HZ = 48_000.0`) |
| `resonator.rs` | `ModalResonator` (sample-rate-invariant stereo 32-mode complex damped phasor bank: `String`/`Bar`/`Membrane`/`Plate`) | `resonator_mode_descriptors` (`RESONATOR_MODES = 32`, `RESONATOR_MIN_HZ = 20.0`, `RESONATOR_MAX_HZ = 12_000.0`, `RESONATOR_MIN_DECAY_MS = 15.0`, `RESONATOR_MAX_DECAY_MS = 8_000.0`) |
| `sampler_voice.rs` | `SamplerVoice` (4-point Catmull-Rom Hermite stereo sample voice with loop crossfades & pitch CV) | `sample_hermite_stereo` |
| `dynamics.rs` | `Compressor`, `Limiter` (lookahead) | `soft_knee_gr_db`/`f64`, `limiter_lookahead_frames`, `COMPRESSOR_KNEE_DB = 6.0`, `LIMITER_KNEE_DB = 2.0`, `LIMITER_LOOKAHEAD_SEC = 0.0015` |
| `chorus.rs` | `Chorus`, `QuadratureLfo` | `eval_quadrature` |
| `delay.rs` | `Delay`, `DelayLine` (int/linear/cubic) | O(1) validity reset (`DelayLine::reset`), cubic single-channel & stereo Hermite taps |
| `reverb.rs` | `Reverb` (8×8 Hadamard FDN + 4 diffusers) | `reverb_rt60_envelope`, `reverb_hf_rt60_envelope` |
| `resample.rs` | `Halfband2x` (8-tap polyphase 2× oversampling decimator), `resample_stereo` | Zero-alloc 2× oversampling for FM & `Drive`; off-RT Catmull-Rom stereo sample-rate conversion for audio input capture |

---

## 2. Unified Eurorack Modular Rack Architecture (`crates/core/src/rack/`, `crates/engine/src/modular_voice/`)

Every `InstrumentInstance` owns one unified `rack: RackPatch` and routes stereo audio to an existing `ChannelKind::Audio` bus. Single-sound synths and samplers are 1-zone Racks; drum kits and multisamples are N-zone Racks with key/velocity splits and choke groups.

### 2.1 Rack → Zones → Modules + Patchbay Hierarchy & Parameters

- **Rack (`RackPatch`):** Optional `kit_name`, `1..=MAX_ZONES` (`32`) `RackZone`s, curated Quick Panel pins (`panel_pins: Vec<PanelPin>`, up to `MAX_PANEL_PINS = 16`, pinning any `Rack`, `Zone`, `Module`, or `Cable` target), and **5 `RackParam`s** (all `UpdatePolicy::Continuous`): `Gain` (`-24..+12 dB`), `Pan` (`-1..+1`), `Tune` (`-24..+24 st`), `Cutoff` (`30..20_000 Hz`, log), `Resonance` (`0..0.92`).
- **Zone (`RackZone`):** Stable nonzero `id: u16`, `name` (`1..=32` chars), key range `low_midi..=high_midi`, `root_midi`, velocity layer `low_vel..=high_vel`, `gain_db` (`-36..+12 dB`), `pan` (`-1..+1`), optional `choke_group` (`1..=MAX_CHOKE_GROUPS = 8`, choking active voices in the same group on the same instrument before spawning new layers), `voice: VoiceSettings` (`voice_mode`, `legato`, `glide_ms` `0..=2000 ms`, `pitch_tracking`, `phase_reset`), `1..=MAX_MODULES_PER_ZONE` (`24`) `RackModule`s (with exactly one mandatory `OutModule`), and `0..=MAX_CABLES_PER_ZONE` (`64`) `PatchCable`s.
  - **8 `ZoneParam`s:** `Gain`, `Pan` (`Continuous`); `Glide`, `PitchTracking`, `PhaseReset`, `RootMidi` (`OnTrigger`); `VoiceMode`, `Legato` (`Structural` — non-automatable, rejected by `ParamAddress::is_automatable()`).
  - **Standard Drum MIDI Map (`rack::drum_midi`):** `36` BD, `37` SD, `38` LT, `39` MT, `40` HT, `41` RS, `42` CH (choke 1), `43` CP, `44` CB/CR, `45` CY/RD, `46` OH (choke 1), `47` LC, `48` MC, `49` HC, `50` CL, `51` MA.
- **4 Neutral Bare Engine Templates (`RackTemplate` — zero disk I/O):** `Synth` (`Init Synth`: `VCO 1 -> Filter -> VCA -> Out`, `Amp Env -> VCA.Cv`, `Filter Env -> Filter.Cv` + 10 `panel_pins`), `Sampler` (`Blank Sampler`: `Sampler -> Filter @ 18.5 kHz -> VCA -> Out`, `Amp Env -> VCA.Cv` + 9 `panel_pins`), `Kit` (`Blank Kit`: 1 full-range `[Sampler -> Out]` zone), and `Modular` (`Blank Modular`: minimal `[Out]` canvas).

### 2.2 All 11 Orthogonal Eurorack Modules (`ModuleKind`, 62 `ModuleParam`s)

Every module reads summed stereo `[f64; 2]` signals on its input ports and writes stereo `[f64; 2]` to `OutPort::Out` (mono modules write `[x, x]`; `Out` is the zone terminal sink):

| # | `ModuleKind` | Patchbay Column | Accepted `InPort`s | Parameters & DSP Implementation (`cadence-core::dsp`) |
| --- | --- | --- | --- | --- |
| 1 | **`Vco`** | `0` (`SOURCES`) | `PitchCv`, `FmIn`, `PwCv` | `Waveform::{Sine, Triangle, Saw, Pulse}` (bandlimited PolyBLEP/PolyBLAMP with exact Pulse DC comp), `tune` `±48 st`, `fine` `±100 ct`, `pulse_width` `0.05..0.95`, `phase` `0..360°`, `level` `0..1`, `vco_pitch_track` (`Chromatic`/`Fixed`), `fixed_hz` `20..20k Hz`, unison `1..=7` (`MAX_UNISON = 7`, `unison_gain_comp = 1/sqrt(N)`, `unison_detune` `0..50 ct`, `unison_width` `0..1` via `pan_mono_equal_power`); 2× oversampled signed-`dt` through-zero linear FM via `FmIn` |
| 2 | **`Sampler`** | `0` (`SOURCES`) | `PitchCv`, `TrigIn` | First-class modular stereo sample player: `root_midi`, `coarse_tune` `±48 st`, `fine` `±100 ct`, `sample_start`/`sample_end`/`loop_start`/`loop_end` (frame-based `OnTrigger`), `loop_mode` (`Off`/`Forward`/`PingPong`, *Structural*), `loop_xfade` `0..100 ms` (*Structural*), `one_shot` (*Structural*), `level`; 4-point Catmull-Rom cubic Hermite playback (`sample_hermite_stereo`) with audio-rate `PitchCv` and external `TrigIn` retriggering |
| 3 | **`Noise`** | `0` (`SOURCES`) | *(none)* | `NoiseColor::{White, Pink}` (Paul Kellet 7-pole Pink), TPT `highpass_hz` `20..18k Hz` (`TptOnePoleHpf`), `lowpass_hz` `20..20k Hz` (`TptOnePoleLpf`), `level`; freely patchable pre- or post-filter |
| 4 | **`Env`** | `1` (`MODULATORS`) | `TrigIn` | `ModularEnv` composing `AdsrState` + `HoldDecayEnv`: `env_mode` (`Adsr`/`HoldDecay`), `attack` `0.1..10_000 ms`, `hold` `0..500 ms`, `decay` `1..20_000 ms`, `sustain` `0..1`, `release` `1..20_000 ms`, `trigger_mode` (`Gate`/`OneShot`), pre-delay `delay_ms` `0..500 ms`, multi-burst `bursts` `1..=8` & `burst_spacing_ms` `2..50 ms` (native TR-909 clap/flam contours), Acid accent decay snap when modulating `AcidLowPass`, and `3.5 ms` choke fade |
| 5 | **`Lfo`** | `1` (`MODULATORS`) | *(none)* | `LfoShape::{Sine, Triangle, Saw, Square, SampleHold}`, free `lfo_rate` `0.01..40 Hz` or tempo-synced (`lfo_sync`) across 12 `LFO_DIVISIONS` (`1/64..4/1`), `lfo_retrigger`, `lfo_bipolar` (`[-1, 1]` vs `[0, 1]`) |
| 6 | **`Filter`** | `2` (`PROCESSORS`) | `In`, `Cv` | 2-pole ZDF `Svf` (`LowPass`, `BandPass`, `HighPass`), 1-pole `6 dB/oct` `TptOnePoleLpf` (`LowPass6`), and 4-pole `24 dB/oct` nonlinear ZDF `AcidLadder` (`AcidLowPass`, `ACID_LADDER_K_SCALE = 3.85`, `ACID_BASS_COMP_SCALE = 0.55`, pre-drive `soft_clip_drive_f64`): `cutoff` `20..20k Hz` (log), `resonance` `0..0.95`, `drive` `0..1`, `key_track` `0..1`, `vel_amount` `0..4 oct` (Acid accent above `ACID_ACCENT_THRESHOLD = 0.85`, `acid_filter_decay_ms`, `filter_velocity_mod` normalized by `ACID_FILTER_VEL_MAX = 4.0`), `mix` `0..1`, audio-rate `Cv` in octaves |
| 7 | **`Drive`** | `2` (`PROCESSORS`) | `In`, `Cv` | `DriveMode::{Tape, HardClip, Fold}` backed by `drive_shape_mode_asym_f64` (2× oversampled for `HardClip` & `Fold`), `drive` `0..1`, `asymmetry` `-1..+1`, `tone_hz` `600..18k Hz` (`TptOnePoleLpf`), `output_db` `-12..+6 dB`, `mix` `0..1`, `DcBlocker`, plus audio-rate `Cv` modulation |
| 8 | **`LoFi`** | `2` (`PROCESSORS`) | `In` | `LoFiProcessor`: continuous mid-tread odd-symmetric bit-depth quantizer `bits` `2..24` (transparently bypassed at `>= 23.95`), phase-accumulating zero-order hold clock decimator `sample_rate_hz` `500..48_000 Hz` (bypassed when `>= sr`), linear dry/wet `mix` `0..1` |
| 9 | **`Resonator`** | `2` (`PROCESSORS`) | `In`, `PitchCv`, `Cv` | `ModalResonator` (up to `MAX_RESONATORS_PER_ZONE = 4` heap-hosted stereo 32-mode banks per voice): `resonator_mode` (`String`/`Bar`/`Membrane`/`Plate`), `vco_pitch_track` (`Chromatic`/`Fixed`), `tune` `±48 st`, `fine` `±100 ct`, `fixed_hz` `20..12k Hz`, `structure` `0..1`, `resonator_decay` `15..8_000 ms` (exact `-60 dB` RT60), `brightness` `0..1` (modulatable via `Cv`), `position` `0..1`, `mix` `0..1`; phase-continuous `PitchCv` + `z-1` nonlinear feedback safe |
| 10 | **`Vca`** | `3` (`VCA / OUT`) | `In`, `Cv` | `vca_gain` `0..2`, `vca_bias` `0..1`. When `Cv` is patched, evaluates `in * gain * (bias + cv_sum)` — acting as an envelope VCA (`Env -> Vca.Cv`, `bias = 0`) or 4-quadrant audio-rate Ring Modulator (`Vco -> Vca.Cv`). Passes `in * gain` cleanly when `Cv` is unpatched |
| 11 | **`Out`** | `3` (`VCA / OUT`) | `In`, `Cv` | Singular required terminal zone module (`has_output() == false`): `gain_db` `-36..+12 dB`, `pan` `-1..+1`, M/S `spread` `0..1`, optional `Cv`, `DcBlocker`, and per-voice `soft_guard_f64` (`C¹` transparent below `±0.95`, strictly for voice anti-blowup) |

**Universal Bounded Sample Decoding & `SampleBank` Keying (`crates/core/src/sampler.rs`, `crates/core/src/rack/types.rs`):**
- Supports PCM/Float WAV, FL Ogg-in-RIFF WAV, FLAC, Ogg Vorbis, MP3, AIFF, and pure-Rust WavPack `.wv` via Symphonia + custom RIFF chunk parser, bounded by `MAX_SAMPLE_FILE_BYTES = 128 MiB`, `MAX_DECODED_SAMPLES = 33_554_432` floats, `MAX_CACHE_BYTES = 256 MiB`, `MAX_LIBRARY_ENTRIES = 25_000`, `MAX_LIBRARY_SCAN_ITEMS = 100_000`, with precomputed `PEAK_BINS = 512` `(min, max)` envelope bins.
- RIFF `smpl` & `inst` chunks automatically populate `SampleMetadata` (`root_midi`, `has_root`, `fine_cents`, `low_midi`/`high_midi`, `low_vel`/`high_vel`, `SampleLoopMode`, `loop_start`, `loop_end`) via `SamplerModule::apply_metadata` / `apply_sample_defaults`.
- **Single Sample SPOT:** `RackPatch::sampler_modules()` walks every `SamplerModule` across every zone; `module_sample_key(inst_id, zone_id, module_id)` packs `(1 << 63) | ((inst_id & 0x7FFF_FFFF) << 32) | (zone_id << 16) | module_id` for collision-free `SampleBank` lookup.

### 2.3 Ports, Bipolar Attenuverter Cables & `CompiledZoneGraph` DAG Compiler

Every `PatchCable { id, from_module, from_port: OutPort::Out, to_module, to_port: InPort, amount }` carries a first-class modulatable bipolar attenuverter depth (`cable_amount_range` in `crates/core/src/rack/modules.rs`):

| Target Port | Native Range | Unit | Default | Semantics |
| --- | --- | --- | --- | --- |
| `In` *(Audio)* | `-2.0..=+2.0` | `%` | `1.0` | Stereo audio gain / phase inversion (multiple cables sum) |
| `PitchCv` | `-48.0..=+48.0` | `st` | `12.0` | Pitch modulation in semitones (`Vco`, `Sampler`, `Resonator`) |
| `FmIn` | `-1.0..=+1.0` | `%` | `0.35` | Audio-rate through-zero linear FM depth (`Vco`) |
| `PwCv` | `-0.45..=+0.45` | `%` | `0.20` | Pulse-width modulation (`Vco`) |
| `TrigIn` | `-2.0..=+2.0` | plain | `1.0` | Gate / trigger input (`Env`, `Sampler`; defaults to voice gate if unpatched) |
| `Cv` on `Filter` | `-6.0..=+6.0` | `oct` | `2.0` | Audio-rate cutoff modulation in octaves |
| `Cv` on `Vca` | `-2.0..=+2.0` | `%` | `1.0` | VCA / 4-quadrant Ring Modulator CV depth |
| `Cv` on `Drive` / `Out` | `-1.0..=+1.0` | `%` | `0.5` | Drive / Output CV modulation depth |
| `Cv` on `Resonator` | `-1.0..=+1.0` | `%` | `0.35` | Modal brightness / high-frequency damping modulation depth |

**Deterministic DAG Compilation (`compile_zone_graph` in `crates/core/src/rack/compile.rs`):**
- Computes Kahn topological execution order on the main thread using deterministic `(column, module_id)` tie-breaking across `PATCHBAY_COLUMNS = ["SOURCES", "MODULATORS", "PROCESSORS", "VCA / OUT"]`.
- **Feedback safety (`z-1` 1-sample delay):** Cycles and self-loops never crash or deadlock; any back-edge satisfying `from_slot >= to_slot` is flagged `feedback: true` and reads `prev_slots[from_slot]` (soft-bounded via `FEEDBACK_SOFT_KNEE = 4.0` in `ModularVoice`).
- Emits flat, zero-allocation `CompiledZoneGraph` (`ops: Box<[CompiledOp]>`, contiguous `to_slot`-grouped `cables: Box<[CompiledCable]>`, sorted `module_slot_by_id` and `cable_index_by_id` for O(log N) lookup, `out_slot`, reverse-BFS `lifetime_envs`, `sampler_slots`, and SPOT `column_layout`).

### 2.4 Real-Time Execution (`ModularVoice` & Unified `Voice`)

- **Zero-alloc, stack-safe voice pool (`crates/engine/src/audio/voices.rs`, `crates/engine/src/modular_voice/`):** Each of the `VOICES = 256` slots owns a pre-allocated heap-boxed `Box<ModularVoice>` with fixed `[ModuleState; MAX_MODULES_PER_ZONE]`, `[ModuleParams; MAX_MODULES_PER_ZONE]`, `[f32; MAX_CABLES_PER_ZONE]`, stereo `slots`/`prev_slots`, and inline per-voice P-lock tracking (`MAX_VOICE_LOCKS = 32`).
- **Sub-block `f64` graph evaluation & Rack bus:** `VoicePool::render_block` renders one full event-free sub-block (`<= AUDIO_SUB_BLOCK_FRAMES = 64`) per active voice into heap-boxed `VoiceBlockOut` scratch buffers for L1D cache locality, running `CompiledZoneGraph` with sample-rate-scaled `CONTROL_FRAMES` continuous parameter updates, then passing voice output through a dedicated Rack-bus LowPass `Svf` (transparently bypassed when `cutoff >= 19.9 kHz` and `res <= 0.001`), `pan_stereo_balance_f64`, and unclamped smoothed `db_to_lin(rack.gain_db + zone.gain_db)` headroom gain.
- **Voice allocation & audition:** Supports polyphonic and per-`(track_id, zone_id)` monophonic/legato retargeting with click-free `glide_coeff_ms` and `ModularEnv::adopt_progress_from`, pre-spawn choke group fades (`3.5 ms`), P-lock vs continuous `ModLane` precedence across `Rack`/`Zone`/`Module`/`Cable` targets, and zero-alloc `AudioRuntime::preview_rack(patch, midi, velocity)`.

---

## 3. All 8 Native Zero-Alloc Studio Effects

Each bus (Audio, Group, Return, Master) holds up to `MAX_INSERTS = 4` inserts, up to `MAX_EFFECT_DEVICES = 24` total across the mixer (`crates/core/src/mixer.rs`). Every effect implements `StereoEffect` (`process`, `process_block`, `process_block_with_gr`, `reset`, `is_quiet`, and `advance_silent_frames`) with smoothed parameters (`Smoother` / `LogSmoother`), NaN/denormal sanitization, click-free bypass crossfading (`BypassFader`), and identical smoother/clock advancement across both silent-channel sleeping and settled bypass-idle.

| # | `EffectKind` | Parameters & Native Ranges | DSP Implementation (`cadence-core::dsp`) |
| --- | --- | --- | --- |
| 1 | **`Eq`** | `hpf` `20..600 Hz` (log), `low` `-12..+12 dB`, `mid` `-12..+12 dB`, `mid_hz` `160..6000 Hz` (log), `mid_q` `0.3..8.0` (log), `high` `-12..+12 dB` | 4 cascaded TDF-II `StereoBiquad`s (RBJ 12 dB/oct HPF, Butterworth shelves `S=1`, peaking mid) with `4 ms` coefficient smoothing; exact `eq_response_db` shared with UI |
| 2 | **`Filter`** | `mode` (`LP`/`BP`/`HP`/`Acid LP`/`LP 6dB`), `cutoff` `20..18k Hz` (log), `resonance` `0..0.95`, `drive` `0..1`, `env_pattern` `0..127` (`FILTER_ENV_PATTERN_COUNT = 128`), `env_pattern_id: Option<Id>`, `env_amount` `-4..+4 oct`, `env_decay` `15..1500 ms` (log), `mix` `0..1` | Stereo ZDF `Svf` + 4-pole `AcidLadder` + 1-pole `TptOnePoleLpf` (`RhythmicFilter`), pre-drive `soft_clip_drive_f64`, `DcBlocker`, complex parallel response `filter_mix_magnitude_db`, plus rhythmic `RhythmicEnv` driven by sparse-safe O(1) externalized multi-groove banks (`presets/filter_grooves.json`, 24 clean-room factory grooves, extensible up to 128 slots, `8`/`16`/`32` res, length `1..=64`, missing IDs fall back to `neutral()`) or any compiled project `Pattern` (`CompiledFilterPattern`, including `TimeScale` 1/8 & 1/32 normalized `Values`) |
| 3 | **`Compressor`** | `threshold` `-48..0 dB`, `ratio` `1..20` (log), `attack` `0.5..100 ms` (log), `release` `10..1000 ms` (log), `makeup` `-6..+24 dB`, `mix` `0..1` | Stereo-linked feed-forward soft-knee (`COMPRESSOR_KNEE_DB = 6.0 dB`, `soft_knee_gr_db_f64`) smoothing `gr_db` *after* the gain computer (zero startup lag); exposes real-time `gain_reduction_db` & peak-held `gr_hold_db` |
| 4 | **`Limiter`** | `ceiling` `-18.0..-0.1 dB`, `input_drive` `0..24 dB`, `release` `10..1000 ms` (log), `mix` `0..1` | Stereo-linked lookahead brickwall (`LIMITER_LOOKAHEAD_SEC = 1.5 ms` = `72` frames @ 48 kHz, `LIMITER_KNEE_DB = 2.0 dB`), O(1) sliding-window peak deque, dB-domain GR smoothing, continuous lookahead advance across bypass for glitch-free PDC |
| 5 | **`Drive`** | `drive_mode` (`Tape`/`HardClip`/`Fold`), `drive` `0..1`, `asymmetry` `-1..+1`, `tone` `600..18k Hz` (log), `output` `-12..+6 dB`, `mix` `0..1` | Shared `drive_shape_mode_asym_f64` with exact `f(x, 0, a) == x` and `f(0, d, a) == 0` invariants (`Tape`: asymmetric `0..14×` tube/tape bias; `HardClip`: `1..64.5×` crisp `C²` `tanh` knee at `0.96` with optional diode curvature + asymmetric `+/-` clip rails; `Fold`: West-Coast sine wavefolder with symmetry offset); zero-alloc **2× oversampling** (midpoint upsample + `TptOnePoleLpf` at `0.42·sr` + 2-tap decimate) for `HardClip` & `Fold`; `3 ms` mode crossfade, `DcBlocker`, `tone_lpf` |
| 6 | **`Chorus`** | `rate` `0.05..6.0 Hz` (log), `depth` `0..1`, `width` `0..1`, `mix` `0..1` | Dual-voice stereo BBD-style `QuadratureLfo` (`0°..90°` spread), cubic Hermite reads from pre-allocated `64 ms` `DelayLine`, `8.5 kHz` `TptOnePoleLpf` warmth, true `(1 - mix)·dry + mix·wet` crossfade |
| 7 | **`Delay`** | `delay_mode` (`PingPong`/`Stereo`), `time` `0.5..32.0` steps (capped at `MAX_DELAY_SECONDS = 12.0 s`), `feedback` `0..0.88`, `mix` `0..1` | Cubic Hermite `DelayLine`, `30 ms` raised-cosine delay-time crossfade (zero Doppler chirp), `8.5 kHz` feedback damping; `PingPong` (mono-seeded L→R→L) vs `Stereo` (independent L/R preserving panning) blended by `5 ms` BIBO-stable `ping_pong_blend` smoother; O(1) ring validity reset on tail drain |
| 8 | **`Reverb`** | `size` `0..1`, `decay` `0.2..12.0 s`, `damping` `500..18k Hz` (log), `predelay` `0..120 ms`, `width` `0..1`, `mix` `0..1` | Stereo 8×8 Hadamard FDN + 4 Schroeder allpass diffusers (`ALLPASS_GAIN = 0.6`), pairwise-coprime integer prime delay lengths quantized for active sample rate, smooth `size` crossfades, exact `-60 dB` RT60 envelopes (`reverb_rt60_envelope`, `reverb_hf_rt60_envelope`) |

---

## 4. Mixer Topology, Nested `Group → Group` DAG & Chained PDC

`MixerEngine` (`crates/engine/src/mix_engine.rs`) routes audio strictly inside fixed, bounded tables:

- **Channel limits:** `MAX_AUDIO_CHANNELS = 16`, `MAX_GROUPS = 4`, `MAX_RETURNS = 4` (`MAX_MIXER_CHANNELS = 24`), `MAX_SENDS = 4` per Audio/Group channel, `MAX_DUCKS = 16` (`recovery_ms: 10.0..=2000.0 ms`, quadratic recovery `1 - depth * (1 - x)^2` triggered by `source_part` events), `MAX_MIXER_AUTOMATIONS = 256`.
- **DAG routing rules (`Mixer::validate`):**
  - `Audio` outputs to `Master` or any `Group`.
  - `Group` outputs to `Master` or any other `Group`, with an explicit walk rejecting self-loops and multi-hop cycles (`Group A → Group B → … → Group A`).
  - `Return` outputs strictly to `Master` and may not hold outgoing sends.
- **Causal topological execution, dense active lists & sleeping:** On every `adopt_prepared()`, `MixerEngine` computes zero-allocation Kahn topological order (`group_order`) for chained groups, pre-builds dense active Audio/Return/Duck/Effect/automated/dynamic index lists, resolves ancestor-aware `source_enabled`, and swaps off-RT prepared `Box<ActiveEffect>` instances (`0` bytes for unused slots) while moving evicted effects into a pre-sized off-RT retirement sink. During `process_block()`, lazy bus/return/master activity flags and `StereoEffect::{is_quiet, advance_silent_frames}` let silent and settled-muted channels skip insert DSP without losing active pre-insert metering, tails, smoothers, or GR averages.
- **Parallel-path Plugin Delay Compensation (PDC):** Sums `Limiter::latency_frames()` on each Audio, Group, and Return branch and across chained `Group → Group` hops, computing exact differential `pdc_delay` frames applied via integer `DelayLine` reads (`apply_pdc`) before summing into parent Groups or `Master`. When no non-Master Limiter exists, every `pdc_delay == 0` (zero latency overhead).

---

## 5. 32-Bit Float Signal Chain, Pan Laws & `+6 dB` Headroom

### 32-Bit Float Headroom & Clipping Semantics

- **Zero hidden Master soft-clipping:** All internal instruments, buses, sends, returns, and Master inserts run in unclipped 32-bit floating point with no hidden `tanh` or soft-clipper behind Master.
- **Strict final boundary clamping:** Samples are clamped to `[-1.0, 1.0]` *only* at the hardware DAC sample conversion boundary (`crates/engine/src/audio/runtime.rs`) and the 16-bit PCM WAV export boundary (`crates/engine/src/audio/render.rs`, preceded by a `5 ms` linear fade-out at the end of rendered WAVs).
- **`hot` vs `clipped` metering (`MeterState::observe`):**
  - **`hot`** latches on any channel (including Master) whenever `|input| > 1.0` or `|output| > 1.0` (`> 0 dBFS` informational 32-bit float headroom — not audible clipping on internal buses).
  - **`clipped`** latches strictly when a non-finite (`NaN`/`±Inf`) sample appears anywhere OR Master post-insert post-fader output exceeds `1.0` (`> 0 dBFS` at the DAC/WAV boundary).
- **Preview isolation:** Sample/preset preview audio sums *after* Master mute and Master strip so muting Master never silences browser auditioning.

### Dual Pan Laws (`crates/core/src/dsp/gain.rs`)

1. **Mono constant-power pan (`pan_mono_equal_power`):**
   $$\theta = (p + 1)\frac{\pi}{4}, \quad [L, R] = [\cos\theta, \sin\theta], \quad L^2 + R^2 = 1.0$$
   Delivers `-3.01 dB` (`[0.7071, 0.7071]`) at center (`p = 0.0`) and exact `[1, 0]` / `[0, 1]` at hard L/R. Used for mono voice sources (`Vco` unison copies, zone/`Out` mono panning).
2. **Unity-normalized equal-power stereo balance (`pan_stereo_balance`):**
   - `p = 0.0` → `[1.0, 1.0]` (exact unity pass-through for centered stereo buses).
   - `p > 0.0` → $[\tan((1 - p)\pi/4), 1.0]$; `p < 0.0` → $[1.0, \tan((1 + p)\pi/4)]$.
   - Strictly bounded in `[0.0, 1.0]` (never boosts an already full-scale stereo bus) while matching the exact `L/R` ratio of `pan_mono_equal_power` (`-3.50 dB` at `p = 0.25`, `-6.02 dB` at `p = 0.41`, `-7.66 dB` at `p = 0.50`). Used for Rack-bus stereo pan and stereo mixer strips/buses (`apply_strip_gain`).

### `+6 dB` Mixer Fader Headroom SPOT (`dsp/gain.rs`, `params.rs`)

- **Linear gain range:** `0.0..=MAX_CHANNEL_GAIN` (`2.0`, `MAX_CHANNEL_GAIN_DB = 6.0`).
- **Console power taper (`norm_to_channel_gain` / `channel_gain_to_norm`):**
  $$g(n) = 2.0 \cdot n^{p}, \quad p = \frac{\ln 2}{\ln(4/3)} \approx 2.4094208$$
  Maps `0.0 → 0.0` (`-inf dB`), `CHANNEL_GAIN_UNITY_NORM = 0.75 → 1.0` (`0.0 dB` exact unity detent), and `1.0 → 2.0` (`+6.02 dB`).
- **Unified nudging, formatting & parsing:**
  - `nudge_channel_gain_db(gain, delta_db)`: steps in dB across `-60..=+6.0 dB`, snapping `< -60 dB` to `0.0` (`-inf`), stepping up from `0.0` at `-60 dB`, and snapping `|db| < 1e-4` to exact `1.0`.
  - `format_db_value` / `format_gain_db`: formats `"-inf"` below `LIN_FLOOR_F32` and normalizes `"-0.0"` to `"+0.0"`.
  - `parse_channel_gain`: accepts `"-inf"` / `"-inf dB"`, `"±N dB"` (`<= +6.021 dB`, snapping `0 dB → 1.0`), `"0%..200%"`, or linear `"0.0..2.0"`.

---

## 6. Real-Time Safety Invariants & Lock-Free Execution (`crates/engine/src/audio/`)

Cadence enforces strict real-time safety on every audio callback and silent-clock frame:

| Invariant | Enforcement Mechanism | Exact Constants / Structures |
| --- | --- | --- |
| **Zero RT heap allocations & drops** | Verified directly under `#[global_allocator] CheckedAllocator` in `crates/engine/src/mix_engine/tests.rs` & `audio/tests.rs` (including `ModularVoice::render_block`, `MixerEngine::{process_block, adopt_prepared}`, and `Engine::render_block`) | Publications swap pre-allocated boxes via `AtomicBoxSlot<ProjectPublication>` (`PreparedEffect` allocated off-RT; evicted `ActiveEffect`s pushed into pre-sized `retired_effects` and dropped strictly on the control thread) |
| **Event-split 64-frame sub-blocks** | Zero-latency partitioning across control ticks, step downbeats, and `PendingSubEvent.fire_frame` timestamps | `AUDIO_SUB_BLOCK_FRAMES = 64`; bit-exact parity with frame-by-frame stepping across arbitrary callback sizes |
| **Zero RT locks, panics & false sharing** | No `Mutex`, `RwLock`, unbounded `mpsc`, `.unwrap()`, `.expect()`, or `panic!` on audio paths; `#[repr(C)]` `Shared` isolates control, Output-RT, Input-RT, and SPSC cursors on 128-byte boundaries | `CacheLineAlign` (`#[repr(align(128))]`) with compile-time `offset_of!` assertions; lock-free atomics with `Acquire`/`Release` ordering |
| **Debug stack safety** | Large engine structs heap-boxed; verified by `cargo test -p cadence-engine --lib` | `Box<MixerEngine>`, `Box<MixerBlockScratch>`, `Box<VoiceBlockOut>`, `Box<[Voice]>` (`VOICES = 256`, each owning `Box<ModularVoice>`), `Box<ActiveEffect>`, `Box<[Option<PendingSubEvent>]>`, `Box<[HeldNotes]>`, `Arc<Shared>` |
| **Sub-step frame scheduler** | Zero-alloc pending sub-step event queue firing at exact fractional sample frames | `SUB_EVENT_QUEUE = 256` slots, each carrying up to `MAX_INLINE_LOCKS = 16` inline `ParamLock`s (`MAX_VOICE_LOCKS = 32` per active `ModularVoice`) |
| **Lock-free control mailboxes** | Bounded atomic slots bridging UI/MIDI/host to audio thread | `MAILBOXES = 64` real-time note/MIDI slots, `TRANSPORT_MAILBOXES = 16` transport intents (`TRANSPORT_STEP_MASK` = 40 bits), tear-free `pack_step_position` (`48-bit` step + `16-bit` frac) |
| **SR-scaled control & telemetry cadence** | Power-of-two `control_stride_mask` cadences + click-free voice-steal fades | `CONTROL_TARGET_HZ = 750 Hz` (`64` fr @ 44.1/48 kHz), `METER_PUBLISH_TARGET_HZ = 187.5 Hz` (`256` fr), `WAVE_DOWNSAMPLE_TARGET_HZ = 1500 Hz` (`32` fr), `STEAL_FADE_SLOTS = 8` (`2 ms` fadeout) |
| **Lock-free audio input & recording** | CPAL input callback writes strictly to bounded lock-free SPSC rings with zero disk I/O or locks on RT threads | Stereo monitor ring feeds post-master monitor (`Off`/`Auto`/`On`) with hardware sample-rate resampling; capture ring drains off-thread to 24-bit WAV with latency trim (`-250..+250 ms`) |
| **Lock-free audio tap ring** | SPSC ring buffer written by audio callback only when `tap_enabled == true`, drained off-RT by `AudioTapHandle` (`cadence ctl listen --live`) | `AUDIO_TAP_CAPACITY = 1 << 18` (`262_144` stereo frames ≈ `5.46 s` @ 48 kHz, heap-boxed) |
| **Audio host, hot-plug & parallel stems** | Multi-format CPAL stream (`F32`, `F64`, `I32`, `I16`, `U16`, `8_000..=384_000 Hz`) with hot-plug/disconnect detection, transport-step preservation across `reconfigure_device()`, or paced `--silent` thread; parallel offline stem export via `std::thread::scope` | `SILENT_RATE = 48_000 Hz`, `SILENT_BLOCK = 256` frames, `WAVE_SAMPLES = 256` scope samples |
