#!/usr/bin/env python3
"""Sample an existing Linux process without changing its lifecycle or IPC state."""

import argparse
import csv
import json
import math
import os
from pathlib import Path
import platform
import shutil
import signal
import subprocess
import sys
import time
from datetime import datetime, timezone


def parse_stat(text, ticks_per_second):
    """Parse /proc/PID/stat even when comm contains spaces or parentheses."""
    fields = text.rsplit(")", 1)[1].split()  # fields[0] is field 3 (state)
    return {
        "cpu_seconds": (int(fields[11]) + int(fields[12])) / ticks_per_second,
        "minor_faults": int(fields[7]),
        "major_faults": int(fields[9]),
        "threads": int(fields[17]),
        "start_ticks": int(fields[19]),
    }


def parse_kib(text):
    """Read the byte-valued counters from status or smaps_rollup."""
    result = {}
    for line in text.splitlines():
        key, _, value = line.partition(":")
        parts = value.split()
        if len(parts) == 2 and parts[1] == "kB":
            result[key] = int(parts[0]) * 1024
    return result


def read_sample(pid, ticks_per_second):
    """Collect process CPU, memory, fault, and thread counters from procfs."""
    root = Path(f"/proc/{pid}")
    sample = parse_stat((root / "stat").read_text(), ticks_per_second)
    status = parse_kib((root / "status").read_text())
    sample.update(
        rss_bytes=status.get("VmRSS", 0),
        peak_rss_bytes=status.get("VmHWM", 0),
        swap_bytes=status.get("VmSwap", 0),
        pss_bytes=None,
        private_bytes=None,
    )
    try:
        memory = parse_kib((root / "smaps_rollup").read_text())
    except PermissionError:
        pass  # RSS and CPU still work when smaps is restricted.
    else:
        sample["rss_bytes"] = memory.get("Rss", sample["rss_bytes"])
        sample["pss_bytes"] = memory.get("Pss")
        if "Private_Clean" in memory and "Private_Dirty" in memory:
            sample["private_bytes"] = memory["Private_Clean"] + memory["Private_Dirty"]
    return sample


def positive_float(value):
    """Reject durations that cannot form a finite sampling schedule."""
    number = float(value)
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError("must be finite and greater than zero")
    return number


