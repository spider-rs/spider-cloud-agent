#!/usr/bin/env python3
"""Reads back what a coding agent CLI printed and scores the take.

The scene shows the agent finding its way: a `spider-agent schema` call, the
invocation it settled on after reading that, and the four numbers out of the
run report. All three are parsed here rather than trusted, so a take where the
agent talked its way around the task, skipped the schema, or ran something that
was never a spider-agent fetch is thrown away instead of published.

The fetch has to carry `--selectors`. That is the shape the scene claims, and
it is also what keeps the frames publishable: `--goal metadata` answers with an
account id in every record.

`note` runs from the shell's prompt hook when the invocation returns. It prints
the one line on screen the agent did not write, marked `[recorder]` so nobody can
read it as the model's own answer, and every number in it comes off a clock this
script read or a usage block the CLI itself wrote down. In the scene where
Claude Code calls the tool instead of a shell, that line says which tool the
model called and what the server answered, read out of Claude Code's own
transcript. `score` runs afterwards,
under the tape's Hide, and writes `media/out/<scene>.summary.json` for judge.py
and `<scene>.exit` for render.sh.
"""
import json
import os
import re
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "media/out"
COMMAND = re.compile(r"\bspider-agent\b")
DISCOVERY = re.compile(r"\bspider-agent\s+schema\b")
FETCH = re.compile(r"\bspider-agent\b.*--selectors\b")
ACCOUNT = re.compile(r"--goal[= ]metadata\b|user_id")
MARK = "[recorder]"
TOOL_SCENE = "mcp-in-claude"
TOOL = "mcp__spider__spider_scrape"


def read(path):
    return path.read_text() if path.exists() else ""


def scan(scene):
    """The commands the agent printed, and the report line under them."""
    lines = [line.strip() for line in read(OUT / f"{scene}.stdout").splitlines()]
    commands = [line for line in lines if COMMAND.search(line)]
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
    return commands, report


def stamp(scene, name):
    values = read(OUT / f"{scene}.{name}").split()
    return int(values[0]) if values else None


def elapsed_ms(scene):
    """How long the invocation ran, keystroke to exit.

    The tape sleeps a fixed stretch afterwards so a slow run still fits, and
    none of that belongs in the number a take is scored on.
    """
    started, finished = stamp(scene, "started"), stamp(scene, "finished")
    if started is None or finished is None:
        return None
    return finished - started


def claude_transcripts(work):
    """Every transcript Claude Code wrote for the scratch directory, parsed."""
    found = []
    fresh = time.time() - 1800
    for path in (Path.home() / ".claude" / "projects").glob("*/*.jsonl"):
        try:
            if path.stat().st_mtime < fresh:
                continue
            lines = path.read_text().splitlines()
        except OSError:
            continue
        records = []
        for line in lines:
            try:
                records.append(json.loads(line))
            except ValueError:
                continue
        here = any(os.path.realpath(record.get("cwd") or "/") == work
                   for record in records)
        if here:
            found.append(records)
    return found


def claude_tokens(work):
    """Everything Claude Code recorded for this run in its own transcript.

    One transcript per working directory, and the scratch directory is new, so
    the run is the only thing in it. Usage lands on the transcript twice for
    some turns, hence the keying by message id.
    """
    runs = []
    for records in claude_transcripts(work):
        usage = {}
        for record in records:
            message = record.get("message") or {}
            if record.get("type") == "assistant" and message.get("usage"):
                usage[message.get("id")] = message["usage"]
        runs.append(usage)

    # The run leaves one transcript. Anything else the directory picked up,
    # such as the copy a side session keeps, repeats turns this one already
    # counted, so the longest transcript is the run and the rest are dropped.
    usage = max(runs, key=len, default={})
    total = sum(
        entry.get("input_tokens", 0)
        + entry.get("cache_creation_input_tokens", 0)
        + entry.get("cache_read_input_tokens", 0)
        + entry.get("output_tokens", 0)
        for entry in usage.values()
    )
    return total or None


def tool_calls(work):
    """The spider tool calls in the transcript, and the last report one returned.

    Claude Code prints only its answer under -p, so the call itself never
    reaches the screen. The transcript holds both halves: the call the model
    made, and the JSON document the server answered with, closing report
    included.
    """
    calls, report, cut = {}, {}, False
    for records in claude_transcripts(work):
        for record in records:
            content = (record.get("message") or {}).get("content")
            if not isinstance(content, list):
                continue
            for block in content:
                if not isinstance(block, dict):
                    continue
                if block.get("type") == "tool_use" and block.get("name") == TOOL:
                    calls[block.get("id")] = True
                if block.get("type") != "tool_result" or block.get("tool_use_id") not in calls:
                    continue
                answer = block.get("content")
                if isinstance(answer, list):
                    answer = "".join(part.get("text", "") for part in answer
                                     if isinstance(part, dict))
                try:
                    parsed = json.loads(answer or "")
                except (TypeError, ValueError):
                    continue
                if isinstance(parsed, dict) and isinstance(parsed.get("report"), dict):
                    report = parsed["report"]
                    # A model that caps the answer can get a page cut short,
                    # and then it has nothing to answer from.
                    cut = cut or "[truncated " in (answer or "")
    return len(calls), report, cut


