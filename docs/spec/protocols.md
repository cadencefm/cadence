# Automation Protocols, Remote `--ctl` Socket, Takes, Presets & ReBirth `.rbs` Import

This specification defines Cadence's external interfaces and on-disk contracts: the unified JSONL host protocol (`--script`, `--stdio`, `--ctl`), the GUI Unix remote-control socket, key-script replay, crash-safe performance takes, external presets and `--render-kit`, and the clean-room ReBirth RB-338 `.rbs` importer.

> **Related specifications:** [Material & Timing](material-and-timing.md) · [DSP & Audio Engine](dsp-and-audio-engine.md) · [Decks & Interaction](decks-and-interaction.md) · [CLI & Scripting Guide](../guide/08-cli-scripting-and-live-ctl.md)

---

## 1. Unified JSONL Host Protocol (`--script`, `--stdio`, `--ctl`)

All three machine-control surfaces share one request envelope (`HostRequest`) and one dispatch function (`execute_host_request`) in `src/host/protocol.rs`:

- **Line framing:** Newline-delimited UTF-8 JSON; blank lines are ignored. Maximum line length is `MAX_REQUEST_BYTES = 65_536` (64 KiB). Oversized lines return `{"ok": false, "error": "JSONL request exceeds 64 KiB"}` without dropping the connection.
- **Request envelope (`#[serde(deny_unknown_fields)]`):**
  ```json
  {
    "id": "optional-client-token",
    "expected_revision": 12,
    "compact": true,
    "action": { "op": "set_tempo", "bpm": 132.0 }
  }
  ```
  Exactly **one** operation field must be present per request:
  - **`action`** — a typed `cadence_app::Action` (`#[serde(tag = "op", rename_all = "snake_case")]`), including `get_state`, `edit`, `transaction` (atomic up to `16_384` `Edit`s), `create_*` (`create_instrument` / `placed_part` taking unified `SoundChoice::{template, preset, sampler, folder}`), `set_param` (`ParamAddress::{rack, zone, module, cable, channel, send, effect}`), `set_rack`, `rack_add_module`/`rack_remove_module`/`rack_rename_module`, `rack_connect_cable`/`rack_disconnect_cable`/`rack_set_cable_amount`, `rack_pin_control`/`rack_unpin_control`, `load_preset`, `set_macro`/`rename_macro`/`bind_macro`/`unbind_macro`/`clear_macro`, `zone_*`, `launch`/`release`/`toggle_lane`/`stage`/`finish_staging`/`advance`, `bounce` (`BounceTarget::{new_sampler, rack_zone}`), `save`/`load`/`export` (plus `import_rbs` when built with `--features rbs`), and `command`.
  - **`cmd`** — a Cadence Ex command string (dispatched identically to `Action::Command { line }`, pushing onto the interactive `u` undo stack).
  - **`meters: true`** — returns real-time playback status, channel meters (`input_peak`, `input_peak_hold`, `output_peak`, `output_peak_hold`, `hot`, `clipped`), and Compressor/Limiter `effect_meters` (`device_id`, `gain_reduction_db`, `gr_hold_db`).
  - **`listen`** — structured audio capture (`ListenRequest::Bars` or `ListenRequest::Live`; see §2).
- **Optimistic concurrency (`expected_revision`):** If supplied and `expected_revision != app.revision()`, the request is rejected without side effects (`ok: false`, `"Revision conflict: expected X, current Y"`). Entity IDs (`Id = u64`) are monotonically allocated and never recycled across undo/redo.
- **Full vs. compact responses:**
  - `compact: false` (default): returns `{ id, ok, revision, message, status, dirty, bpm, project, flags, selection, inspection, ready, sample_issues, required_sample_issues, audition, follow, live, transport?, export? }`.
  - `compact: true`: omits `project`/`flags`/`selection` for fast polling, returning `{ id, ok, revision, playing, step, bpm, status, message, dirty, ready, channel_meters, effect_meters, inspection?, transport?, export? }`.
