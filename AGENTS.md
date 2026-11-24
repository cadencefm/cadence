# Cadence Agent & Swarm Guidelines

Read this before designing, editing, or reviewing code in this repository.

## 1. Core Engineering Philosophy
- **Pre-release — zero legacy:** Never add backwards-compatibility shims, schema converters, transitional adapters, or dead-code fallbacks. When a model or interface is wrong, replace it cleanly and update all callers, tests, and examples together.
- **Git is for history — code describes the present:** Never write filenames, test names, fixtures, docstrings, comments, or strings narrating prior audits, wave/track/sprint/phase/item numbers (`Wave*`, `Track*`, `Sprint*`, `Phase*`, `B1`/`C11`/`D17`/`E1`/`F3`/`H12`), `"smell"`/`"sweep"`/`"followup"`/`"regression"` grab-bags, historical workarounds, retired architectures (`v4`/`v5`/`v6`, `TUI`, `MultiSampler`, `SynthPatch`, `DrumKit`), or `"fix for X"` lore. Code, tests, and comments must always describe the current domain and invariants directly.
- **Single Point of Truth (SPOT) & DRY:** Every piece of math, validation, parameter parsing/formatting, metadata loading, and state transition must live in **one** authoritative place and be composed by all consumers.
- **100% MIT licensed & self-contained tests:** Never read, copy, commit, or depend on the private gitignored `./samples/` folder in code or tests. All tests must build synthetic fixtures (e.g., via `tempfile::tempdir()`).
- **Pushing to `origin`:** You may push to `origin` after clean, verified local commits or merges (we can always branch off earlier commits on the remote if needed). **Never rewrite remote history or force-push** (`--force`, `--force-with-lease`, etc.) without explicit user discussion.

## 2. Crate Boundaries & Hard Invariants
- **`crates/core` (`cadence-core`):** Pure domain model, material algebra, sample/RIFF codecs, AND **all pure composable DSP (`crates/core/src/dsp/`)**. Single mathematical source of truth for audio processing, pan laws, curves, transfer functions, envelopes, and parameter metadata. Zero audio I/O, MIDI I/O, GUI, or third-party song-format importer dependencies.
- **`crates/formats` (`cadence-formats`):** Feature-gated third-party song format importers and exporters (currently `feature = "rbs"` for clean-room ReBirth `.rbs` import; future `.mid` SMF, etc.) plus a format-agnostic import façade. Constructs strictly public `cadence_core::material::Project` values with zero runtime hooks into `cadence-core` or `cadence-engine`.
- **`crates/engine` (`cadence-engine`):** Real-time audio orchestration, `MixEngine` routing, voice management, CPAL, lock-free rings, MIDI I/O, and offline WAV rendering.
  - **MUST compose `cadence_core::dsp::*`** — never re-implement DSP math inside `cadence-engine`.
  - **Real-time safety:** Zero heap allocations, zero locks (`Mutex`/`RwLock`), zero unbounded channels (`mpsc::channel`), and zero `.unwrap()` / `.expect()` / `panic!` on audio-thread paths.
  - **Unclipped 32-bit float signal chain:** No hidden post-master `tanh`/soft-clipping; clamp `[-1.0, 1.0]` strictly at the final DAC/WAV conversion boundary. Internal float buses report `hot` (`> 1.0`), while `clipped` fires strictly on Master post-fader `> 1.0` or non-finite samples anywhere.
  - **Debug stack safety:** Heap-box large engine structs (`MixerEngine`, `Shared`, delay pools) so `cargo test -p cadence-engine --lib` never overflows debug stacks.
- **`crates/app` (`cadence-app`):** Pure controller, workflows, commands, undo/document, and `FrameView` projection.
  - **`Cargo.toml` purity:** Depends ONLY on `anyhow`, `cadence-core`, `cadence-formats` (feature-gated), `serde`, `serde_json` (plus dev-deps).
  - **Zero duplicated DSP/UI math:** Views (`crates/app/src/view/`) call `cadence_core::dsp::*` directly and resolve live modulation/Macros exclusively through `view/live_params.rs` (matching exact compiler precedence).
  - **Unified parameter handling:** Route `ParamAddress` resolution, parsing, formatting, and nudging through `crates/app/src/param_ops.rs`.
