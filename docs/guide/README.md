# Cadence User Guide

Cadence is a GPU-accelerated, keyboard-driven musical studio built around **retained relationships**. Patterns exist independently of the instruments that play them and the arrangement lanes that schedule them: edit one harmonic progression, rhythm, or mini-notation line and every chord, arpeggio, bassline, and filter envelope derived from it updates together—live, without expanding or losing the link.

## Where to Start (By Background)

- **Coming from a DAW (Ableton Live, Logic, FL Studio):** Start with the [Quick Tour](01-quick-tour.md) to learn decks and transport, then jump to [Mixing, FX & Signal Graph](04-mixing-fx-and-routing.md), [Project Macros & Modulation](05-macros-and-modulation.md), and [Audio Devices, Live Recording & Hardware MIDI](06-devices-audio-and-midi.md). Think of **Lanes** as tracks, **Parts** as reusable pattern+instrument clips, **Placements** as arranged spans, and **Macros (`M1–M8`)** as song-wide performance racks.
- **Coming from a Hardware Groovebox, Modular Synth or Tracker (Eurorack, Elektron, Renoise, ReBirth):** You will feel right at home with the unified 11-module Eurorack Modular Rack (`Panel | Patchbay | Zones` with bipolar attenuverters, 2× through-zero FM, 32-mode physical `Resonator`, and `z-1` feedback), per-note P-locks (`:link`), multi-zone `:zone` drum machines (`TR-808`, `TR-909`, `EDM Kit` — baked & live synth kits), rhythmic Pattern-Controlled Filtering (`Filter`), and 1-key live staging. Start with [Quick Tour](01-quick-tour.md), [Patterns & Algebra](02-patterns-and-algebra.md), and [Modular Rack Instruments & Assets](03-instruments-and-assets.md).
- **Coming from Live-Coding (Strudel, TidalCycles, SuperCollider):** Jump straight into [Patterns & Algebra](02-patterns-and-algebra.md) for inline `:mini` notation and all 22 composable `PatternKind` operations, then read [CLI, Scripting & Live `--ctl`](08-cli-scripting-and-live-ctl.md) to drive a live Cadence GUI window from scripts or agents.

---

## Deck Map & Modeless Grammar

Every workspace view in Cadence is a **deck**—either a song-level deck or an object inspector deck. Cadence follows one unified **modeless 4-tier keyboard grammar** across every deck with zero axis flips:

- **Tier 1 · Navigate (`h j k l` + Arrows):** Pure spatial cursor movement matching screen axes—never mutating audio.
- **Tier 2 · Coarse Nav / Tabs (`H J K L`):** Coarse jumps or deck tabs (`Panel | Patchbay | Zones` on Rack, `Audio | MIDI` on Devices, bars `±16`, sections, Mixer pages)—never mutating audio.
- **Tier 3 · Direct 2D Manipulate & Reorder (`Alt+h j k l` fine · `Alt+H J K L` coarse):** Directly sculpt knobs, faders, pitch/gate, placement positions, curve breakpoints, or reorder inserts/zones/pins/bindings along their visual axis with coalesced single-gesture undo (`u` / `Ctrl+z`).
- **Tier 4 · One-Hand Scalar Adjust (`, / .` fine · `< / >` coarse · `Alt+Backspace` reset):** Nudge any focused scalar one-handed; press **`Enter`** on any scalar for exact **Value Entry** (`ValueInput`).
- **Focused Input Modes (only where needed):** **`i`** enters **Step Insert** piano entry on Pattern Notes; **`gj` / `Ctrl+Shift+j`** toggles modal **Jam Mode** (`1–0` sections, `q–p` lanes, `z–,` piano, `Tab`/`Shift-Tab` bank).

Press **`g<letter>`** or unshifted **`Ctrl+<letter>`** to jump straight to any of the 10 docked decks, **`Ctrl+Shift+<key>`** (or direct triggers like `:`, `?`, `n`, `B`) for modals and overlays, **`z1`–`z4`** for 1-key workspace layouts, **`Tab`** to cycle focus, **`zz`** to zoom full-screen, **`zp`** to pin/unpin, **`Esc`** to unwind your trail, and **`?`** for the tabbed, **`/`** searchable Help reference.

