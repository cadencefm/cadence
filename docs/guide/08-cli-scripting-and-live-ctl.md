# Chapter 8 · CLI, Headless Rendering, Formats & Remote Control (`--ctl`)

> Previous: [Live Performance & Armed Takes](07-performing-and-takes.md) · **Chapter 8** · [Guide Home](README.md)

Everything you can do interactively in Cadence can also be validated, rendered, imported, scripted, or driven externally from tools and AI co-producers via the CLI and Unix control socket (`--ctl`).

---

## 1. Offline Verification, Audio Export & Recovery

All offline commands run headless with zero GUI window or audio device required:

```sh
# Validate schema, derivations, and sample readiness (prints JSON summary)
cargo run --release -- --project examples/demo/project.json --check

# Render stereo 16-bit WAV + JSON overload/RMS/tail report
cargo run --release -- --project examples/demo/project.json \
  --render afterglow.wav \
  --render-report afterglow-report.json \
  --sample-rate 48000

# Render Master + isolated per-channel stereo WAV stems (optional --bars limit)
cargo run --release -- --project examples/iron-circuit/project.json \
  --stems ./stems/iron-circuit --bars 16 --sample-rate 48000

# Render a saved performance take offline (with 2-bar reverb/delay tail)
cargo run --release -- --project examples/fivefold-live/project.json \
  --take examples/fivefold-live/takes/set.take.jsonl \
  --take-tail-bars 2 --render fivefold.wav

# Export the built-in Afterglow demo as a standalone JSON project
cargo run --release -- --write-demo my-demo.json

# Restore unsaved edits from a crash-recovery snapshot (never overwrites disk until you save)
cargo run --release -- --project my-song.json --recover
```

### Useful Session, Audio & Discovery Flags
- `--silent`: run interactively without opening an OS audio output device.
- `--audio-device NAME`: preferred audio output device name (substring match; overrides `config.json` for this session).
- `--buffer-size FRAMES`: preferred audio callback buffer size (`32..=8192` frames; overrides `config.json` for this session).
- `--sample-dir DIR` / `--preset-dir DIR`: repeatable extra search roots (`--sample-roots-only` restricts Sample Browser discovery to explicit `--sample-dir` roots).
- `--session-dir DIR`: override the per-project session directory (instance lock, `recovery.json`, armed `takes/`, default `ctl.sock`).
- `--list-midi`, `--midi-in [PORT]`, `--midi-out [PORT]`: inspect or connect hardware MIDI ports at launch.

---

## 2. Clean-Room Formats Crate & ReBirth RB-338 `.rbs` Import (`crates/formats`)

Third-party song format importers are isolated in the feature-gated **`cadence-formats`** crate (`crates/formats/`), keeping `cadence-core` and `cadence-engine` free of external format parsers. It houses Cadence's clean-room MIT parser and converter for classic **ReBirth RB-338 `.rbs`** songs behind **`feature = "rbs"`** (enabled by default in the root `cadence` binary)—supporting both **ReBirth 1.x** (`MThd` SMF with 7-bit-packed `TinY` SysEx: 2× `TB-303` + `TR-808`) and **ReBirth 2.0** (`CAT RB40` IFF, file versions `4.0`, `4.1`, and `4.2`, adding `TR-909`):

```sh
# Open a ReBirth .rbs song directly in the GUI (or convert headless with --output)
cargo run --release -- --import-rbs classic-acid.rbs
cargo run --release -- --import-rbs classic-acid.rbs --output classic-acid.cadence.json
cargo run --release -- --import-rbs classic-acid.rbs --render classic-acid.wav

# Passing an .rbs file to --project also auto-imports it cleanly
cargo run --release -- --project classic-acid.rbs
```