def profile(args):
    """Write samples, reproducibility metadata, and an aggregate summary."""
    ticks = os.sysconf("SC_CLK_TCK")
    initial = read_sample(args.pid, ticks)
    if args.warmup:
        time.sleep(args.warmup)
    first = read_sample(args.pid, ticks)
    if first["start_ticks"] != initial["start_ticks"]:
        raise RuntimeError("target PID was reused during warmup")

    args.output.mkdir(parents=True, exist_ok=False)
    metadata = {
        "pid": args.pid,
        "process_name": Path(f"/proc/{args.pid}/comm").read_text().strip(),
        "start_ticks": first["start_ticks"],
        "recorded_at": datetime.now(timezone.utc).isoformat(),
        "kernel": platform.release(),
        "machine": platform.machine(),
        "logical_cpus": os.cpu_count(),
        "clock_ticks_per_second": ticks,
        "requested_duration_seconds": args.duration,
        "interval_seconds": args.interval,
        "warmup_seconds": args.warmup,
        "cpu_percent_basis": "100% = one fully occupied logical CPU; all target threads included",
    }
    (args.output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")

    profiler = None
    profiler_log = None
    perf_error = None
    samples = []
    start = time.monotonic()
    try:
        if args.cpu:
            profiler_log = (args.output / "perf.log").open("w")
            if not shutil.which("perf"):
                perf_error = "perf is not installed"
                profiler_log.write(perf_error + "\n")
            else:
                try:
                    profiler = subprocess.Popen(
                        ["perf", "record", "-e", "cpu-clock:u", "-F", "99", "--call-graph",
                         "dwarf,16384", "-p", str(args.pid), "-o", str(args.output / "perf.data")],
                        stdout=profiler_log, stderr=subprocess.STDOUT,
                    )
                except OSError as error:
                    perf_error = str(error)
                    profiler_log.write(perf_error + "\n")

        with (args.output / "samples.csv").open("w", newline="") as output:
            writer = csv.DictWriter(output, fieldnames=["elapsed_seconds", "cpu_percent", *first])
            writer.writeheader()
            previous = None
            while True:
                try:
                    sample = read_sample(args.pid, ticks)
                except (FileNotFoundError, ProcessLookupError):
                    break
                if sample["start_ticks"] != first["start_ticks"]:
                    break  # Never silently switch to a different process after PID reuse.
                now = time.monotonic()
                sample["elapsed_seconds"] = now - start
                sample["cpu_percent"] = None
                if previous:
                    sample["cpu_percent"] = 100 * (
                        sample["cpu_seconds"] - previous["cpu_seconds"]
                    ) / (sample["elapsed_seconds"] - previous["elapsed_seconds"])
                writer.writerow(sample)
                output.flush()
                samples.append(sample)
                previous = sample
                remaining = args.duration - (now - start)
                if remaining <= 0:
                    break
                time.sleep(min(args.interval, remaining))
    finally:
        if profiler is not None and profiler.poll() is None:
            profiler.send_signal(signal.SIGINT)  # Stop perf only; leave the target running.
            try:
                profiler.wait(timeout=10)
            except subprocess.TimeoutExpired:
                profiler.kill()
                profiler.wait()
        if profiler_log is not None:
            profiler_log.close()

    if not samples:
        raise RuntimeError("target exited before any samples could be collected")
    elapsed = samples[-1]["elapsed_seconds"] - samples[0]["elapsed_seconds"]
    cpu = samples[-1]["cpu_seconds"] - samples[0]["cpu_seconds"]
    summary = {
        "sample_count": len(samples),
        "measured_seconds": elapsed,
        "cpu_seconds": cpu,
        "average_cpu_percent": 100 * cpu / elapsed if elapsed else None,
        "minor_faults": samples[-1]["minor_faults"] - samples[0]["minor_faults"],
        "major_faults": samples[-1]["major_faults"] - samples[0]["major_faults"],
    }
    for key in ("rss_bytes", "pss_bytes", "private_bytes", "swap_bytes", "threads"):
        values = [sample[key] for sample in samples if sample[key] is not None]
        summary[f"peak_sampled_{key}"] = max(values) if values else None
        summary[f"final_{key}"] = samples[-1][key]
    summary["process_lifetime_peak_rss_bytes"] = samples[-1]["peak_rss_bytes"]
    summary["perf_exit_code"] = profiler.returncode if profiler else None
    summary["perf_error"] = perf_error
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary, indent=2))
    print(f"Profile saved to {args.output}")
    if perf_error or (profiler and profiler.returncode != 0):
        raise RuntimeError(f"perf failed; see {args.output / 'perf.log'} (counter samples were saved)")


def main():
    """Parse profiling options; profiling never sends signals to the target."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, required=True)
    parser.add_argument("--duration", type=positive_float, default=30)
    parser.add_argument("--interval", type=positive_float, default=1)
    parser.add_argument("--warmup", type=positive_float, default=0,
                        help="seconds to wait before sampling (default: 0)")
    parser.add_argument("--cpu", action="store_true", help="also record CPU stacks using perf")
    parser.add_argument("--output", type=Path, help="new directory for results")
    args = parser.parse_args()
    if sys.platform != "linux" or args.pid <= 0:
        parser.error("requires Linux and a positive PID")
    if args.output is None:
        stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
        args.output = Path("perf-results") / f"{stamp}-{args.pid}"
    try:
        profile(args)
    except (OSError, RuntimeError) as error:
        parser.exit(1, f"profile: {error}\n")


if __name__ == "__main__":
    main()
