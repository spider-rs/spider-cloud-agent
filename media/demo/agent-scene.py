#!/usr/bin/env python3
"""Reads back what a coding agent CLI printed and scores the take.

The scene shows two lines: the command the agent chose, and the four numbers it
read out of that command's run report. Both are parsed here rather than trusted,
so a take where the agent talked its way around the task, or ran something other
than spider-agent, is thrown away instead of published.

Writes `media/out/<scene>.summary.json` for judge.py and `<scene>.exit` for
render.sh.
"""
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "media/out"
INVOCATION = re.compile(r"\bspider-agent\s+scrape\b")


def read(path):
    return path.read_text() if path.exists() else ""


def scan(scene):
    """The command line and the report line the agent left on screen."""
    lines = [line.strip() for line in read(OUT / f"{scene}.stdout").splitlines()]
    command = next((line for line in lines if INVOCATION.search(line)), "")
    report = {}
    for line in lines:
        if not line.startswith("{"):
            continue
        try:
            parsed = json.loads(line)
        except ValueError:
            continue
        if isinstance(parsed, dict) and "served" in parsed:
            report = parsed
    return command, report


def elapsed_ms(scene):
    """How long the invocation ran, keystroke to exit.

    The tape sleeps a fixed stretch afterwards so a slow run still fits, and
    none of that belongs in the number a take is scored on.
    """
    started = read(OUT / f"{scene}.started").split()
    finished = read(OUT / f"{scene}.finished").split()
    if not started or not finished:
        return None
    return int(finished[0]) - int(started[0])


def main(scene, status):
    command, report = scan(scene)
    summary = {
        "scene": scene,
        "elapsed_ms": elapsed_ms(scene),
        "served": report.get("served"),
        "refused": report.get("refused"),
        "spider_agent": bool(INVOCATION.search(command)),
    }
    (OUT / f"{scene}.summary.json").write_text(json.dumps(summary) + "\n")

    usable = status == 0 and summary["spider_agent"] and summary["served"] is not None
    (OUT / f"{scene}.exit").write_text(f"{0 if usable else 1}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1], int(sys.argv[2])))