Inside the GUI, use **`:import-rbs PATH`** (or open **File Hub `gf`** → **Import** tab). The importer translates 303 patterns (6-module `AcidLowPass` Racks with `filter_vel_amount` accent, slide, deduplicated transposed parts), `TR-808`/`TR-909` multi-zone Rack kits, `Synths`/`Drums` Groups, `Echo` Return + automated `Send`, `Drive`, Pattern Controlled `Filter` (PCF), `Compressor`, Global Shuffle (`Swing`), Song Mode sections, and knob/mixer automation into an editable native Cadence project with calibrated mix headroom.

---

## 3. Provenance Factory Kit Builder (`--render-kit`)

Build portable, 100% MIT multi-zone **Rack** drum kits deterministically from pure-synthesis source projects (`factory-sources/{tr-808,tr-909,edm-kit}/project.json`) with 1:1 baked-vs-synth loudness parity:

```sh
cargo run --release -- \
  --render-kit factory-sources/tr-808/project.json \
  --out-samples factory-samples/tr-808 \
  --out-preset presets/kits/tr-808.json \
  --out-synth-preset presets/kits/tr-808-synth.json \
  --out-voices-dir presets/drums \
  --kit-name "TR-808" \
  --sample-rate 48000
```

Each Audio channel in the source project is rendered, trimmed cleanly, measured (duration, peak, DC offset), written as a WAV with embedded RIFF `inst` + `smpl` metadata into `--out-samples`, and wired into a ready-to-load **`v1` `InstrumentPreset`** JSON (`category: "Kit"`, 1-based `RackZone` pads with `[Sampler -> Out]`) at `--out-preset`. Passing optional `--out-synth-preset` also writes the matching synthesized multi-zone Rack kit, and `--out-voices-dir` exports each individual drum voice as a standalone 1-zone preset.

---

## 4. Reproducible Automation: `--key-script`, `--script` & `--stdio`

### Key Scripts (`--key-script FILE.keys.jsonl`)
Replay real keyboard sessions directly through the pure `cadence-app` controller, advance the headless performance clock, and assert exact project/mode invariants (including JSON Pointer assertions like `"/bpm": 132.0`):

```json
{"label": "Enter Jam and play", "keys": "gj ", "expect": {"mode": "JAM", "playing": true}}
{"label": "Jump to section 2 after 4 bars", "keys": "2", "advance": "4 bars", "expect": {"step": 64}}
```

Note that `:` (and `Ctrl+Shift+p`) opens the **Fuzzy Action Palette (`PALETTE`)**, which automatically surfaces any recognized Ex command line as its top `▶ Run command:` passthrough slot—so `:bpm 132<Enter>` or `:derive ...<Enter>` key scripts execute deterministically alongside interactive deck workflows (`NORMAL`, `STEP INSERT`, `JAM`, `PRESETS`, `SONG PROPS`, `MINI EDITOR`, `VALUE ENTRY`).

Supported special key tokens include `<Tab>`, `<s-tab>`, `<Enter>`, `<Esc>`, `<Home>`, `<End>`, `<Space>`, `<bs>`, `<up>`/`<down>`/`<left>`/`<right>`, `<pgup>`/`<pgdn>`, `<lt>`/`<gt>`, `<c-x>` (`Ctrl+x`), `<c-s-x>` (`Ctrl+Shift+x`), `<m-x>` / `<m-X>` (`Alt+x` / `Alt+Shift+x`), and `<m-enter>`.

```sh
cargo run --release -- --empty \
  --key-script examples/material-lab/build.keys.jsonl \
  --check --output built-garden.json
```

### Typed JSONL Protocol (`--script` & `--stdio`)
Both batch `--script` and interactive headless `--stdio` speak Cadence's unified newline-delimited **`HostRequest`** JSON protocol (`id`, `expected_revision`, `compact`, `action`, `cmd`, `meters`, `listen`):

```sh
# Batch-apply typed actions from examples/agent-session/session.jsonl and render
cargo run --release -- --empty \
  --script examples/agent-session/session.jsonl \
  --output session.json --render session.wav

# Run an interactive headless engine over stdin/stdout (transport ticks every ~16 ms while idle)
cargo run --release -- --empty --stdio
```

