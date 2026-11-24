# Verification Architecture & Measured Showcase Baseline

Cadence's automated verification gates, test suites, and measured 48 kHz mastering baselines across all five showcase tracks. Repository contribution guidelines live in [CONTRIBUTING.md](https://github.com/cadencefm/cadence/blob/main/CONTRIBUTING.md).

> **Related specifications:** [Decks, Keymap & Interaction](spec/decks-and-interaction.md) · [Material & Timing](spec/material-and-timing.md) · [DSP & Audio Engine](spec/dsp-and-audio-engine.md) · [Protocols & Persistence](spec/protocols.md)

---

## 1. Mandatory Automated Verification Gates

Every change must pass all applicable verification gates cleanly (and CI in `.github/workflows/ci.yml` enforces them on every PR across Linux and macOS):

| # | Gate Command | What It Verifies |
| --- | --- | --- |
| 1 | `cargo fmt --all -- --check` | Uniform workspace formatting across all crates, tests, and examples |
| 2 | `cargo clippy --workspace --all-targets -- -D warnings` | Zero lint warnings across default features (`audio`, `midi`, `gui`, `rbs`) |
| 3 | `cargo clippy --no-default-features --all-targets -- -D warnings` | Zero lint warnings in minimal headless configuration |
| 4 | `cargo test -p cadence-engine --lib` | Debug-mode thread stack safety (heap-boxed `MixerEngine`, `Engine`, `Shared`, `Box<ModularVoice>` pool, and delay pools), `CheckedAllocator` zero-RT-allocation enforcement, and engine DSP invariants |
| 5 | `cargo test --workspace` | Full unit, integration, Modular Rack, modeless UX, audio recording, devices, `.rbs` import, pure `MemoryHostIo`, and headless Metal/Vulkan GPU test suite |
| 6 | `cargo test --release --test audio_invariants` | Release-mode determinism, offline-vs-live parity, pan/headroom laws, portability, and full signal hygiene across all 5 showcase tracks + the *Fivefold Live* take |
| 7 | Visual PNG inspection (on GUI edits) | Manual inspection of regenerated Metal/Vulkan GPU renders from `crates/gui/tests/*_gpu.rs` and `tests/docs_screenshots_gpu.rs` (all 17 `1440×860` flagship screenshots backed by live 48 kHz engine telemetry, plus embedded app icon PNGs) |

---

## 2. Test Architecture by Layer

### Pure Core, Modular Rack & DSP (`crates/core`, `crates/formats`)
- **Pattern algebra & `:mini`:** Bounded shared-DAG validation (`MAX_DEPTH = 32`), cycle/type/period rejection, stateless `det_hash` determinism, exact sub-step offsets across all 22 `PatternKind`s, and `Reverse(Reverse(x)) == x` involution.
- **Unified Eurorack Modular Rack (`crates/core/src/rack/`):** `Project` v1 and `InstrumentPreset` v1 schema validation, all 11 orthogonal modules (`Vco`/`Sampler`/`Noise`/`Env`/`Lfo`/`Filter`/`Vca`/`Drive`/`LoFi`/`Resonator`/`Out`), singular mandatory `Out` protection, `MAX_RESONATORS_PER_ZONE = 4` validation, Kahn DAG topological compiler (`CompiledZoneGraph`) with `z-1` 1-sample feedback cycles, unified `InstrumentParam` / `ParamAddress` resolution, and atomic stale-reference/macro pruning.
- **Pure DSP (`crates/core/src/dsp/`):** Sample-rate invariance (`44.1 / 48 / 96 / 192 kHz`) across `Eq`, `Svf`/`AcidLadder`/`TptOnePoleLpf`, `NoiseGen` & `PinkFilter`, `Halfband2x` oversampling (`step_vco_bank_fm_2x` & 2× `Drive`), `ModularEnv` (`Adsr`/`HoldDecay` + pre-delay + multi-burst), `decay_coeff_60db_ms` SPOT, `LoFiProcessor`, `ModalResonator` (`String`/`Bar`/`Membrane`/`Plate`), `SamplerVoice`, `Compressor`, `Limiter` lookahead PDC, `Chorus`, `Delay` (`PingPong`/`Stereo` & 32-step ceiling), `Reverb` prime-coprime RT60, dual pan laws, and `+6 dB` power-taper roundtrips.
- **Persistence & codecs:** Atomic writes, `store::read_bounded` limits, bounded `read_dir` scans, `.take.jsonl` crash recovery (`TAKE_VERSION = 1`), `PresetCatalog` graceful degradation, RIFF `inst`/`smpl` metadata roundtrips, and clean-room ReBirth `.rbs` import housed in `crates/formats` behind `feature = "rbs"` (`tests/rbs_import.rs`).

