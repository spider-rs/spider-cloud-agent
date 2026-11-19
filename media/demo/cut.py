#!/usr/bin/env python3
"""Joins the standing scenes into one walkthrough, media/out/walkthrough.mp4.

Each scene is the take already in `out/`, played as recorded. It is scaled down
only when it is wider or taller than the frame, never up, and set on the scene
background. The last frame of a scene holds for a moment so its closing line
can be read. A title card of one plain line goes in front of each scene. The
cards carry no figures: every number in the walkthrough is one a scene printed
when it ran.

A scene with no standing take is left out and named, so a walkthrough can be
cut on a machine that could not record all of them.

Prints one JSON line: the scenes used, the ones left out, and the seconds the
walkthrough runs.
"""
import json
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "media/out"
WIDTH, HEIGHT, FPS = 1280, 720, 24
BACKGROUND = "0x10151e"
# A scene's frames carry no padding of their own, so the frame keeps some.
MARGIN = 40
CARD_SECONDS = 2.0
HOLD_SECONDS = 2.0

# The order a newcomer meets things in: install and a first page, the agent
# clients on the machine, a model calling it, then what a run looks like.
SCENES = [
    ("first-run", "Install it. The first page needs no account."),
    ("store-to-rows", "Signed in, one command turns a whole store into rows."),
    ("setup", "One command adds it to the agent clients you have."),
    ("mcp-in-claude", "Claude Code searches the web and reads what it finds."),
    ("claude-calls-agent", "Or from a shell: the agent reads the schema and picks the call."),
]

TAPE = """Output {frames}/

Set Shell "bash"
Set FontSize 30
Set FontFamily "Menlo"
Set Width {width}
Set Height {height}
Set Padding 32
Set Framerate 24
Set Theme {{ "background": "#10151e", "foreground": "#e0e6ef", "cursorColor": "#10151e", "black": "#10151e", "white": "#e0e6ef" }}

Hide
Type@1ms "bash media/demo/card.sh {text}"
Enter
Sleep 2s
Show
Sleep 500ms
"""


def run(command, **kwargs):
    subprocess.run(command, check=True, **kwargs)


def card(text, directory, index):
    """One title card as a PNG, drawn by vhs in the scene font."""
    # vhs takes an Output path relative to where it runs, and no other kind.
    frames = Path(directory).relative_to(ROOT) / f"card-{index}"
    tape = Path(directory) / f"card-{index}.tape"
    # A tape string cannot hold a double quote, and the shell must see one word.
    quoted = "'" + text.replace("'", "'\\''") + "'"
    tape.write_text(TAPE.format(frames=frames, width=WIDTH, height=HEIGHT, text=quoted))
    run(["vhs", str(tape)], cwd=ROOT, stdout=subprocess.DEVNULL)
    stills = sorted((ROOT / frames).glob("frame-text-*.png"))
    if not stills:
        raise SystemExit(f"vhs drew no frame for the card: {text}")
    return stills[-1]


def fit(label):
    """Onto the frame at its own size, or smaller when it does not fit."""
    return (f"scale=w='min({WIDTH - 2 * MARGIN},iw)':h='min({HEIGHT - 2 * MARGIN},ih)'"
            f":force_original_aspect_ratio=decrease,"
            f"pad={WIDTH}:{HEIGHT}:(ow-iw)/2:(oh-ih)/2:color={BACKGROUND},"
            f"setsar=1,fps={FPS},format=yuv420p[{label}]")


def cut():
    used = [(name, text) for name, text in SCENES if (OUT / f"{name}.mp4").exists()]
    missing = [name for name, _ in SCENES if not (OUT / f"{name}.mp4").exists()]
    if not used:
        raise SystemExit("no standing scene to cut a walkthrough from")

    # The cards are drawn under the ignored frames directory and go with it.
    scratch = OUT / "frames/cards"
    shutil.rmtree(scratch, ignore_errors=True)
    scratch.mkdir(parents=True)
    try:
        inputs, filters, labels = [], [], []
        for index, (name, text) in enumerate(used):
            still = card(text, scratch, index)
            inputs += ["-loop", "1", "-t", str(CARD_SECONDS), "-i", str(still)]
            inputs += ["-i", str(OUT / f"{name}.mp4")]
            filters.append(f"[{2 * index}:v]" + fit(f"c{index}"))
            filters.append(f"[{2 * index + 1}:v]tpad=stop_mode=clone:"
                           f"stop_duration={HOLD_SECONDS}," + fit(f"s{index}"))
            labels += [f"[c{index}]", f"[s{index}]"]
        filters.append("".join(labels) + f"concat=n={len(labels)}:v=1:a=0[out]")

        target = OUT / "walkthrough.mp4"
        run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", *inputs,
             "-filter_complex", ";".join(filters), "-map", "[out]",
             "-c:v", "libx264", "-pix_fmt", "yuv420p", "-movflags", "+faststart",
             str(target)])

        # The poster is the first scene's closing frame, on the same frame.
        run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
             "-i", str(OUT / f"{used[0][0]}.png"),
             "-vf", fit("p").rsplit(",fps=", 1)[0], "-frames:v", "1",
             str(OUT / "walkthrough.png")])
    finally:
        shutil.rmtree(scratch, ignore_errors=True)

    seconds = float(subprocess.run(
        ["ffprobe", "-v", "error", "-show_entries", "format=duration",
         "-of", "csv=p=0", str(target)],
        check=True, capture_output=True, text=True).stdout.strip())
    print(json.dumps({"scenes": [name for name, _ in used], "missing": missing,
                      "seconds": round(seconds, 2)}))


if __name__ == "__main__":
    sys.exit(cut())
