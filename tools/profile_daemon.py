#!/usr/bin/env python3
"""Profile an isolated daemon with real media, cold/warm startup, and IPC lifecycle work."""

import argparse
import importlib.util
import json
import os
import re
import signal
from pathlib import Path
import socket
import subprocess
import time
from types import SimpleNamespace

_spec = importlib.util.spec_from_file_location("kroma_counters", Path(__file__).with_name("profile.py"))
counters = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(counters)


def gpu_snapshot(pid):
    """Read resident memory and engine time once per unique DRM client."""
    memory, engines, seen = {}, {}, set()
    for entry in Path(f"/proc/{pid}/fdinfo").glob("*"):
        try:
            fields = dict(line.split(":", 1) for line in entry.read_text().splitlines() if ":" in line)
        except (OSError, ProcessLookupError):
            continue
        fields = {key: value.strip() for key, value in fields.items()}
        if "drm-client-id" not in fields:
            continue
        client = tuple(fields.get(key) for key in ("drm-driver", "drm-pdev", "drm-client-id"))
        if client in seen:
            continue
        seen.add(client)
        resident = {key.removeprefix("drm-resident-") for key in fields if key.startswith("drm-resident-")}
        for key, value in fields.items():
            parts = value.split()
            if not parts or not parts[0].isdigit():
                continue
            number, unit = int(parts[0]), parts[1] if len(parts) > 1 else ""
            region = None
            if key.startswith("drm-resident-"):
                region = key.removeprefix("drm-resident-")
            elif key.startswith("drm-memory-") and key.removeprefix("drm-memory-") not in resident:
                region = key.removeprefix("drm-memory-")
            if region is not None and unit in ("", "B", "KiB", "MiB"):
                memory[region] = memory.get(region, 0) + number * {"": 1, "B": 1, "KiB": 1024, "MiB": 1048576}[unit]
            elif key.startswith("drm-engine-") and unit == "ns":
                engine = key.removeprefix("drm-engine-")
                engines[engine] = engines.get(engine, 0) + number
    return {"resident_bytes": memory, "engine_ns": engines}


def mapping_snapshot(pid):
    """Attribute resident memory to named mappings and anonymous allocator pages."""
    groups = {}
    current = None
    try:
        lines = Path(f"/proc/{pid}/smaps").read_text().splitlines()
    except OSError:
        return []
    for line in lines:
        fields = line.split()
        if len(fields) >= 5 and re.fullmatch(r"[0-9a-f]+-[0-9a-f]+", fields[0]):
            name = " ".join(fields[5:]) or "[anonymous]"
            current = groups.setdefault(name, {"mapping": name, "rss_bytes": 0, "pss_bytes": 0, "private_bytes": 0})
        elif current is not None and len(fields) == 3 and fields[2] == "kB":
            key = {"Rss:": "rss_bytes", "Pss:": "pss_bytes", "Private_Clean:": "private_bytes", "Private_Dirty:": "private_bytes"}.get(fields[0])
            if key:
                current[key] += int(fields[1]) * 1024
    return sorted(groups.values(), key=lambda row: row["rss_bytes"], reverse=True)[:20]


def thread_snapshot(pid):
    """Read cumulative user/system CPU time for each daemon thread."""
    result = {}
    for entry in Path(f"/proc/{pid}/task").glob("*"):
        try:
            stat = (entry / "stat").read_text()
            value = counters.parse_stat(stat, os.sysconf("SC_CLK_TCK"))
            result[entry.name] = {"name": stat.split("(", 1)[1].rsplit(")", 1)[0], "cpu_seconds": value["cpu_seconds"]}
        except (OSError, ProcessLookupError):
            pass
    return result


class Connection:
    """Keep one IPC connection throughout a run to avoid connection-thread churn."""
    def __init__(self, path):
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(15)
        self.socket.connect(str(path))
        self.reader = self.socket.makefile("r")

    def send(self, command):
        self.socket.sendall(json.dumps(command).encode() + b"\n")

    def request(self, command, expected):
        self.send(command)
        while True:
            line = self.reader.readline()
            if not line:
                raise RuntimeError("daemon disconnected during request")
            response = json.loads(line)
            if response["type"] in ("Error", "LoadRejected"):
                raise RuntimeError(str(response))
            if response["type"] in expected:
                if response.get("success") is False:
                    raise RuntimeError(str(response))
                return response

    def status(self):
        return self.request({"type": "StatusQuery"}, {"Status"})

    def close(self):
        self.reader.close()
        self.socket.close()


