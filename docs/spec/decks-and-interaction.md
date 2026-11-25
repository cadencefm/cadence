# GPU Presentation, Direct Keymap & Deck Contract

This specification defines the strict purity boundary between `cadence-app` and `cadence-gui`, the GPU display-list and semantic theming pipeline, the unified keyboard grammar and global modifier boundaries, the six visual-axis deck alignments, all workspace decks and modal overlays, 24-bit audio recording, and the hardware MIDI contract.

> **Related specifications:** [Material & Timing](material-and-timing.md) · [DSP & Audio Engine](dsp-and-audio-engine.md) · [Protocols & Persistence](protocols.md) · [User Guide](../guide/README.md)

---

## 1. Pure Controller (`cadence-app`) & `HostIo` Boundary

`cadence-app` owns all session state, keyboard routing, undo history (`HISTORY_LIMIT = 128`), navigation trail (capped at `32`), drafts, commands, device/recording state, MIDI dispatch, and semantic view projection with **zero audio, MIDI, GPU, or direct OS filesystem dependencies**:

- **Minimal pure `Cargo.toml` contract (`crates/app/Cargo.toml`):** Depends strictly on `anyhow`, `cadence-core`, `cadence-formats` (feature-gated `rbs`), `serde`, and `serde_json` (plus `hound` and `tempfile` for dev tests). Direct `std::fs`, `std::env`, or `File::` calls inside `crates/app/src/` are forbidden and verified by integration tests.
- **`HostIo` abstraction (`crates/app/src/host_io.rs`):** All path queries, bounded reads, bounded directory scans, atomic writes, `UserConfig` persistence, `PresetCatalog` / `SampleLibrary` discovery, filter groove loading, portable bundle export, sample decoding, and take-journal sinks route through `Arc<dyn HostIo>`.
  - `CoreHostIo` delegates to `cadence-core::store` in production binaries.
  - `MemoryHostIo` provides a 100% pure in-memory virtual filesystem and `MemoryJournalSink` for deterministic, disk-free testing.
- **Pure `Outcome` intents:** Mutating operations (`App::apply`, `App::handle_key`, `App::handle_midi`) return an `Outcome` value describing required host side effects (`changed`, `samples_changed`, `project_boundary`, `transport`, `live_notes_on`/`off`, `rack_audition`, `audition_instrument`, `preview_sample`, `clear_mixer_meters`, `audio_device`, `audio_record`, `export`, `bounce`, `inspection`, `midi_feedback`, `midi_connect`, `file_dialog`, `quit`).
- **Single-point parameter & visual math:**
  - All `ParamAddress` (`Rack`, `Zone`, `Module`, `Cable`, `Channel`, `Send`, `Effect`) resolution, parsing, formatting, and nudging routes through `crates/app/src/param_ops.rs` composing `cadence_core::rack::*` and `cadence_core::dsp::*`.
  - All modulated parameter and Macro values across every deck resolve through `crates/app/src/view/live_params.rs`, mirroring the exact 3-tier compiler precedence (Static Macro → Automated Macro → Direct Modulation) at smooth sub-step `live_query_time`.

---

## 2. GPU Presentation Pipeline (`FrameView` → `DisplayList` → `wgpu`)

### Semantic View Projection (`crates/app/src/view/mod.rs`)

`App::frame_view(&self, snapshot: &AudioSnapshot, audio_status: &str) -> FrameView` produces an immutable, frontend-neutral snapshot of the frame:

```text
FrameView
├── chrome: ChromeView         (theme, title, dirty, transport, tempo/key, live timing,
│                               take_armed/take_recording/take_capped, audio_recording,
│                               overrides/pending/staged, MIDI/Macro badges, peak L/R,
│                               master_hot/clipped, mode_name, mode_kind, pending_key_count,
│                               hints, status, trail)
├── zoomed: bool
├── focused: Spot
├── decks: Vec<DeckSlot>       (ordered visible decks with DeckView payload)
├── jam_hud: Option<JamHudView>
├── lane_layer: Option<LaneLayerView>
└── overlay: Option<OverlayView>
```

### Ordered `DisplayList`, App Icon & WGSL Pipelines (`crates/gui/src/{display.rs,gpu/mod.rs,icon.rs}`)

`cadence-gui` transforms `(&FrameView, &Theme)` into a logical-pixel `DisplayList` (scaled seamlessly to physical pixels on upload), split into `base: DisplayLayer` (decks & chrome) and `overlay: DisplayLayer` (modal overlays strictly above base text):

- **Single ordered painter's pass:** Both `QuadInstance`s (rounded/gradient/bordered/hatched SDF rectangles) and `CurveVertex` geometry (anti-aliased waveforms, filter/EQ frequency curves, Bezier Signal Graph & Patchbay cables, filled ADSR/Hold-Decay/reverb envelopes) emit into one `Vec<DrawCmd>` (`DrawCmd::Quads(DrawBatch)` | `DrawCmd::Curves(DrawBatch)`), coalescing adjacent same-kind, same-clip batches while preserving exact back-to-front draw order.
- **Shared multi-column cable-graph SPOT (`crates/gui/src/decks/cable_graph.rs`):** One reusable renderer powers both the Project Signal Graph (`gG`, `decks/graph.rs`) and the Eurorack Instrument Deck `Patchbay` tab (`decks/instrument/`), sharing column layout, node cards, arc-length-dashed feedback Bezier curves (flagged with ASCII `"z-1"` for JetBrains Mono glyph compatibility), selected-cable halos/ringed jacks, collision-free midpoint pill badges, and upstream/downstream path illumination.
- **Balanced scissor clipping:** Every container clip pairs `push_clip(rect)` with `pop_clip()`; `begin_overlay()` resets the clip stack to the full viewport.
- **Embedded cross-platform app icon (`crates/gui/src/icon.rs`):** Embeds the canonical 256×256 PNG derived from `assets/icons/cadence.svg`, attaching `winit::window::Icon` (+ Windows taskbar icon) on window creation and setting macOS `NSApplication::setApplicationIconImage` on startup so unbundled terminal runs show the Cadence Dock/Cmd-Tab icon.
- **3-stage `wgpu` + `glyphon` renderer (`Rgba16Float` HDR target):**
  1. **Main + Selective Emissive MRT pass:** `sdf_quad.wgsl` + `scope.wgsl` write both main color and selective emissive channels (playhead, active notes, focused/active deck borders, scopes, clip indicators).
  2. **Half-resolution separable Gaussian bloom:** `bloom.wgsl` applies a 2-pass phosphor bloom scaled by `theme.glow.bloom_strength`.
  3. **Additive composite + hermetic HiDPI typography:** `glyphon` composites razor-sharp text via a hermetic `FontSystem` (`create_embedded_font_system`) backed **exclusively** by the 2 embedded `JetBrains Mono` faces—**Regular (weight 400)** and **Bold (weight 700)**—with zero OS font scanning or proportional fallback, **programming ligatures disabled** (`calt`/`liga`/`clig`/`dlig` via `no_ligatures_features()`, ensuring hints like `-/=`, `!=`, `->`, and `</>` always shape as literal separate monospace glyphs), and a generation-swept `TextBufferCache` (`TEXT_CACHE_EVICT_AGE = 120`, `TEXT_CACHE_TRIM_INTERVAL = 60`, `TEXT_CACHE_SOFT_CAP = 2048`).