### Real-Time Audio Engine (`crates/engine`)
- **Zero-RT-allocation proof:** `crates/engine/src/mix_engine/tests.rs` installs a `#[global_allocator] CheckedAllocator` asserting **0 heap allocations** during `MixerEngine::process`, `update_control_block`, `adopt`, `ModularVoice` execution, and `Engine::next_frame`.
- **Routing, PDC, live input & metering:** Heap-boxed `Box<ModularVoice>` graph evaluation, nested `Group → Group` causal summing, ancestor-aware mute/solo, chained Limiter PDC alignment, 32-bit float `hot` vs. Master `clipped` semantics, `256`-slot sub-step frame scheduling, lock-free `AudioTapHandle` streaming, and lock-free SPSC audio input monitoring + 24-bit WAV recording (`tests/audio_recording.rs`).

### Pure Controller, Modeless Input & Host Protocols (`crates/app`, `tests/`)
- **Hermeticity & purity:** All integration and GPU tests run hermetically against synthetic temp fixtures (`tests/common/mod.rs`) and `SampleLibrary::from_prefixed_roots` with zero reliance on host `$HOME` configs or private `./samples/`. `crates/app` enforces zero direct `std::fs`/`std::env`/`File::` I/O (`HostIo` + `MemoryHostIo`).
- **Domain symmetry, Rack & modeless UX (`tests/`):** `modular_rack.rs`, `rack_zones_bounce.rs`, `zone_params_and_routing.rs`, `external_presets.rs`, `dsp_eq_limiter_fm.rs`, `macros.rs`, `live_staging_midi_pan.rs`, `devices_audio_deck.rs`, `audio_recording.rs`, `modeless_input.rs`, `modal_authoring.rs`, `overlays_and_palette.rs`, `workflow_drafts.rs`, `file_hub_and_assets.rs`, `asset_readiness.rs`, `search_and_help.rs`, `signal_graph.rs`, `take_workflow.rs`, and `navigation_confirms_and_hints.rs` verify end-to-end parity across the 4-tier modeless grammar, 3-tab Eurorack Rack (`Panel | Patchbay | Zones`), unified `Audio | MIDI` Devices deck, live 24-bit recording, 6-category live Preset Browser (`P`), Filter Groove Browser/Fork/Save, 8 FX, 8 Macros, Signal Graph, Song Properties (`gP`, 13 `NAMED_SCALES`), Mini Editor, `ValueInput`, Fuzzy Action Palette (`:` / `Ctrl+Shift+p`), and explicit Take arming (`tr`/`v`/`X`).
- **Host protocols & CLI:** `tests/ctl_bridge.rs` exercises the live Unix `--ctl` socket (`0o600`, `SUN_LEN` fallback, `state`/`meters`/`cmd`/`action`/`listen --bars`/`listen --live`, `expected_revision`, RAII tap busy-guard cleanup); `tests/cli.rs`, `tests/material_sessions.rs`, `tests/format.rs`, and `tests/robustness.rs` cover `--script`, `--stdio`, `--key-script`, `--check`, `--render`, `--stems`, `--render-kit`, `--import-rbs`, and `--recover`.

### Headless GPU Presentation (`crates/gui/tests/`, `tests/docs_screenshots_gpu.rs`)
- Real Metal/Vulkan `HeadlessGpu` suites (`arrange_graph_gpu.rs`, `float_metering_gpu.rs`, `font_help_gpu.rs`, `fx_visuals_gpu.rs`, `graph_and_assets_gpu.rs`, `live_staging_pan_gpu.rs`, `macros_gpu.rs`, `midi_settings_help_gpu.rs`, `modeless_direct_edit_gpu.rs`, `overlays_gpu.rs`, `rack_instrument_gpu.rs`, `rack_zones_gpu.rs`, plus root `tests/docs_screenshots_gpu.rs` [all **17** `1440×860` flagship screenshots backed by live 48 kHz `cadence-engine` telemetry across all 3 GPU themes]) render full `FrameView`s, asserting hermetic 2-face embedded `JetBrains Mono` Regular (400) + Bold (700) typography with coding ligatures disabled and zero `.notdef` / tofu glyphs (including ASCII `"z-1"` feedback badges), embedded 256×256 app icon decoding (`icon::tests`), all 12 `ModuleVisualView` visualizers, shared `cable_graph.rs` Patchbay & Signal Graph rendering, 6 aligned deck layouts, balanced `push_clip`/`pop_clip`, zero hardcoded RGBA outside `crates/gui/src/theme.rs`, and non-empty visual geometry.

### Release Audio Invariants (`tests/audio_invariants.rs`)
1. `no_example_project_or_take_contains_absolute_sample_paths` & `absolute_sample_paths_are_rejected_and_save_stays_portable` — guarantees 100% repo-relative portability.
2. `same_host_render_is_sample_deterministic` — repeated renders are bit-for-bit identical.
3. `offline_compile_and_live_ring_window_have_exact_parity` — offline song compilation and sliding live-ring compilation match exactly.
4. `all_showcase_examples_and_fivefold_take_pass_signal_hygiene` — finite statistics, `|DC| < 0.005`, RMS in `[-32.0, -4.0] dBFS`, peak in `[-18.0, -0.05] dBFS`, `0` full-scale clipped samples, `0` hot (`> 0 dBFS`) or clipped channels/groups/returns/Master across every bus, and clean tail decay (`< -30 dBFS` on explicit tails).
5. `equal_power_pan_law_preserves_stereo_power_across_synth_and_sampler`, `unity_master_and_limiter_ceiling_enforce_exact_dbfs_bounds`, and multi-zone Rack velocity-split, gain, and choke-layer invariants.

