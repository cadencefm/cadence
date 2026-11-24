# Material Graph, Typed Edits & Single-Clock Timing

This specification defines Cadence's authoritative domain model, retained pattern algebra, atomic mutation invariants, single-clock timeline semantics, and flat executable score compilation. Every type, constant, and bound below is verified directly against `crates/core/src/`.

> **Related specifications:** [DSP & Audio Engine](dsp-and-audio-engine.md) · [Decks & Interaction](decks-and-interaction.md) · [Protocols & Persistence](protocols.md) · [Design Decisions](../design-decisions.md)

---

## 1. Authoring Document Schema (`Project` v1)

Retained musical definitions — not flattened clip audio or compiled note lists — are Cadence's saved source of truth (`crates/core/src/material/types.rs`). The on-disk JSON document (`crates/core/src/material/codec.rs`) flattens `Project` alongside optional sibling `flags: LiveFlags` (omitted when empty), with `#[serde(deny_unknown_fields)]` enforced on every domain type.

```text
Document JSON
├── version: 1                      (Project::VERSION = 1)
├── title: String                   (1..=120 printable chars)
├── bpm: f64                        (20.0..=300.0)
├── root_midi: u8                   (0..=127)
├── scale: Vec<i8>                  (1..=12 strictly ascending offsets in 0..=11, scale[0] == 0)
├── bars: usize                     (1..=MAX_BARS)
├── patterns: Vec<Pattern>          (retained literal & derived DAG nodes)
├── instruments: Vec<InstrumentInstance> (each owns one unified `rack: RackPatch` + `channel`)
├── parts: Vec<Part>                (Pattern + Instrument + transpose/velocity + ParameterLinks)
├── lanes: Vec<Lane>                (launchable LaneParts + non-overlapping Placements)
├── sections: Vec<Section>          (named step spans inside the arrangement)
├── mixer: MixerGraph               ({ master: MasterChannel, channels: Vec<Channel> })
├── ducks: Vec<Duck>                (event-triggered sidechain envelopes)
├── modulations: Vec<Modulation>    (global Values automation -> instruments/FX/mixer/macros)
├── macros: Vec<MacroControl>       (up to 8 assignable Project Macros, omitted when empty)
├── timing: LiveTiming              (launch/edit boundaries & late tolerance, omitted if default)
├── end: EndRule                    ("loop" | "stop" | "continue")
└── flags?: LiveFlags               (live lane/channel mute & solo — saved, never undone)
```

### Unified Eurorack Modular Rack Hierarchy & Parameter Addressing

Every `InstrumentInstance { id, name, rack: RackPatch, channel }` uses one unified Eurorack instrument architecture (`Rack -> Zones -> Modules [Panel] + Patchbay`) defined in `crates/core/src/rack/`. Single-sound synths and samplers are 1-zone Racks; drum kits and multisamples are N-zone Racks with key/velocity splits and choke groups:

```text
RackPatch (bus gain_db/pan/tune_semitones/cutoff_hz/resonance + panel_pins: Vec<PanelPin>)
└── zones: Vec<RackZone> (1..=32; stable id, key/vel splits, choke_group, VoiceSettings)
    ├── modules: Vec<RackModule> (1..=24; 11 orthogonal ModuleKind variants, <= 4 Resonators, exactly 1 Out)
    └── cables: Vec<PatchCable>  (0..=64; OutPort::Out -> InPort::{In,Cv,PitchCv,FmIn,PwCv,TrigIn}, bipolar amount)
```

All P-locks (`ParameterLink`), global `Modulation`s, Project Macros, MIDI Learn, Live CC Recording, Panel Pins (`PanelPin`), and `:param` commands address one orthogonal parameter model (`crates/core/src/params.rs`, `crates/core/src/rack/params.rs`):

