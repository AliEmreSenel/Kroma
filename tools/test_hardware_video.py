"""Opt-in tests for GPU-decoded video shader inputs and explicit CPU fallback."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

CLI = os.environ.get("KROMA_TEST_CLI")
DAEMON = os.environ.get("KROMA_TEST_DAEMON")
ENABLED = CLI and DAEMON and os.environ.get("KROMA_TEST_HW_VIDEO") == "1"
HARDWARE_MODE = os.environ.get("KROMA_TEST_VIDEO_BACKEND", "auto")


@unittest.skipUnless(ENABLED, "set KROMA_TEST_CLI, KROMA_TEST_DAEMON and KROMA_TEST_HW_VIDEO=1")
class HardwareVideoTests(unittest.TestCase):
    def fixture(self, root, codec="libx264", inputs=1):
        source = root / "source.mkv"
        subprocess.run([
            "ffmpeg", "-nostdin", "-v", "error", "-f", "lavfi", "-i",
            "color=red:s=1920x1080:r=30:d=1", "-vf",
            "drawbox=x=960:y=0:w=960:h=1080:color=blue:t=fill",
            "-an", "-c:v", codec, "-pix_fmt", "yuv420p", "-threads", "1", str(source),
        ], check=True)
        (root / "video.frag").write_text(
            "#version 450\nlayout(set=1,binding=0) uniform texture2D first;\n"
            "layout(set=1,binding=1) uniform sampler first_sampler;\n"
            + ("layout(set=1,binding=2) uniform texture2D second;\n"
               "layout(set=1,binding=3) uniform sampler second_sampler;\n" if inputs == 2 else "")
            + "layout(location=0) out vec4 color;\nvoid main(){"
            "vec2 uv=gl_FragCoord.xy/vec2(1920,1080);"
            "vec3 value=texture(sampler2D(first,first_sampler),uv).rgb;"
            + ("value=(value+texture(sampler2D(second,second_sampler),uv).rgb)*0.5;" if inputs == 2 else "")
            + "color=vec4(value.bgr,1.0);}\n"
        )
        config = (
            "[meta]\nname='Hardware video shader test'\nauthor='Kroma'\n"
            "[states.active]\nlength=0\n[states.active.textures.scene]\n"
            "type='shader'\nshader='video.frag'\nwidth=1920\nheight=1080\n"
        )
        for index, name in enumerate(["first", "second"][:inputs]):
            config += (
                f"[states.active.textures.scene.textures.{name}]\n"
                f"type='video'\nsource='source.mkv'\nlooping=true\nbinding={index}\n"
            )
        (root / "config.toml").write_text(config)
        package = root.parent / (root.name + ".shade")
        subprocess.run([CLI, "pack", str(root), "--output", str(package)], check=True, capture_output=True)
        return package

    def preview(self, root, package, mode):
        root.mkdir()
        for name in ["config/kroma", "cache", "run"]:
            (root / name).mkdir(parents=True)
        (root / "run").chmod(0o700)
        (root / "config/kroma/config.toml").write_text(
            f"current_shade={json.dumps(str(package))}\ntarget_fps=60\n"
            f"video_decode={json.dumps(mode)}\n"
            "[runtime]\npersist_current_shade=false\n"
        )
        poster = root / "poster.jpg"
        job = root / "job.json"
        job.write_text(json.dumps({"thumbnail": str(poster), "video": None,
                                  "width": 640, "height": 360, "fps": 4, "seconds": 0.5}))
        environment = dict(os.environ, XDG_CONFIG_HOME=str(root / "config"),
                           XDG_CACHE_HOME=str(root / "cache"), XDG_RUNTIME_DIR=str(root / "run"),
                           XDG_CURRENT_DESKTOP="KDE", XDG_SESSION_TYPE="wayland", KROMA_PREVIEW_JOB="1",
                           KROMA_HEADLESS_SIZE="1920x1080",
                           RUST_LOG="info")
        for key in ["DISPLAY", "WAYLAND_DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE"]:
            environment.pop(key, None)
        result = subprocess.run([DAEMON, "--render-preview", str(job)], env=environment,
                                capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        pixels = subprocess.check_output([
            "ffmpeg", "-nostdin", "-v", "error", "-i", str(poster),
            "-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1",
        ])
        return pixels, result.stderr

    def assert_swapped_colors(self, pixels):
        left = pixels[(180 * 640 + 160) * 3: (180 * 640 + 160) * 3 + 3]
        right = pixels[(180 * 640 + 480) * 3: (180 * 640 + 480) * 3 + 3]
        self.assertGreater(left[2], left[0] + 100, "nested shader must turn source red into blue")
        self.assertGreater(right[0], right[2] + 100, "nested shader must turn source blue into red")

    def test_nested_video_uses_hardware_and_matches_software_rendered_pixels(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "nested"; source.mkdir()
            package = self.fixture(source)
            hardware, log = self.preview(root / "hardware", package, HARDWARE_MODE)
            self.assertIn("HARDWARE VIDEO DECODING:", log)
            self.assertNotIn("SOFTWARE VIDEO DECODING:", log)
            software, log = self.preview(root / "software", package, "software")
            self.assertIn("*** SOFTWARE VIDEO DECODING:", log)
            self.assertEqual(hardware, software)
            self.assert_swapped_colors(hardware)

    def test_two_video_inputs_share_the_rendering_device_safely(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "multiple"; source.mkdir()
            package = self.fixture(source, inputs=2)
            pixels, log = self.preview(root / "hardware", package, HARDWARE_MODE)
            self.assertGreaterEqual(log.count("HARDWARE VIDEO DECODING:"), 2)
            self.assertNotIn("SOFTWARE VIDEO DECODING:", log)
            self.assert_swapped_colors(pixels)

    def test_paused_reloads_and_unload_release_nested_video_workers(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "nested"; source.mkdir()
            package = self.fixture(source)
            sequence = root / "sequence.json"
            sequence.write_text(json.dumps([
                {"label": "paused-reloads", "command": {"type": "Pause"}, "seconds": 0.1, "reloads": 4},
                {"label": "resumed-reloads", "command": {"type": "Resume"}, "seconds": 1},
                {"label": "unload", "command": {"type": "UnloadShade"}, "seconds": 1},
                {"label": "unloaded", "seconds": 1},
                {"label": "restore", "path": str(package), "seconds": 1},
                {"label": "restored", "seconds": 1},
                {"label": "final-unload", "command": {"type": "UnloadShade"}, "seconds": 1},
                {"label": "final-unloaded", "seconds": 1},
            ]))
            output = root / "profile"
            environment = dict(os.environ)
            result = subprocess.run([
                os.sys.executable, str(Path(__file__).with_name("profile_daemon.py")),
                "--binary", DAEMON, "--media", str(package), "--size", "1920x1080",
                "--duration", "1", "--pause-duration", "0.2", "--rounds", "1",
                "--video-decode", HARDWARE_MODE,
                "--sequence", str(sequence), "--output", str(output),
            ], env=environment, capture_output=True, text=True, timeout=90)
            log = (output / "daemon-0.log").read_text()
            self.assertEqual(result.returncode, 0, result.stderr + log)
            self.assertIn("HARDWARE VIDEO DECODING:", log)
            self.assertNotIn("SOFTWARE VIDEO DECODING:", log)
            phases = {phase["label"]: phase["measurement"] for phase in
                      json.loads((output / "results.json").read_text())["rounds"][0]["phases"]}
            for label in ["unloaded", "final-unloaded"]:
                self.assertEqual(phases[label]["status"]["current_phase"], "terminal")
                self.assertFalse(any(thread["name"].startswith("kroma-video-")
                                     for thread in phases[label]["threads"]), label)
            self.assertTrue(any(thread["name"].startswith("kroma-video-")
                                for thread in phases["restored"]["threads"]))

    def test_unsupported_codec_warns_and_keeps_shader_video_working(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "fallback"; source.mkdir()
            package = self.fixture(source, codec="ffv1")
            pixels, log = self.preview(root / "software", package, HARDWARE_MODE)
            self.assertIn("*** SOFTWARE VIDEO DECODING:", log)
            self.assertIn("Hardware decoding unavailable:", log)
            self.assertNotIn("HARDWARE VIDEO DECODING:", log)
            self.assert_swapped_colors(pixels)


if __name__ == "__main__":
    unittest.main()