- **`crates/gui` (`cadence-gui`):** Pure `wgpu`/`winit` GPU renderer consuming `FrameView`.
  - **Zero hardcoded RGBA outside `crates/gui/src/theme.rs`:** All colors come from `&Theme`.
  - **Painter's order:** Emit quads and curves into the single ordered `DisplayList` (`Vec<DrawCmd>`) with balanced `push_clip`/`pop_clip`.
- **`src/` (`cadence` host binary):** Owns CLI, OS I/O, native file dialogs, audio/MIDI host bridge, and control socket.

## 3. Code Smell Radar — Actively Hunt & Bubble Up
While reading or modifying code for your assigned task, **keep an active eye out for structural smells** — both inside your changes and in neighboring code you inspect. Fix in-scope smells cleanly; **bubble up every out-of-scope smell** in a `### Smells / Follow-ups Spotted` section (with exact `file:line` references) in your completion report so it gets logged in `TODO.md` instead of silently accumulating.

Watch specifically for:
1. **Snowflake paths & backdoor constructors:** A preset, Ex command, or special flag creating or mutating an entity type that isn't a first-class peer in the main model/UI (e.g., a preset backdoor spawning an instrument variant missing from `SoundChoice`, or `LoadPreset` silently changing an instrument's engine variant).
2. **Duplicate logic & drifted SPOTs:** Copy-pasted parsers, formatters, lookup tables, curve/DSP formulas, validation rules, undo/commit pipelines, or magic constants across files or crates.
3. **Leaky internal variants & scattered ban checks:** An internal-only enum variant exposed on a public type and then rejected in multiple `if`/`match` ban checks across callers. Split the internal target cleanly instead of scattering bans.
4. **Ad-hoc conversions & silent data/metadata drops:** Inline unit conversions (`20.0 * log10`, manual pan/dB math), lossy clamps that destroy headroom or accents, or secondary creation/load paths that forget to apply metadata (e.g., RIFF `smpl`/`inst` chunks) or relativize paths on Save As / Bundle.
5. **Encapsulation leaks & reaching into another module's state:** Shadow fields duplicating authoritative state (causing desync or cross-deck key leaks), mutating `Project`/`Flags` directly around `Action`/`Document::commit`, bypassing undo coalescing, or peeking into another module's internals instead of calling one cohesive query/method.
6. **Asymmetric coverage across sibling variants:** A capability wired for some variants (`Synth`, `Sampler`, `ChannelGain`) but silently missing for peers (`MultiSampler` zones, `Send`, all 8 `EffectKind`s, all 22 `PatternKind`s) across UI creation, `:commands`, `Action`, `ParamAddress`, `ModulationTarget`, MIDI Learn, Live CC Rec, Signal Graph, Help (`?`), and Hints.
7. **Unbounded I/O or missing dirty-state guards:** Raw `std::fs::read` without `store::read_bounded`, or destructive open/import flows that skip `Mode::Confirm` when `dirty()`.

## 4. Verification Gates (Before Posting `[done]`)
Run all applicable gates after your final edit and report exact results:
1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo clippy --no-default-features --all-targets -- -D warnings`
4. `cargo test -p cadence-engine --lib` (debug stack safety)
5. `cargo test --workspace` (all unit, integration, and headless GPU suites)
6. `cargo test --release --test audio_invariants` (required whenever touching `core`, `engine`, or `examples`)
7. Visually inspect regenerated Metal GPU PNGs via `read_file` whenever modifying GUI rendering or layouts.
8. Include **`### Smells / Follow-ups Spotted`** (or explicitly state `None spotted`) in every `[done]` report.