- **`InstrumentParam`:** `Rack { param: RackParam }`, `Zone { zone: u16, param: ZoneParam }`, `Module { zone: u16, module: u16, param: ModuleParam }`, and `Cable { zone: u16, cable: u16 }`.
- **`ParamAddress`:** `Rack`, `Zone`, `Module`, `Cable`, `Channel`, `Send`, and `Effect` (`from_instrument()` / `as_instrument()` / `is_automatable()`).
- **Unified path resolution (`InstrumentParam::parse_for_rack`):** Resolves `<zone>.<module>.<param>` (e.g. `z1.filter.cutoff`, `BD.body_vco.tune`), `<zone>.cable.<id>` / `<zone>.c<id>` (and `c<id>` / `cable.<id>` on 1-zone Racks), `rack.<param>` / `bus.<param>`, `<zone>.<zone_param>`, `<module>.<param>` (on 1-zone Racks), and unambiguous shorthand (`cutoff`, `res`, `decay`, `env_mod`, `bits`, etc.) checking curated `panel_pins` first, then unique module parameters (disambiguating multiple `Env`s toward the primary audio-path lifetime envelope), then zone/rack parameters.

### Live State Separation (`LiveFlags`)

`LiveFlags` (`muted_lanes`, `solo_lanes`, `muted_channels`, `solo_channels`, `master_muted`) is persisted with the document so a workspace reopens as left, but mutating `LiveFlags` never enters undo history and never increments the authored song revision. Unknown or deleted lane/channel IDs are rejected by `Project::validate_flags` and pruned on save via `Project::prune_flags`.

### Authoritative Workspace & Domain Bounds

| Constant | Value | Source | Meaning |
| --- | --- | --- | --- |
| `VERSION` | `1` | `material/types.rs` | Exact authoring schema version (zero legacy converters) |
| `STEPS_PER_BAR` | `16` | `music.rs` | Fixed 16th-note grid per 4/4 bar |
| `MAX_BARS` | `2048` | `material/types.rs` | Maximum arrangement length (`MAX_TIMELINE_STEPS = 32_768` steps) |
| `MAX_PATTERNS` | `2048` | `material/types.rs` | Maximum pattern nodes in the project DAG |
| `MAX_LITERAL_STEPS` | `1024` | `material/types.rs` | Maximum stored steps for `Notes`, `Rhythm`, `Mini`, `Euclidean`, `Freeze` |
| `MAX_DEPTH` | `32` | `material/types.rs` | Maximum root-to-leaf DAG depth (enforced per path, memoized by `(Id, depth)`) |
| `MAX_INSTRUMENTS` | `16` | `material/types.rs` | Maximum instrument instances |
| `MAX_LANES` | `16` | `material/types.rs` | Maximum arrangement lanes |
| `MAX_PARTS` | `1024` | `material/types.rs` | Maximum part bindings |
| `MAX_PLACEMENTS` | `4096` | `material/types.rs` | Maximum total placements across all lanes |
| `MAX_LINKS` | `16` | `material/types.rs` | Maximum per-note `ParameterLink`s (P-locks) per `Part` |
| `MAX_GLOBAL_MODULATIONS` | `256` | `material/types.rs` | Maximum global `Modulation` relationships |
| `MAX_SECTIONS` | `256` | `material/types.rs` | Maximum named arrangement `Section`s |
| `MAX_TRANSACTION_EDITS` | `16_384` | `material/types.rs` | Maximum `Edit` operations per atomic transaction |
| `MAX_DERIVED_OFFSET` | `1_048_576` | `material/types.rs` | Maximum `\|offset_steps\|` on `Placement` or `LanePart` |
| `MAX_CHORD_TONES` | `5` | `music.rs` | Extra stacked voices per `Step` (up to 6 simultaneous notes) |
| `MIN_DEGREE` / `MAX_DEGREE` | `-127` / `128` | `music.rs` | Scale degree bounds covering full MIDI `0..=127` in any scale |
| `MAX_ZONES` / `MAX_CHOKE_GROUPS` | `32` / `8` | `rack/types.rs` | Key/velocity zones and choke groups per `RackPatch` |
| `MAX_MODULES_PER_ZONE` / `MAX_CABLES_PER_ZONE` | `24` / `64` | `rack/types.rs` | Eurorack modules and patch cables per `RackZone` |
| `MAX_PANEL_PINS` | `16` | `rack/types.rs` | Curated Quick Panel performance pins (`PanelPin`) per `RackPatch` |
| `MAX_PROJECT_MACROS` | `8` | `macros.rs` | Project Macros `M1..M8` (`index` in `0..8`) |
| `MAX_MACRO_BINDINGS` | `16` | `macros.rs` | Parameter bindings per Project Macro (`MAX_MACRO_NAME_LEN = 32`) |
| `MAX_LANE_PARTS` / `MAX_PARTS_ON_LANES` | `32` / `256` | `material/set.rs` | Launchable parts per lane and total across all lanes |
| `MAX_BOUNDARY_BARS` / `MAX_LATE_TOLERANCE` | `64` / `4` | `material/set.rs` | Maximum `Boundary::Bars` span and late-tolerance steps |
| `LIVE_RING_BARS` / `LIVE_RING_STEPS` | `64` / `1024` | `material/live.rs` | Sliding live compilation window length |
| `MAX_PROJECT_BYTES` | `16 MiB` | `cadence-app` | Bounded project JSON file size limit |

