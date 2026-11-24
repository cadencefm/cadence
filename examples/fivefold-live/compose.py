#!/usr/bin/env python3
"""Build Fivefold Live's key scripts, canonicalize its project, and record its take.

Fivefold Live is Fivefold Current played live over its own arrangement. Setup
configures the Lift lane's noise riser to restart on launch and quantizes live
edits to the launch boundary, through Cadence's real key controller. The
performance loops the first section, moves through the sections with the number
row (each change lands at the end of the current pass), plays clap builds and
the noise lift over the arrangement with lane keys, edits the shared harmony
live and undoes it, then saves the recorded take.

Every gesture is timed by bars: `advance` drives the headless clock, `wait_ms`
the real-time clock. Changes land on bar boundaries, so the music is the same
whichever clock runs it.

Run from the repository root after `cargo build --release --locked`:

    python3 examples/fivefold-live/compose.py
"""
import argparse
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
BINARY = ROOT / "target/release/cadence"
SOURCE = HERE / "project.json"
BPM = 136.0
STEP_MS = 60_000 / BPM / 4
BAR_MS = STEP_MS * 16
KEY_DELAY = 0.012  # seconds per key in real-time replay

# Parts configured on their lanes, (lane, part, phase):
# Hands already holds [Backbeat, Build Eighths, Build Sixteenths, Crest Throw]
# in song order from its placements, so u2 / u3 launch the two builds.
# Lift holds [Lift / Noise]; set its phase to restart so i1 restarts the riser.
PARTS = [
    ("Lift", "Lift / Noise", "restart"),
]


def quote(name: str) -> str:
    return f'"{name}"' if " " in name or "/" in name else name


def setup_steps():
    keys = "".join(f":lane {lane} add {quote(part)} {phase}<Enter>" for lane, part, phase in PARTS)
    return [
        {"label": "Set the lift part to restart on launch", "keys": keys},
        {
            "label": "Song edits made while playing land on the launch boundary",
            "keys": ":quantize edits launch<Enter>",
        },
    ]


# (bar to act in, label, keys). Gestures happen half a bar in; section keys
# land at the end of the current pass, everything else on the next downbeat.
# Lane keys: q Cell, w Pulse, e Prism, r Weight, t Spark, y Breath, u Hands,
# i Lift.
PERFORMANCE = [
    (3, "Pulse waits for Seed's pass to end", "2"),
    (7, "Weight: the tonic bass joins", "3"),
    (13, "A clap build in eighths over Weight", "u2"),
    (15, "Sixteenths and the noise lift; Prism waits for the pass", "u3i14"),
    (16, "Release the build as Prism arrives", "uuii"),
    (23, "Unfold: the breakdown exposes the 64-step harmony", "5"),
    (27, "Live edit: lift Harmonic Current's third quarter", ':value "Harmonic Current" 33 4<Enter>'),
    (31, "Current: rolling bass under the edited harmony", "6"),
    (35, "Drop the air for two bars", "e<m-m>"),
    (37, "Bring it back", "e<m-m>"),
    (39, "Crest", "7"),
    (43, "Undo the harmony edit, on the bar", ":undo<Enter>"),
    (47, "Landing: withdraw to cell, kick and bass", "8"),
    (55, "Afterglow", "9"),
]
END_BAR = 61


def perform_steps():
    steps = [
        {
            "label": "Follow playback, loop Seed, and arm take recording",
            "keys": "tftltr",
            "wait_ms": 1000,
        },
        {"label": "Jam", "keys": "gj", "expect": {"mode": "JAM"}, "wait_ms": 1000},
    ]
    first = PERFORMANCE[0][0] - 1 + 0.5
    steps.append(
        {
            "label": "Seed: the five-step cell alone",
            "keys": " ",
            "advance": f"{round(first * 16)} steps",
            "wait_ms": round(first * BAR_MS),
        }
    )
    position = first
    for index, (bar, label, keys) in enumerate(PERFORMANCE):
        target = PERFORMANCE[index + 1][0] - 1 + 0.5 if index + 1 < len(PERFORMANCE) else END_BAR - 1
        bars = target - position
        typing = len(keys) * KEY_DELAY * 1000
        steps.append(
            {
                "label": label,
                "keys": keys,
                "advance": f"{round(bars * 16)} steps",
                "wait_ms": max(150, round(bars * BAR_MS - typing)),
            }
        )
        position = target
    steps.append(
        {
            "label": "Save the recorded take after one pass of Afterglow",
            "keys": ":take save examples/fivefold-live/takes/set.take.jsonl<Enter>",
            "wait_ms": 600,
        }
    )
    steps.append({"label": "Pause, leave Jam and return to the song", "keys": " <Esc><Home>", "wait_ms": 1200})
    return steps


def write(path: Path, steps):
    path.write_text("".join(json.dumps(step) + "\n" for step in steps))


def run(binary, *args):
    result = subprocess.run([str(binary), *args], cwd=ROOT, capture_output=True, text=True)
    if result.returncode != 0:
        raise SystemExit(result.stdout + result.stderr)
    return result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--source", type=Path, default=SOURCE)
    args = parser.parse_args()
    binary = args.binary.resolve()
    source = args.source.resolve()
    assert binary.is_file(), f"binary not found: {binary}"
    assert source.is_file(), f"source project not found: {source}"
    write(HERE / "setup.keys.jsonl", setup_steps())
    run(
        binary,
        "--project", str(source.relative_to(ROOT)),
        "--sample-roots-only",
        "--key-script", str((HERE / "setup.keys.jsonl").relative_to(ROOT)),
        "--output", str((HERE / "project.json").relative_to(ROOT)),
        "--check",
    )
    write(HERE / "perform.keys.jsonl", perform_steps())
    run(
        binary,
        "--project", str((HERE / "project.json").relative_to(ROOT)),
        "--sample-roots-only",
        "--key-script", str((HERE / "perform.keys.jsonl").relative_to(ROOT)),
    )
    print(f"Wrote {HERE / 'project.json'}, {HERE / 'perform.keys.jsonl'}, and {HERE / 'takes/set.take.jsonl'}")


if __name__ == "__main__":
    main()
