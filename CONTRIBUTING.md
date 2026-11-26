# Contributing to Cadence

Thank you for contributing to Cadence. Before designing, editing, or reviewing code, please review these core repository rules, crate boundary invariants, and verification gates.

## Core Engineering & Repository Rules

- **Private `./samples/` folder:** The gitignored root `./samples/` directory is private and must never be read, copied, committed, packaged, depended on in code or tests, or shown in captures. Use `--sample-roots-only` with explicit `--sample-dir` roots for shareable sessions, and build synthetic fixtures (e.g., via `tempfile::tempdir()` and `tests/common/mod.rs`) in all tests.
- **100% MIT licensed & self-contained assets:** Shipped presets (`presets/`), factory drum samples (`factory-samples/`), app icons (`assets/icons/`), and example audio are original MIT-licensed assets, and each example folder carries its own license note. Binary audio and video assets (`*.wav`, `*.flac`, `*.mp4`, etc.) use Git LFS; PNG screenshots and icons are stored directly in Git.
- **Pre-release — zero legacy:** Never add backwards-compatibility shims, schema migration converters, transitional adapters, or dead-code fallbacks. When a model or interface needs to change, replace it cleanly and update all callers, tests, examples, and docs together.
- **Single Point of Truth (SPOT) & DRY:** Every piece of math, validation, parameter parsing/formatting, preset/metadata loading, and state transition lives in one authoritative module and is composed by all consumers.
- **Pull-request workflow:** Open PRs targeting `main`. CI (`.github/workflows/ci.yml`) gates merges on formatting, strict Clippy (full workspace + headless `--no-default-features`), Linux & macOS test suites (including Metal GPU tests and release audio invariants), and the `mdBook` docs build.

## Crate Boundaries & Hard Invariants

See [ARCHITECTURE.md](ARCHITECTURE.md) for the full 6-crate map. Changes must respect these boundaries:

- **`crates/core` (`cadence-core`):** Pure domain model (`Project` v1, unified Eurorack Modular `RackPatch`, `InstrumentPreset` v1, `Take` v1), material algebra, sample/RIFF codecs, AND **all pure composable DSP (`crates/core/src/dsp/`)**. Zero audio I/O, MIDI I/O, GUI, or third-party song-format importer dependencies.
- **`crates/formats` (`cadence-formats`):** Feature-gated third-party song format importers and exporters (including clean-room ReBirth `.rbs` import behind `feature = "rbs"`). Constructs strictly public `cadence_core::material::Project` values with zero runtime hooks into `cadence-core` or `cadence-engine`.
- **`crates/engine` (`cadence-engine`):** Real-time audio orchestration (`AudioEngine`, `ModularVoice`, `MixerEngine`), CPAL input/output + MIDI I/O, 24-bit WAV capture, and offline WAV/stem rendering.
  - Always compose `cadence_core::dsp::*` — never duplicate DSP math inside `cadence-engine`.
  - **Real-time safety:** Zero heap allocations, zero locks (`Mutex`/`RwLock`), zero unbounded channels, and zero `.unwrap()` / `.expect()` / `panic!` on audio-thread paths.
  - **Unclipped 32-bit float signal chain:** Preserve internal float headroom (`hot` when `> 1.0`), fire `clipped` strictly on Master post-fader `> 1.0` or non-finite samples, and clamp `[-1.0, 1.0]` exclusively at the final DAC/WAV output boundary.
  - **Debug stack safety:** Heap-box large engine structs (`MixerEngine`, `MixerBlockScratch`, `VoiceBlockOut`, `Engine`, `Shared`, `Box<ModularVoice>` slots, and `Box<ActiveEffect>` instances) so debug builds never overflow test stacks.
- **`crates/app` (`cadence-app`):** Pure zero-I/O session controller.
  - `Cargo.toml` depends **only** on `anyhow`, `cadence-core`, `cadence-formats` (feature-gated `rbs`), `serde`, and `serde_json` (plus dev-deps). Route all external I/O through `HostIo` (`crates/app/src/host_io.rs`).
  - Route `ParamAddress` resolution, parsing, formatting, and nudging through `crates/app/src/param_ops.rs` and effective modulation/Macro resolution through `crates/app/src/view/live_params.rs`.
  - Use bounded reads (`store::read_bounded`), bounded directory scans, and dirty-state confirmation guards (`Mode::Confirm`) on destructive load/import flows.
- **`crates/gui` (`cadence-gui`):** Pure `wgpu`/`winit`/`glyphon` renderer consuming `FrameView`.
  - **Zero hardcoded RGBA outside `crates/gui/src/theme.rs`:** Every visual element queries semantic `&Theme` tokens.
  - Emit quads and curves in painter's order into `DisplayList` (`crates/gui/src/display.rs`) with balanced `push_clip`/`pop_clip`.
- **`src/` (`cadence` host binary):** CLI, unified `Outcome` dispatcher (`src/host/mod.rs`), native file dialogs, CPAL input/output + midir host threads, `--render-kit`, `--import-rbs`, and `--ctl` remote-control Unix socket.

## Verification Gates

Run all applicable gates before opening a PR or completing a milestone:

```sh
# 1. Formatting
cargo fmt --all -- --check

# 2. Strict Clippy (full workspace + headless feature set)
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --no-default-features --all-targets -- -D warnings

# 3. Debug stack safety & zero-alloc engine unit tests
cargo test -p cadence-engine --lib

# 4. Full workspace test suite (unit, integration, and headless Metal/Vulkan GPU suites)
cargo test --workspace

# 5. Deterministic audio, parity & signal-hygiene invariants (required for core/engine/examples)
cargo test --profile release-fast --no-default-features --test audio_invariants
```

### Headless GPU Regression Tests & Visual Inspection

Whenever modifying `crates/gui` rendering, shaders, themes, or deck layouts, run the headless `wgpu` test suite and visually inspect regenerated PNGs:

```sh
cargo test -p cadence-gui --test gpu
```

Set `CADENCE_DUMP_GPU_DIR` to dump rendered Metal/Vulkan PNGs for visual inspection without launching an interactive window (for example, `CADENCE_DUMP_GPU_DIR=/tmp/cadence-gpu-check cargo test -p cadence-gui --test gpu`, or filter a single module such as `mixer_and_metering`).

Keep [docs/verification.md](docs/verification.md) aligned with the automated test layers and gates rather than duplicating per-song numeric render logs across docs.

## Examples, Presets & Docs

- **Reproducible showcases & kits:** Showcase tracks are generated reproducibly through Rust score builders (`examples/*/build.rs`, e.g. `cargo run --release --example build-demo -- --analyze`) or real controller key scripts (`*.keys.jsonl`) verified in integration tests (`tests/integration/cli.rs`). Drumkit sample packs and `v1` Rack kit presets can be baked reproducibly via `cadence --render-kit`.
- **Writing docs & `cadence.fm`:** Write for a musician or contributor who has never seen an earlier version. Describe what Cadence does today; keep architecture and specs synchronized across [ARCHITECTURE.md](ARCHITECTURE.md), [docs/guide/README.md](docs/guide/README.md), and [docs/spec/](docs/spec/decks-and-interaction.md). Documentation in `docs/` is published as the static site at `cadence.fm` via `book.toml` (`mdbook build`).