### Musical Step Primitives (`music.rs`)

Each `Step` encodes `active: bool`, `degree: i16` (`-127..=128`), `accidental: i8` (`-6..=6`), `velocity: f32` (`0.0..=1.0`), `gate: f32` (`0.05..=64.0` steps), `condition: StepCondition { turn, every }` (`1 <= turn <= every <= 16`, default `1:1` = `ALWAYS`), and `chord: Vec<ChordTone>` (up to `MAX_CHORD_TONES = 5`). Source steps in persisted projects must have `source_part: None`.

Scale degrees resolve relative to `Project::{root_midi, scale}` via `music::pitch_midi`, where degree `1` is the tonic (`root_midi + scale[0]`) and degree `0` is the scale tone immediately below tonic. All **13** built-in named scales live in one authoritative `music::NAMED_SCALES` table (shared by `scale_name`, `scale_label`, `named_scale`, `cycle_scale`, `:key`, and the Song Properties modal `gP`): `major`, `minor`, `dorian`, `phrygian`, `lydian`, `mixolydian`, `harmonic_minor`, `melodic_minor`, `major_pentatonic`, `pentatonic`, `blues`, `whole_tone`, and `chromatic`.

---

## 2. Pattern Algebra & Pure Evaluation

Every `Pattern` carries a stable `Id` (`u64`), unique name, and typed `PatternKind`. Validation resolves a static signature `(PatternType, period: u64)` for every node, rejecting missing references, cycles, type mismatches, non-positive periods, and depth `>= MAX_DEPTH`.

### Pattern Types (`PatternType`)

- `PatternType::Notes` — polyphonic musical note events with sub-step offsets.
- `PatternType::Rhythm` — boolean trigger gates (with optional sub-step offsets when derived from temporal operators).
- `PatternType::Values(ValueUnit::Degrees)` — integral scale-degree offsets (`-63..=63`, `ValueMode::Hold` only).
- `PatternType::Values(ValueUnit::Normalized)` — continuous or stepped control values (`0.0..=1.0`, `ValueMode::Hold` or `ValueMode::Ramp`).

### All 22 `PatternKind` Variants

#### Leaf & Literal Generators (6)

| Variant | Fields & Bounds | Output Type | Natural Period |
| --- | --- | --- | --- |
| `Notes` | `steps: Vec<Step>` (`1..=1024`) | `Notes` | `steps.len() * lcm(step.condition.every)` |
| `Rhythm` | `steps: Vec<bool>` (`1..=1024`) | `Rhythm` | `steps.len()` |
| `Values` | `unit`, `period` (`1..=MAX_BARS*16`), `mode` (`Hold`/`Ramp`), sorted unique `points` (`1..=period`) | `Values(unit)` | `period` |
| `SeededValues` | `unit`, `period` (`1..=MAX_BARS*16`), `seed: u64`, `min <= max` | `Values(unit)` | `period` (SplitMix64 hash, `Hold`) |
| `Mini` | `code: String`, `steps: u16` (`1..=1024`) | `Notes` | `steps * mini_cycle_multiplier(ast)` |
| `Euclidean` | `pulses: u16`, `steps: u16`, `rotation: i16` (`0 <= pulses <= steps <= 1024`) | `Rhythm` | `steps` (Bjorklund/Bresenham predicate) |

*Note:* `PatternKind::is_literal()` returns `true` for `Notes`, `Rhythm`, `Values`, `Mini`, and `Euclidean` (all resizable via `Edit::ResizePattern`). `SeededValues` is a zero-input deterministic generator whose points are frozen before per-step editing.

