# Fivefold Live

**60-bar arrangement (62-bar recorded take) · 136 BPM · D natural minor · ~1:49**

*Fivefold Current* played live over its own arrangement from the computer keyboard. The song loops one section at a time. The number row moves it to the next section, and each move lands cleanly at the end of the current pass. Lane keys launch clap builds and a noise riser over the arrangement and release them again. In the breakdown, an edit to the shared 64-step harmony lands on the next bar and shifts the acid lead, stereo air layer, and rolling bass together — and its `:undo` lands on a bar too.

[The project](project.json) · [The recorded take](takes/set.take.jsonl) · [Setup keys](setup.keys.jsonl) · [Performance keys](perform.keys.jsonl) · [Compose script](compose.py)

## Play & Perform

```sh
# Open Fivefold Live in Cadence
cargo run --release -- --project examples/fivefold-live/project.json --sample-roots-only

# Replay the scripted performance and regenerate takes/set.take.jsonl
cargo build --release --locked
python3 examples/fivefold-live/compose.py

# Render the recorded take (62 bars, 17 performance changes) to 48 kHz stereo WAV
target/release/cadence --project examples/fivefold-live/project.json --sample-roots-only \
  --take examples/fivefold-live/takes/set.take.jsonl \
  --render fivefold-live-take.wav --render-report fivefold-live-take.report.json

# Or render the base 60-bar arrangement directly
target/release/cadence --project examples/fivefold-live/project.json --sample-roots-only \
  --render fivefold-live-song.wav --render-report fivefold-live-song.report.json
```

## The project

Fivefold Current's arrangement spans nine sections over 60 bars. [setup.keys.jsonl](setup.keys.jsonl) sets the `Lift / Noise` part to `restart` phase on the `Lift` lane (`i1`) and sets `:quantize edits launch` so song edits during playback land on the next bar boundary.

The number row addresses the sections:

| Key | Section | Bars |
| --- | --- | --- |
| `1` | Seed | 1–4 |
| `2` | Pulse | 5–8 |
| `3` | Weight | 9–16 |
| `4` | Prism | 17–24 |
| `5` | Unfold | 25–32 |
| `6` | Current | 33–40 |
| `7` | Crest | 41–48 |
| `8` | Landing | 49–56 |
| `9` | Afterglow | 57–60 |

Each lane holds its arrangement parts in song order (plus any configured phase overrides):

| Key | Lane | Parts |
| --- | --- | --- |
| `q` | Cell | 1 Cell / Acid · 2 Current / Acid |
| `w` | Pulse | 1 Quarter / Kick |
| `e` | Prism | 1 Cell / Air · 2 Current / Air |
| `r` | Weight | 1 Tonic Offbeat · 2 Current Roll · 3 Current Offbeat |
| `t` | Spark | 1 Eighths / Closed |
| `y` | Breath | 1 Offbeats / Open |
| `u` | Hands | 1 Backbeat · 2 Build Eighths · 3 Build Sixteenths · 4 Crest Throw |
| `i` | Lift | 1 Lift / Noise (restart phase) |

## The performance

`tf` makes the view follow playback, `tl` loops `Seed` (the section under the cursor), and `tr` arms take recording. `gj` (or `Ctrl+Shift+j`) turns the keyboard into Jam mode, and the armed take recorder captures every gesture into a take. Every gesture happens mid-bar: a section key lands at the end of the current pass; lane launches, releases, mutes, and edits land on the next downbeat.

