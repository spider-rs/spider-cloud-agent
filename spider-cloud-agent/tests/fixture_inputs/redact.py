"""Rebuild the F6 HTTP response fixtures through xtask, never by direct writes.

Run from the repository root:
    python3 spider-cloud-agent/tests/fixture_inputs/redact.py

The JSON inputs here are synthetic contract examples, not live recordings. Their
HTTP envelope keeps status and headers out of the body given to the decoder.
Older body fixtures remain unchanged; their new envelopes identify their routes.
"""

import json
from pathlib import Path
import subprocess
import tempfile


inputs = Path(__file__).resolve().parent
fixtures = inputs.parent / "fixtures"


def redact(source, output):
    subprocess.run(
        ["cargo", "run", "--locked", "-p", "xtask", "--", "redact",
         str(source), "-o", str(output)],
        check=True,
    )


for source in sorted(inputs.glob("*.json")):
    redact(source, fixtures / source.name)

for route, name in [
    ("/scrape", "scrape_markdown"),
    ("/crawl", "crawl_mixed"),
    ("/search", "search_results"),
    ("/screenshot", "screenshot"),
    ("/data/credits", "credits"),
    ("/data/crawl_logs", "crawl_logs"),
]:
    record = {
        "route": route,
        "http_status": 200,
        "headers": {"content-type": "application/json"},
        "body": json.loads((fixtures / (name + ".json")).read_text()),
    }
    with tempfile.TemporaryDirectory() as directory:
        source = Path(directory) / "response.json"
        source.write_text(json.dumps(record))
        redact(source, fixtures / (name + "_response.json"))