def measure(process, connection, seconds):
    """Measure actual daemon CPU, memory, thread activity, and GPU counters."""
    ticks = os.sysconf("SC_CLK_TCK")
    first = counters.read_sample(process.pid, ticks)
    gpu_before, threads_before = gpu_snapshot(process.pid), thread_snapshot(process.pid)
    start, samples = time.monotonic(), []
    while True:
        if process.poll() is not None:
            raise RuntimeError(f"daemon exited with {getattr(process, 'returncode', 'unknown')}")
        row = counters.read_sample(process.pid, ticks)
        row["elapsed_seconds"] = time.monotonic() - start
        samples.append(row)
        remaining = seconds - row["elapsed_seconds"]
        if remaining <= 0:
            break
        time.sleep(min(0.25, remaining))
    elapsed = time.monotonic() - start
    gpu_after, threads_after = gpu_snapshot(process.pid), thread_snapshot(process.pid)
    cpu = samples[-1]["cpu_seconds"] - first["cpu_seconds"]
    threads = []
    for tid, last in threads_after.items():
        if tid in threads_before:
            delta = last["cpu_seconds"] - threads_before[tid]["cpu_seconds"]
            threads.append({"tid": tid, "name": last["name"], "cpu_seconds": delta, "cpu_percent": 100 * delta / elapsed})
    result = {
        "seconds": elapsed, "cpu_seconds": cpu, "cpu_percent": 100 * cpu / elapsed,
        "status": connection.status(), "gpu_before": gpu_before, "gpu_after": gpu_after,
        "threads": sorted(threads, key=lambda row: row["cpu_seconds"], reverse=True), "samples": samples,
    }
    for key in ("rss_bytes", "pss_bytes", "private_bytes", "swap_bytes", "threads"):
        values = [row[key] for row in samples if row[key] is not None]
        result[f"peak_sampled_{key}"] = max(values) if values else None
    return result


def parse_timings(log):
    """Extract opt-in stage durations; no events are emitted per rendered frame."""
    return [{"stage": stage, "elapsed_ms": float(elapsed)}
            for stage, elapsed in re.findall(r"stage=([\w.]+) elapsed_ms=([\d.]+)", log)]