#### Retained Derivations & Combinators (16)

| Variant | Input Types | Parameters & Bounds | Output Type & Period | Semantics |
| --- | --- | --- | --- | --- |
| `AddDegrees` | `Notes`, `Values(Degrees)` | — | `Notes`, `lcm(p_n, p_v)` | Shifts each active step & chord tone by rounded degree value sampled at `time + offset` |
| `Mask` | `Notes`, `Rhythm \| Notes` | — | `Notes`, `lcm(p_n, p_r)` | Passes source note events only on steps where mask is active |
| `Transpose` | `Notes` | `semitones: -96..=96` | `Notes`, `p` | Chromatically transposes root & chord tones, remapped via `midi_to_degree` |
| `NotesFromValues` | `Values(Degrees)`, `Rhythm \| Notes` | `degree_offset: -63..=63`, `velocity`, `gate` | `Notes`, `lcm(p_v, p_r)` | Generates notes on every rhythm sub-step offset with `degree = round(val) + degree_offset + 1` |
| `TimeScale` | `Notes \| Rhythm` | `num: 1..=16`, `den: 1..=16` | Same, `max(1, lcm(p*den, num)/num)` | Compresses (`num > den`, e.g. `3/2` triplets) or expands (`num < den`) timing & scales gate by `den/num` |
| `Swing` | `Notes` | `amount: 0.0..=0.45` | `Notes`, `lcm(p, 2)` | Delays odd 16th steps by `amount`, carrying `offset + amount >= 1.0` cleanly into the next even step |
| `Ratchet` | `Notes` | `count: 1..=8` | `Notes`, `p` | Subdivides each hit into `count` evenly-spaced retriggers (5% per-hit velocity taper floored at `0.5×`, `gate / count`) |
| `Shift` | Any | `steps: i32` | Same, `p` | Rotates pattern in time (`source(time - steps)`) |
| `Reverse` | Any | — | Same, `p` | Exact period involution (`Reverse(Reverse(x)) == x`); mirrors sub-step `offset > 0` to `1.0 - offset` |
| `Chance` | `Notes \| Rhythm` | `probability: 0.0..=1.0`, `seed: u32` | Same, `p` | Deterministic per-hit gate using `det_hash(seed, id, time, i) < probability` |
| `Every` | Two same-type sources | `interval: 2..=64` | Same, `lcm(p_base * interval, p_alt)` | Evaluates `alt` on every `interval`-th cycle of `base` (`cycle mod interval == interval - 1`) |
| `Arp` | `Notes` | `mode: ArpMode`, `rate: 1..=4` | `Notes`, `p` | Collects all pitches held/sounding at `time` (lookback up to `min(p, 65)` steps), steps `Up`/`Down`/`UpDown`/`Random` at `rate` sub-steps/step |
| `RangeFold` | `Notes` | `low_midi <= high_midi <= 127`, span `>= 11` | `Notes`, `p` | Octave-folds all pitches & chord tones into `[low_midi, high_midi]` |
| `Concat` | `1..=16` same-type sources | `parts: Vec<Id>` | Same, `sum(p_i)` | Plays `parts[0]`, `parts[1]`, … end-to-end |
| `Stack` | `1..=16` same-type `Notes \| Rhythm` | `parts: Vec<Id>` | Same, `lcm(p_i)` | Layers sources simultaneously (merged sub-step events sorted by `offset`, or logical OR for `Rhythm`) |
| `Shape` | `Notes`, `Values(Norm)`, opt. `Values(Norm)` | `velocity`, `gate: Option<Id>` | `Notes`, `lcm` | Multiplies velocity by `v` and (when `gate` present) scales gate by `g * 2.0` |

### Inline Mini-Notation Parser (`crates/core/src/mini.rs`)

`PatternKind::Mini { code, steps }` parses a Strudel/Tidal-inspired notation into a validated `Vec<WeightedNode>` AST with zero mutable state:

