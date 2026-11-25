# Glass Cathedral — Generative Ambient / IDM Showcase

**32 bars · 108 BPM · D Dorian · ~1:11**

*Glass Cathedral* highlights Cadence's contemplative, polymetric generative side — built from 100% pure synthesis, retained pattern algebra, playable ambient Macros (`M1..M4`), automated spatial `Send` swells, and lush native DSP (`Chorus`, polymetric 5/16 `Delay`, 5.8s `Reverb`, surgical `Eq`, and a 4-stage studio mastering chain).

## Play & Inspect

```sh
# Open in Cadence
cargo run --release -- --project examples/glass-cathedral/project.json

# Render full 48 kHz stereo WAV
cargo run --release -- --project examples/glass-cathedral/project.json --render glass-cathedral.wav

# Rebuild project.json from the reproducible Rust score & inspect 48 kHz section dynamics
cargo run --release --example build-glass-cathedral -- --analyze
```

Once open:
- Press **`Space`** to play from Bar 1, or jump directly to **`⌥1` Nave**, **`⌥2` Refraction**, **`⌥3` Resonance**, or **`⌥4` Stillness** on the next bar boundary.
- Press **`gk`** to open the **Macros Deck** and perform **`M2 Glass Shimmer`**, **`M3 Echo Orbit`**, and **`M4 Cello Depth`** live while **`M1 Vault Bloom`** glides across its automated 32-bar arc.
- Press **`gG`** to inspect the living Signal Graph, including the dedicated `Glass` / `Chimes` silk EQs, `Tap` 5/16 delay, `Cathedral` 5.8s reverb, and 4-stage `Master` chain.
- Press **`:open "Cathedral Bells"`** then **`gp`** to edit the inline `:mini` chimes live.

## Musical Structure (4 Sections × 8 Bars)

| Section | Bars | Key | Musical Arc |
| --- | --- | --- | --- |
| **1. Nave** | 1–8 | `⌥1` | Chorused D-Dorian 9th pad opens inside the stone vault; mono legato Cello drone & 7×11 Glass Marimba enter at Bar 5 |
| **2. Refraction** | 9–16 | `⌥2` | Celesta-voiced `Crystal Chime` Mini bells enter on the right; `Euclidean(5,16)` Soft Pulse & swung Micro Dust arrive at Bar 13 |
| **3. Resonance** | 17–24 | `⌥3` | `Glass 5/11` shifts into cascading `Triplet Rain` (`3/2` random arp); `M1 Vault Bloom` peaks with breathing 5/16 delay swells |
| **4. Stillness** | 25–32 | `⌥4` | Layers recede gracefully into Pad + Cello; `Pad -> Cathedral` blooms on Bar 31 and Bar 32 dissolves naturally into the 5.8s reverb tail |

## Playable Macros & Automated Spatial Sends

| Control | Automation | Targets |
| --- | --- | --- |
| **`M1 Vault Bloom`** | Automated 32-bar ramp (`0.24 → 0.86 → 0.10`) | `Cathedral Pad` cutoff (`340–3200 Hz`), `Chimes -> Cathedral` (`0.36–0.78`), `Glass -> Cathedral` (`0.32–0.68`), `Vault Verb` damping (`2100–3900 Hz`) |
| **`M2 Glass Shimmer`** | Live performance macro (rests at `0.50`) | `Glass Marimba` cutoff (`1150–4600 Hz`), `Crystal Chime` cutoff (`1050–4200 Hz`), `Glass Silk` & `Chime Silk` high shelves |
| **`M3 Echo Orbit`** | Live performance macro (rests at `0.50`) | `Tap 5/16` feedback (`0.22–0.72`), `Tap` return gain trim, `Tap HPF` high shelf (`-4.0..0.0 dB`) |
| **`M4 Cello Depth`** | Live performance macro (rests at `0.50`) | `Warm Cello` cutoff (`240–1400 Hz`), `SubLevel` (`0.18–0.66`), `Cello Warm` low shelf (`-1.0..+4.0 dB`) |
| **`Glass Tap Swell`** | Automated 8-bar `Send` ramp (`0.34–0.66`) | `Glass -> Tap` 5/16 delay send blooms organically at 4-bar and 8-bar harmonic cadences |
| **`Pad Vault Swell`** | Automated 32-bar `Send` ramp (`0.50–0.74`) | `Pad -> Cathedral` reverb send opens gently across the song and floats the final Bar 31 chord into Bar 32 stillness |

## Key Generative Techniques

- **Polymetric Phasing (`SeededValues(7)` × `Euclidean(5, 11)`):** A 7-step deterministic seeded melody masked by an 11-step Euclidean rhythm creates a 77-step evolving glass marimba canon, mirrored every 3rd cycle via **`Every(3, GlassStream, Reverse(GlassStream))`**.
- **Triplet Rain (`TimeScale(3/2)` over `Arp(Random)`):** A random arpeggiator sampling sustained D-Dorian 9th chords (`Dm9 → G13 → Cmaj7 → Am7`) is time-compressed `3:2` into floating triplet raindrops.
- **Dedicated Celesta-Voiced Mini Bells (`Cathedral Bells`):** `d5 ~ [a5 c6] ~ ~ f5 [e5 d5] ~ a4 ~ [c5 e5 g5] ~ ~ [f5 a5] ~ ~` plays through its own softer `Crystal Chime` patch (panned right against `Glass Marimba` left) so the climax shimmers without upper-mid clash.
- **Grounded Cello & Micro-Percussion:** `RangeFold(36..48)` keeps the mono legato Cello Drone locked in `C2..C3`, paired with an `Euclidean(5, 16)` warm D2 soft pulse and `Swing(0.22)` → `Chance(0.36)` bandpass micro-dust.

## Studio Mix & Mastering Chain

- **Spatial Returns:** `Tap` (`5/16` ping-pong `Delay` + `260 Hz` HPF / gentle `3.2 kHz` & high-shelf taming) and `Cathedral` (`5.8s` `Vault Verb`, `3200 Hz` damping + `190 Hz` HPF).
- **Master Bus (4 Inserts):** `Tape Warmth` (`Drive(Tape)` whisper saturation) → `Cathedral Glue` (`Compressor`) → `Master Tone` (`Eq`, `26 Hz` HPF, gentle low warmth & `3.2 kHz` silk dip) → `Master Limiter` (`-1.5 dBFS` ceiling). Run `--render-report` or `build-glass-cathedral -- --analyze` to inspect live section dynamics and gain reduction.

## Explore Further

- **User Guide:** [Patterns & Algebra](../../docs/guide/02-patterns-and-algebra.md) · [Mixing, FX & Routing](../../docs/guide/04-mixing-fx-and-routing.md) · [Macros & Modulation](../../docs/guide/05-macros-and-modulation.md)
