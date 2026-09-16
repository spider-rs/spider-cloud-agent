"""Exercise the live gate without credentials, service calls or spent credits."""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("verify_live", Path(__file__).with_name("verify-live.py"))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)
NAMES = [f"live_case_{n}" for n in range(8)] + [gate.EXCEPTION]


class LiveGateTests(unittest.TestCase):
    def run_gate(self, names=NAMES, results=None, key="synthetic", revision="test-revision",
                 endpoint="https://example.com"):
        if results is None:
            results = [(name, "FAILED" if name == gate.EXCEPTION else "ok") for name in NAMES]
        listing = subprocess.CompletedProcess([], 0, "".join(f"{n}: test\n" for n in names), "")
        run = subprocess.CompletedProcess([], 101, "".join(f"test {n} ... {s}\n" for n, s in results), "")
        with tempfile.TemporaryDirectory() as directory:
            previous = Path.cwd()
            try:
                os.chdir(directory)
                with patch.dict(os.environ, {"SPIDER_API_KEY": key, "SPIDER_API_URL": endpoint,
                                             "SPIDER_SERVICE_REVISION": revision}, clear=True), \
                     patch.object(gate.subprocess, "run", side_effect=[listing, run]) as calls, \
                     patch.object(gate.subprocess, "check_output", return_value="client-revision\n"), \
                     contextlib.redirect_stdout(io.StringIO()):
                    gate.main()
                self.assertEqual(calls.call_count, 2)
                for call in calls.call_args_list:
                    argv = call.args[0]
                    self.assertIn("--locked", argv)
                    self.assertIn("--ignored", argv)
                    self.assertEqual(argv[argv.index("--test") + 1], "live")
                records = list(Path("target/live-verification").glob("*.json"))
                return json.loads(records[0].read_text())
            finally:
                os.chdir(previous)

    def test_tracks_failure_separately(self):
        record = self.run_gate()
        self.assertEqual(record["status"], "passed with tracked exception")
        self.assertEqual(record["results"][gate.EXCEPTION], "FAILED")
        self.assertEqual(record["service_revision"], "test-revision")

    def test_missing_key_or_revision_fails(self):
        for values in ({"key": " "}, {"revision": ""}):
            with self.subTest(values=values), self.assertRaises(SystemExit):
                self.run_gate(**values)

    def test_missing_or_invalid_endpoint_fails(self):
        for endpoint in ["", " ", "example.com", "ftp://example.com"]:
            with self.subTest(endpoint=endpoint), self.assertRaises(SystemExit):
                self.run_gate(endpoint=endpoint)

    def test_empty_inventory_fails(self):
        with self.assertRaises(SystemExit):
            self.run_gate(names=[])

    def test_zero_executed_fails(self):
        with self.assertRaises(SystemExit):
            self.run_gate(results=[])

    def test_other_failure_or_skip_fails(self):
        for status in ["FAILED", "ignored"]:
            results = [(n, "FAILED" if n == gate.EXCEPTION else "ok") for n in NAMES]
            results[0] = (NAMES[0], status)
            with self.subTest(status=status), self.assertRaises(SystemExit):
                self.run_gate(results=results)

    def test_exception_passing_requires_review(self):
        with self.assertRaises(SystemExit):
            self.run_gate(results=[(n, "ok") for n in NAMES])

    def test_duplicate_result_fails(self):
        with self.assertRaises(SystemExit):
            self.run_gate(results=[(NAMES[0], "ok")] * 8 + [(gate.EXCEPTION, "FAILED")])


if __name__ == "__main__":
    unittest.main()