- **Pitches:** Note names (`c4`, `eb3`, `f#5`, `cs4`, default octave `4`) or 1-based scale degrees (`1`, `0`, `-2`, `5#`, `3b`).
- **Rests:** `~` or `.`.
- **Recursive subdivisions:** `[c4 e4 g4]`, `[c4 [e4 g4]]` divide their enclosing span proportionally by node weights.
- **Chords:** `[c4,e4,g4]` or `c4,e4,g4` (up to `MAX_CHORD_TONES + 1 = 6` notes, validated at parse time).
- **Cycle alternations:** `<c4 e4 g4 b4>` selects one branch per cycle `cycle.rem_euclid(len)`.
- **Chainable modifiers:** `*N` repeat (`1..=16`), `/N` slow (`1..=16`), `@W` relative weight (`0.125..=32.0`), `(k,n)` / `(k,n,r)` Euclidean (`0 <= k <= n <= 64`), `?` (`0.5`) / `?p` (`0.0..=1.0`) deterministic chance via `det_hash`, `!` accent (`velocity = 0.98`, default `0.80`; gate = `0.85 * dur`, clamped `0.08..=64.0`).
- **Cycle period:** `mini_cycle_multiplier` computes the exact LCM of all nested `/N` slow factors and `<...>` alternation lengths so `PatternKind::Mini` integrates cleanly with `Every`, `Concat`, `Reverse`, and `Boundary::Cycle`.

### Pure Stateless Evaluation (`crates/core/src/evaluate.rs`)

Offline rendering, the live ring compiler, and UI projections all evaluate through `cadence_core::evaluate`:

- **`det_hash(seed: u32, pattern_id: Id, step: i64, salt: u32) -> f32`:** Pure stateless SplitMix64 finalizer mapping `(seed, pattern_id, step, salt)` deterministically onto `[0.0, 1.0)`. Querying any step in any order (scrubbing, scrolling, live ring window, or offline render) produces bit-identical results.
- **`EvaluatedEvent { offset: f32, step: Step }`:** Every `Notes` evaluation at integer 16th-note `step` yields 0, 1, or many events sorted ascending by fractional `offset ∈ [0.0, 1.0)`.
- **Value interpolation (`eval_value_points`):** `ValueMode::Hold` latches the prior breakpoint; `ValueMode::Ramp` linearly interpolates across wrapped period boundaries at arbitrary fractional `f64` step time. `Project::value_mode` propagates `Ramp` cleanly through `Shift`, `Reverse`, `Every`, and `Concat`.

---

## 3. Atomic Typed Edits & Structural Invariants

All persistent document mutations enter through `Edit` transactions (`Project::apply_transaction_validated` / `apply_transaction_with_flags` in `crates/core/src/material/mutate.rs`):

1. **Copy-on-write atomicity:** Up to `MAX_TRANSACTION_EDITS` (`16_384`) edits execute against a cloned candidate `Project`. Any failure discards the candidate completely; the live document is untouched.
2. **Monotonic stable IDs:** Entity IDs (`Id = u64`) are allocated monotonically by `Document` and never recycled across undo/redo.
3. **Unified modular Rack instrument mutations:**
   - `Edit::CreateInstrument { instrument }` — creates a new `Rack` instrument (fails if ID already exists).
   - `Edit::UpdateInstrumentMeta { id, name, channel }` — renames or re-routes an instrument without touching its `RackPatch`.
   - `Edit::UpdateRackPatch { id, rack }` — replaces an instrument's validated `RackPatch` (e.g., loading a preset or editing zone splits).
   - **Atomic modular authoring edits:** `Edit::AddRackZone`, `Edit::RemoveRackZone`, `Edit::ReorderRackZone`, `Edit::AddRackModule`, `Edit::RemoveRackModule`, `Edit::RenameRackModule`, `Edit::ConnectCable`, `Edit::DisconnectCable`, `Edit::SetCableAmount`, `Edit::PinPanelControl`, and `Edit::UnpinPanelControl`.
   - **Zone & graph structural invariants (`RackPatch::validate`, `RackZone::validate`):** Every Rack holds `1..=32` zones with unique nonzero `id: u16`; every zone holds `1..=24` modules with **exactly one mandatory `Out` module** (`RemoveRackModule` and `AddRackModule` both protect the singular `Out` module) and `0..=64` patch cables with validated `OutPort::Out` → accepted `InPort` compatibility, unique `(from_module, from_port, to_module, to_port)` edges, and finite attenuverter amounts within `cable_amount_range(target_kind, to_port)`.