See [**Agent Session**](https://github.com/cadencefm/cadence/blob/main/examples/agent-session/README.md) (`examples/agent-session/session.jsonl`) and the [Protocols Spec](../spec/protocols.md) for the complete `Action` and `HostRequest` schema.

---

## 5. Remote GUI Control Socket (`--ctl` & `cadence ctl`)

Co-produce alongside a script, terminal, or AI agent while **watching and hearing Cadence's GPU window update in real time**! Every edit sent over `--ctl` passes through the exact same `Document` pipeline and lands cleanly on your **`u` undo stack**.

### 1. Launch Cadence with `--ctl`
```sh
# Bind at default <session_dir>/ctl.sock (auto-falls back to a short $TMPDIR path on macOS/Unix if >96 bytes)
cargo run --release -- --project examples/demo/project.json --ctl

# Or bind an explicit socket path
cargo run --release -- --project examples/demo/project.json --ctl /tmp/cadence.sock
```

The control socket is created with strict **`0o600`** (owner-only) permissions, checks for active listeners before replacing stale files, wakes the GUI event loop immediately on incoming requests, and cleans up both the socket file and temporary `ear.wav` on exit.

### 2. Drive & Audition the Running Session with `cadence ctl`
`cadence ctl` auto-discovers `--sock PATH`, `--session-dir DIR`, `CADENCE_CTL_SOCK`, or the most recently active session socket:

| Subcommand | Description |
|---|---|
| `cadence ctl state [--compact]` | Fetch current session state, transport position, revision, and (without `--compact`) full project JSON. |
| `cadence ctl meters` | Fetch 32-bit float channel input/output peaks, peak holds, `hot`/`clipped` flags, and Compressor/Limiter gain-reduction meters. |
| `cadence ctl cmd "<LINE>" [--expected-revision N] [--compact]` | Execute any Cadence Ex command line on the running GUI session (pushed onto `u` undo stack). |
| `cadence ctl action '<JSON>' [--expected-revision N] [--compact]` | Execute any typed `cadence_app::Action` JSON object with optional optimistic-concurrency revision checking. |
| `cadence ctl listen --bars A..B [--solo CH,...] [--sr 24000] [--out WAV]` | Render an exact 1-based half-open `A..B` (or inclusive `A..=B`) bar window from current in-memory state (temporary `--solo` never alters GUI flags) with peak/RMS/clip telemetry. |
| `cadence ctl listen --live SECS [--sr 24000] [--out WAV]` | Capture `0.25..=30.0` seconds of real-time speaker output from the lock-free audio tap ring on a background thread so Jam/MIDI/GPU rendering never stalls. |

### Example Remote Co-Production Loop
```sh
# 1. Inspect current revision & compact telemetry
cadence ctl state --compact

# 2. Mutate the running project cleanly (guarded by expected revision)
cadence ctl cmd "bpm 132" --expected-revision 4 --compact
cadence ctl cmd "macro 1 75%" --compact
cadence ctl action '{"op":"play","playing":true}' --compact

# 3. Check 32-bit float headroom & bus compression
cadence ctl meters

# 4. Audition bars 17..21 with temporary solo on Drums + Bass (without mutating GUI solo flags!)
cadence ctl listen --bars 17..21 --solo Drums,Bass --out /tmp/drop-check.wav

# 5. Or tap 4 seconds of whatever is currently playing out of the speakers
cadence ctl listen --live 4.0 --out /tmp/speaker-tap.wav
```

---

> **Go deeper:** Read the [Specifications](../spec/decks-and-interaction.md) ([Material & Timing](../spec/material-and-timing.md), [DSP & Audio Engine](../spec/dsp-and-audio-engine.md), [Protocols](../spec/protocols.md)), [Design Decisions](../design-decisions.md), and [Verification & Testing](../verification.md), or return to the [User Guide Home](README.md).
