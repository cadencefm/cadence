# Structured Authoring & Live Control Session

These JSONL files exercise the same typed `Action` model used by interactive keyboard editing. They start from an empty project, so the assigned stable IDs are completely reproducible.

## Batch `--script` Execution

```sh
cargo run --release --locked -- --empty --script examples/agent-session/session.jsonl --output /tmp/session.json
cargo run --release --locked -- --project /tmp/session.json --script examples/agent-session/mixer.jsonl --output /tmp/mixed-session.json --check
```

`session.jsonl` creates a playable lead, a separate harmonic source, and a retained `AddDegrees` derivation, then updates the part binding to reference it. Optimistic `expected_revision` guards make stale assumptions fail visibly. `mixer.jsonl` adds an independent `Return`, inserts a native `Reverb`, and wires a `Send` from the Lead `Audio` channel. Responses include the authored state; running interactively with `--stdio` adds live transport and meter observations.

## Live `--ctl` Control Socket

To drive a live GPU Cadence session from scripts or an AI/agent co-producer (with every change mirrored immediately on the `u` undo stack and audible in real time), start Cadence with `--ctl` and use `cadence ctl`:

```sh
# Launch live GUI session with Unix control socket enabled
cadence --empty --ctl

# Inspect compact state, run typed actions or Ex commands, and listen to stereo audio
cadence ctl state --compact
cadence ctl cmd "bpm 128" --compact
cadence ctl action '{"op":"play","playing":true}' --compact
cadence ctl meters
cadence ctl listen --bars 1..5 --out /tmp/preview.wav
```

For a keyboard-driven construction recording, see [Signal Garden](../material-lab/README.md). For full CLI, JSONL, and `--ctl` protocol details, read [CLI, Scripting & Live Control](../../docs/guide/08-cli-scripting-and-live-ctl.md) and the [Protocols Specification](../../docs/spec/protocols.md).
