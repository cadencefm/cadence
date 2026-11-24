# Cadence TODO

Active post-`v0.1.0` backlog, polish items, and future roadmap. Completed work lives in git history.

---

## Deferred Core, Engine & Architecture Work

- [ ] **Modular Rack Performance, Live Activity Telemetry & Shared Visualizer Helpers:**
  - Publish fixed-size per-module / per-cable live activity telemetry in `AudioSnapshot` (`crates/core/src/runtime.rs`) driving `PatchbayNodeView.activity` → `cable_graph::CableNode.activity` shimmer bars and live Patchbay audio/CV cable glow.
  - Keep `SampleBank` sorted for O(log N) binary search across `crates/engine/src/audio/voice_pool.rs` and `crates/engine/src/modular_voice/` instead of linear scan.
  - Extract a shared GUI deck visualizer helper (`crates/gui/src/decks/vis.rs`) unifying `crates/gui/src/decks/instrument/` and `crates/gui/src/decks/effect/` card, curve, and badge painters.

---

## Polish & Structural Smells Backlog

### `cadence-engine` & `cadence` Host
- [ ] Pre-build dense `audio_order` / `return_order` active channel index lists in `MixerEngine::adopt()` (`crates/engine/src/mix_engine/`) so `MixerEngine::process()` loops strictly over active channels.

### `cadence-app`
- [ ] Unify `ZERO_ARG_COMMANDS`/`ARG_COMMANDS` (`crates/app/src/palette.rs`) with `crates/app/src/commands/*.rs` into one authoritative Ex verb registry SPOT.
- [ ] Add typed `editable_literal: bool` / `is_placement_scope: bool` flags to `PatternDeckView` (`crates/app/src/view/pattern.rs`) to remove string checks in `crates/gui/src/decks/pattern.rs` and show non-literal cues on derived `Values` curves.
- [ ] Group `Navigation` snapshot fields (`crates/app/src/lib.rs`), audit remaining invariant-guarded non-test `.unwrap()` calls across `crates/app/src/`, and review remaining Bank 2+ (`>10` sections) badge/key offsets (`input/jam.rs`, `view/{arrange,bank}.rs`, `sections.rs`).

### `cadence-gui`
- [ ] Extract a shared `draw_deck_tab_bar` helper unifying the Rack tab bar (`crates/gui/src/decks/instrument/mod.rs`) and the Devices `Audio | MIDI` tab bar (`crates/gui/src/decks/midi.rs`).
- [ ] Evaluate decomposing remaining large GUI modules (`cable_graph.rs`, `gpu/mod.rs`, `decks/mixer.rs`, `decks/pattern.rs`) when next touched.

---

## Future Features & Open Questions

- [ ] **Open Question — Per-Groove `presets/filter_grooves/*.json` Files Alongside Multi-Groove Banks:** Evaluate whether to support individual per-groove JSON files alongside multi-groove `presets/filter_grooves.json` bank files (weighing single-groove sharing convenience against multi-root merge order and slot-ID collisions).
- [ ] **Filter Groove Bank File Picker & Loader (`File Hub` / `:groove-bank`):** Allow browsing and loading arbitrary `filter_grooves.json` bank files directly from the file system (via File Hub / Filter Groove Browser action / `:groove-bank <path>`) instead of relying solely on preset search roots (`<project>/presets/`, `--preset-dir`, `UserConfig.preset_dirs`).
- [ ] **MIDI Clock & Outbound Note/CC Sequencing:** Sync external hardware synths/drum machines via MIDI Clock + Start/Stop and route selected Cadence lanes out to external MIDI channels.
- [ ] **Chord-Aware Harmony & Structure Extraction:** Rich chord-symbol sources (root, quality, extensions, voice-leading) and one-key extraction of harmony or transposed derivations from literal played notes.
- [ ] **Hold Gestures & Cue Output:** Momentary hold modifiers (mute/transpose/ratchet while held) and a dedicated stereo cue output bus for live pre-listening.
- [ ] **Sampler Transient/Grid Slicing:** Automatic onset detection and slice-to-Rack kit conversion directly from a `Sampler` module.
- [ ] **Standard MIDI File Export & CLAP Plugin Hosting:** Export arrangements to `.mid` and host CLAP instruments/effects behind the existing device trait boundary.
- [ ] **Browser / WASM Frontend:** Compile `cadence-core`, `cadence-engine`, and `cadence-app` to WebAudio + WebGPU against the same `FrameView` contract.

---

## Current Engine Bounds

16th-note primary grid with fractional sub-step timing; 2048 arrangement bars; 2048 patterns; 1024 literal steps; 16 instruments and lanes; 1024 parts; 4096 placements; 32 zones per Rack (`24` modules, `64` patch cables per zone, `4` resonators per zone; `16` Quick Panel pins and `8` choke groups per Rack); 16 audio channels + 4 groups + 4 returns + Master (24 mixer channels); 24 active effect devices; 64 filter envelope steps (`MAX_FILTER_ENV_STEPS`); 256 mixer/global automations; 16 sidechain ducks; 8 project macros × 16 bindings; 1 primary + 5 chord tones per step.