### Semantic Theme System (`crates/gui/src/{theme.rs,chrome.rs}`)

**Zero hardcoded hex/RGBA colors are permitted outside `crates/gui/src/theme.rs`.** Every deck, chrome bar, visualizer, and overlay queries `&Theme`:

- Token groups: `SurfaceTokens`, `BorderTokens`, `TextTokens`, `AccentTokens` (`focus`, `edit`, `jam`, `routing`, `playhead`, `warning_key`, `modulation`, `alert`, `override_bar`, `audition`), `MeterTokens` (`low`, `mid`, `high`, `hot`, `clip`, `pre_fader`, `zero_db`, `gr`, `track_bg`, `hold_tick`), `tracks: [Color; 8]`, `RadiusTokens`, `GlowTokens`, and `FontTokens`.
- **Deck framing (`draw_deck_frame`):** Focused decks display the crisp cyan focus border (`theme.accent.focus`); `Mode::Insert` signals step-entry mode in amber (`theme.accent.edit`); `Mode::Jam` displays the glowing violet border (`theme.accent.jam`) and floating Jam HUD.
- **3 Built-in Themes (`ThemeId`):** `Cadence Dark` (default slate + phosphor), `Midnight Neon` (deep obsidian + high-bloom stage contrast), and `Studio Light` (daylight high-contrast slate).

---

## 3. Direct Keybinding Grammar & Input Pipeline (`crates/app/src/input/`)

Cadence is strictly keyboard-driven, consistent, and direct. Parameter sculpting, note shaping, placement resizing, and item reordering happen directly in place without toggling a general edit mode.

### 3A. Mode Inventory

Only genuine input/selection modes exist:

| Mode | Trigger | Role |
| --- | --- | --- |
| **`Normal`** | Default | Direct workspace: pure spatial navigation (`hjkl`/`HJKL`), direct 2D manipulation & reorder (`Alt+hjkl`/`HJKL`), one-hand scalar adjust (`,/.` `</>`), and canonical action verbs. |
| **`Insert`** | `i` on editable Pattern Notes | Piano step entry (`zsxdcvgbhnjm,` + `Shift` accents, `1–8` scale degrees, `<`/`>` octave, `.` rest, `Left`/`Right` step cursor). |
| **`Jam`** | `gj` / `Ctrl+Shift+j` | Modal floating performance keyboard (top-row lanes, number-row sections, bottom-row chromatic piano, `Tab`/`Shift-Tab` bank). |
| **`ValueInput`** | `Enter` on any scalar control | Exact number/unit/text entry with instant validation and formatted preview. |
| **`MiniEditor`** | `Enter` on a `Mini` pattern | Instant-validated Mini-Notation code editor with Cycle 1 note preview. |
| **Overlays & Browsers** | `Ctrl+Shift+<key>`, `:`, `/`, `?`, `n`, `P`, `B`, `g`/`t`/`z`/`v` | Fuzzy Action Palette, File Hub, Preset/Groove/Sample Browsers, Song Properties, Settings, Workflows, Pickers, Help, Inspect, Confirm, Prefix HUDs. |

### 3B. Deterministic Routing Order (`crates/app/src/input/route.rs`)

Every keyboard event (`KeyEvent`) passes through `App::handle_key` in one strict priority order:

```text
KeyEvent
  1. Normalize shifted char; on Press break undo coalescing & MIDI CC rec gesture
  2. KeyEventKind::Release -> release matching held computer-keyboard notes (live_notes_off)
  3. KeyCode::Home         -> clear count, Action::Stop (non-repeat)
  4. Active lane_layer HUD -> intercept part digit (1..9) or toggle lane across ALL modes;
                              any other key closes the layer and falls through
  5. Ctrl+Shift+j          -> toggles Mode::Jam ON and OFF (even inside Jam)
  6. Universal Alt Layer   -> Alt+Enter (stage/fire), Alt+1..0 (sections), Alt+q..p (lane layer),
                              Alt+m/M (mute/unmute all), Alt+s/S (solo/clear all),
                              Alt+x/X (release/release all), Alt+c/C (keep/keep all) — active everywhere
                              (Alt+hjkl / Alt+HJKL / Alt+Backspace flow to Tier 3 direct manipulation;
                               Alt+b/f/d stay free in text inputs)
  7. Ctrl+c                -> in Normal: Mode::Confirm(Quit); in any other mode: cancel to Normal
                              (restoring preview browsers cleanly if active)
  8. Modal forms & popups  -> Create, Workflow, Browser, PresetBrowser, GrooveBrowser, SongProperties,
                              MiniEditor, Palette, FileHub, Pick, Authoring, ValueInput,
                              Interpolation, ArrangeViews, ArrangementInspect
  9. Ctrl / Ctrl+Shift     -> Ctrl+<deck> (all 10 docked decks), Ctrl+Shift+<modal> (all modals),
                              Ctrl+s Save, Ctrl+z/Ctrl+Shift+z/Ctrl+r Undo/Redo, Ctrl+w Window,
                              Ctrl+=/−/0 zoom, Ctrl+u clear input
 10. Jam & Stage modes     -> Mode::Jam (Alt layer locked on + z–, 13-key piano HUD + </> octave + Tab/Shift-Tab bank)
                              Mode::Stage (Enter/S fire staged batch, x discard, Esc keep staging)
 11. Esc unwinding         -> cancel MIDI Learn -> clear count -> discard staging -> clear Take marks
                              -> clear sub-search filter -> exit Insert -> cancel preview browser
                              -> resume Jam -> unwind trail (`back`)
 12. Prefix / overlay /    -> Search (/), Transport (t), Window (z), Go (g), Help (?),
     Insert / Normal          Settings (g,), Inspect, Assets, Tutorial, Confirm, Insert (i), Normal
```

### 3C. The Four Directional Tiers

Every deck obeys the same four directional tiers with zero axis flips:

| Tier | Keys | Invariant | Undo / Repeat |
| --- | --- | --- | --- |
| **1. Navigate** | **`h j k l`** *(+ Arrows)* | Pure spatial cursor movement matching the visual screen axes. **Never mutates.** Bare `h`/`l` is strictly a no-op on 1-axis vertical inspector decks. | Key-repeat enabled; no undo |
| **2. Coarse Nav / Tabs** | **`H J K L`** *(Shift + nav)* | Coarse spatial jump or deck-level tabs (Rack tabs, Devices `Audio \| MIDI` tabs, Modal tabs, Arrange sections, Pattern bars `±16`, Mixer strip pages). **Never mutates.** | Key-repeat enabled; no undo |
| **3. Direct 2D Manipulate & Reorder** | **`Alt+h j k l`** *(fine)*<br>**`Alt+H J K L`** *(coarse)*<br>*(+ `Alt+Arrows` / `Alt+Shift+Arrows`)* | Directly manipulate, move, nudge, or reorder the focused item along its visual axis (turn knob, push fader, shift pitch/gate, move placement bar/lane, slide breakpoint, reorder inserts/zones/pins/bindings). | Key-repeat enabled with coalesced single-gesture undo (`u` / `Ctrl+z`) |
| **4. One-Hand Scalar Adjust** | **`, / .`** *(fine `−` / `+`)*<br>**`< / >`** *(coarse `−` / `+`)* | Decrease / increase the focused scalar value right below `k`/`l` (zero modifier for fine). Works on every knob, fader, pan, send, duck, macro dial, cable amount, BPM, curve value, note velocity, and placement length. | Key-repeat enabled with coalesced single-gesture undo (`u` / `Ctrl+z`) |