4. **Automatic stale Rack reference pruning:** Whenever `UpdateRackPatch`, `RemoveRackZone`, `RemoveRackModule`, or `DisconnectCable` removes zones, modules, or cables (with `RemoveRackModule` also severing all attached cables), `mutate` immediately runs `prune_stale_instrument_refs` to prune dead `panel_pins`, `project.modulations`, and `part.parameter_links` targeting that instrument. At transaction commit, any edit touching instruments, channels, or Master also runs `Project::prune_macro_bindings()` before final `validate()`.
5. **Compile-time typed channel & effect edits (`Edit` mixer invariance):**
   - `Edit::CreateChannel { channel }` — creates a new channel (rejects duplicate channel or insert device IDs).
   - `Edit::UpdateChannelMeta { id, name, output, sends }` — updates channel name, bus output, or sends without touching `ChannelKind`, `strip`, or `inserts`.
   - `Edit::RemoveChannel { id }` — removes an existing channel (pruning stale macro bindings at transaction commit).
   - `Edit::AddEffect { channel, device, index }` — inserts a strictly new effect device onto any channel or Master.
   - `Edit::RemoveEffect { device }` — removes an existing effect device by ID from whichever channel/Master owns it.
   - `Edit::MoveEffect { device, target_channel, target_slot }` — reorders or moves an existing effect across channels/Master while preserving `id`, `name`, `bypassed`, `kind`, and Macro bindings.
   - `Edit::UpdateEffect { device, name, bypassed, kind }` — updates device metadata/bypass/patch while enforcing `std::mem::discriminant(&existing.kind) == std::mem::discriminant(&incoming)` when `kind` is `Some`.
   - Channel strip `gain` / `pan` updates route through `Edit::SetParam` (`ParamAddress::Channel`), and transaction pre-state checks (`check_channel_kind_invariance`, `check_effect_kind_invariance`) additionally prevent delete+recreate kind swaps within a single transaction.
6. **Placement, overlap & asset portability invariants:**
   - Placements on the same `Lane` may not overlap (`[start_step, start_step + length_steps)`).
   - Placements sharing the same `InstrumentInstance` across different lanes may not overlap; simultaneous parts require separate instrument instances.
   - Placement-local `Exception` steps must have unique relative offsets `< length_steps` and must be unconditional (`StepCondition::ALWAYS`).
   - Sample paths on every `SamplerModule` across every zone (walked via one SPOT iterator `RackPatch::sampler_modules()` across validation, asset discovery, `SampleBank` loading, and Save As / Bundle relativization) must never be absolute filesystem paths — only project-relative paths or catalog labels are accepted.

---

## 4. Single-Clock Timeline & Live Performance

### Pure Timeline Mapping (`crates/core/src/timeline.rs`)

One monotonic transport clock (`u64` 16th-note steps) drives both arrangement playback and live performance via `Timeline`, an ordered non-empty list of `Segment` transitions:

```rust
pub struct Segment {
    pub from: u64,                  // transport step where this mapping takes effect
    pub song: u64,                  // song step aligned with `from`
    pub region: Option<(u64, u64)>, // optional loop (start, length) in song steps
    pub end: EndRule,               // Loop | Stop | Continue
}
```

- **`song_step(transport, song_length)`:** Maps `raw = song + (transport - from)`. If `region = Some((start, len))` with `len > 0` and `raw >= start`, wraps into `start + (raw - start) % len`. Otherwise, once `raw >= song_length`: `EndRule::Loop` wraps `raw % song_length`, while `EndRule::Stop` and `EndRule::Continue` return `None`.
- **`song_step_unbounded(transport)`:** Evaluates without wrapping or stopping at `song_length`, used when `Keep` extends the arrangement past its current end.
- **`pass_end(transport, song_length)`:** Returns the exact transport step where the active loop region (or full-song loop pass) completes, so section jumps finish the current pass cleanly.
- **`stop_at(transport, song_length)`:** Computes the automatic stop step when `EndRule::Stop` is active with no loop region and no earlier scheduled remap.
- **`remap`, `cancel_after`, `prune_before`:** O(log n) binary search via `partition_point`; historical superseded segments are pruned to keep live memory bounded.

