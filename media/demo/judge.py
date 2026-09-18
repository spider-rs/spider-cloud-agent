#!/usr/bin/env python3
"""Decides whether a take is usable, and which take a scene should keep.

`check <scene>` reads the summary the scene wrote and says whether the run did
what the scene claims on screen. A crawl that served two pages instead of three
is not a faster take, it is a different one.

`keep <scene> <takes dir>` compares this run's takes against the one already in
`out/`, copies the winner there and records it in `out/<scene>.take.json`. The
service is slower some hours than others, so a run that only produced slow takes
leaves the standing recording alone. The standing recording is only defended
while the tape and the script behind it are unchanged: edit either and the next
run replaces it whatever the clock says.

A scene that times itself is scored on its own elapsed milliseconds. One that
does not, such as the local payload table, is scored on how long its video runs.
"""
import hashlib
import json
import shutil
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "media/out"

# What the scene has to have done for the recording to be honest.
EXPECTED = {
    "agent-calls-agent": {"served": 2, "refused": 0},
    "streaming-crawl": {"served": 3, "refused": 0},
    "claude-calls-agent": {"served": 1, "refused": 0, "spider_agent": True, "discovered": True,
                           "note": True},
    "codex-calls-agent": {"served": 1, "refused": 0, "spider_agent": True, "discovered": True,
                          "note": True},
}

# Everything that decides what the scene shows. Change one and the standing
# recording no longer describes the same thing.
RECIPE = {
    "agent-calls-agent": ["media/tapes/agent-calls-agent.tape", "media/demo/consume.py"],
    "streaming-crawl": ["media/tapes/streaming-crawl.tape", "media/demo/streaming-crawl.sh",
                        "media/demo/consume.py"],
    "route-and-cost": ["media/tapes/route-and-cost.tape", "media/demo/route-and-cost.sh"],
    "payload-savings": ["media/tapes/payload-savings.tape", "media/demo/payload-savings.py"],
    "claude-calls-agent": ["media/tapes/claude-calls-agent.tape", "media/demo/agent-scene.sh",
                           "media/demo/agent-scene.py"],
    "codex-calls-agent": ["media/tapes/codex-calls-agent.tape", "media/demo/agent-scene.sh",
                          "media/demo/agent-scene.py"],
}


def recipe_sha(scene):
    digest = hashlib.sha256()
    for name in RECIPE[scene]:
        digest.update((ROOT / name).read_bytes())
    return digest.hexdigest()[:16]


def check(scene):
    wanted = EXPECTED.get(scene)
    if wanted is None:
        return True
    path = OUT / f"{scene}.summary.json"
    if not path.exists():
        print(f"discarded: {scene} wrote no summary")
        return False
    summary = json.loads(path.read_text())
    for key, value in wanted.items():
        if summary.get(key) != value:
            print(f"discarded: {scene} {key} was {summary.get(key)}, wanted {value}")
            return False
    print(f"kept: {summary['elapsed_ms']} ms, served {summary['served']}")
    return True


def score(take):
    """Lower is faster."""
    summary = take / "summary.json"
    if summary.exists():
        elapsed = json.loads(summary.read_text()).get("elapsed_ms")
        if elapsed is not None:
            return elapsed
    encoded = take / "encode.json"
    if encoded.exists():
        return json.loads(encoded.read_text())["seconds"] * 1000
    return float("inf")


def keep(scene, directory):
    takes = [path for path in sorted(Path(directory).iterdir())
             if (path / f"{scene}.gif").exists()]
    standing = OUT / f"{scene}.take.json"
    sha = recipe_sha(scene)

    held = None
    if standing.exists() and (OUT / f"{scene}.gif").exists():
        held = json.loads(standing.read_text())
        if held.get("recipe") != sha:
            print(f"{scene}: the tape or its script changed, so the standing take is out")
            held = None

    if not takes:
        if held:
            print(f"{scene}: no usable take this run, keeping {held['score']} ms from {held['recorded']}")
            return 0
        print(f"{scene}: no usable take, and nothing standing")
        return 1

    fastest = min(takes, key=score)
    best = round(score(fastest))
    if held and held["score"] <= best:
        print(f"{scene}: keeping the standing {held['score']} ms take, this run's best was {best} ms")
        return 0

    for suffix in ("gif", "mp4", "png"):
        shutil.copyfile(fastest / f"{scene}.{suffix}", OUT / f"{scene}.{suffix}")
    recorded = json.loads((fastest / "encode.json").read_text())
    standing.write_text(json.dumps({
        "scene": scene,
        "score": best,
        "seconds": recorded["seconds"],
        "recipe": sha,
        "recorded": time.strftime("%Y-%m-%d"),
    }, indent=2) + "\n")
    was = f", replacing {held['score']} ms" if held else ""
    print(f"{scene}: took {best} ms, {recorded['seconds']}s of video{was}")
    return 0


if __name__ == "__main__":
    if len(sys.argv) < 3:
        raise SystemExit(__doc__)
    scene, action = sys.argv[1], sys.argv[2]
    if action == "check":
        sys.exit(0 if check(scene) else 1)
    if action == "keep":
        sys.exit(keep(scene, sys.argv[3]))
    raise SystemExit(f"unknown action {action}")
