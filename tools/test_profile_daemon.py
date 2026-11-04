"""Regression checks for daemon profiling counters, IPC, and launch isolation."""

from pathlib import Path
from types import SimpleNamespace
import json
import socket
import tempfile
import threading
import unittest
from unittest import mock

import profile_daemon as profiler


class DaemonProfileTests(unittest.TestCase):
    def test_gpu_clients_are_deduplicated_and_resident_memory_takes_priority(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            client = "drm-driver: amdgpu\ndrm-pdev: 0000:07:00.0\ndrm-client-id: 3\ndrm-resident-gtt: 10 KiB\ndrm-memory-gtt: 100 KiB\ndrm-memory-vram: 2 MiB\ndrm-engine-gfx: 123 ns\n"
            (root / "1").write_text(client)
            (root / "2").write_text(client)
            (root / "3").write_text(client.replace("client-id: 3", "client-id: 4"))
            with mock.patch.object(profiler, "Path", return_value=root):
                result = profiler.gpu_snapshot(42)
            # Path resolves to root, so its fdinfo glob sees these three fixtures.
            self.assertEqual(result["resident_bytes"], {"gtt": 20480, "vram": 4194304})
            self.assertEqual(result["engine_ns"], {"gfx": 246})

    def test_ipc_requests_skip_unrelated_events_and_report_rejections(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "private.sock"
            server = socket.socket(socket.AF_UNIX)
            server.bind(str(path))
            server.listen(1)
            errors = []

            def respond():
                try:
                    peer, _ = server.accept()
                    with peer, peer.makefile("r") as reader:
                        self.assertEqual(json.loads(reader.readline()), {"type": "StatusQuery"})
                        peer.sendall(b'{"type":"Ready"}\n{"type":"Status","fps":30}\n')
                        self.assertEqual(json.loads(reader.readline()), {"type": "Reload"})
                        peer.sendall(b'{"type":"CompileResult","success":false}\n')
                except BaseException as error:
                    errors.append(error)

            thread = threading.Thread(target=respond)
            thread.start()
            connection = profiler.Connection(path)
            try:
                self.assertEqual(connection.status()["fps"], 30)
                with self.assertRaises(RuntimeError):
                    connection.request({"type": "Reload"}, {"CompileResult"})
            finally:
                connection.close()
                thread.join(timeout=3)
                server.close()
            self.assertFalse(thread.is_alive())
            if errors:
                raise errors[0]

    def test_mapping_rss_groups_anonymous_pages_and_paths_with_spaces(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "smaps"
            path.write_text("1000-2000 rw-p 00000000 00:00 0\nRss: 4 kB\nPss: 3 kB\nPrivate_Dirty: 2 kB\n2000-3000 rw-p 00000000 00:00 0\nRss: 8 kB\nPss: 8 kB\nPrivate_Clean: 8 kB\n3000-4000 r--p 00000000 00:00 4 /path with spaces\nRss: 1 kB\nPss: 1 kB\n")
            with mock.patch.object(profiler, "Path", return_value=path):
                rows = profiler.mapping_snapshot(42)
            self.assertEqual(rows[0], {"mapping": "[anonymous]", "rss_bytes": 12288, "pss_bytes": 11264, "private_bytes": 10240})
            self.assertEqual(rows[1]["mapping"], "/path with spaces")

    def test_stage_timings_preserve_repeated_operations(self):
        rows = profiler.parse_timings("stage=shader.compile elapsed_ms=12.500\nnoise\nstage=shader.compile elapsed_ms=1.250")
        self.assertEqual(rows, [{"stage": "shader.compile", "elapsed_ms": 12.5}, {"stage": "shader.compile", "elapsed_ms": 1.25}])

    def test_invalid_sequence_creates_no_output_or_process(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            sequence = root / "sequence.json"
            sequence.write_text('{}')
            args = SimpleNamespace(binary=Path('/bin/true'), sequence=sequence, output=root / 'output')
            with mock.patch.object(profiler.subprocess, 'Popen') as launch:
                with self.assertRaises(ValueError):
                    profiler.run(args)
                launch.assert_not_called()
            self.assertFalse(args.output.exists())


if __name__ == '__main__':
    unittest.main()
