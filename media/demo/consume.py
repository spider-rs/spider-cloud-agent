#!/usr/bin/env python3
"""A calling agent: fetch pages, dispatch decisions, and honor the tool exit code."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[2]
BINARY = ROOT / "target/release/spider-agent"


def emit(record):
    print(json.dumps(record, separators=(",", ":")), flush=True)


def scene_name():
    return "streaming-crawl" if sys.argv[1:] == ["--stdin"] else "agent-calls-agent"


def write_summary(**fields):
    """What render.sh scores a take on. Never shown on screen."""
    path = ROOT / f"media/out/{scene_name()}.summary.json"
    path.write_text(json.dumps({"scene": scene_name(), **fields}) + "\n")


def consume(lines, started):
    for line in lines:
        record = json.loads(line)
        kind = record["type"]
        if kind in ("page", "failed"):
            url = urlsplit(record["url"])
            # Dispatch usable pages to the agent's in-memory index.
            usable = kind == "page" and 200 <= record["status"] < 300 and bool(record.get("body"))
            if usable:
                index[record["url"]] = record["body"]
            emit({"ms": round((time.monotonic() - started) * 1000),
                  "action": "index" if usable else "review",
                  "page": url.netloc + url.path,
                  "status": record["status"]})
        elif kind == "report":
            summary = {key: record[key] for key in
                       ("type", "served", "refused", "cost_credits", "stopped")}
            # Credits arrive as a float sum, so trim the binary tail off the display.
            summary["cost_credits"] = round(summary["cost_credits"], 6)
            emit(summary)
            write_summary(elapsed_ms=round((time.monotonic() - started) * 1000),
                          served=record["served"], refused=record["refused"],
                          pages=len(index))
            return True
        elif kind == "error":
            emit({"type": "error", "action": "retry_or_escalate"})
    return False


index = {}


def main():
    started = time.monotonic()
    write_summary(elapsed_ms=None, served=0, refused=0, pages=0)
    if sys.argv[1:] == ["--stdin"]:
        return 0 if consume(sys.stdin, started) else 1
    args = ["scrape", "https://example.com", "https://example.org",
            "--ndjson", "--budget", "2", "--wall", "10",
            "--no-update", "--no-router", "--quiet"]
    emit({"tool": "spider-agent", "argv": args})
    env = dict(os.environ, NO_COLOR="1", SPIDER_AGENT_NO_UPDATE="1")
    with subprocess.Popen([str(BINARY), *args], stdout=subprocess.PIPE,
                          text=True, bufsize=1, env=env) as child:
        try:
            complete = consume(child.stdout, started)
        except (ValueError, KeyError):
            child.terminate()
            child.wait()
            raise
        finally:
            child.stdout.close()
        code = child.wait()
    emit({"report_received": complete, "indexed": len(index), "exit": code})
    return code


if __name__ == "__main__":
    status = main()
    if sys.argv[1:] != ["--stdin"]:
        # Let render.sh check the scene without the tape typing on screen. In
        # --stdin mode the shell script records the pipeline's status instead.
        (ROOT / "media/out/agent-calls-agent.exit").write_text(f"{status}\n")
    sys.exit(status)
