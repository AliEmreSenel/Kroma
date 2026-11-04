"""Tests for procfs parsing and sampling a real, isolated process."""

import argparse
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

import profile as profiler


class ProfileTests(unittest.TestCase):
    def test_stat_with_spaces_and_closing_parentheses_in_name(self):
        fields = ["S"] + ["0"] * 49
        for index, value in {7: 13, 9: 2, 11: 120, 12: 30, 17: 4, 19: 12345}.items():
            fields[index] = str(value)
        sample = profiler.parse_stat("42 (worker ) name) " + " ".join(fields), 100)
        self.assertEqual(sample["cpu_seconds"], 1.5)
        self.assertEqual(sample["minor_faults"], 13)
        self.assertEqual(sample["major_faults"], 2)
        self.assertEqual(sample["threads"], 4)
        self.assertEqual(sample["start_ticks"], 12345)

    def test_memory_units_and_non_memory_fields(self):
        self.assertEqual(
            profiler.parse_kib("Rss: 12 kB\nPss: 7 kB\nThreads: 3\nName: daemon\n"),
            {"Rss": 12 * 1024, "Pss": 7 * 1024},
        )

    @unittest.skipUnless(sys.platform == "linux", "requires procfs")
    def test_missing_perf_still_saves_counters_with_a_clear_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            args = argparse.Namespace(pid=os.getpid(), duration=0.02, interval=0.01,
                                      warmup=0, cpu=True, output=Path(temp) / "profile")
            with mock.patch.object(profiler.shutil, "which", return_value=None):
                with contextlib.redirect_stdout(io.StringIO()):
                    with self.assertRaisesRegex(RuntimeError, "counter samples were saved"):
                        profiler.profile(args)
            summary = json.loads((args.output / "summary.json").read_text())
            self.assertEqual(summary["perf_error"], "perf is not installed")
            self.assertGreaterEqual(summary["sample_count"], 2)
            self.assertIn("not installed", (args.output / "perf.log").read_text())

    @unittest.skipUnless(sys.platform == "linux", "requires procfs")
    def test_profile_leaves_target_alive_and_writes_cpu_and_memory(self):
        with tempfile.TemporaryDirectory() as temp:
            target = subprocess.Popen([sys.executable, "-c", "while True: pass"])
            try:
                args = argparse.Namespace(pid=target.pid, duration=0.3, interval=0.05,
                                          warmup=0, cpu=False, output=Path(temp) / "profile")
                with contextlib.redirect_stdout(io.StringIO()):
                    profiler.profile(args)
                summary = json.loads((args.output / "summary.json").read_text())
                self.assertGreater(summary["cpu_seconds"], 0)
                self.assertGreater(summary["peak_sampled_rss_bytes"], 0)
                self.assertGreaterEqual(summary["sample_count"], 3)
                self.assertTrue((args.output / "samples.csv").is_file())
                self.assertTrue((args.output / "metadata.json").is_file())
                self.assertIsNone(target.poll())
            finally:
                target.terminate()
                target.wait()


if __name__ == "__main__":
    unittest.main()