def run(args):
    """Launch only private headless instances; never share the live IPC socket."""
    # Validate inputs before creating the output directory or launching anything.
    if not args.binary.is_file():
        raise ValueError(f"binary does not exist: {args.binary}")
    sequence = json.loads(args.sequence.read_text()) if args.sequence else []
    if not isinstance(sequence, list):
        raise ValueError("sequence must be a JSON list")
    args.output.mkdir(parents=True, exist_ok=False)
    root = args.output.resolve()
    for directory in ("run", "config/kroma", "cache"):
        (root / directory).mkdir(parents=True)
    (root / "run").chmod(0o700)
    config = f"target_fps = 60\ngpu_power = \"low\"\n[runtime]\npersist_current_shade = false\n"
    if args.media:
        config = f"current_shade = {json.dumps(str(args.media.resolve()))}\n" + config
    (root / "config/kroma/config.toml").write_text(config)
    environment = dict(os.environ, XDG_CONFIG_HOME=str(root / "config"),
                       XDG_CACHE_HOME=str(root / "cache"), XDG_RUNTIME_DIR=str(root / "run"),
                       XDG_CURRENT_DESKTOP="KDE", XDG_SESSION_TYPE="wayland",
                       RUST_LOG="info,kroma_profile=info", KROMA_PROFILE_TIMINGS="1",
                       KROMA_HEADLESS_SIZE=args.size)
    environment.pop("DISPLAY", None)
    environment.pop("WAYLAND_DISPLAY", None)
    results = {"binary": str(args.binary.resolve()), "media": str(args.media.resolve()) if args.media else None,
               "mode": f"isolated KDE offscreen backend, {args.size}", "rounds": []}
    for round_index in range(args.rounds):
        log_path = root / f"daemon-{round_index}.log"
        connection = None
        with log_path.open("w") as log:
            start = time.monotonic()
            command = [str(args.binary.resolve())]
            if args.heaptrack:
                command = ["heaptrack", "--record-only", "--output", str(root / f"heap-{round_index}"), *command]
            process = subprocess.Popen(command, env=environment, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            record = {"cache_state": "cold-application-cache" if round_index == 0 else "warm-application-cache", "phases": []}
            try:
                deadline = start + 30
                while "Entering render loop" not in log_path.read_text():
                    if process.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError(f"startup failed; see {log_path}")
                    time.sleep(0.01)
                if f"Created headless virtual monitor: {args.size}" not in log_path.read_text():
                    raise RuntimeError(f"binary did not create the requested {args.size} output; see {log_path}")
                record["startup_to_render_loop_ms"] = (time.monotonic() - start) * 1000
                record["startup_timings"] = parse_timings(log_path.read_text())
                # heaptrack is a wrapper process; sample the daemon child itself.
                if args.heaptrack:
                    children = Path(f"/proc/{process.pid}/task/{process.pid}/children").read_text().split()
                    candidates = [int(pid) for pid in children if Path(f"/proc/{pid}/exe").resolve() == args.binary.resolve()]
                    if len(candidates) != 1:
                        raise RuntimeError("cannot identify heaptrack's daemon child")
                    target = SimpleNamespace(pid=candidates[0], poll=process.poll)
                else:
                    target = process
                record["pid"] = target.pid
                connection = Connection(root / "run/kroma.sock")
                time.sleep(0.3)
                scenarios = [{"label": "initial", "seconds": args.duration},
                             {"label": "paused", "command": {"type": "Pause"}, "seconds": args.pause_duration},
                             {"label": "resumed", "command": {"type": "Resume"}, "seconds": args.duration}]
                scenarios.extend(sequence)
                for scenario in scenarios:
                    phase = {"label": scenario["label"]}
                    log_offset = len(log_path.read_text())
                    command = scenario.get("command")
                    if "path" in scenario:
                        command = {"type": "LoadShade", "path": str(Path(scenario["path"]).resolve()), "force": scenario.get("force", not bool(scenario.get("transition"))),
                                   "transition": scenario.get("transition")}
                        begin = time.monotonic()
                        phase["response"] = connection.request(command, {"ShadeLoaded", "CompileResult", "Ready"})
                        phase["load_response_ms"] = (time.monotonic() - begin) * 1000
                    elif command:
                        connection.send(command)
                    time.sleep(0.2)
                    phase["measurement"] = measure(target, connection, scenario.get("seconds", args.duration))
                    phase["mapping_rss"] = mapping_snapshot(target.pid)
                    phase["reload_response_ms"] = []
                    for _ in range(scenario.get("reloads", args.reloads if "path" in scenario else 0)):
                        begin = time.monotonic()
                        connection.request({"type": "Reload"}, {"ShadeLoaded", "CompileResult", "Ready"})
                        phase["reload_response_ms"].append((time.monotonic() - begin) * 1000)
                    if phase["reload_response_ms"]:
                        phase["post_reload_memory"] = counters.read_sample(target.pid, os.sysconf("SC_CLK_TCK"))
                    phase["timings"] = parse_timings(log_path.read_text()[log_offset:])
                    record["phases"].append(phase)
                    measurement = phase["measurement"]
                    print(f"round={round_index} {phase['label']}: cpu={measurement['cpu_percent']:.2f}% rss={measurement['peak_sampled_rss_bytes']/1048576:.1f}MiB fps={measurement['status']['fps']:.1f}", flush=True)
                    (root / "results.json").write_text(json.dumps({**results, "current_round": record}, indent=2) + "\n")
                connection.send({"type": "Shutdown"})
                process.wait(timeout=10)
                record["exit_code"] = process.returncode
                results["rounds"].append(record)
                (root / "results.json").write_text(json.dumps(results, indent=2) + "\n")
            finally:
                if connection:
                    connection.close()
                # The private session includes profiler wrappers and their child.
                # Clean up the whole owned group on startup/IPC failures too.
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                if process.poll() is None:
                    try:
                        process.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--media", type=Path)
    parser.add_argument("--sequence", type=Path, help="JSON list of labels/paths/commands/durations")
    parser.add_argument("--output", type=Path, required=True, help="new artifact directory")
    parser.add_argument("--duration", type=counters.positive_float, default=5)
    parser.add_argument("--pause-duration", type=counters.positive_float, default=3)
    parser.add_argument("--rounds", type=int, default=2)
    parser.add_argument("--reloads", type=int, default=3)
    parser.add_argument("--heaptrack", action="store_true")
    parser.add_argument("--size", default="1920x1080", help="headless output WIDTHxHEIGHT; requires a binary supporting KROMA_HEADLESS_SIZE")
    args = parser.parse_args()
    try:
        dimensions = args.size.split("x")
        if len(dimensions) != 2 or any(int(value) <= 0 for value in dimensions):
            raise ValueError()
    except ValueError:
        parser.error("size must be positive WIDTHxHEIGHT")
    if args.rounds < 1 or args.reloads < 0:
        parser.error("rounds must be positive and reloads must be nonnegative")
    run(args)


if __name__ == "__main__":
    main()