| Bar | Keys | What happens |
| --- | --- | --- |
| 1 | `tf` `tl` `tr`, `gj`, Space | Seed loops with take recording armed: the five-step acid cell alone |
| 5 | `2` | Pulse: 55 Hz kick and sidechain pumping enter |
| 9 | `3` | Weight: the tonic offbeat bass locks in |
| 14 | `u2` | The eighths clap build plays over Weight |
| 16 | `u3` `i1` `4` | Sixteenths clap build and noise lift for the last bar; Prism queued |
| 17 | `uu` `ii` | Prism arrives and the build and lift are released |
| 25 | `5` | Unfold: the breakdown exposes the 64-step harmony |
| 28 | `:value "Harmonic Current" 33 4` | The third quarter of every harmonic cycle lifts two degrees across lead, air, and bass at once |
| 33 | `6` | Current: rolling bass under the edited harmony |
| 36, 38 | `e` Alt-m, `e` Alt-m | The stereo air layer drops out for two bars, then returns |
| 41 | `7` | Crest: full-energy climax with clap throws |
| 44 | `:undo` | The harmony edit is undone cleanly on the bar |
| 49 | `8` | Landing: withdraws to acid cell, kick, and bass |
| 57 | `9` | Afterglow: one pass of delay and reverb tails, then `:take save` writes [takes/set.take.jsonl](takes/set.take.jsonl) |

## Performance Macros (`M1`–`M4`)

Four performance macros are pre-mapped for real-time shaping in the **Macros Deck (`gk`)** or Jam mode without colliding with the arrangement's 60-bar cutoff and bass modulations:

| Macro | Name | Rest | What it shapes |
| --- | --- | --- | --- |
| `M1` | **Acid Bite** | `35%` | 4-pole Diode Ladder resonance & filter envelope depth on *Five-step Cell* + `Acid Bite` Shock-Port drive, mix, and tone |
| `M2` | **Air Bloom** | `35%` | `Air Chorus` depth & mix, `Music → Prism Space` reverb send, `Reverb` size, and `Air Shimmer EQ` high shelf |
| `M3` | **Dub Echo** | `35%` | `Music → Dotted Echo` (3/16 ping-pong) & `Long Echo` (5/16 ping-pong) sends and both delay feedback levels |
| `M4` | **Club Glue** | `40%` | `Master Glue` & `Drum Glue` compressor thresholds/mix + `Master Tape` saturation drive & mix |

## Sound Design, Mixing & Mastering

- **Instruments:** *Five-step Cell* runs through Cadence's nonlinear 4-pole `diode_ladder` filter with velocity-sensitive bite; *Pulse Source* is tuned to a tight 55 Hz (`A1`, fifth of D minor) club kick sitting cleanly beneath *Harmonic Weight*'s dual-saw + sine-fundamental D2 bass; *Refracted Air* stacks 4-voice wide stereo unison; *Closed/Open Spark* pan symmetrically (`+12%` / `-12%`); *Handclap* plays the bundled [samples/clap.wav](samples/clap.wav); *Current Lift* builds stereo-spread filtered noise.
- **Studio Mix & Mastering Chain (18 native DSP devices):** Per-channel sculpting (`Acid Bite` drive + `Cell Sculpt EQ`, `Kick Weight EQ` + `Kick Punch` tape, `Air Chorus` + `Air Shimmer EQ`, `Bass Tape` + `Bass Pocket EQ`, `Clap Snap EQ`), `Drums` bus `Drum Glue` compressor, `Music` bus `Music Clear EQ`, three stereo returns (`Dotted Echo` 3/16 ping-pong, `Long Echo` 5/16 ping-pong, `Prism Space` reverb + `Space Clean EQ` HPF), five kick-triggered sidechain ducks, and a 3-stage master bus (`Master Glue` compressor → `Master Tape` drive → `Master Limiter` at `-1.0 dBFS` ceiling). Pass `--render-report` to inspect loudness and bus metering for either the 62-bar recorded take or the 60-bar arrangement.

**Screenshots & User Guide:** [Take Deck & Visual Range](../../docs/screenshots/15-take-deck.png) · [Live Performance & Armed Takes](../../docs/guide/07-performing-and-takes.md) · [Macros & Modulation](../../docs/guide/05-macros-and-modulation.md)

See [LICENSE.md](LICENSE.md).
