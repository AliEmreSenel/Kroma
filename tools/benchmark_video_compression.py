#!/usr/bin/env python3
"""Compare .shade video block compression with byte verification and isolated playback."""

import argparse
import hashlib
import json
from pathlib import Path
import statistics
import subprocess
import sys
import time

from profile_daemon import counters


def summarize(paths):
    """Aggregate completed playback runs, keeping paused samples separate."""
    active, paused, startup = [], [], []
    for path in paths:
        record = json.loads(path.read_text())
        if len(record["rounds"]) != 1 or record["rounds"][0]["exit_code"] != 0:
            raise ValueError(f"incomplete or unsuccessful playback: {path}")
        run = record["rounds"][0]
        if [phase["label"] for phase in run["phases"]] != ["initial", "paused", "resumed"]:
            raise ValueError(f"unexpected playback phases: {path}")
        startup.append(run["startup_to_render_loop_ms"])
        for phase in run["phases"]:
            target = paused if phase["label"] == "paused" else active
            target.append(phase["measurement"])
    if not active:
        raise ValueError("no completed playback samples")
    return {
        "cpu_percent_median": statistics.median(row["cpu_percent"] for row in active),
        "cpu_percent_samples": [row["cpu_percent"] for row in active],
        "rss_mib_median": statistics.median(row["peak_sampled_rss_bytes"] / 1048576 for row in active),
        "rss_mib_samples": [row["peak_sampled_rss_bytes"] / 1048576 for row in active],
        "paused_cpu_percent_median": statistics.median(row["cpu_percent"] for row in paused),
        "startup_to_render_loop_ms_median": statistics.median(startup),
        # Status FPS counts render-loop iterations, not distinct decoded frames.
        "status_fps_samples": [row["status"]["fps"] for row in active],
    }


def run(args):
    """Pack each input unchanged with none/LZ4/Zstd, then alternate playback order."""
    cli, binary = args.cli.resolve(), args.binary.resolve()
    sources = [path.resolve(strict=True) for path in args.media]
    if not cli.is_file() or not binary.is_file():
        raise ValueError("--cli and --binary must name existing executables")
    dimensions = []
    for source in sources:
        probe = json.loads(subprocess.check_output([
            "ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries",
            "stream=width,height", "-of", "json", str(source),
        ], text=True))["streams"][0]
        dimensions.append(f'{probe["width"]}x{probe["height"]}')
    args.output.mkdir(parents=True, exist_ok=False)
    root = args.output.resolve()
    report = {"binary": str(binary), "cli": str(cli), "cases": {}}
    for index, (source, size) in enumerate(zip(sources, dimensions)):
        directory = root / f"input-{index}"
        (directory / "assets").mkdir(parents=True)
        entry = "assets/video" + source.suffix
        (directory / entry).symlink_to(source)
        (directory / "config.toml").write_text(
            "[meta]\nname='Video compression benchmark'\nauthor='Kroma'\n"
            "[states.active]\nlength=0\n[states.active.textures.iChannel0]\n"
            f"type='video'\nsource={json.dumps(entry)}\nlooping=true\nbinding=0\n"
        )
        expected = source.read_bytes()
        for codec in ("none", "lz4", "zstd:3"):
            name = f"input-{index}-{codec.replace(':', '-')}"
            package = root / (name + ".shade")
            command = [str(cli), "pack", str(directory), "--output", str(package),
                       "--entry-compression", entry + "=" + codec]
            pack_seconds = []
            for _ in range(args.pack_rounds):
                start = time.monotonic()
                subprocess.run(command, stdout=subprocess.DEVNULL, check=True)
                pack_seconds.append(time.monotonic() - start)
            extracted = json.loads(subprocess.check_output([
                str(cli), "inspect", str(package), "dump-entry", entry,
                "--full", "--format", "json",
            ], text=True))
            if bytes.fromhex(extracted["preview_hex"]) != expected:
                raise ValueError(f"embedded video differs from input: {package}")
            report["cases"][name] = {
                "source": str(source), "source_bytes": len(expected),
                "source_sha256": hashlib.sha256(expected).hexdigest(),
                "package": str(package), "package_bytes": package.stat().st_size,
                "compression": codec, "output_size": size, "media_bytes_identical": True,
                "pack_seconds_median": statistics.median(pack_seconds),
                "pack_seconds_samples": pack_seconds,
            }
        del expected
    (root / "packages.json").write_text(json.dumps(report, indent=2) + "\n")
    profiler = Path(__file__).with_name("profile_daemon.py")
    with (root / "playback.log").open("w") as log:
        for repetition in range(args.rounds):
            names = list(report["cases"])
            if repetition % 2:
                names.reverse()
            for name in names:
                case = report["cases"][name]
                print(f"playback {repetition + 1}/{args.rounds}: {name}", flush=True)
                subprocess.run([
                    sys.executable, str(profiler), "--binary", str(binary),
                    "--media", case["package"], "--size", case["output_size"],
                    "--output", str(root / f"{name}-run{repetition}"),
                    "--duration", str(args.duration), "--pause-duration", str(args.pause_duration),
                    "--rounds", "1",
                ], stdout=log, stderr=subprocess.STDOUT, check=True)
    for name, case in report["cases"].items():
        case.update(summarize([root / f"{name}-run{n}" / "results.json" for n in range(args.rounds)]))
    (root / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
    print(root / "summary.json")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--media", type=Path, action="append", required=True)
    parser.add_argument("--output", type=Path, required=True, help="new artifact directory")
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--pack-rounds", type=int, default=5)
    parser.add_argument("--duration", type=counters.positive_float, default=4)
    parser.add_argument("--pause-duration", type=counters.positive_float, default=0.5)
    args = parser.parse_args()
    if args.rounds < 1 or args.pack_rounds < 1:
        parser.error("round counts must be positive")
    run(args)


if __name__ == "__main__":
    main()
