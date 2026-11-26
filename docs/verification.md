# Verification & Testing Architecture

Cadence enforces audio determinism, real-time safety, pure controller hermeticity, and GPU rendering correctness through automated CI gates (`.github/workflows/ci.yml` across Linux and macOS) and a 4-layer DSP/engine contract pyramid. Contributor rules live in [CONTRIBUTING.md](https://github.com/cadencefm/cadence/blob/main/CONTRIBUTING.md).

> **Related specifications:** [Decks, Keymap & Interaction](spec/decks-and-interaction.md) · [Material & Timing](spec/material-and-timing.md) · [DSP & Audio Engine](spec/dsp-and-audio-engine.md) · [Protocols & Persistence](spec/protocols.md)

---

## 1. Mandatory Automated Verification Gates

| # | Gate Command | What It Verifies |
| --- | --- | --- |
| 1 | `cargo fmt --all -- --check` | Uniform formatting across all crates, tests, and examples |
| 2 | `cargo clippy --workspace --all-targets -- -D warnings` | Zero lint warnings across default features (`audio`, `midi`, `gui`, `rbs`) |
| 3 | `cargo clippy --no-default-features --all-targets -- -D warnings` | Zero lint warnings in minimal headless configuration |
| 4 | `cargo test -p cadence-engine --lib` | Debug stack safety (heap-boxed `MixerEngine`, `MixerBlockScratch`, `VoiceBlockOut`, `Engine`, `Shared`, `Box<ModularVoice>`, `Box<ActiveEffect>`), `CheckedAllocator` zero-RT-allocation/drop proofs, and engine/mixer contracts |
| 5 | `cargo test --workspace` | Full unit, integration, Modular Rack, direct-edit UX, audio recording, devices, `.rbs` import, pure `MemoryHostIo`, and headless Metal/Vulkan GPU suites |
| 6 | `cargo test --profile release-fast --no-default-features --test audio_invariants` | Release-mode determinism, offline-vs-real-time parity, pan/limiter/headroom laws, asset portability, and showcase signal hygiene |
| 7 | Visual PNG inspection *(on GUI edits)* | Visual review of regenerated headless Metal/Vulkan PNG renders (`crates/gui/tests/*_gpu.rs` and `tools/render_docs_screenshots.rs`) |

---

## 2. DSP & Audio Engine Contract Pyramid

Core audio verification is deliberately decoupled from demo projects so new effects, operating modes, or routing topologies cannot slip past default/neutral patches:

1. **Exhaustive `StereoEffect` Contract Matrix (`crates/core/src/dsp/contract_tests.rs`):**
   - Compile-time exhaustive `match` factory over `EffectKindTag::ALL`, `FilterMode::ALL` (static + clocked rhythmic envelope), `DriveMode::ALL`, and `DelayMode::ALL` using non-neutral `(PresetA, PresetB)` parameter pairs across `48 kHz` and `96 kHz`.
   - Verifies six universal invariants for every entry: 0-ULP frame-vs-block parity across active/bypass/idle-bypass/un-bypass and odd block sizes, silent-sleep equivalence (`is_quiet` + `advance_silent_frames` under concurrent parameter mutations), bypass-idle parameter-mutation equivalence, reset determinism, non-finite/hot input hygiene, and dynamics gain-reduction parity.
2. **Exhaustive `ModularVoice` & Rack Contract Matrix (`crates/engine/src/modular_voice/tests.rs`):**
   - Synthetic racks covering every `ModuleKindTag::ALL`, `FilterMode::ALL`, `DriveMode::ALL`, `SampleLoopMode::ALL`, `VoiceMode::ALL`, `ResonatorMode::ALL`, and `z-1` 1-sample feedback path.
   - Verifies `render_block` vs `next_frame` 0-ULP parity, zero RT heap operations, runaway feedback stability, and post-VCA tail retirement.
3. **Synthetic Kitchen-Sink Engine Rig & Sleeping-vs-Awake Oracle (`crates/engine/src/test_fixtures.rs`, `audio/tests.rs`, `mix_engine/tests.rs`):**
   - Self-contained 4-bar synthetic rig exercising all 8 non-neutral effects, chained `Group → Group` buses, multi-Return sends, parallel + send + sidechain-duck PDC, muted channels with active pre-insert metering, late-entering `Chorus` & clocked `RhythmicFilter` channels sleeping across bars under continuous automation, sub-step offsets, drum choke groups, mono legato, P-locks, and real-time effect hot-swaps.
   - Verifies arbitrary block-size parity (`1..512`), `CheckedAllocator` zero-RT-allocation/drop proofs across `Engine::render_block`, `MixerEngine::{process_block, adopt_prepared}`, and rapid coalesced `replace_project_with_assets` swaps, deterministic parallel stem exports, transport-step preservation across `reconfigure_device`, and a **Sleeping-vs-Awake Oracle** comparing full channel sleeping against an always-awake engine across audio, channel meters, and dynamics GR stats.
4. **Showcase Smoke Health (`tests/audio_invariants.rs`):**
   - Validates all five bundled showcase compositions ([Afterglow](https://github.com/cadencefm/cadence/blob/main/examples/demo/README.md), [Iron Circuit](https://github.com/cadencefm/cadence/blob/main/examples/iron-circuit/README.md), [Glass Cathedral](https://github.com/cadencefm/cadence/blob/main/examples/glass-cathedral/README.md), [Fivefold Live](https://github.com/cadencefm/cadence/blob/main/examples/fivefold-live/README.md) song + take, and [Signal Garden](https://github.com/cadencefm/cadence/blob/main/examples/material-lab/README.md)) for schema validity, repo-relative asset portability, finite audible levels, zero digital clipping (`!ch.clipped` and `0` full-scale samples — internal 32-bit float bus headroom is allowed), bounded DC offset (`|DC| < 0.005`), and clean tail decay.
   - Run `cadence --project <path> --render out.wav --render-report report.json` (or `cargo run --release --example build-demo -- --analyze`) anytime to inspect loudness, peak, DC, and per-channel metering on any project.

---

## 3. Core, Controller, Protocol & GPU Suites

- **Pure Core & Formats (`crates/core`, `crates/formats`):** Pattern algebra DAG bounds (`MAX_DEPTH = 32`), all 22 `PatternKind`s and `:mini` sub-step offsets, Rack `Project`/`InstrumentPreset` v1 validation and Kahn `CompiledZoneGraph` compilation, sample-rate-invariant DSP unit suites (`44.1 / 48 / 96 / 192 kHz`), RIFF `inst`/`smpl` codecs, crash-safe `.take.jsonl` journals, bounded I/O (`store::read_bounded`), and clean-room `.rbs` import (`tests/rbs_import.rs`).
- **Pure Controller & Workflows (`crates/app`, `tests/`):** Hermetic in-memory testing via `HostIo` / `MemoryHostIo` (zero host `$HOME` or private `./samples/` reliance) across the 4-tier keyboard grammar, 3-tab Eurorack Rack, unified `Audio | MIDI` Devices deck, 24-bit WAV recording, Preset & Filter Groove browsers, Macros, Signal Graph, File Hub, Takes, and repeatable key-script sessions (`tests/material_sessions.rs`).
- **Host Protocols & CLI (`tests/ctl_bridge.rs`, `tests/cli.rs`):** Unix `--ctl` socket (`0o600`, `SUN_LEN` fallback, `state`/`meters`/`cmd`/`action`/`listen`, optimistic revision guards), `--script`, `--stdio`, `--key-script`, `--check`, `--render`, `--stems`, `--render-kit`, `--import-rbs`, and `--recover`.
- **Headless Metal/Vulkan GPU Suites (`crates/gui/tests/*_gpu.rs`, plus `tools/render_docs_screenshots.rs`):** Renders full `FrameView`s across all 3 GPU themes using `HeadlessGpu`, verifying hermetic embedded `JetBrains Mono` typography (ligatures disabled, zero `.notdef` glyphs), embedded app icon decoding, all 12 Rack visualizers, shared `cable_graph.rs` Patchbay & Signal Graph rendering, balanced `push_clip`/`pop_clip`, and zero hardcoded RGBA outside `crates/gui/src/theme.rs`.