---

## 3. Verified 48 kHz Mastering Baseline — Showcase Suite

All five showcase compositions render directly from Cadence's native 32-bit float engine at **48 kHz** with **no external mastering or post-export normalization**, using strictly **100% MIT repo-relative assets** (`factory-samples/`, `examples/*/samples/`). All tests remain self-contained and never read or depend on any private local sample directories.

| Showcase | Length / Tempo / Key | True Peak | Integrated RMS | Tail Peak (last 0.5 s) | Clips / Hot Buses | Key Capabilities Verified |
| --- | --- | --- | --- | --- | --- | --- |
| [**Afterglow 3.0**](https://github.com/cadencefm/cadence/blob/main/examples/demo/README.md) | 64 bars · 128 BPM · F minor (2:00) | `-0.80 dBFS` (`[0.9120, 0.9120]`) | `-16.19 dBFS` (Climax `-12.70 dBFS`, Drop 1 `-13.27 dBFS`, Breakdown `-19.26 dBFS`) | `-64.73 dBFS` | `0` clips · `0` hot/clipped (`DC 0.000002`) | Retained `Harmony` DAG, `:mini`, `9` Rack instruments/lanes (incl. `FX Sweep` riser/downlifter), 9-zone MIT `EDM Kit` Rack + `ZoneParam` P-locks, 6 Macros (`M1–M6`), `20` FX (incl. `Kick Sculpt`, `Sweep Sculpt`, Master `Comp → Eq → Limiter @ -0.8 dBFS`), `5` ducks |
| [**Iron Circuit**](https://github.com/cadencefm/cadence/blob/main/examples/iron-circuit/README.md) | 48 bars · 132 BPM · A minor (1:27) | `-0.80 dBFS` (`[0.9120, 0.9120]`) | `-13.67 dBFS` (`-13.62 / -13.72 dBFS`, `0.10 dB` L/R balance) | `-59.18 dBFS` | `0` clips · `0` hot/clipped (`DC 0.000014`) | Unified Modular Racks across every role (`TR-808` 16-zone & `TR-909` 11-zone Kits, `Sampler` Racks, `AcidLowPass` 303 & wavefolded Synth Racks), nested `Percussion → Rhythm → Master` DAG, all 8 FX & modes (`Drive(HardClip/Fold/Tape)`, `Delay(Stereo/PingPong)`, custom `FilterEnv`, 22 FX), all 8 Macros (`M1–M8`) |
| [**Glass Cathedral**](https://github.com/cadencefm/cadence/blob/main/examples/glass-cathedral/README.md) | 32 bars · 108 BPM · D Dorian (1:11) | `-1.50 dBFS` (`< 0.001 dB` L/R diff) | `-14.62 dBFS` (`0.17 dB` L/R balance) | `-40.77 dBFS` | `0` clips · `0` hot/clipped (`DC 0.000004`) | 100% synthesis, polymetric `SeededValues(7) × Euclidean(5,11)`, `TimeScale(3/2)` `Arp(Random)`, `:mini` chimes, 4 Macros (`M1–M4`), automated `Send` swells, 4-stage Master (`Tape → Comp → Eq → Limiter @ -1.5 dBFS`) |
| [**Fivefold Live**](https://github.com/cadencefm/cadence/blob/main/examples/fivefold-live/README.md) *(62-bar take & 60-bar song)* | 60 bars / 62-bar take · 136 BPM · D minor (1:49) | `-1.00 dBFS` (take & song) | `-15.32 dBFS` take (`-13.09` Crest) · `-15.19 dBFS` song | `-240 dBFS` (digital silence) | `0` clips · `0` hot/clipped (`DC 0.000107`) | Scripted live performance (`compose.py` + `takes/set.take.jsonl`, 17 live gestures incl. live harmony edit & bar-quantized `:undo`), `AcidLowPass`, 4 Macros (`M1–M4`), 18 FX (`Comp → Tape → Limiter @ -1.0 dBFS`), bit-identical offline take renders |
| [**Signal Garden**](https://github.com/cadencefm/cadence/blob/main/examples/material-lab/README.md) | 10 bars (8 + 2 tail) · 136 BPM · D minor (0:18) | `-1.82 dBFS` | `-15.50 dBFS` | `-240 dBFS` (digital silence) | `0` clips · `0` hot/clipped (`DC 0.000036`) | Rebuilt identically from `--empty` via `build.keys.jsonl` (`tests/material_sessions.rs`), live fork vs. freeze, placement-local ending, seeded per-note pan, sidechain duck + gain contour, plus 5 companion workflow key scripts |

*Note:* Automated audio invariants verify determinism, headroom hygiene, DC bounds, pan/limiter accuracy, and tail completion; subjective musical balance and hardware audio/MIDI device acceptance are audited separately by ear.
