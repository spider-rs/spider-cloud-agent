"""Run the entire live inventory and require every test to pass."""
import datetime
import json
import os
from pathlib import Path
import re
import subprocess
from urllib.parse import urlsplit

EXPECTED_COUNT = 9


def fail(message):
    raise SystemExit(message)


def main():
    for name in ("SPIDER_API_KEY", "SPIDER_API_URL", "SPIDER_SERVICE_REVISION"):
        if not os.environ.get(name, "").strip():
            fail(f"{name} must be explicitly set and non-empty")
    endpoint = urlsplit(os.environ["SPIDER_API_URL"])
    if endpoint.scheme not in ("http", "https") or not endpoint.hostname:
        fail("SPIDER_API_URL must be an explicit HTTP or HTTPS endpoint")
    command = ["cargo", "test", "--locked", "-p", "spider-cloud-agent",
               "--features", "full", "--test", "live", "--"]
    listing = subprocess.run(command + ["--ignored", "--list", "--format", "terse"],
                             text=True, capture_output=True)
    if listing.returncode:
        fail("could not list live tests:\n" + listing.stderr)
    inventory = re.findall(r"^(\S+): test$", listing.stdout, re.MULTILINE)
    if len(inventory) != EXPECTED_COUNT or len(set(inventory)) != EXPECTED_COUNT:
        fail(f"live inventory changed: expected {EXPECTED_COUNT}, got {len(inventory)}; review the gate")

    record_dir = Path("target/live-verification")
    record_dir.mkdir(parents=True, exist_ok=True)
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    record_path = record_dir / (stamp + ".json")
    record = {
        "timestamp": stamp,
        "service_revision": os.environ["SPIDER_SERVICE_REVISION"],
        # Omit user info, query and fragment so endpoint credentials cannot enter the record.
        "service_endpoint": endpoint._replace(netloc=endpoint.netloc.rsplit("@", 1)[-1],
                                              query="", fragment="").geturl(),
        "revision_source": "operator supplied from the deployment serving SPIDER_API_URL",
        "client_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
        "inventory": inventory,
        "status": "started",
    }
    record_path.write_text(json.dumps(record, indent=2) + "\n")
    print(f"Live release record: {record_path}", flush=True)
    run = subprocess.run(command + ["--ignored", "--format", "pretty", "--color", "never", "--test-threads=1"],
                         text=True, capture_output=True)
    results = re.findall(r"^test (\S+) \.\.\. (ok|FAILED|ignored)\s*$", run.stdout, re.MULTILINE)
    record["results"] = dict(results)
    record["exit_code"] = run.returncode
    valid = (len(results) == EXPECTED_COUNT and set(dict(results)) == set(inventory)
             and all(dict(results).get(name) == "ok" for name in inventory)
             and run.returncode == 0)
    record["status"] = "passed" if valid else "failed"
    record_path.write_text(json.dumps(record, indent=2) + "\n")
    # Do not copy response bodies or credentials from a failed test into release logs.
    for name, status in results:
        print(f"{name}: {status}")
    print(f"Executed {len(results)}/{EXPECTED_COUNT}; record: {record_path}")
    if not valid:
        fail("live gate failed: inventory or results changed; inspect the service")
    print(f"{len(results)} passed; 0 failed; 0 skipped")


if __name__ == "__main__":
    main()
