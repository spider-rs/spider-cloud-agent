#!/usr/bin/env python3
"""Turn one scene's captured frames into a GIF, an MP4 and a still.

Three things get cut. The tape sleeps a fixed stretch after Enter so a slow run
still fits, which leaves a long frozen tail on a fast one, and that tail goes.
A wait longer than the cap also gets shortened to the cap, so a slow DNS lookup
does not sit on screen for four seconds. The cap is per scene, because a wait
with output still landing either side of it is not the same as a wait on a
coding agent that prints nothing until it is done. Last, a trailing stretch of
frames nobody held is dropped, so a keystroke meant for the hidden teardown
cannot end the scene. Nothing between the first and last visible change is
dropped, and the millisecond stamps printed on screen are whatever the run
measured.

Prints one JSON line: the frame counts before and after, and the seconds the
encoded scene runs.
"""
import hashlib
import json
import shutil
import subprocess
import sys
from pathlib import Path

CAPTURE_FPS = 24
GIF_FPS = 12
# What a viewer needs to read a line before the next one lands, and to read the
# last line before the scene loops.
MAX_IDLE_SECONDS = 1.5
TAIL_HOLD_SECONDS = 1.5
# vhs stops capturing at the Hide that ends a scene, but one frame is already in
# flight when it does, and the hidden teardown starts typing a millisecond
# later. Roughly one take in twenty ends on that frame, showing the first
# keystroke of a command the scene never meant to show. It is always a single
# frame nobody held. A scene ends on a screen that sat still, so a trailing
# stretch shorter than this is not the end of the scene.
SETTLE_SECONDS = 0.5
# A scene that drives a coding agent waits on a model, twelve seconds and up,
# with nothing arriving on screen meanwhile. That gap holds no output to read,
# so these two hold it shorter than a scene where lines are still landing. The
# other four keep the 1.5 seconds above.
SCENE_MAX_IDLE = {"claude-calls-agent": 0.6, "codex-calls-agent": 0.6}


def digest(path):
    return hashlib.blake2b(path.read_bytes(), digest_size=16).digest()


def settled_end(marks):
    """Where the scene ends once the frames nobody held are off the back."""
    hold = round(SETTLE_SECONDS * CAPTURE_FPS)
    end = len(marks)
    while end > hold:
        start = end - 1
        while start and marks[start - 1] == marks[end - 1]:
            start -= 1
        if end - start >= hold:
            break
        end = start
    return end


def kept_frames(frames, max_idle=MAX_IDLE_SECONDS):
    """The frames to encode, with dead air cut."""
    marks = [digest(frame) for frame in frames]
    settled = settled_end(marks)

    last_change = 0
    for index in range(1, settled):
        if marks[index] != marks[index - 1]:
            last_change = index
    end = min(settled, last_change + 1 + round(TAIL_HOLD_SECONDS * CAPTURE_FPS))

    cap = round(max_idle * CAPTURE_FPS)
    kept, run = [], 0
    for index in range(end):
        run = run + 1 if index and marks[index] == marks[index - 1] else 0
        if run < cap:
            kept.append(frames[index])
    return kept


def encode(name, frames_dir, out_dir):
    frames = sorted(Path(frames_dir).glob("frame-text-*.png"))
    if not frames:
        raise SystemExit(f"no frames captured for {name}")

    kept = kept_frames(frames, SCENE_MAX_IDLE.get(name, MAX_IDLE_SECONDS))
    staged = Path(frames_dir) / "kept"
    shutil.rmtree(staged, ignore_errors=True)
    staged.mkdir()
    for position, frame in enumerate(kept):
        (staged / f"{position:05d}.png").symlink_to(frame.resolve())

    out = Path(out_dir)
    pattern = str(staged / "%05d.png")
    run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
         "-framerate", str(CAPTURE_FPS), "-i", pattern,
         "-filter_complex",
         f"fps={GIF_FPS},split[a][b];[a]palettegen=max_colors=64:stats_mode=diff[p];"
         "[b][p]paletteuse=dither=bayer:bayer_scale=5",
         "-loop", "0", str(out / f"{name}.gif")])
    run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
         "-framerate", str(CAPTURE_FPS), "-i", pattern,
         "-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2",
         "-c:v", "libx264", "-pix_fmt", "yuv420p", "-movflags", "+faststart",
         str(out / f"{name}.mp4")])
    shutil.copyfile(kept[-1], out / f"{name}.png")

    print(json.dumps({"scene": name, "captured": len(frames), "kept": len(kept),
                      "seconds": round(len(kept) / CAPTURE_FPS, 2)}))


def run(command):
    subprocess.run(command, check=True)


if __name__ == "__main__":
    encode(sys.argv[1], sys.argv[2], sys.argv[3])