- **Execution modes:**
  - **`--script FILE`:** Headless offline execution (`audio: None`). Successful requests remain applied if a later request fails; process exits non-zero when any request fails.
  - **`--stdio`:** Interactive engine mode (hardware CPAL or `--silent`). A reader thread decouples stdin reads so the host ticks `observe_transport` every `IDLE_TICK = 16 ms` while idle, landing boundary changes and sliding the rolling ring without waiting for input.

---

## 2. GUI Remote Control Socket (`--ctl` & `cadence ctl`)

Launching the GUI with `--ctl [<SOCK>]` binds a local Unix domain control socket (`src/host/ctl.rs`) wired directly into the running `winit` event loop so external tools and AI co-producers can inspect, edit, and audition the active session interactively.

### Socket Lifecycle, Security & Path Resolution

- **Default socket & `SUN_LEN` short-path fallback (`SessionDir::ctl_socket_path` in `crates/core/src/store.rs`):** Defaults to `<session_dir>/ctl.sock`. On Unix, if the path exceeds `MAX_UNIX_SOCKET_PATH_BYTES = 96` bytes (safely below macOS's 104-byte `sun_path` limit), it hashes the session root with FNV-1a 64-bit to `<temp_dir>/cadence-ctl-<hash16>.sock` (falling back to `/tmp/cadence-ctl-<hash16>.sock`).
- **Owner-only permissions & liveness checks:** Refuses to overwrite non-socket files; probes existing sockets with `UnixStream::connect` to reject a second active server before unlinking any stale socket; binds with `0o600` mode bits.
- **Non-blocking GUI integration:** Accepts connections on `cadence-ctl-accept` and enqueues parsed requests over a bounded `CTL_QUEUE_CAPACITY = 32` `sync_channel` (`CLIENT_IO_TIMEOUT = 40 s`), invoking `set_wake` (`EventLoopProxy`) so requests execute immediately even when the window is idle or occluded. Dropping `CtlServer` stops the listener and deletes both the socket and any auto-generated `ear.wav`.
- **Client discovery (`resolve_ctl_socket`):** Checks in priority order: (1) `--sock PATH`, (2) `--session-dir DIR`, (3) `CADENCE_CTL_SOCK` environment variable, (4) newest active socket under `<platform_state_dir>/sessions/*/ctl.sock`.

### `cadence ctl` CLI Subcommands (`src/main.rs`)

| Subcommand | Request Dispatched | Notes |
| --- | --- | --- |
| `cadence ctl state [--compact]` | `action: GetState` | Full or lightweight telemetry snapshot |
| `cadence ctl meters` | `meters: true` | Real-time channel peaks/holds/`hot`/`clipped` + Comp/Limiter GR |
| `cadence ctl cmd <LINE> [--expected-revision N] [--compact]` | `cmd: LINE` | Runs any Ex command on the running GUI (undoable via `u`) |
| `cadence ctl action <JSON> [--expected-revision N] [--compact]` | `action: JSON` | Runs any typed `Action` JSON object |
| `cadence ctl listen --bars A..B [--solo ...] [--sr HZ] [--out PATH]` | `listen: { mode: "bars", ... }` | Offline deterministic render of 1-based half-open `A..B` or inclusive `A..=B` (up to `MAX_LISTEN_BARS = 256`, default `24_000 Hz`, range `8_000..=192_000 Hz`) from current in-memory state; `--solo` accepts channel/lane/instrument names or IDs temporarily without mutating GUI flags; returns per-bar & overall peak/RMS/clip/channel/effect metrics |
| `cadence ctl listen --live SECONDS [--sr HZ] [--out PATH]` | `listen: { mode: "live", ... }` | Captures `0.25..=30.0 s` of real-time speaker output from `AudioRuntime`'s lock-free SPSC stereo tap ring (`AUDIO_TAP_CAPACITY = 262_144` frames) on a background `cadence-live-listen` thread (`LiveListenJob`, guarded by atomic `live_tap_busy` + RAII `TapBusyGuard`), resamples via `dsp::resample_stereo`, writes 16-bit WAV atomically, and returns `{ path, sample_rate, source_sample_rate, frames, duration_sec, peak_db, rms_db, clipped, full_scale_samples, dropped_frames }` |

---

## 3. Headless Key-Script Replay (`--key-script`)

`--key-script FILE` (`src/host/script.rs`) replays deterministic keyboard sessions directly through `cadence-app`'s input router. Lines are JSON objects (blank lines and `#` comment lines are skipped):

```json
{"label": "Enter Jam and advance 2 bars", "keys": "gj ", "advance": "2 bars", "expect": {"mode": "JAM", "playing": true, "/bpm": 136.0}}
```

- **`keys`** — parsed by `cadence_app::parse_key_sequence`, supporting chord tokens (`<c-s-j>`, `<c-s-p>`, `<m-enter>`, `<m-h>`, `<c-a>`, `<c-r>`, `<esc>`, `<enter>`, `<tab>`, `<s-tab>`, `<space>`, `<home>`, etc.). Note that `:` opens the Fuzzy Action Palette (`Mode::Palette`) whose top `▶ Run command:` passthrough slot executes recognized Ex command lines on `Enter`.
- **`advance`** *(optional)* — duration string parsed by `cadence_app::parse_duration` (e.g., `"2 bars"`, `"4 beats"`, `"16 steps"`) to advance the headless transport clock.
- **`expect`** *(optional)* — map of assertions checked after `keys` and `advance`:
  - Plain keys match controller summary fields (`title`, `patterns`, `instruments`, `parts`, `lanes`, `placements`, `channels`, `sections`, `ducks`, `revision`, `dirty`, `ready`, `mode`, `draft_open`, `value_entry_open`, `step`, `playing`, `active`, `pending`, `lane_parts`, `workspace`).
  - Keys starting with `/` are evaluated as RFC 6901 JSON Pointers against the serialized `Project` JSON.

---

## 4. Crash-Safe Take Journal & Atomic Session Storage

### Bounded Atomic Storage (`crates/core/src/store.rs`)

- **Bounded reads (`store::read_bounded`):** Checks `metadata().len() <= max_bytes` *and* caps reads via `File::take(max_bytes + 1)` before allocating memory.
- **Atomic writes (`store::write_atomic` / `write_atomic_bytes`):** Rejects payloads over `max_bytes` before touching disk, creates a unique sibling temp file `.name.<pid>.<counter>.tmp` (`create_new(true)`), flushes + `sync_all()`, atomically `rename`s over destination, and `fsync`s the parent directory on Unix.
- **Per-project `SessionDir` (`<platform_state_dir>/sessions/<stem>-<fnv1a64>/` or `--session-dir`):**
  - `lock` (`InstanceLock`): exclusive PID lock rejecting concurrent active editors while cleaning stale locks left by dead PIDs.
  - `recovery.json` (`MAX_RECOVERY_BYTES = 16 MiB`): periodic atomic autosave restored non-destructively via `--recover` (never overwrites the user's project file on disk).
  - `takes/`: armed session take journals (explicitly armed via `tr` / `:take arm`, default OFF, or opt-in via `UserConfig::auto_record_takes`), pruned to `DEFAULT_UNKEPT_TAKES = 20`.

### Crash-Safe `.take.jsonl` Format (`crates/core/src/take.rs`)

A take records the exact score of a performance (`TAKE_VERSION = 1`, `MAX_TAKE_BYTES = 32 MiB`) as newline-delimited JSON whenever take recording is armed (`Action::ToggleTakeArmed` / `Action::SetTakeArmed` / `UserConfig::auto_record_takes`; `Action::DiscardTake` discards an unsaved take; `TakeJournal::is_capped()` surfaces a `● TAKE CAPPED` warning badge if the in-memory cap is reached):

1. **Header (line 1):** `TakeHeader { take: 1, title, start_step, active: Vec<Override>, project: Box<Project>, flags?, segment? }` (where `take: 1` and `project` are validated strictly against `TAKE_VERSION = 1` and `Project::VERSION = 1`).
2. **Events & marks:** `TakeEvent { at: u64, label, change: TakeChange }` (`Launch`, `Release`, `ReleaseAll`, `Flags`, `Edits`, `Replace`, `Segment`) interleaved with periodic transport `TakeMark { mark: u64 }` heartbeats.
3. **Seal line:** `TakeEnd { end: u64 }` followed by `sync_data()`.

**Off-thread streaming:** `open_disk_journal_sink` spawns a dedicated `cadence-take-journal` background thread so UI/audio loops never stall on disk I/O; in-memory tests substitute `MemoryJournalSink`. Unsealed files left by a hard crash remain valid up to their last flushed event or mark. Offline `--take FILE --render OUT.wav --take-tail-bars N` folds every event through the exact same `PerformanceState::apply` transition as real-time playback, producing bit-identical WAV renders.

---

## 5. External `InstrumentPreset` Schema (`v1`), `PresetCatalog` & `--render-kit`

### Unified External `InstrumentPreset` (`crates/core/src/presets.rs`)

Zero instrument presets are compiled into the binary; only the 4 bare neutral engine templates (`Init Synth`, `Blank Sampler`, `Blank Kit`, `Blank Modular` via `RackTemplate::{Synth, Sampler, Kit, Modular}` and `InstrumentPreset::bare_default`) are defined in code. Every external preset `.json` (`INSTRUMENT_PRESET_VERSION = 1`, `MAX_PRESET_BYTES = 512 KiB`, `#[serde(deny_unknown_fields)]`) shares one unified `RackPatch` schema:

```json
{
  "version": 1,
  "name": "TR-808",
  "category": "Kit",
  "patch": {
    "kit_name": "TR-808",
    "zones": [
      {
        "id": 1,
        "name": "BD",
        "low_midi": 36,
        "high_midi": 36,
        "root_midi": 36,
        "modules": [ "...RackModule..." ],
        "cables": [ "...PatchCable..." ]
      }
    ],
    "panel_pins": []
  }
}
```

- `name`: `1..=80` printable non-control characters; optional `category`: `1..=48` characters (conventional categories `"Synth"`, `"Sampler"`, `"Drums"`, `"Kit"`, `"Modular"` backing the Preset Browser's 6 category filter tabs `All` / `Synth` / `Sampler` / `Drums` / `Kit` / `Modular`).
- **`PresetCatalog`:** Discovers `.json` presets across configured `UserConfig::preset_dirs`, CLI `--preset-dir`, and project-adjacent directories (`MAX_PRESET_FILES = 2_048`, `MAX_PRESET_SCAN_ITEMS = 16_384`), exposing `by_category`, `find`, `find_by_category`, `is_bare_default_token`, and `names_hint`. Missing or empty roots degrade cleanly with zero errors. `normalize_preset_key` maps common short aliases (`808`/`tr808` → `tr-808`, `909`/`tr909` → `tr-909`, `edm`/`edmkit`/`kit` → `edm-kit`, `303`/`acid`/`tb303`/`tb-303` → `acid-303`, `lead` → `pulse-lead`, `pad` → `warm-pad`, `bass`/`sub` → `sub-bass`, `hat` → `closed-hat`).

### Provenance Factory Kit Builder (`--render-kit` — `src/host/kit_render.rs`)

```sh
cadence --render-kit source-kit.json \
  --out-samples factory-samples/my-kit \
  --out-preset presets/kits/my-kit.json \
  [--kit-name "My Kit"] [--sample-rate 48000]
```

For each unmuted `ChannelKind::Audio` channel of the source project:
1. Solos and renders the channel in 32-bit float (`render_schedule_f32`) with 1:1 baked-vs-synth loudness parity, rejecting silent or clipped voices.
2. Trims leading silence at `ONSET_THRESHOLD = 1e-6`, trims trailing silence at `TAIL_THRESHOLD = -72 dBFS` (`0.00025118864`) plus a `TAIL_FADE_MS = 2.0 ms` half-cosine tail fade, and writes mono WAVs automatically when `|L - R| < MONO_EPSILON = 1e-5`.
3. Writes each 16-bit PCM WAV atomically via `cadence_core::sampler::write_wav_with_metadata`, embedding both **RIFF `inst`** (7-byte MIDI unity note, fine tune, key & velocity range) and **RIFF `smpl`** (unity note, pitch fraction, and optional loop points) chunks.
4. Maps canonical drum channel names (`BD`, `SD`, `CH`/`OH` → choke group `1`, etc.) to `rack::drum_midi` (`36..=51`), anchors portable sample locators at `factory-samples/...`, builds 1-based `RackZone::sample_pad` zones (`[Sampler -> Out]`), and writes the validated `v1` `Rack` `InstrumentPreset` JSON (`category: "Kit"`).

---

## 6. Clean-Room ReBirth RB-338 `.rbs` Import Contract

`crates/formats/src/rbs/` (`cadence-formats`, gated behind `feature = "rbs"`, enabled by default in the root `cadence` binary) implements a clean-room MIT binary parser and converter invoked via `--import-rbs FILE`, `--project FILE.rbs`, `:import-rbs`, or `FileHub`.

- **Bounded inputs:** `MAX_RBS_BYTES = 16 MiB`, `TICKS_PER_BAR = 768`, `POS_PER_BAR = 32` (32nd-note automation resolution), `MAX_RBS_POS = 32_000` (1000 bars). `peek_rbs_header` reads title/version/BPM/mode metadata for File Hub previews without full decoding.
- **Supported binary formats:**
  - **ReBirth 1.x (`MThd`):** Standard MIDI File with a 7-bit-packed `TinY` SysEx block → creates 3 instruments & 3 lanes (`TB-303 A`, `TB-303 B`, `TR-808`; omits absent `TR-909`).
  - **ReBirth 2.0 (`CAT .... RB40`, file versions `4.0`, `4.1`, `4.2`):** IFF container → creates 4 instruments & 4 lanes (adding `TR-909` with flam subdivisions encoded as `PatternKind::Mini`).
- **Target Modular Rack & Mixer Mapping:**
  - `TB-303 A/B` → canonical 6-module Acid `RackPatch` (`Vco -> Filter(AcidLowPass) -> Vca -> Out`, `Amp Env -> Vca.Cv`, `Filter Env -> Filter.Cv`) with curated `panel_pins`, official `song.vintage` shorter filter-envelope decay scaling, mono legato glide, 2-step slide-into-rest ties, and cross-bar/cross-segment Song Mode slide lookahead.
  - `TR-808` / `TR-909` → multi-zone `Rack` kits resolved from external `PresetCatalog` with Global Accent (`ctrl 2`) velocity calibration, BPM-accurate TR-909 flams (`10..=42 ms` / `step_ms`), `PatternKind::Stack` overflow safety for dense `>6`-voice simultaneous hits, and TR-808 primary/alt voice switch timelines (`LT/MT/HT/RS/CP` vs `LC/MC/HC/CL/MA`; missing external presets degrade gracefully with non-fatal `RbsImportSummary::notices`).
  - **Mixer & FX:** `Synths` and `Drums` Group buses; Master Level (`ctrl 0`) automation via `ChannelGain { channel: ChannelRef::Master }`; v4.2 per-channel Enable/Mute (`ctrls 5,11,17,23`) composed with channel gain; `Echo` Return using `DelayMode::Stereo` (straight 16ths / eighth-note triplets `4/3x`) fed via post-fader, pre-strip-pan `ChannelSend` + `ModulationTarget::Send`; `Dist` using `DriveMode::HardClip` with synced output trim; PCF → rhythmic `Filter` referencing `presets/filter_grooves.json`; `Comp` → `Compressor` (zero auto makeup gain).
  - **Musical structure & automation:** Declarative per-device control tables (`controls.rs`), deduplicated shared patterns, derived `Part.transpose` for transposed 303 motifs, full launchpad `lane.parts`, Global Shuffle via `PatternKind::Swing`, event-driven sub-bar Song Mode placements preserving bar-locked/odd-length phase, automatic section detection, and jump-vs-sweep compressed `Values` automation curves (targeting `(zone, module, param)` and `(zone, cable)` cleanly while leaving `project.macros` empty so direct edits respond immediately).