### Boundaries & Late Tolerance (`crates/core/src/material/set.rs`, `crates/core/src/live.rs`)

Live changes (`Change::{Song, Flags, Launch, Timeline, Release, ReleaseAll}`) land on quantized `Boundary` steps:

| `Boundary` Variant | Step Quantum | Notes |
| --- | --- | --- |
| `Now` | `None` (immediate) | Lands on `next_step` |
| `Beat` | `4` | Next quarter-note downbeat |
| `Bar` | `16` | Default launch boundary (`LiveTiming::default()`) |
| `Bars { count }` | `count * 16` | `count ∈ 1..=64` |
| `Cycle { pattern }` | `pattern_period(pattern)` | Next start of referenced pattern's full repeat period |

`LiveTiming` also specifies `edits: EditTiming` (`Now` default, or `Launch` so live song edits queue onto the launch boundary) and `late_tolerance_steps: u32` (`0..=4`, default `1`).

**Late-tolerance rule (`Performance::boundary`):** While stopped, every change lands immediately at `next_step`. While playing with quantum `q`, compute `upcoming = next_step.div_ceil(q) * q`. If `next_step > 0 && upcoming > next_step` (never snapping backward when already on an exact boundary), let `previous = ((next_step - 1) / q) * q` and `missed = next_step - previous`. If `missed <= late_tolerance_steps`, the change snaps back onto `previous`; otherwise it queues at `upcoming`.

### Live Overrides, Staging & Ring Compilation (`crates/core/src/material/live.rs`)

- **Lane overrides (`Override { lane, part: LanePart, anchor }`):** Launching a part overrides only that lane; every other lane continues playing the underlying arrangement segment. The newest launch claims its part's `InstrumentInstance` from any other active override. `PhaseMode::Song` follows the timeline's song step; `PhaseMode::Restart` counts from `step - anchor`.
- **Ordering invariants:** Queued `Change::Song` and `Change::Flags` never overtake one another (`at = at.max(last)`). A new `Launch`/`Release` replaces any pending launch/release on the same lane; a new `Timeline` remap replaces any pending remap.
- **Staging (`StagedChange`):** Collects `Launch`, `Release`, `Mute`, `Solo`, and `JumpSection` gestures without landing them until fired atomically on a single boundary (or discarded).
- **Sliding live ring:** `Performance::compile` compiles `[base, base + LIVE_RING_STEPS)` (`64` bars = `1024` steps, aligned to the bar of `next_step - 1`) into a `ScoreWindow` with `live_window = Some(LiveWindow { start, end, stop_at })`. When `next_step >= base + LIVE_RING_STEPS / 2`, the controller automatically recompiles and slides the ring forward.
- **Transient `Audition`:** Compiles onto the same live ring (either `solo: true` when stopped or layered over one instrument when playing), synthesizing `AUDITION_INSTRUMENT_ID` if the project has no instruments. Auditions are never authored, recorded in takes, or written by `Keep`.

---

## 5. Flat `ScoreWindow` Compilation & Macro/Modulation Precedence

### Flat Executable `ScoreWindow` (`crates/core/src/score.rs`)

Both offline song compilation (`Project::compile_with_flags`) and live sliding-ring compilation (`Project::compile_live_window_with_audition`) produce a single flat, zero-legacy `ScoreWindow`:

