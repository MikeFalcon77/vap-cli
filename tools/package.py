"""Package one native binary and record size, offline startup and peak memory measurements."""
import argparse
import hashlib
import json
import os
import statistics
import subprocess
import time
import tomllib
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--target", required=True)
parser.add_argument("--binary", type=Path, required=True)
parser.add_argument("--out", type=Path, default=Path("dist"))
parser.add_argument("--backend", choices=["reqwest", "toolkit-http"])
args = parser.parse_args()
version = tomllib.loads((Path(__file__).resolve().parents[1] / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
if os.environ.get("GITHUB_REF_TYPE") == "tag" and os.environ.get("GITHUB_REF_NAME") != f"v{version}":
    raise SystemExit("Release tag must match the Cargo package version")
args.out.mkdir(exist_ok=True)
suffix = ".exe" if "windows" in args.target else ""
name = f"vap-{'%s-' % args.backend if args.backend else ''}{args.target}{suffix}"
binary = args.out / name
binary.write_bytes(args.binary.read_bytes())
binary.chmod(0o755)
times = []
env = {**os.environ, "NO_COLOR": "1", "VAP_CONFIG_DIR": str(args.out.resolve() / "empty-config")}
for _ in range(7):
    started = time.perf_counter()
    subprocess.run([str(binary.resolve()), "--help"], stdout=subprocess.DEVNULL, check=True, env=env)
    times.append((time.perf_counter() - started) * 1000)
peak = None
if os.name != "nt":
    # wait4 measures this child, avoiding inherited/cumulative compiler process statistics.
    process = subprocess.Popen([str(binary.resolve()), "--help"], stdout=subprocess.DEVNULL, env=env)
    _, status, usage = os.wait4(process.pid, 0)
    process.returncode = os.waitstatus_to_exitcode(status)
    if process.returncode:
        raise SystemExit(process.returncode)
    peak = usage.ru_maxrss
    if "apple" not in args.target:
        peak *= 1024
else:
    import ctypes
    from ctypes import wintypes
    class Counters(ctypes.Structure):
        _fields_ = [("cb", wintypes.DWORD), ("PageFaultCount", wintypes.DWORD)] + [
            (field, ctypes.c_size_t) for field in ["PeakWorkingSetSize", "WorkingSetSize", "QuotaPeakPagedPoolUsage", "QuotaPagedPoolUsage", "QuotaPeakNonPagedPoolUsage", "QuotaNonPagedPoolUsage", "PagefileUsage", "PeakPagefileUsage"]]
    process = subprocess.Popen([str(binary.resolve()), "--help"], stdout=subprocess.DEVNULL, env=env)
    process.wait()
    counters = Counters(); counters.cb = ctypes.sizeof(counters)
    get = ctypes.windll.psapi.GetProcessMemoryInfo
    get.argtypes = [wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD]
    if get(wintypes.HANDLE(int(process._handle)), ctypes.byref(counters), counters.cb):
        peak = counters.PeakWorkingSetSize
digest = hashlib.sha256(binary.read_bytes()).hexdigest()
(args.out / f"{name}.sha256").write_text(f"{digest}  {name}\n", encoding="utf-8")
metrics = {"version": version, "target": args.target, "backend": args.backend or "reqwest", "binary_bytes": binary.stat().st_size,
           "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
           "help_median_ms": round(statistics.median(times), 2), "help_peak_memory_bytes": peak,
           "sha256": digest, "measurement": "local process startup including OS overhead; not a cold-cache benchmark"}
(args.out / f"{name}.metrics.json").write_text(json.dumps(metrics, indent=2) + "\n", encoding="utf-8")
print(json.dumps(metrics, indent=2))