def tool_note(scene, work):
    """The line under the answer, saying what the model called to get it."""
    calls, report, cut = tool_calls(work)
    (OUT / f"{scene}.tool.json").write_text(json.dumps({
        "calls": calls, "served": report.get("served"), "refused": report.get("refused"),
        "whole": bool(report) and not cut,
    }) + "\n")
    if not calls:
        return 0
    times = "once" if calls == 1 else f"{calls} times"
    said = f"claude called spider_scrape {times}."
    if report:
        said += (f" The tool answered {report.get('served')} served, "
                 f"{report.get('refused')} refused, in {report.get('elapsed_ms')} ms.")
    print(MARK, said, flush=True)
    return 0


def codex_tokens(work):
    """The usage block Codex closes its own event stream with."""
    total = None
    for line in read(Path(work) / "turn.jsonl").splitlines():
        try:
            record = json.loads(line)
        except ValueError:
            continue
        entry = record.get("usage")
        if isinstance(entry, dict):
            total = entry.get("input_tokens", 0) + entry.get("output_tokens", 0)
    return total or None


def fetch_ms(scene):
    """The run report, once the copy of the screen has caught up with it."""
    for _ in range(30):
        report = scan(scene)[1]
        if report:
            return report.get("elapsed_ms")
        time.sleep(0.05)
    return None


def note(scene, work):
    """The cost line, printed by the prompt hook once the invocation returns."""
    if stamp(scene, "started") is None:
        return 0
    (OUT / f"{scene}.finished").write_text(
        f"{int(time.time() * 1000)}\n")
    if scene == TOOL_SCENE:
        return tool_note(scene, work)

    driver = "codex" if "codex" in scene else "claude"
    tokens = codex_tokens(work) if driver == "codex" else claude_tokens(work)
    spent = elapsed_ms(scene)
    fetch = fetch_ms(scene)

    said = f"{driver} took {spent / 1000:.1f} s" if spent else f"{driver} took"
    if tokens:
        said += f" and {tokens:,} tokens"
    said += " to decide."
    took = f" The fetch took {fetch} ms and no model tokens." if fetch else \
        " The fetch cost no model tokens."
    print(MARK, said + took, flush=True)
    return 0


def score_tool(scene, status):
    path = OUT / f"{scene}.tool.json"
    tool = json.loads(path.read_text()) if path.exists() else {}
    path.unlink(missing_ok=True)
    summary = {
        "scene": scene,
        "elapsed_ms": elapsed_ms(scene),
        "served": tool.get("served"),
        "refused": tool.get("refused"),
        "tool": bool(tool.get("calls")),
        "whole": bool(tool.get("whole")),
        "note": MARK in read(OUT / f"{scene}.stdout"),
    }
    (OUT / f"{scene}.summary.json").write_text(json.dumps(summary) + "\n")
    usable = (status == 0 and summary["tool"] and summary["note"]
              and summary["served"] is not None)
    (OUT / f"{scene}.exit").write_text(f"{0 if usable else 1}\n")
    return 0


def score(scene, status):
    if scene == TOOL_SCENE:
        return score_tool(scene, status)
    commands, report = scan(scene)
    printed = "\n".join(commands)
    summary = {
        "scene": scene,
        "elapsed_ms": elapsed_ms(scene),
        "served": report.get("served"),
        "refused": report.get("refused"),
        "commands": len(commands),
        "discovered": any(DISCOVERY.search(line) for line in commands),
        "note": MARK in read(OUT / f"{scene}.stdout"),
        "spider_agent": (any(FETCH.search(line) for line in commands)
                         and not ACCOUNT.search(printed)),
    }
    (OUT / f"{scene}.summary.json").write_text(json.dumps(summary) + "\n")

    usable = (status == 0 and summary["spider_agent"] and summary["note"]
              and summary["served"] is not None)
    (OUT / f"{scene}.exit").write_text(f"{0 if usable else 1}\n")
    return 0


if __name__ == "__main__":
    action = sys.argv[1]
    if action == "note":
        try:
            raise SystemExit(note(sys.argv[2], os.path.realpath(sys.argv[3])))
        except SystemExit:
            raise
        except Exception:
            raise SystemExit(0)
    if action == "score":
        raise SystemExit(score(sys.argv[2], int(sys.argv[3])))
    raise SystemExit(f"unknown action {action}")