- **`instruments: Vec<ScoreInstrument>`** — dense `0..MAX_INSTRUMENTS` array where each `ScoreInstrument { id, name, rack: RackPatch, compiled_zones: Vec<CompiledZoneGraph>, channel }` carries pre-compiled Kahn topologically sorted `CompiledZoneGraph` programs (1:1 with `rack.zones`, including `z-1` feedback edge flags, `lifetime_envs`, `sampler_slots`, and SPOT `column_layout`).
- **`events: Vec<ScoreEvent>` & `step_ranges: Vec<(u32, u32)>`** — all scheduled hits flattened and sorted by `(step_slot, offset, inst_idx)` for O(1) zero-allocation lookup via `step_events(abs_step)`. Each `ScoreEvent` carries fractional `offset ∈ [0.0, 0.9999]`, resolved MIDI note, velocity, gate, inline `[ResolvedChordTone; MAX_CHORD_TONES]`, and `(lock_start, lock_len)` slice bounds into `locks: Vec<ParamLock>`.
- **`mod_lanes: Vec<ModLane>`** — sparse per-step normalized (`0.0..=1.0`) curves enforcing the **Value Rule**: unmodulated parameters have no lane, so base patch parameters remain directly authoritative.
- **`mixer: Mixer`** — validated channel strips, inserts, sends, ducks, `LiveFlags`, pre-compiled custom `Filter` trigger grids (`filter_env_triggers`), compiled external filter groove bank snapshot (`filter_grooves`), and `MixerAutomation` lanes.
- **`required_instrument_ids` & `provenance: Vec<EventOrigin>`** — exact set of instruments that actually fire events (for sample readiness checks) and origin records linking every emitted event back to `(song_step, lane, placement, part, pattern, instrument, pattern_time, local_exception)`.

### Fast Sound-Only Patch Path (`Project::patch_score_sound`)

When an edit alters only BPM, Rack patches/modules/cables, mixer strips/effects/sends, or static Macro values without changing note timing or pattern structure, `Project::patch_score_sound` skips pattern evaluation entirely and calls `ScoreWindow::with_sound_updates(bpm, instruments, mixer)`. It re-applies static Macros, recompiles every `ScoreInstrument`'s `compiled_zones` (so `CompiledCable.amount` reflects static Macro overrides), and re-synthesizes any `ChannelSend` entries required by existing automated `MixerParamTarget::Send` lanes so automated send throws survive fast sound updates intact.

### Exact Macro & Modulation Precedence

Compilation (`crates/core/src/material/compile.rs`) and live UI visual projection (`crates/app/src/view/live_params.rs`) share the exact same 3-tier deterministic precedence order (independent of `project.modulations` ordering):

1. **Tier 1 — Static Project Macros (`apply_static_macros`):** Iterates `project.macros` in ascending `index` order (`0..8`), skipping any macro driven by a `ModulationTarget::Macro`. Evaluates `binding.evaluate(mac.value) = (min + (max - min) * x).clamp(0.0, 1.0)` (explicitly supporting inverted `min > max` spans) and writes directly onto `ScoreInstrument.rack` (before `inst.compile_zones()`) or `Mixer`.
   - *Frame-based sampler guard:* Static Macro bindings targeting frame-based `ModuleParam` controls (`SampleStart`, `SampleEnd`, `LoopStart`, `LoopEnd`, matched via `InstrumentParam::is_frame_based()`) cannot resolve against `total_frames = 0` at compile time, so they emit a constant `AutomationMode::Hold` `ModLane` (unless already overridden by automation) which the audio engine resolves against the loaded sample's actual `sample.frames`.
2. **Tier 2 — Automated Macro Fan-Out (`ScoreBuilder::write_modulations`):** Processes all `ModulationTarget::Macro { index }` modulations sorted by ascending `index` (`0..8`). For each macro binding whose target is automatable (`UpdatePolicy != Structural` / `is_automatable()`), fans out the mapped curve into `mod_lanes` or `mixer.automations` (creating missing `ChannelSend` slots when targeting `ParamAddress::Send`).
3. **Tier 3 — Direct `Modulation`s:** Processes all non-Macro `ModulationTarget`s (`Instrument`, `Effect`, `ChannelGain`, `ChannelPan`, `Send`), unconditionally overriding any Macro fan-out targeting the same parameter slot via `upsert_mod_lane` / `upsert_mixer_automation`.

#### Multiplicative `ChannelGain` Rule

A `MacroBinding` targeting `ParamAddress::Channel { channel, param: ChannelParam::Gain }` drives the channel's fader trim directly (`MixerParamTarget::Channel { channel, param: ChannelParam::Gain }`, mapped through the `+6 dB` console power taper). By contrast, a direct `ModulationTarget::ChannelGain { channel }` compiles to the separate internal target `MixerParamTarget::ChannelGainScale { channel }`. In `MixEngine`, channel trim (`gain`), direct level shape (`scale`), and event-triggered sidechain ducking (`duck`) multiply independently (`gain * scale * duck`), so adjusting a fader trim never fights an authored volume envelope or sidechain pump.