### 3D. Global Modifier & Prefix Boundaries

No individual deck may shadow any global prefix, performance key, or `Ctrl` shortcut in this section. In native `winit`, `physical_fallback` keeps `Ctrl+i` and `Ctrl+m` distinct from `Tab` and `Enter`, and on macOS `Cmd+=/−/0` maps onto the `Ctrl` zoom chords.

#### All 10 Docked Decks (`Ctrl+<letter>`, zero Shift)

| Deck | `g` Prefix | Direct `Ctrl` |
| --- | --- | --- |
| **Arrange** | `ga` | **`Ctrl+a`** |
| **Pattern** | `gp` | **`Ctrl+p`** |
| **Sample / Waveform** | `gw` *(also `W` from Rack)* | — |
| **Bank** | `gb` | **`Ctrl+b`** |
| **Mixer** | `gm` | **`Ctrl+m`** |
| **Instrument Rack** | `gi` | **`Ctrl+i`** |
| **Macros** | `gk` | **`Ctrl+k`** |
| **Take** | `gt` | **`Ctrl+t`** |
| **Signal Graph** | `gG` | **`Ctrl+g`** |
| **Devices (`Audio \| MIDI`)** | `gd` *(preserves active tab; `:audio` / `:midi` jump directly)* | **`Ctrl+d`** |
| **Output Monitor** *(toggle)* | `go` | **`Ctrl+o`** |

#### All Modals & Overlays (`Ctrl+Shift+<key>`, always Shift)

| Modal / Overlay | Direct / `g` Prefix | `Ctrl+Shift` Chord |
| --- | --- | --- |
| **Action Palette** | `:` | **`Ctrl+Shift+p`** *(works in Normal & Jam)* |
| **Jam** *(modal floating overlay)* | `gj` | **`Ctrl+Shift+j`** *(toggles Jam ON and OFF even inside Jam)* |
| **File Hub** | `gf` | **`Ctrl+Shift+f`** |
| **File Hub: Quick Open** | `gf` (tab 1) | **`Ctrl+Shift+o`** |
| **File Hub: Browse Files** | `gf` (tab 2) | `:` → *Browse Files* |
| **File Hub: Save As / Bundle** | `gf` (tab 3) | **`Ctrl+Shift+s`** |
| **File Hub: Export Audio** | `gf` (tab 5) | **`Ctrl+Shift+e`** |
| **Song Properties / Info** | `gP` | **`Ctrl+Shift+i`** |
| **Settings** | `g,` | **`Ctrl+Shift+,`** *(plus `Ctrl+,` alias)* |
| **Sample Browser** | `B` / `gS` | **`Ctrl+Shift+b`** |
| **Create New Object Menu** | `n` | **`Ctrl+Shift+n`** |
| **Help** | `?` | **`Ctrl+Shift+/`** *(i.e. `Ctrl+?`)* |

#### Core CUA & Window Shortcuts

| Chord | Scope | Behavior |
| --- | --- | --- |
| `Ctrl+s` | Normal *(also Settings / Devices / Take)* | Save immediately (opens File Hub Save tab only if untitled) |
| `Ctrl+z` / `Ctrl+Shift+z` *(+ `Ctrl+r`)* | Normal | Undo / Redo *(alongside `u` / `U`)* |
| `Ctrl+c` | Any mode | Cancel overlay/input back to Normal (restoring preview browsers); in Normal opens Quit confirm |
| `Ctrl+w` | Normal | Window / Deck prefix (`Mode::Window`, alias of `z`) |
| `Ctrl+=` / `Ctrl+-` / `Ctrl+0` | Normal / Insert | Horizontal zoom in / out / reset *(when Arrange, Pattern, or Sample focused)* |
| `Ctrl+u` | Text inputs / Search | Clear active input buffer |

#### `Alt` — Top-Row Live Performance + Home-Row Direct Manipulation

| Chord | Scope | Behavior |
| --- | --- | --- |
| `Alt+1..0` | Any mode (non-repeat) | Section jump 1–10 in current bank (or stage jump) |
| `Alt+q..p` | Any mode (non-repeat) | Open lane layer for bank lanes 1–10 |
| `Alt+m` / `Alt+M` | Any mode (non-repeat) | Mute focused lane (or Mixer channel) / Unmute all lanes |
| `Alt+s` / `Alt+S` | Any mode (non-repeat) | Solo focused lane (or non-Master Mixer channel) / Clear all solos |
| `Alt+x` / `Alt+X` | Any mode (non-repeat) | Release focused lane / Release all lanes |
| `Alt+c` / `Alt+C` | Any mode (non-repeat) | Commit / Keep focused override (or Take selection on Take deck) / Keep all overrides |
| `Alt+Enter` | Any mode (non-repeat) | Toggle / fire Staging (`Mode::Stage`) |
| `Alt+Backspace` | Non-text decks (non-repeat) | Reset focused parameter / fader / pan / send / macro / cable / setting to default |
| `Alt+h/j/k/l` & `Alt+H/J/K/L` | Non-text decks (repeat OK) | Tier 3 Direct 2D Manipulation & Reorder *(plus `Alt+Arrows` / `Alt+Shift+Arrows`)* |

Text-entry fields keep `Alt+b`, `Alt+f`, `Alt+d`, and `Alt+[` free.

#### Global Normal Keys & Prefixes

| Key | Behavior |
| --- | --- |
| `g` | Go prefix: `ga` Arrange · `gp` Pattern · `gw` Sample/Wave · `gb` Bank · `gm` Mixer · `gi` Instrument · `gk` Macros · `gG` Signal Graph · `gd` Devices (`Audio \| MIDI`) · `gt` Take · `go` Output · `gj` Jam · `gf` File Hub · `gP` Song Properties · `g,` Settings · `gS` Sample Browser · `gs` Follow Source / Pattern Def · `gg` Start · `ge` End |
| `t` | Transport prefix: `tf` Follow · `tg` Cue bar · `tl` Loop section · `tx` Clear loop · `tX` Cancel pending · `ts` Stop · `te` End rule · `tr` Take arm · `tR` Restart loop · **`ta` Toggle 24-bit audio record · `tA` Record Audio dialog · `ti` Cycle input monitor (`Off\|Auto\|On`)** · `tc` Cue · `tp` Cue & play · `tb` Bounce dialog · `tq` Launch quantize · `tQ` Cycle boundary · `tE` Edit timing · `tC` Clear all clipped meters |
| `z` | Window prefix: `zw`/`Tab` cycle · `zz` zoom deck · `zp` pin · `zx` close · `zo` only · `zh/j/k/l` directional focus · `z=/z-/z0` timeline zoom · `z1..4` layout presets |
| `:` / `/` / `?` | Action Palette / Bank Material search / Help |
| `Space` / `Home` | Play / Pause · Stop & rewind |
| `u` / `U` | Pop staged change or Undo / Redo |
| `S` / `M` / `R` | Stage toggle/fire · MIDI Learn · MIDI Record |
| `n` / `B` / `\` / `'` | Global Create menu / Sample Browser / Toggle song loop / Arm From-Here fork |
| `;` | Repeat last single-step pattern edit |
| `Tab` / `BackTab` | Cycle focus across open decks forward / backward |
| `0` / `$` / `G` / `End` | Start / end of the deck's active focused list or timeline *(never leaks to hidden cursors)* |
| `PgUp` / `PgDn` | Page along the deck's primary scrollable axis |
| `Esc` | Cancel MIDI Learn → clear count → discard staging → clear Take marks → `back()` (stop audition / deselect cable / unzoom / pop trail) |
| `Backspace` | Cancel pending timeline jump else `back()` |

