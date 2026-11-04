"""Opt-in live tests. Mutations are restricted to one newly-created, journaled environment."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
STATE = ROOT / "dist/live-smoke-state.json"


def save(state):
    STATE.parent.mkdir(exist_ok=True)
    tmp = STATE.with_suffix(".tmp")
    tmp.write_text(json.dumps(state, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    tmp.replace(STATE)


class Live:
    def __init__(self, binary, profile):
        self.binary = str(Path(binary).resolve())
        self.profile = profile
        self.env = {k: v for k, v in os.environ.items() if not k.startswith("VAP_")}
        self.state = json.loads(STATE.read_text(encoding="utf-8")) if STATE.exists() else {"checks": []}

    def call(self, args, data=None, write=False, timeout=90, record=True, output=None):
        if write:
            name = self.state.get("environment", "")
            assert name.startswith("vap-cli-smoke-") and name not in self.state["baseline_names"]
            assert args[:2] == ["env", "create"] or ("--env" in args and args[args.index("--env") + 1] == name)
        command = [self.binary, *args, "--profile", self.profile, "--json", "--timeout", str(timeout)]
        command += ["--yes"] if write else ["--read-only"]
        if data is not None:
            command += ["--input", "-"]
        if output:
            command += ["--output", str(output)]
        started = time.monotonic()
        p = subprocess.run(command, input=json.dumps(data, ensure_ascii=False) if data is not None else None,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8",
                           env=self.env, timeout=timeout + 30)
        try:
            response = json.loads(p.stdout)
        except ValueError:
            raise RuntimeError("CLI did not return a JSON envelope") from None
        check = {"command": args[:5], "write": write, "exit": p.returncode,
                 "ok": response.get("ok"), "seconds": round(time.monotonic() - started, 3)}
        if not response.get("ok"):
            check["error"] = response.get("error")
        if record:
            self.state["checks"].append(check)
            save(self.state)
            print(json.dumps(check, ensure_ascii=False), flush=True)
        if p.returncode or not response.get("ok"):
            raise RuntimeError(json.dumps(check, ensure_ascii=False))
        return response["data"]

    def environments(self):
        data = self.call(["env", "list"], record=False)
        assert isinstance(data, list)
        return data

    def owned(self):
        name = self.state["environment"]
        assert name.startswith("vap-cli-smoke-") and name not in self.state["baseline_names"]
        match = next((x for x in self.environments() if x["env"]["envName"] == name), None)
        if match and self.state.get("appid"):
            assert match["env"]["appid"] == self.state["appid"], "Environment identity changed"
        return match

    def read(self):
        if self.state.get("creation_attempted") and not self.state.get("cleanup_complete"):
            raise RuntimeError("Do not replace the baseline while a disposable environment needs cleanup")
        envs = self.environments()
        self.state["baseline_names"] = sorted(x["env"]["envName"] for x in envs)
        self.state["baseline_nodes"] = {x["env"]["envName"]: [n["id"] for n in x["nodes"]] for x in envs}
        self.state["started_utc"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        for info in envs:
            nodes = self.call(["node", "list", "--env", info["env"]["envName"]])
            assert [n["id"] for n in nodes] == [n["id"] for n in info["nodes"]]
        projects = self.call(["projects", "list"])
        project = next(p for p in projects["responses"] if p.get("isDefault"))
        self.state["project"] = str(project["engineProjectId"])
        buckets = self.call(["s3", "list", "--project", self.state["project"]])
        self.state["baseline_buckets"] = sorted(b["name"] for g in buckets["bucketsList"] for b in g.get("buckets", []))
        for group in buckets["bucketsList"]:
            for bucket in group.get("buckets", []):
                self.call(["s3", "objects", "--project", self.state["project"], "--param", f"bucketName={bucket['name']}",
                           "--param", f"hostGroup={group['hostGroup']['hostGroupUniqueName']}", "--param", "maxKeys=5"])
        quotas = self.call(["billing", "quotas"])
        self.state["delete_confirmation_type"] = next(q["value"] for q in quotas["array"] if q["quota"]["name"] == "environment.delete.confirm.type")
        tariffs = self.call(["billing", "pricing"])
        self.state["pricing_groups"] = [p["hardNodeGroup"] for p in tariffs["array"]]
        templates = self.call(["env", "templates"])
        nginx = next(t for t in templates["array"] if t["nodeType"] == "nginxphp-dockerized" and t["enabled"])
        tag = next(t["name"] for t in nginx["tags"] if t.get("isDefault") and t.get("isEnabled"))
        assert "eu1" in nginx["availableRegions"]
        self.state["topology"] = {"env": {"region": "eu1", "displayName": "Disposable VAP CLI smoke test"},
                                  "nodes": [{"nodeType": nginx["nodeType"], "nodeGroup": "cp", "tag": tag, "count": 1,
                                             "fixedCloudlets": 1, "flexibleCloudlets": 4, "diskLimit": 5, "extip": 0}]}
        self.call(["env", "regions"])
        save(self.state)

    def create(self):
        assert self.state.get("baseline_names") is not None
        if self.state.get("creation_attempted"):
            raise RuntimeError("Creation already attempted; inspect status instead of retrying")
        if self.state["delete_confirmation_type"] == 0:
            password()  # Ensure the explicitly supplied credential exists before allocating a resource.
        self.state["environment"] = "vap-cli-smoke-" + uuid.uuid4().hex[:12]
        assert not any(x["env"]["envName"] == self.state["environment"] for x in self.environments())
        self.state["creation_attempted"] = True
        save(self.state)
        print(json.dumps({"event": "create_requested", "environment": self.state["environment"],
                          "node_type": "nginxphp-dockerized", "fixed_cloudlets": 1, "max_cloudlets": 4,
                          "disk_gb": 5, "public_ips": 0}), flush=True)
        self.call(["env", "create", "--name", self.state["environment"], "--disk-gb", "5", "--wait", "--poll-interval", "10"],
                  self.state["topology"], write=True, timeout=1800)
        self.status()

    def status(self):
        info = self.owned()
        if info:
            self.state["appid"] = info["env"]["appid"]
            if info.get("nodes"):
                self.state["node"] = info["nodes"][0]["id"]
            summary = {"environment": self.state["environment"], "status": info["env"]["status"],
                       "nodes": [{k: n.get(k) for k in ("id", "nodeType", "status", "fixedCloudlets", "flexibleCloudlets", "diskLimit")} for n in info["nodes"]]}
        else:
            summary = {"environment": self.state.get("environment"), "absent": True}
        self.state["last_status"] = summary
        save(self.state)
        print(json.dumps(summary), flush=True)
        return info

    def mutate(self, args, data=None, timeout=90):
        info = self.owned()
        assert info is not None, "Owned test environment no longer exists"
        if self.state.get("node"):
            assert self.state["node"] in [n["id"] for n in info.get("nodes", [])]
        return self.call([*args, "--env", self.state["environment"]], data, write=True, timeout=timeout)

    def execute(self, command):
        data = self.mutate(["exec", "--node", str(self.state["node"])], {"commandList": [{"command": command}]})
        # Command execution errors can be nested in per-node responses or exitStatus.
        def check(value):
            if isinstance(value, dict):
                if value.get("result", 0) != 0 or value.get("exitStatus", 0) != 0:
                    raise RuntimeError("Remote command failed inside the disposable environment")
                for v in value.values(): check(v)
            elif isinstance(value, list):
                for v in value: check(v)
        check(data)
        return data

    def exercise(self):
        name, node = self.state["environment"], str(self.state["node"])
        marker = "VAP_CLI_SMOKE_" + uuid.uuid4().hex
        self.state["marker"] = marker
        path = "/tmp/" + name + ".txt"
        renamed = path + ".renamed"
        self.state["test_paths"] = [path, renamed]
        save(self.state)
        assert marker in json.dumps(self.execute(f"printf '%s' '{marker}'"))
        self.mutate(["files", "write", "--node", node], {"path": path, "body": marker + "\nUTF-8: Привет ✓\n"})
        content = self.call(["files", "read", "--env", name, "--node", node], {"path": path})
        assert marker in json.dumps(content, ensure_ascii=False)
        self.mutate(["files", "write", "--node", node], {"path": path, "body": "append-verified\n", "isAppendMode": True})
        content = self.call(["files", "read", "--env", name, "--node", node], {"path": path})
        assert "append-verified" in json.dumps(content)
        self.mutate(["api", "environment", "file", "rename", "--node", node], {"oldPath": path, "newPath": renamed})
        self.call(["files", "list", "--env", name, "--node", node], {"path": "/tmp", "filter": name + "*"})
        self.mutate(["files", "delete", "--node", node], {"path": renamed})
        assert "VAP_FILE_ABSENT" in json.dumps(self.execute(f"test ! -e '{renamed}' && printf VAP_FILE_ABSENT"))
        variable = "VAP_CLI_SMOKE_" + uuid.uuid4().hex[:8].upper()
        self.state["variable"] = variable
        save(self.state)
        self.mutate(["vars", "set", "--node", node], {"vars": {variable: marker}})
        with tempfile.TemporaryDirectory(prefix="vap-cli-live-") as directory:
            raw = Path(directory) / "vars.json"
            self.call(["vars", "list", "--env", name, "--node", node], output=raw)
            assert marker in raw.read_text(encoding="utf-8")
        self.mutate(["vars", "delete", "--node", node], {"vars": [variable]})
        self.mutate(["node", "scale", "--node", node, "--param", "fixedCloudlets=1", "--param", "flexibleCloudlets=5"])
        scaled = self.call(["node", "list", "--env", name])
        assert next(n for n in scaled if n["id"] == int(node))["flexibleCloudlets"] == 5
        self.mutate(["node", "scale", "--node", node, "--param", "fixedCloudlets=1", "--param", "flexibleCloudlets=4"])
        self.mutate(["env", "stop", "--wait", "--poll-interval", "5"], timeout=600)
        assert self.owned()["env"]["status"] == 2
        self.mutate(["env", "start", "--wait", "--poll-interval", "5"], timeout=600)
        assert marker in json.dumps(self.execute(f"printf '%s' '{marker}'"))
        self.state["exercise_complete"] = True
        save(self.state)

    def delete(self):
        info = self.owned()
        if info:
            confirmation = self.state["environment"] if self.state["delete_confirmation_type"] == 1 else password()
            self.mutate(["env", "delete", "--wait", "--poll-interval", "5"], {"password": confirmation}, timeout=900)
        assert self.owned() is None
        current = self.environments()
        assert self.state["baseline_names"] == sorted(x["env"]["envName"] for x in current)
        assert self.state["baseline_nodes"] == {x["env"]["envName"]: [n["id"] for n in x["nodes"]] for x in current}
        buckets = self.call(["s3", "list", "--project", self.state["project"]])
        assert self.state["baseline_buckets"] == sorted(b["name"] for g in buckets["bucketsList"] for b in g.get("buckets", []))
        self.state["cleanup_complete"] = True
        save(self.state)
        print(json.dumps({"event": "cleanup_verified", "environment": self.state["environment"],
                          "baseline_environment_and_node_ids_preserved": True, "baseline_buckets_preserved": True}), flush=True)


def password():
    values = []
    for line in (ROOT / ".env").read_text(encoding="utf-8-sig").splitlines():
        key, separator, value = line.partition("=")
        if separator and key.strip() == "VZ_PWD":
            value = value.strip()
            if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
                value = value[1:-1]
            values.append(value)
    if len(values) != 1 or not values[0]:
        raise RuntimeError("Expected exactly one nonempty VZ_PWD in the explicitly supplied .env")
    return values[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["read", "create", "status", "exercise", "delete"])
    parser.add_argument("--binary", default=str(ROOT / "dist/vap-x86_64-pc-windows-msvc.exe"))
    parser.add_argument("--profile", default="vhc")
    parser.add_argument("--allow-write", action="store_true")
    args = parser.parse_args()
    if args.mode in ("create", "exercise", "delete") and not args.allow_write:
        parser.error("Live mutations require --allow-write and an isolated environment journal")
    live = Live(args.binary, args.profile)
    getattr(live, args.mode)()


if __name__ == "__main__":
    main()
