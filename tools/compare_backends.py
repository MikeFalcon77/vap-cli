"""Compare measured release artifacts; optionally verify both against the same read-only live API."""
import argparse
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--live-profile")
args = parser.parse_args()
dist = ROOT / "dist"
report = {"release_profile": {"opt_level": "z", "lto": True, "codegen_units": 1,
                              "panic": "abort", "strip": True, "windows_crt": "static"}, "platforms": {}}

def canonical(value):
    if isinstance(value, dict):
        return {k: canonical(v) for k, v in sorted(value.items())}
    if isinstance(value, list):
        return sorted((canonical(v) for v in value), key=lambda v: json.dumps(v, sort_keys=True))
    return value
for target in ["x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"]:
    extension = ".exe" if "windows" in target else ""
    rows = {}
    for backend in ["reqwest", "toolkit-http"]:
        path = dist / f"vap-{backend}-{target}{extension}.metrics.json"
        if path.exists():
            rows[backend] = json.loads(path.read_text(encoding="utf-8"))
    if len(rows) == 2:
        assert rows["reqwest"]["rustc"] == rows["toolkit-http"]["rustc"], "Compiler versions differ"
        difference = rows["toolkit-http"]["binary_bytes"] - rows["reqwest"]["binary_bytes"]
        report["platforms"][target] = {"measurements": rows, "toolkit_delta_bytes": difference,
                                     "toolkit_delta_percent": round(100 * difference / rows["reqwest"]["binary_bytes"], 2)}

if args.live_profile:
    assert os.name == "nt", "Native Windows artifacts are used for the imported Windows keychain profile"
    environment = {k: v for k, v in os.environ.items() if not k.startswith("VAP_")}
    commands = [["env", "list"], ["projects", "list"], ["s3", "list", "--project", "673"],
                ["billing", "quotas"], ["billing", "pricing"], ["env", "templates"], ["env", "regions"]]
    summaries = {}
    for backend in ["reqwest", "toolkit-http"]:
        binary = dist / f"vap-{backend}-x86_64-pc-windows-msvc.exe"
        summary = {}
        for command in commands:
            process = subprocess.run([str(binary), *command, "--profile", args.live_profile, "--read-only", "--json", "--timeout", "60"],
                                     capture_output=True, text=True, encoding="utf-8", env=environment, timeout=90)
            response = json.loads(process.stdout)
            if process.returncode or not response.get("ok"):
                raise RuntimeError(json.dumps({"backend": backend, "command": command[:2], "error": response.get("error")}))
            data = response["data"]
            key = " ".join(command[:2])
            if key == "env list":
                value = {i["env"]["envName"]: sorted(n["id"] for n in i["nodes"]) for i in data}
            elif key == "projects list":
                value = sorted((p["name"], str(p["engineProjectId"]), p.get("isDefault")) for p in data["responses"])
            elif key == "s3 list":
                value = sorted((g["hostGroup"]["hostGroupUniqueName"], b["name"]) for g in data["bucketsList"] for b in g["buckets"])
            elif key == "billing quotas":
                # Preserve duplicate names at different policy scopes; response order is unspecified.
                value = canonical([{ "quota":q["quota"]["name"], "type":q["type"], "value":q["value"]} for q in data["array"]])
            elif key == "billing pricing":
                value = canonical({p["hardNodeGroup"]: p["pricingModel"] for p in data["array"]})
            elif key == "env templates":
                value = sorted(p["nodeType"] for p in data["array"])
            else:
                value = sorted(p["uniqueName"] for p in data["array"])
            summary[key] = value
            print(json.dumps({"backend": backend, "command": key, "ok": True}), flush=True)
        summaries[backend] = summary
    report["live_read_comparison"] = {}
    for command in summaries["reqwest"]:
        match = summaries["reqwest"][command] == summaries["toolkit-http"][command]
        report["live_read_comparison"][command] = {"stable_fields_match": match, "items": len(summaries["reqwest"][command])}
        assert match, f"Stable API fields differ for {command}"

report["toolkit_0_8_limitations"] = ["No HttpClientBuilder custom CA injection", "No proxy connector configuration", "No separate connect timeout setting"]
(dist / "http-backend-comparison.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
print(json.dumps(report, indent=2))