### 3E. Canonical Action Verbs

| Key | Intent | Scope |
| --- | --- | --- |
| **`Enter`** | Primary action: open / drill-down on links & containers · launch on Bank Parts · toggle on booleans & Rhythm · open exact `ValueInput` on scalars | Every deck |
| **`a`** | Contextual Add (item into current deck/list) | Rack Panel/Patchbay (Add Module), Zones (Add Zone picker), Macros (Add Binding), Bank Parts (Add Part to Lane), `Detail::Part` (Add P-lock Link), Settings (Add Dir), Devices/MIDI (Add Mapping), Mixer/Channel (Add Insert/Send/Duck picker) |
| **`A`** | Universal Audition / Preview | Pattern, Part, Placement, Bank Parts, Rack (all 3 tabs), PresetBrowser, SampleBrowser, Pickers |
| **`x`** | Remove / unlink / release focused child item | Arrange (release override else remove placement), Notes (delete voice), Values (delete point), Panel (remove module), Patchbay (disconnect cable else remove module), Zones (delete zone), Mixer/Channel (remove Insert/Send/Duck), Macros (unbind binding), Graph (unlink cable), Bank Parts (release lane), MIDI (delete user mapping), Settings (remove dir), Take (clear marks) |
| **`X`** | Delete whole entity / clear container (`Mode::Confirm` for entities) | Channel, Instrument, Effect, Pattern, Part, Lane, Section, Notes (clear step), Macro slot, Graph (unlink all), Take (discard), MIDI (clear all user mappings) |
| **`r`** | Rename / edit metadata | Instrument, Module, Quick Pin, Zone, Channel, Effect, Pattern (incl. literal length/mode), Part, Lane, Section, Duck, Macro slot, Devices (`r` rescans audio/MIDI ports) |
| **`d`** | Derive / duplicate (non-destructive) | Pattern (`Draft::derive`), Zones (duplicate zone), Arrange (duplicate placement), Mixer/Channel/Graph (Add Duck) |
| **`e`** | Edit / inspect Modulation & Macro links | Any focused parameter on Rack (`Panel`/`Zones`), `Detail::Channel`, `Detail::Effect`, `Mixer` slots, `Macros` header (bind to `M1–M8`, attach/edit/unlink Modulation, jump to source), plus Signal Graph cables & FileHub inline path edit |
| **`m`** | Mute / Bypass / Mode toggle | Lane Mute (Arrange, Bank), Channel Mute (Mixer, Channel), Effect Bypass (`Detail::Effect`; `b` on Mixer Insert slot when channel Mute owns `m`), Values `Hold ↔ Ramp` toggle, Notes step mute/rest (preserves pitch/chord) |
| **`s`** | Solo | Lane Solo (Arrange, Bank), Channel Solo (Mixer, Channel) |
| **`y` / `p` / `P`** | Clipboard *(Arrange Placements & Pattern only)* · Rack Pin (`p` on Rack) · Preset / Groove Browser (`P` on Rack / Filter) | Save Preset/Groove lives inside the `P` browser and `gf` Save tab; never leaks Paste errors onto non-clipboard decks |
| **`v`** | View / Visual / Move FX | Arrange views prefix (`vp`/`vo`/`vr`), Bank `Parts ↔ Material`, Take visual range, Move Effect (`Detail::Channel`/`Effect`/Mixer Insert) |
| **`f`** | Fork *(or Add Effect on Mixer/Channel)* | Pattern fork, Placement fork, Filter groove fork |
| **`o`** | Open detail / local occurrence / route output / stack chord | Mixer → `Detail::Channel`; Placement/Inspect → local occurrence; Rack/Graph → route output; Pattern Notes → stack chord voice |
| **`i`** | Step Insert *(Pattern Notes only)* | Enters piano-keyboard `Mode::Insert` |
| **`C`** | Clear clipped meter *(Mixer / Channel / Output)* · Cable exact amount *(Patchbay)* | Pairs with `tC` (clear all clips) |

---

## 4. Deck Layout Engine, Visual Alignments & Deck-by-Deck Contract

### 4A. Pure Layout Model (`crates/app/src/layout.rs`, `crates/app/src/decks.rs`)

`Layout` stores `pinned: Vec<Deck>`, `working: Option<Deck>`, `focus: Spot::{Pinned(usize), Working}`, and `zoomed: bool`. Every workspace and focus query (`workspace()`, `focus()`, `focused_canvas()`, `focused_deck_kind()`) is derived purely from `self.layout` with zero shadow state.

- **Deck shapes (`Shape`):** `Wide` decks (`Arrange`, `Output`, `Take`, `Graph`, `Midi` [Devices], `Macros`) stack horizontally; `Tall` decks (`Bank`, `Mixer`) dock along the right column; `Block` decks (`Pattern` and object decks) adapt to available slots.
- **Working-deck selection tracking:** While another deck holds focus, an unpinned working deck automatically retargets to follow the current selection; pinning (`zp`) locks a deck's target indefinitely.
- **Navigation trail:** Navigating via `g<key>` / `Ctrl+<key>` or `Enter` records a `Navigation` snapshot on `back` (up to `32` entries) so `Esc` unwinds cleanly without affecting musical undo.

### 4B. Six Visual-Axis Deck Layout Alignments

Six deck layout alignments guarantee every `hjkl` and `Alt+hjkl` binding matches the screen 1:1:

