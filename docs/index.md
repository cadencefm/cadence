# Cadence

**A fast, keyboard-driven musical studio where your synths, drum machines, studio effects, audio recording, and live performance live in one native binary — and musical relationships stay alive instead of flattening into frozen copy-paste clips.**

Built on a native `wgpu` GPU canvas (`Metal` / `Vulkan` / `DX12`) and a zero-allocation 32-bit float audio engine, Cadence lets you compose, sound-design, mix, record, and perform without plugin overhead or mouse fatigue. Change one chord in a 4-bar progression and your pads, stabs, arpeggios, and bassline follow together in lockstep — while you ride macros, sculpt studio FX, and launch live lane overrides on exact bar boundaries.

![Afterglow 3.0 in Cadence Arrange, Bank, and 32-bit float Output](screenshots/01-arrange.png)

---

## Quick Start

Requires a current stable Rust toolchain. macOS uses `Metal` + `CoreAudio` + `CoreMIDI`; Linux uses `Vulkan` + `ALSA` (`libasound2-dev`). Pass `--silent` to run without an audio device, or `--no-default-features` for headless CLI builds.

```sh
git clone https://github.com/cadencefm/cadence.git
cd cadence

# Flagship 64-bar melodic EDM showcase ("Afterglow 3.0")
cargo run --release -- --project examples/demo/project.json

# 48-bar Acid Electro & hardware groovebox showcase ("Iron Circuit")
cargo run --release -- --project examples/iron-circuit/project.json

# 32-bar generative ambient / IDM showcase ("Glass Cathedral")
cargo run --release -- --project examples/glass-cathedral/project.json

# Live keyboard performance set & replayable take ("Fivefold Live")
cargo run --release -- --project examples/fivefold-live/project.json

# Start from a blank canvas
cargo run --release -- --empty
```

---

## Core Capabilities

- **Modeless Keyboard Grammar:** Navigate spatially with `h/j/k/l` (`H/L` coarse/tabs), sculpt values and reorder items directly in place with `Alt+h/j/k/l` (`Alt+H/J/K/L` coarse) or one-hand `,`/`.` (`<`/`>`), enter exact values with `Enter`, and press `:` / `Ctrl+Shift+p` for the Fuzzy Action Palette or `?` for searchable in-app Help.
- **11-Module Unified Eurorack Rack:** Every instrument is a patchable `Rack` (`Panel` | `Patchbay` | `Zones`) built from `Vco` (7-voice unison + 2× through-zero FM), `Sampler` (automatic RIFF `smpl`/`inst` loops), `Noise`, `Env` (ADSR/Hold-Decay + pre-delay + multi-burst), `Lfo`, `Filter` (`LowPass6`..`AcidLowPass`), `Vca`/RingMod, `Drive`, `LoFi`, `Resonator` (32-mode physical String/Bar/Membrane/Plate bank), and `Out`, with bipolar attenuverter cables and sample-accurate `z-1` feedback — backed by a 100% MIT external preset and drumkit catalog (`TR-808`, `TR-909`, `EDM Kit`, live synth kits, and 38 single-voice drum presets).
- **Retained Material Algebra & Inline `:mini`:** 22 composable pattern operations (`AddDegrees`, `Mask`, `Transpose`, `NotesFromValues`, `Arp`, `Euclidean`, `TimeScale`, `Swing`, `Ratchet`, `Shift`, `Reverse`, `Chance`, `Every`, `RangeFold`, `Concat`, `Stack`, `Shape`, `SeededValues`, and live-validated `:mini` notation) with fractional sub-step timing (`offset ∈ [0.0, 1.0)`).
- **32-Bit Float Mixer & 8 Studio Effects:** Native `Eq`, `Filter` (with extensible 128-slot rhythmic `FilterEnv` groove bank), `Compressor`, `Limiter` (with lookahead PDC), `Drive`, `Chorus`, `Delay`, and `Reverb`, nested `Group → Group` buses, automated `Send` levels, sidechain `Duck` envelopes, and `+6 dB` fader headroom.
- **Live Performance, 24-Bit Audio Recording & Hardware MIDI:** Launch parts on exact bar boundaries, enter modal Jam Mode (`gj` / `Ctrl+Shift+j`), stage atomic multi-gesture drops (`S` / `Alt+Enter`), arm crash-safe performance Takes (`tr`), record live 24-bit WAV audio straight into a project Sampler (`ta` / `tA` / `ti` / `:rec`), and map hardware MIDI controllers via 1-key Learn (`M`) with bi-directional feedback in the unified `Audio | MIDI` Devices deck (`gd` / `Ctrl+d`).
- **Scriptable & Agent-Ready:** Clean human-readable `v1` JSON projects, presets, and JSONL takes, deterministic WAV + per-channel stem export (`--render`, `--stems`, `--render-kit`), clean-room ReBirth `.rbs` import (`--import-rbs`), and a live `--ctl` Unix control socket (`cadence ctl`).

---

## Explore the Documentation

- **[User Guide](guide/README.md)** — Start here for a guided walkthrough from your first 5 minutes through sound design, mixing, hardware MIDI, live recording, performance takes, and CLI scripting.
- **[Decks, Keymap & Interaction Spec](spec/decks-and-interaction.md)** — Complete modeless keybinding contract, directional tiers, canonical verbs, deck layouts, and modal overlay behavior.
- **[Material & Timing Spec](spec/material-and-timing.md)** — Formal `Project` v1 graph, 22 `PatternKind` operations, `:mini` syntax, fractional sub-step timing, and live launch/keep semantics.
- **[DSP & Audio Engine Spec](spec/dsp-and-audio-engine.md)** — Pure `cadence-core::dsp` signal math, 11-module Rack runtime, 32-bit float mixer, PDC, and real-time audio invariants.
- **[Protocols & Persistence Spec](spec/protocols.md)** — JSON schemas, `--script` / `--stdio` JSONL actions, live `--ctl` Unix socket protocol, and audio/preset/bundle formats.
- **[Design Decisions](design-decisions.md)** & **[Verification Log](verification.md)** — Architectural rationale and test/audio verification matrix.
- **[Source Code on GitHub](https://github.com/cadencefm/cadence)** — MIT-licensed Rust workspace (`cadence-core`, `cadence-formats`, `cadence-engine`, `cadence-app`, `cadence-gui`, `cadence`).