```text
  SONG DECKS (g prefix / Ctrl+<key>)           OBJECT DECKS & MODALS (Ctrl+Shift+<key>)
  ──────────────────────────────────           ────────────────────────────────────────
  ga / Ctrl+a · Arrange (Placements/Roll) ───▶ gi / Ctrl+i · Instrument Rack (Panel|Patchbay|Zones)
  gb / Ctrl+b · Bank (Parts / Material)   ───▶ Enter       · Object Deck (Placement/Part/Channel/FX)
  gp / Ctrl+p · Pattern (2D/Values/Mini)  ───▶ P           · Preset / Filter Groove Browser (live)
  gm / Ctrl+m · Mixer (32f Strips + GR)   ───▶ B / gS      · Sample Browser (Ctrl+Shift+b)
  gG / Ctrl+g · Signal Graph Patchbay     ───▶ gf          · File Hub (Ctrl+Shift+f/o/s/e)
  gk / Ctrl+k · Project Macros (M1–M8)    ───▶ gP          · Song Properties (Ctrl+Shift+i)
  gd / Ctrl+d · Devices (Audio | MIDI)    ───▶ g,          · Settings & Themes (Ctrl+,)
  gt / Ctrl+t · Take Deck (tr / v / X)    ───▶ :           · Fuzzy Action Palette (Ctrl+Shift+p)
  go / Ctrl+o · Output Scope & Meters     ───▶ gj          · Jam Mode HUD (Ctrl+Shift+j)
```

### Quick Navigation Cheatsheet

| Prefix / Key | Action |
|---|---|
| `h/j/k/l` · `H/J/K/L` · `w/b` | Strictly navigate & switch tabs (`H/L` tabs/bars/sections/pages · `w/b` groups/events/breakpoints/cables) |
| `Alt+h/j/k/l` (`Alt+H/J/K/L`) · `,`/`.` (`<`/`>`) | Direct 2D manipulation & reorder · one-hand scalar adjust · `Alt+Backspace` reset |
| `Enter` / `i` / `gj` / `Esc` | Open / launch / exact **Value Entry** · **Step Insert** (Pattern Notes) · **Jam Mode** · unwind trail |
| `:` or `Ctrl+Shift+p` | Open **Fuzzy Action Palette** (fuzzy-search actions, decks, FX, song objects + live top-slot Ex passthrough) |
| `ga` `gp` `gb` `gm` *(or `Ctrl+a/p/b/m`)* | Jump to **Arrange**, **Pattern**, **Bank**, or **Mixer** |
| `gi` `gk` `gt` `gG` `gd` `go` *(or `Ctrl+i/k/t/g/d/o`)* | Jump to **Instrument Rack**, **Macros**, **Take**, **Signal Graph**, **Devices (`Audio \| MIDI`)**, toggle **Output** |
| `gs` `gS`/`B` `gP` `gf` `g,` | Follow **Source**, **Sample Browser**, **Song Properties**, **File Hub**, **Settings** |
| `z1` `z2` `z3` `z4` | 1-key workspace presets: **Compose**, **Perform**, **Sound Design**, **Mix** |
| `zz` `zp` `zx` `zo` | **Zoom** active deck full-screen, **Pin/unpin**, **Close**, or **Only** (close others) |
| `Tab` · `A` · `?` | Cycle visible decks · universal **Audition** · open **searchable Help** (`H`/`L`/`Tab` tabs, `/` filter) |

---

## Chapters

