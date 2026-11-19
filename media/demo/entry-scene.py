#!/usr/bin/env python3
"""Times and judges the scenes a person types by hand.

`start` and `finish` run from the shell's PS0 and prompt hook, once either side
of each command the scene types. `finish` also keeps the command's exit status.
`score` runs afterwards, under the tape's Hide, and looks at what the commands
left behind in the scratch home instead of trusting the screen:

- first-run: the installer left a binary that reports the version this checkout
  builds, the scrape after it exited 0, and no key was in the environment or on
  disk when it did.
- setup: both runs exited 0, and each client that keeps a JSON file now names
  the server in it.
- device-login: the command exited 0 and left a pending code. The summary keeps
  when that code expires and nothing else about it, because the code is on
  screen and a recording is only publishable once it is dead.

- store-to-rows: every command exited 0, the crawl refused nothing, and the file
  it wrote holds the thousand products the store has, each with a title and a
  price. The time it is scored on is the crawl's own, out of its closing report.

It writes `media/out/<scene>.summary.json` for judge.py and `<scene>.exit` for
render.sh.
"""
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "media/out"
HOME = Path("/tmp/spider-demo")
JSON_CLIENTS = [
    ".cursor/mcp.json",
    ".codeium/windsurf/mcp_config.json",
    "Library/Application Support/Claude/claude_desktop_config.json",
]


def lines(scene, name):
    path = OUT / f"{scene}.{name}"
    return path.read_text().split("\n")[:-1] if path.exists() else []


def now():
    return int(time.time() * 1000)


def start(scene):
    with open(OUT / f"{scene}.started", "a") as handle:
        handle.write(f"{now()}\n")


def finish(scene, status):
    # The prompt also returns once before the first command, when `open` ends.
    # A finish with no start in front of it belongs to no command.
    if len(lines(scene, "finished")) >= len(lines(scene, "started")):
        return
    with open(OUT / f"{scene}.finished", "a") as handle:
        handle.write(f"{now()} {status}\n")


def version():
    text = (ROOT / "Cargo.toml").read_text()
    return re.search(r'^version = "(.*)"$', text, re.M).group(1)


def names_server(path):
    try:
        return "spider" in json.loads(path.read_text()).get("mcpServers", {})
    except (OSError, ValueError):
        return False


def store(work):
    """What the crawl wrote: how many rows are products, and its closing report."""
    rows, report = 0, {}
    try:
        text = (Path(work) / "books.ndjson").read_text()
    except OSError:
        return rows, report
    for line in text.splitlines():
        try:
            record = json.loads(line)
        except ValueError:
            continue
        if record.get("type") == "report":
            report = record
            continue
        fields = record.get("fields") or {}
        if (fields.get("title") or [None])[0] and (fields.get("price") or [None])[0]:
            rows += 1
    return rows, report


def score(scene, work):
    started = [int(line) for line in lines(scene, "started")]
    finished = [line.split() for line in lines(scene, "finished")]
    # The command that closes the scene starts the clock once more and never
    # stops it, so only the pairs count.
    runs = [(int(stop) - begin, int(status))
            for begin, (stop, status) in zip(started, finished)]
    summary = {
        "scene": scene,
        "elapsed_ms": sum(spent for spent, _ in runs),
        "commands": len(runs),
        "clean": bool(runs) and all(status == 0 for _, status in runs),
    }

    credentials = (HOME / ".spider/credentials").exists()
    if scene == "first-run":
        binary = HOME / ".local/bin/spider-agent"
        said = ""
        if binary.exists():
            said = subprocess.run([str(binary), "--version"], capture_output=True,
                                  text=True, env={"SPIDER_AGENT_NO_UPDATE": "1"}).stdout
        summary["installed"] = said.strip() == f"spider-agent {version()}"
        summary["keyless"] = not credentials and not any(
            os.environ.get(name) for name in ("SPIDER_API_KEY", "SPIDER_CLOUD_API_KEY"))
        usable = summary["commands"] == 2 and summary["installed"] and summary["keyless"]
    elif scene == "setup":
        summary["clients"] = sum(names_server(HOME / name) for name in JSON_CLIENTS)
        usable = summary["commands"] == 2 and summary["clients"] == len(JSON_CLIENTS)
    elif scene == "store-to-rows":
        rows, report = store(work)
        summary.update(rows=rows, served=report.get("served"),
                       refused=report.get("refused"))
        if report.get("elapsed_ms") is not None:
            summary["elapsed_ms"] = report["elapsed_ms"]
        usable = summary["commands"] == 5 and bool(report)
    elif scene == "device-login":
        pending = HOME / ".spider/device-login.json"
        try:
            summary["expires_at"] = int(json.loads(pending.read_text())["expires_at"])
        except (OSError, ValueError, KeyError, TypeError):
            summary["expires_at"] = None
        summary["signed_in"] = credentials
        usable = (summary["commands"] == 1 and summary["expires_at"] is not None
                  and not credentials)
    else:
        raise SystemExit(f"unknown scene {scene}")

    (OUT / f"{scene}.summary.json").write_text(json.dumps(summary) + "\n")
    (OUT / f"{scene}.exit").write_text(f"{0 if usable and summary['clean'] else 1}\n")


if __name__ == "__main__":
    action, scene = sys.argv[1], sys.argv[2]
    if action == "start":
        start(scene)
    elif action == "finish":
        finish(scene, sys.argv[3])
    elif action == "score":
        score(scene, sys.argv[3] if len(sys.argv) > 3 else str(HOME))
    else:
        raise SystemExit(f"unknown action {action}")
