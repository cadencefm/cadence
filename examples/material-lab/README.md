# Signal Garden

**10 bars (8 musical bars + 2-bar tail) · 136 BPM · D natural minor · ~0:18**

A compact, original construction lesson built entirely through Cadence's normal key controller: a five-step motif, independent 64-step harmony, shared lead and bass derivations, two drum parts, a placement-local ending, seeded per-note pan, native Delay and Reverb returns, and kick-triggered sidechain ducking.

[Project](project.json) · [Build script](build.keys.jsonl) · [User Guide](../../docs/guide/README.md) · [Patterns & Algebra](../../docs/guide/02-patterns-and-algebra.md) · [CLI & Scripting](../../docs/guide/08-cli-scripting-and-live-ctl.md)

The project holds eleven pattern definitions — including a linked fork (`MovingCopy`) that keeps its dependency links and a deliberately frozen 32-step snapshot (`EightBeats`). Four parts and five placements schedule four native Synth instruments; `Lead` uses a warm three-voice stereo unison patch.

## Open, rebuild & render

```sh
# Open Signal Garden in Cadence
cargo run --release --locked -- --project examples/material-lab/project.json

# Rebuild project.json from scratch via build.keys.jsonl
cargo run --release --locked -- --empty --sample-roots-only \
  --key-script examples/material-lab/build.keys.jsonl \
  --output /tmp/signal-garden.json --check

# Render to 48 kHz stereo WAV with measurements
cargo run --release --locked -- --project examples/material-lab/project.json --sample-roots-only \
  --render signal-garden.wav --render-report signal-garden.report.json
```

[build.keys.jsonl](build.keys.jsonl) is the authoritative authoring source: every command runs through Cadence's controller, its inline `expect` assertions verify the graph and placement-local ending, and `tests/integration/cli.rs` verifies that replaying `build.keys.jsonl` reproduces the checked-in [project.json](project.json) identically.

## What to try

- Open `Harmony` and edit its point at step 25: `Lead` and `Bass` update together while the placement-local ending at bar 8 stays as written.
- Open `MovingCopy` and inspect its sources: a fork keeps its retained links to `Motif` and `Harmony`.
- Open `EightBeats`: freeze captured literal material independent of future edits.
- Open the Mixer (`gm`): adjust `Music` trim while the kick-triggered `Breath` duck envelope and `Contour` gain modulation stay active.

## Key sessions

Each session runs headlessly with `--key-script FILE` (add `--project examples/material-lab/project.json` unless it starts `--empty`):

| Session | What it does |
| --- | --- |
| [build.keys.jsonl](build.keys.jsonl) | Builds Signal Garden from `--empty` with Ex commands |
| [authoring.keys.jsonl](authoring.keys.jsonl) | Builds a small score through pattern dialogs, typed source pickers, exact point entry, and placement forms (starts `--empty`) |
| [sounds.keys.jsonl](sounds.keys.jsonl) | Creates a Sampler from bundled assets, auditions and cancels a replacement, places a part, and follows playback (starts `--empty --sample-roots-only --sample-dir examples/demo/samples`) |
| [transport.keys.jsonl](transport.keys.jsonl) | Exercises cueing, dry/final scopes, monitor visibility, and stop-to-start without editing |
| [deck.keys.jsonl](deck.keys.jsonl) | Pins `Lead` beside source material and exercises Home/end navigation |
| [performance.keys.jsonl](performance.keys.jsonl) | Opens the evaluated overview and piano roll, follows a source through provenance, edits and undoes the local ending |

Original notes, patches, arrangement, and scripts are available under the repository's MIT license or CC0 1.0, at your choice; see [LICENSE.md](LICENSE.md).