1. **Uniform control lists (`crates/gui/src/decks/object.rs`, `crates/app/src/controls.rs`):** `j`/`k` walks rows and `w`/`b` jumps groups on every control list regardless of 1-column or 2-column visual flow; bare `h`/`l` never mutates.
2. **Rack `Zones` horizontal Zone strip (`crates/gui/src/decks/instrument/zones.rs`):** Renders a scrollable horizontal Zone pill strip at the top of `Zones` (matching `Panel`'s module strip) so `h`/`l` selects zones left-to-right, `j`/`k` walks Zone/Voice/Rack controls below, and `Alt+j`/`k` reorders zones.
3. **Bank `Parts` row-oriented layout (`crates/gui/src/decks/bank.rs`):** Renders Lanes as rows (`j`/`k`) and each Lane's Parts as horizontal cards/pills left-to-right (`h`/`l`) in both unzoomed and zoomed (`zz`) views, matching Arrange with zero zoomed transpose.
4. **Mixer smooth console bank, pinned Master & fixed-height detail stack (`crates/gui/src/decks/mixer.rs`):** Renders uniform-width channel strips (`88–112 px`) in a smoothly sliding horizontal bank (`h`/`l` slides 1 strip at a time with 1-strip lookahead scrolloff when `>= 4` bank strips fit; `H`/`L` and `PgUp`/`PgDn` jump a bank width) with `Master` permanently pinned on the right behind a console divider and `<`/`>` overflow rails showing hidden strip counts. Below the strips, the selected channel's `FX`, `SND`, `OUT`, and `DUCK` slots render in a fixed-height scrollable vertical stack sized strictly from deck height (`2–6` rows) so switching channels never shifts faders vertically (omitting unfocusable `OUT → Master` on `Return` and `Master` channels).
5. **Macros 1D horizontal dial strip + scrollable Bindings (`crates/gui/src/decks/macros.rs`):** Keeps `M1–M8` in a single horizontal strip across all pane widths and scrolls the vertical Bindings table around the focused row.
6. **Devices (`Audio | MIDI`) navigable inspector rows (`crates/gui/src/decks/midi.rs`):** Renders `Audio` tab rows (`AUDIO INTERFACE`, `AUDIO INPUT & RECORDING`, `ENGINE & RECOVERY` — 11 typed `AudioDeckRow`s) and `MIDI` tab rows (`DEVICE & PROFILE` — 5 typed `MidiDeckRow`s: `IN`, `OUT`, `Profile`, `Pads`, `Pickup` — directly above `MAPPINGS`) as standard vertical inspector rows with `H`/`L` switching tabs, keeping global `t`, `i`, and `Backspace` unshadowed.

### 4C. All 12 Primary Deck Surfaces

`DeckKind` defines 15 targetable deck kinds projected into 12 visual surfaces (`Placement`, `Part`, and `Channel` share the unified `ObjectDeckView`):

| # | Deck | Jump | `DeckView` | Role & Key Capabilities |
| --- | --- | --- | --- | --- |
| 1 | **Arrange** | `ga` / `Ctrl+a` | `Arrange` | Placements (`vp`), Evaluated Overview (`vo`), Evaluated Piano Roll (`vr`), live override rows, pending/staged badges, sections (`H`/`L`, `w`/`b`), Event Provenance (`Enter`) |
| 2 | **Bank** | `gb` / `Ctrl+b` | `Bank` | Row-oriented Launchpad (`Parts`) & searchable `Material` pattern/section library (`v` toggles) |
| 3 | **Pattern** | `gp` / `Ctrl+p` | `Pattern` | 2D polyphonic voice-sorted piano roll (`j`/`k` voice, `o` stack, `Alt+j/k` pitch, `Alt+h/l` gate, `,/.` velocity, `i` Step Insert), fractional sub-step visualization, rhythm matrix, `Values` Hold/Ramp curve editor, `MiniEditor` (`Enter`), derived source inspector (`gs`/`Enter`) |
| 4 | **Sample** | `gw` / Rack `W` | `Sample` | Dedicated zoomable Waveform & Slicer deck (`1.0×..128×`, overview minimap, time ruler, `[`/`]` crop & `{`/`}` loop overlays, zero-crossing snap `v`, manual `s` / transient `d` / grid `D` slicing, active-slice audition `A`, one-step `K` Slice→Rack Kit + Pattern, lossless `W` RIFF `inst`/`smpl`/`cue `/`labl` WAV write-back) |
| 5 | **Mixer** | `gm` / `Ctrl+m` | `Mixer` | 32-bit float console (`Audio`, `Group`, `Return`, `Master`), `-60..+6 dB` faders with `0 dB` hairline, pre-fader ticks, `HOT`/`!CLIP`, `CMP`/`LIM` GR bars, vertical detail slots (`Fader`/`Pan`/`Inserts`/`Sends`/`Output`/`Duck`), `o` Channel deck |
| 6 | **Macros** | `gk` / `Ctrl+k` | `Macros` | All 8 Project Macros (`M1..M8`) in 1 horizontal strip + scrollable Bindings, automated sweeps, native-unit spans, inverted badges, MIDI CC badges |
| 7 | **Devices (`Audio \| MIDI`)** | `gd` / `Ctrl+d` | `Midi` | Two-tab hardware deck (`H`/`L` switches **`Audio`** ↔ **`MIDI`**): `Audio` configures output/input device, sample rate, buffer size, cue out, input mode (`Stereo \| Left \| Right`), input monitor (`Off \| Auto \| On`), latency trim (`-250..+250 ms`), audio panic/restart/rescan + real-time input meters & health; `MIDI` configures ports, profile, pads, soft pickup, Smart Macro & RGB pad previews, mappings, and event log |
| 8 | **Graph** | `gG` / `Ctrl+g` | `Graph` | Interactive 5/6-column Signal Graph patchbay (`Sources` → `Derived` → `Parts` → [`Macros`] → `Instruments` [`RACK`, `{z}z · {m}m` badge] → `Mixer / FX`), illuminated cubic-Bezier cables via shared `cable_graph.rs`, undo-safe wiring (`o`/`a`/`s`/`m`/`d`/`e`/`x`/`X`) |
| 9 | **Take** | `gt` / `Ctrl+t` | `Take` | Explicitly armed performance timeline (`tr` toggle, default OFF; `Auto-Record Takes` setting; Chrome `TAKE ARM` / `● REC TAKE` / `● TAKE CAPPED` badges), bar summaries, event markers, `v` Visual range selection, `Enter`/`Alt+c` keep with auto-forking, `Ctrl+s` save, `X` confirm-guarded discard |
| 10 | **Instrument** | `gi` / `Ctrl+i` | `Instrument` | Unified 11-module Eurorack Modular Rack (`Panel` \| `Patchbay` \| `Zones` via `H`/`L`): `Panel` (Quick `panel_pins` + module strip + 12 pre-sampled DSP `ModuleVisualView` visualizers), `Patchbay` (4-column `cable_graph.rs` with bipolar attenuverters & dashed `z-1` feedback), `Zones` (horizontal zone strip + 32-zone key/vel/choke map); universal `P` Preset Browser, `B` Sample Browser, `W` Sample Deck, `A` audition, `r` rename |
| 11 | **Effect / Object** | `Enter` | `Effect` / `Object` | Dedicated visualizers for all 8 `EffectKind`s (EQ response, Filter + `FilterEnv` step bars, Comp/Limiter transfer + GR, Drive 3-mode curve, Chorus LFO, Delay taps, Reverb RT60) + `ObjectDeckView` for `Placement`, `Part`, and `Channel` |
| 12 | **Output** | `go` / `Ctrl+o` | `Output` | Non-focusable stereo oscilloscope, L/R 32-bit float meters, `0 dB` hairline, Master `HOT`/`!CLIP` latch, and Master `LIM`/`CMP` GR badge |

### 4D. Deck-by-Deck Binding Contract

- **Arrange (`ga` / `Ctrl+a` — Placements, Overview, Roll):**
  - *Nav:* `h`/`l` bar `±1` (Placements) or step `±1` (Overview/Roll) · `j`/`k` lane `±1` · `H`/`L` prev/next section · `w`/`b` section (Placements) or note event (Overview/Roll) · `0`/`$`/`G` start/end · `PgUp`/`PgDn` lanes `±6` or visible bar page · `=`/`-` quick zoom.
  - *Direct Manipulation (Placements):* `Alt+h`/`l` move start `±4` steps (`Alt+H`/`L` `±16`) · `Alt+j`/`k` move placement across lanes · `,`/`.` shrink/extend length `±4` steps (`<`/`>` `±16`).
  - *Verbs:* `Enter` open `Detail::Placement` (or Place workflow on empty bar; `ArrangementInspect` on Overview/Roll) · `v` views (`vp`/`vo`/`vr`) · `x` release override else delete placement · `X` delete section (or lane) · `r` edit section (or rename lane) · `d` duplicate placement · `m`/`s` lane mute/solo · `[`/`]` loop start/end · `y`/`p`/`P` yank/paste.
- **Pattern (`gp` / `Ctrl+p` — Notes, Rhythm, Values, Derived, Mini):**
  - *Shared:* `h`/`l` step `±1` · `H`/`L` & `PgUp`/`PgDn` bar `±16` · `0`/`$`/`G` first/last step · `=`/`-` quick zoom · `A` audition · `d` derive · `r` edit metadata/length/rename · `f` fork · `X` delete pattern (confirm) · `y`/`p`/`P` yank/paste · `;` repeat last edit.
  - *Notes:* `j`/`k` chord voice ↓/↑ · `w`/`b` next/prev active note · `Alt+j`/`k` pitch `±1` deg (`Alt+J`/`K` `±1` oct) · `Alt+h`/`l` gate `±0.25` steps (`Alt+H`/`L` `±1.0`) · `,`/`.` velocity `±5%` (`<`/`>` `±15%`) · `[`/`]` (and `#`) flat/sharp accidental · `c`/`C` cycle `StepCondition` · `o` stack chord voice · `m` toggle step rest/active · `x` delete voice · `X` clear step · `i` Step Insert · `Backspace` (in Placement scope) re-inherit step from definition.
  - *Values:* `w`/`b` next/prev breakpoint · `Alt+j`/`k` & `,`/`.` value fine (`±0.01` / `±1` deg) · `Alt+J`/`K` & `<`/`>` coarse (`±0.10` / `±7` deg) · `Alt+h`/`l` slide breakpoint `±1` step (`Alt+H`/`L` `±4`) · `Enter` exact `ValueInput` · `m` toggle `Hold ↔ Ramp` · `x` delete breakpoint · `e` inspect/jump downstream Modulation/Link consumers.
  - *Rhythm:* `w`/`b` next/prev hit · `Enter` / `x` toggle hit directly.
  - *Derived / Mini / Generators:* `j`/`k` source chip (Derived) · `Enter` / `gs` open selected source (Derived) or `Mode::MiniEditor` (Mini) · `F` freeze to literal pattern.
- **Sample / Waveform (`gw` / Rack `W`):**
  - *Nav (Tier 1, non-mutating):* `h`/`l` fine scrub (`1%` visible) · `H`/`L` coarse scrub (`10%` visible) · `w`/`b` jump next/prev marker (`sample_start`, slices, `sample_end`) · `0`/`$`/`G` file start/end · `PgUp`/`PgDn` half-window page · `j`/`k` bottom toolbar control (`Cursor`, `Sensitivity`, `MinGap`, `GridDivisions`, `LoopXfade`, `SnapZero`) · `=`/`-`/`z0` (`Ctrl+=`/`-`/`0`) horizontal zoom `1.0×..128×`.
  - *Direct Manipulation (Tiers 3 & 4):* `Alt+h`/`l` & `,`/`.` fine (`Alt+H`/`L` & `<`/`>` coarse) slide active slice marker (coalesced undo) when on `Cursor`, or adjust focused toolbar control (`Sensitivity`, `MinGap`, `GridDivisions`, `LoopXfade`, `SnapZero`) · `Alt+Backspace` reset crop/loop or toolbar default · `Enter` exact `ValueInput` (or rename active slice / browse if unloaded).
  - *Verbs:* `[`/`]` crop start/end · `{`/`}` loop start/end (auto-enables Forward) · `m` cycle `SampleLoopMode` · `v` toggle zero-crossing snap · `s` split slice at cursor · `x`/`X` delete slice / clear all · `d`/`D` transient auto-slice / equal-grid slice · `r` rename active slice label · `A` audition active slice region · `B` Sample Browser · `K` Slice→Rack Kit picker · `W` lossless RIFF WAV metadata write-back.
  - *Ex Commands:* `:wave [INSTRUMENT]`, `:slice auto|grid|add|rename|del|clear|kit ...`, `:sample write-meta`.
- **Modular Rack (`gi` / `Ctrl+i` — `Panel`, `Patchbay`, `Zones`):**
  - *Shared:* `H`/`L` switch tab (`Panel | Patchbay | Zones`, or `:page panel|patchbay|zones`) · `A` audition · `P` Preset Browser *(incl. Save Preset & `R` reload)* · `B` Sample Browser (auto-inserts `Sampler -> Out.In` via `ensure_zone_sampler_module` when absent and applies RIFF `smpl`/`inst` metadata) · `o` route output channel · `r` rename instrument (or focused module/pin/zone) · `X` delete instrument (confirm).
  - *`Panel`:* `h`/`l` Module pill (`Quick` + every module) · `j`/`k` vertical control (`w`/`b` group) · `Alt+h`/`l` & `,`/`.` fine (`Alt+H`/`L` & `<`/`>` coarse) · `Alt+Backspace` reset · `Enter` exact `ValueInput` (or open Sampler/Link) · `Alt+j`/`k` reorder pin (`Quick` pill) · `a` add module · `x` remove module (never `Out`) · `p` pin/unpin · `e` edit/bind Macro & Modulation links. Displays all 12 pre-sampled GPU `ModuleVisualView` visualizers (`PinnedOverview`, `Vco`, `Sampler`, `Noise`, `Env`, `Lfo`, `Filter`, `Vca`, `Drive`, `LoFi`, `Resonator`, `Out`).
  - *`Patchbay`:* 4-column `cable_graph.rs` (`SOURCES` → `MODULATORS` → `PROCESSORS` → `VCA / OUT`) · `h`/`l` column · `j`/`k` module card · `w`/`b` (`[`/`]`) cycle attached cables · `Alt+h`/`l` & `,`/`.` fine cable amount (`Alt+H`/`L` & `<`/`>` coarse) · `Alt+Backspace` reset · `Enter` jump to module `Panel` or cable exact `ValueInput` · `a` add module · `c` connect cable · `C` select/edit cable amount · `x` disconnect cable (else remove module) · `p` pin cable · `e` edit/bind Macro & Modulation links.
  - *`Zones`:* `h`/`l` horizontal Zone pill · `j`/`k` vertical Zone/Voice/Rack control (`w`/`b` group) · `Alt+h`/`l` & `,`/`.` fine (`Alt+H`/`L` & `<`/`>` coarse) · `Alt+Backspace` reset · `Enter` exact `ValueInput` / Sample Browser · `Alt+j`/`k` reorder active Zone · `a` `"Add Zone…"` picker · `d` duplicate zone · `x` delete zone · `r` rename zone · `e` edit/bind Macro & Modulation links.
  - *Modular Rack Ex commands:* `:new synth|sampler|kit|rack|modular [NAME]`, `:mod add <vco|sampler|noise|env|lfo|filter|vca|drive|lofi|resonator> [NAME]`, `:mod del <ID|NAME>`, `:mod rename <ID|NAME> <NEW_NAME>`, `:patch <FROM> <TO[.PORT]> [AMOUNT]`, `:unpatch <CABLE_ID | FROM TO[.PORT]>`, `:pin [PARAM [LABEL] | LABEL]`, `:unpin [INDEX | PARAM | LABEL]`, `:zone ...`, `:sample PATH`, `:bounce ...`.
- **Mixer (`gm` / `Ctrl+m`) & Object Details (`Channel`, `Effect`, `Part`, `Placement`):**
  - *Mixer:* `h`/`l` channel strip (`H`/`L` & `PgUp`/`PgDn` page; `0`/`$`/`G` first/Master) · `j`/`k` vertical slot (`Fader → Pan → Insert → Send → Output → Duck`) · `Alt+h`/`l` & `,`/`.` fine (`Alt+H`/`L` & `<`/`>` coarse; step sizes aligned with `Detail::Channel`) · `Alt+j`/`k` push Fader dB or reorder Insert · `Alt+Backspace` reset · `Enter` open Insert / bus picker / `EditDuck` / exact `ValueInput` · `o` open `Detail::Channel` · `a` add picker (`c` Channel / `f` Effect / `w` Send / `d` Duck) · `r` rename · `m`/`s` mute/solo (`b` bypass Insert) · `v` move Insert · `e` links · `x` remove child · `X` delete channel (confirm) · `C` clear clip.
  - *Object Details:* `j`/`k` vertical control (`w`/`b` group; **bare `h`/`l` strictly no-op**) · `0`/`$`/`G` first/last · `PgUp`/`PgDn` `±6` · `Alt+h`/`l` & `,`/`.` fine (`Alt+H`/`L` & `<`/`>` coarse) · `Alt+Backspace` reset · `Alt+j`/`k` reorder Insert (`Channel`/`Effect`) · `Enter` drill-down link or exact `ValueInput` · `e` edit/bind links · `r` rename/edit · `x`/`X` remove/delete · deck-specific verbs (`Channel`: `m`/`s`/`v`/`b`/`C`; `Effect`: `m`/`v`, Filter `P`/`f`; `Part`: `A`/`a`; `Placement`: `A`/`r`/`f`/`o`).
- **Macros (`gk` / `Ctrl+k`), Bank (`gb` / `Ctrl+b`), Signal Graph (`gG` / `Ctrl+g`), Take (`gt` / `Ctrl+t`), Devices (`gd` / `Ctrl+d`):**
  - *Macros:* `h`/`l` (`1–8`, `0`/`$`/`G`) Macro slot `M1–M8` · `j`/`k` Dial header ↔ scrollable Bindings · Dial: `Alt+h`/`l` & `,`/`.` `±2%` (`Alt+H`/`L` & `<`/`>` `±10%`), `Alt+Backspace` `0%`, `Enter` `ValueInput`, `a` bind picker, `r` rename, `e` driving Mod, `x`/`X` unbind all / clear slot · Binding: `Alt+h`/`l` & `,`/`.` `min`, `Alt+H`/`L` & `<`/`>` `max`, `Alt+j`/`k` reorder, `Enter` jump target, `v` invert, `x` unbind row.
  - *Bank:* `v` toggle `Parts ↔ Material`. **Parts:** `j`/`k` lane · `h`/`l` part in lane · `0`/`$`/`G` first/last lane · `Enter` launch (`Shift+Enter` immediate `at now`) · `A` audition · `a` add Part · `r` edit phase/offset · `x` release lane · `X` drop Part · `m`/`s` mute/solo. **Material:** `j`/`k` pattern (`PgUp`/`PgDn`, `0`/`$`/`G`) · `Enter`/`l` open Pattern · `h` focus left deck · `A` audition · `d` derive · `f` fork · `r` rename/edit · `X` delete pattern (confirm).
  - *Signal Graph:* `h`/`l` non-empty column · `j`/`k` node row (`0`/`gg` top, `G`/`End` bottom) · `Enter` jump to deck · `o` route output · `e` edit cable · `x`/`X` unlink cable / unlink all · `a` add connection (or `s`/`m`/`d` quick-add).
  - *Take:* `h`/`l` bar (`0`/`$`/`G` start/end) · `v` visual range · `Enter` / `Alt+c` keep range · `x` clear marks · `X` discard take (confirm) · `Ctrl+s` save take.
  - *Devices (`Audio | MIDI`):* `H`/`L` switch `Audio ↔ MIDI` tab · `j`/`k` vertical rows (`w`/`b` group jump; bare `h`/`l` no-op) · `Alt+h`/`l` & `,`/`.` cycle values (`Alt+Backspace` reset) · `Enter` picker / run action / MIDI Learn · `r` rescan audio or MIDI ports · `Ctrl+s` save config. On `MIDI`: `a` add mapping · `v` toggle invert · `x`/`X` delete mapping / clear all user mappings (confirm).
- **Settings (`g,` / `Ctrl+,`), Song Properties (`gP` / `Ctrl+Shift+i`), Modals & Browsers:**
  - *Settings & Song Properties:* `j`/`k` row · `w`/`b` group · bare `h`/`l` no-op · `Alt+h`/`l` & `,`/`.` fine (`Alt+H`/`L` & `<`/`>` coarse) · `Alt+Backspace` reset · `Enter` exact `ValueInput` / title edit · `a` add dir (`g,`) · `x` remove dir (`g,`) · `F` OS folder dialog (`g,`) · `Ctrl+s` save (`g,`) · `c` cycle boundary picker.
  - *Browsers, File Hub & Help:* `h`/`l`, `H`/`L`, `Tab`/`BackTab` switch horizontal tabs/categories (`FileHub` Export rows adjust values with `Alt+h`/`l` and `,`/`.`, keeping `H`/`L`/`Tab`/`1–5` on tabs; `Browse`/`Import` add `Backspace` parent with cursor restoration plus `~` home and `.` project jumps) · `j`/`k` list · `/` search · `A` audition · `Space` play/pause · `Enter` open/commit · `Esc` clear search → cancel/restore.

### 4E. Workspace Presets & Window Keys (`z` / `Ctrl+w`)

| Key | Action / Layout Preset |
| --- | --- |
| `z1` | **Compose:** Pinned `Arrange` + working `Pattern` |
| `z2` | **Perform:** Pinned `Arrange` + `Bank` (`Parts`) + `Output` |
| `z3` | **Sound Design:** Pinned `Pattern` + `Output` + working `Instrument` (fallback `Bank`) |
| `z4` | **Mix:** Pinned `Arrange` + `Output` + working `Mixer` |
| `zz` / `zp` / `zx` / `zo` | Toggle full-screen zoom / pin focused deck / close deck / keep only focused deck |
| `zh`/`zj`/`zk`/`zl`, `zw` | Directional deck focus movement / cycle focus (`Tab`) |
| `z=` / `z-` / `z0` | Zoom Arrange/Pattern timeline in / out / reset (also `Ctrl+=` / `Ctrl+-` / `Ctrl+0`) |

### 4F. Modal Overlays (`OverlayView`)

1. **`Palette` (`:` / `Ctrl+Shift+p`):** Fuzzy Action Palette (subsequence search over actions, decks, layouts, 8 quick-add FX, themes, and dynamic Sections/Patterns/Instruments) with top-slot **`▶ Run command:`** Ex passthrough.
2. **`PresetBrowser` (`P` on Instrument):** Unified Rack preset browser (`engine_label: "RACK"`) with **6 category filter tabs** (`All` / `Synth` / `Sampler` / `Drums` / `Kit` / `Modular`), 4 bare neutral defaults (`Init Synth`, `Blank Sampler`, `Blank Kit`, `Blank Modular`), Save Preset action, and instant non-destructive audio preview from a full `Project` snapshot (`j`/`k` preview, `/` search, `A` audition, `Space` transport, `Enter` single-transaction commit, `Esc` zero-trace restore).
3. **`GrooveBrowser` (`P` on Filter Effect):** Filter groove browser across the 128-slot `FilterEnv` bank (`24` MIT factory grooves + user grooves) with preview on scroll, Save-to-Bank, and Fork-to-Pattern (`f`).
4. **`SongProperties` (`gP` / `Ctrl+Shift+i`):** Modal editor for Title, Tempo (BPM), Key Root, Scale (all **13** `NAMED_SCALES`), Length (Bars), End Rule, and Live Timing on the `u` undo stack.
5. **`MiniEditor` (`Enter` on `Mini`):** Dedicated instant-validated Mini-Notation editor (`cadence_core::mini::parse_mini`) with real-time Cycle 1 event count and resolved note preview.
6. **`ValueInput` (`Enter` on scalar controls):** Unified exact scalar entry modal with instant parse validation and formatted preview across Pattern Values, all `ParamAddress` controls, Rack zone structural fields, Macro dials, Placement offsets, and Song Properties.
7. **`FileHub` (`gf`, `Ctrl+Shift+f/o/s/e`):** 5-tab Quick Open (New, Recent, Bundled Examples, Live Takes), Browse (pure directory manager with subdirectory cursor memory on `Backspace`/`..`, `~` home, `.` project, and `.rbs` import preview when `feature = "rbs"` is enabled), Save & Bundle, Import (auto-closes on preset/profile load), and Export (WAV/Stems), universal `/` search, `H`/`L`/`Tab`/`1–5` tabs, and `F` native OS dialogs.
8. **`Browser` (`B` / `gS` / `Sample` rows):** Universal `/` sample browser with async off-thread preview decoding, instant auditioning, and automatic `Sampler -> Out.In` module insertion + RIFF `smpl`/`inst` metadata application on the active Rack zone.
9. **`Authoring` (`n` / `d` / `r` / `tA` / workflow keys):** Typed modal draft forms for creating/editing patterns, derivations, instruments (`SoundChoice::{Template(RackTemplate), Preset(String), Sampler { path, label }, Folder { path, label }}`), channels, effects, sends, ducks, bounces, and 24-bit audio recording (`RecordDraft`).
10. **`Settings` (`g,` / `Ctrl+,` / `:settings`):** Persisted `UserConfig` editor: theme, startup preset, output monitor, follow, `Auto-Record Takes`, default BPM/key, project Live Timing, and auto-refreshing `sample_dirs` / `preset_dirs`.
11. **`Help` (`?` / `Ctrl+Shift+/`):** 7-tab wrapped, universal `/` searchable reference (`All`, `Navigate`, `Compose`, `Mini & Algebra`, `Sound & FX`, `Live & Jam`, `Files & CLI`; `H`/`L`/`Tab` switch tabs).
12. **`Confirm`:** Unsaved-dirty & destructive protection guarding Quit, New, Open, ImportRbs, DeleteChannel, ClearMidiMappings, and DiscardTake.
13. **`Tutorial`**, **`Picker`**, **`EventInspection`**, **`Inspect`/`Assets`**, and **`PrefixMenu`** legends (`g`, `t`, `z`, `v`, `S`).

---

## 5. 24-Bit Audio Recording & Hardware MIDI Control

### 5A. Audio Input & 24-Bit WAV Recording (`crates/app/src/{devices.rs,recording.rs}`)

- **Device & input setup (`Devices · Audio` tab via `Ctrl+d` then `H`/`L`, or `:audio`):** Select input/output devices, sample rate, buffer size, mono/stereo channel capture (`Stereo`, `Left (Ch 1)`, `Right (Ch 2)`), input monitoring (`Off`, `Auto` [monitor while armed/recording], `On`), latency trim (`-250..+250 ms`), and recovery actions (`Audio Panic`, `Restart Audio`, `Rescan Devices`).
- **Capture workflows (`ta`, `tA`, `ti`, `:rec`):**
  - `ta` toggles quick recording immediately; `tA` opens the `Record Audio` workflow dialog (`RecordingMode::{Synced, CountIn, Free}`, target bar length, destination New Sampler or existing Rack Zone, optional Arrange placement).
  - `:rec [start|stop|cancel|dialog|free|countin [BARS]|bars <N>|zone ...|monitor [off|auto|on]|mode <stereo|left|right>|input <NAME|none>]`.
  - Audio captures off the RT thread via a lock-free SPSC ring, resamples cleanly when input/output hardware sample rates differ, applies latency trim, writes a portable 24-bit PCM WAV under `<project>/recordings/` (or the configured session recordings directory for untitled projects), and commits a `Sampler` Rack instrument + Part + Placement in one undoable transaction.

### 5B. Hardware MIDI Learn, Overdub Recording & Bi-Directional Feedback (`crates/core/src/midi_map.rs`, `crates/app/src/midi/`)

MIDI mapping and feedback are data-driven and hardware-agnostic inside `cadence-app`, with raw wire encoding isolated to `cadence-engine::midi`:

- **Device Profiles (`UserConfig::midi_profile`):** `"generic"`, `"standard_cc"`, and `"minilab3_daw"` (Arturia MiniLab 3 DAW-mode integration).
- **Sources & Value Modes:** `MidiSource::{Cc, Note, PitchBend}` (channel-scoped or omni); `MidiValueMode::{Absolute, RelativeSigned, RelativeOffset64, Trigger, Toggle, Momentary}` with `[min, max]`, `invert`, and soft-pickup takeover (`pickup: true`) so absolute knobs catch the active value smoothly without jumps.
- **Bindable `MidiTarget`s:** `Param { address }` (any automatable `ParamAddress`, including `Rack`, `Zone`, `Module`, and `Cable` attenuverters), `FocusedMacro { slot: 0..8 }` (tracks focused Effect/Channel controls, Project Macros 1–8 outside object decks, or active Rack controls prioritizing continuous `!param.is_discrete()` `panel_pins` first, then continuous active-zone module controls, then `RackParam`s, then discrete pins), `ProjectMacro { slot: 0..8 }`, `LaneGain`, `MasterGain`, `Transport` (10 `MidiTransportAction`s + universal MMC), `LaunchSection`, `LaunchLanePart`, `MuteLane`, `SoloLane`.
- **1-Key MIDI Learn (`M`):** Resolves the focused target automatically (`Macro` slot → `Midi` row → object-deck automatable parameter → `Mixer` channel gain/pan/send → `Bank` launch part → active Rack's first pinned control, first Filter cutoff, or Rack Cutoff). Pressing `x`/`Backspace` while armed unlearns; `Esc` cancels. Learned mappings persist globally in `UserConfig::midi_mappings`.
- **Note & CC Recording (`R`):** Enters notes in step-entry mode when stopped; while playing, overdubs quantized polyphonic chords and lazily creates/updates `Values(Normalized, Ramp)` automation curves for any moved MIDI CC or Project Macro (`1500 ms` gesture undo coalescing).
- **Bi-Directional Feedback (`FeedbackProtocol::{None, StandardCcNote, ArturiaDaw}`):** `App::sync_midi_feedback` emits pure semantic `MidiFeedback` events (`InitDawMode`, 2-line OLED `Display` with value bar, RGB pad LEDs across `MidiPadMode::{Notes, Sections, Mutes}`, `TransportLedKind` LEDs, and CC echo) translated to hardware bytes by `cadence-engine::midi`.
