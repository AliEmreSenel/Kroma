"""Prevent partial runs and paused measurements from appearing as playback gains."""

import copy
import json
from pathlib import Path
import tempfile
import unittest

from benchmark_video_compression import summarize


class CompressionSummaryTests(unittest.TestCase):
    def record(self):
        phases = []
        for label, cpu, rss in [("initial", 20, 100), ("paused", 0, 10), ("resumed", 30, 120)]:
            phases.append({"label": label, "measurement": {
                "cpu_percent": cpu, "peak_sampled_rss_bytes": rss * 1048576,
                "status": {"fps": 30},
            }})
        return {"rounds": [{"exit_code": 0, "startup_to_render_loop_ms": 200, "phases": phases}]}

    def test_pause_is_excluded_from_active_cpu_and_memory(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "results.json"
            path.write_text(json.dumps(self.record()))
            result = summarize([path])
        self.assertEqual(result["cpu_percent_median"], 25)
        self.assertEqual(result["rss_mib_median"], 110)
        self.assertEqual(result["paused_cpu_percent_median"], 0)
        self.assertEqual(result["status_fps_samples"], [30, 30])

    def test_failed_partial_and_unexpected_phase_runs_are_rejected(self):
        valid = self.record()
        failed = copy.deepcopy(valid)
        failed["rounds"][0]["exit_code"] = 1
        partial = copy.deepcopy(valid)
        partial["rounds"][0]["phases"].pop()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "results.json"
            for invalid in [{"rounds": [], "current_round": valid["rounds"][0]}, failed, partial]:
                with self.subTest(record=invalid):
                    path.write_text(json.dumps(invalid))
                    with self.assertRaises(ValueError):
                        summarize([path])


if __name__ == "__main__":
    unittest.main()