1. [**Quick Tour: First 10 Minutes**](01-quick-tour.md) — Learn the modeless 4-tier keyboard grammar, Fuzzy Action Palette, Song Properties (`gP`), themes, transport (`tr` Take Arming & `ta` live audio recording), and build a living 3-part groove.
2. [**Patterns, Algebra & Inline `:mini`**](02-patterns-and-algebra.md) — 2D polyphonic chord editing, `Step Insert`, Patterns/Parts/Placements/Lanes, shared vs. local/`'` *"From here"* editing, all 22 `PatternKind` operations, the value curve editor, and the live-validated **Mini-Notation Editor**.
3. [**Modular Rack Instruments, Patchbay, Zones & Assets**](03-instruments-and-assets.md) — Unified 11-module Eurorack Modular Rack (`Panel | Patchbay | Zones`, incl. `Resonator`), cubic-Bezier cables with bipolar attenuverters & `z-1` feedback, 32-zone kits & choke groups, `:mod`/`:patch`/`:unpatch`/`:pin`/`:unpin`/`:new`, 6-category **Preset Browser (`P`)** with live non-destructive preview, baked + live-synth drum kits, bounce-to-zone, File Hub, and portable bundles.
4. [**Mixing, Studio FX & Interactive Signal Graph**](04-mixing-fx-and-routing.md) — Vertical Mixer channel detail stack (`Fader`, `Pan`, `Inserts`, `Sends`, `Output`, `Duck`), unclipped 32-bit float metering (`-60..+6 dBFS`, `0 dB` hairline, `HOT (32f)` vs `!CLIP`), `+6 dB` console headroom, nested `Group → Group` routing, all 8 zero-alloc studio FX, and direct cable patching in `gG`.
5. [**Project Macros & Modulation**](05-macros-and-modulation.md) — 8 assignable Project Macros (`M1–M8`), exact compilation precedence, global `:mod` curves, and per-note `:link` P-locks across Rack/Zone/Module/Cable/FX/Send/Channel targets.
6. [**Audio Devices, Live Recording & Hardware MIDI**](06-devices-audio-and-midi.md) — Unified `Devices` (`Audio | MIDI`) deck (`gd` / `Ctrl+d` / `:audio`), live CPU/xrun/input metering, live 24-bit WAV audio recording (`ta` / `tA` / `ti` / `:rec`), 1-key MIDI Learn (`M`), live Note+CC recording (`R`), hardware controller profiles & OLED/RGB feedback, plus **Global Settings & GPU Themes (`g,`)**.
7. [**Live Performance & Armed Takes**](07-performing-and-takes.md) — Evaluated Arrange views (`vp`/`vo`/`vr`) and event provenance, row-oriented Bank Launchpad, Jam Mode (`gj` / `Ctrl+Shift+j`), universal `Alt` layer, quantization boundaries, live staging, `Alt+c`/`Alt+C` Keep, and explicit Take Arming (`tr` / `gt` with `v` Visual range marking & `X` discard).
8. [**CLI, Headless Rendering, Formats & Live `--ctl`**](08-cli-scripting-and-live-ctl.md) — Offline validation, WAV/stems rendering, audio/MIDI session flags, clean-room ReBirth `.rbs` import (`--import-rbs`), factory kit rendering (`--render-kit` emitting `v1` Rack kits), `--key-script`/`--script`/`--stdio`, and controlling/auditioning a live GUI session via `--ctl` & `cadence ctl`.

---

## Showcase Projects & Technical Specs

### Showcase Projects (100% MIT)
- [**Afterglow 3.0**](https://github.com/cadencefm/cadence/blob/main/examples/demo/README.md) (`examples/demo/project.json`) — 64-bar modern EDM centerpiece featuring 8 Synth Racks, the 9-zone `EDM Kit` Rack, `Bell Mini` inline `:mini`, 6 automated Project Macros, and studio mastering.
- [**Iron Circuit**](https://github.com/cadencefm/cadence/blob/main/examples/iron-circuit/README.md) (`examples/iron-circuit/project.json`) — 48-bar Acid Electro flagship with `TR-808` & `TR-909` multi-zone Rack kits, Detroit chord `Sampler` Rack, `AcidLowPass` 303, `Fold`/`HardClip` Drive, rhythmic `Filter` PCF, nested `Percussion → Rhythm` groups, and all 8 Macros.
- [**Glass Cathedral**](https://github.com/cadencefm/cadence/blob/main/examples/glass-cathedral/README.md) (`examples/glass-cathedral/project.json`) — 32-bar generative ambient/IDM showcase with polymetric clocks, `Cathedral Bells` `:mini`, automated Send swells, and 4 Ambient Macros.
- [**Fivefold Live**](https://github.com/cadencefm/cadence/blob/main/examples/fivefold-live/README.md) (`examples/fivefold-live/project.json`) — 60-bar live performance set with a replayable `.keys.jsonl` performance script and deterministic `.take.jsonl` recording.
- [**Signal Garden**](https://github.com/cadencefm/cadence/blob/main/examples/material-lab/README.md) (`examples/material-lab/project.json`) — Compact 10-bar retained-material authoring lab accompanied by modular `.keys.jsonl` scripts.
- [**Agent Session**](https://github.com/cadencefm/cadence/blob/main/examples/agent-session/README.md) (`examples/agent-session/session.jsonl`) — Typed JSONL `Request`/`Action` scripting session for headless and `--stdio` co-production.

### Architecture & Implementation Specs
- [Decks, Keymap & Interaction Spec](../spec/decks-and-interaction.md)
- [Material & Timing Spec](../spec/material-and-timing.md)
- [DSP & Audio Engine Spec](../spec/dsp-and-audio-engine.md)
- [Protocols & CLI Spec](../spec/protocols.md)
- [Design Decisions](../design-decisions.md) · [Verification Suite](../verification.md)
