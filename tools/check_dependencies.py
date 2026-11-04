"""Check each backend's selected dependencies; FIPS is forbidden for both."""
import argparse
import json
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--target", required=True)
parser.add_argument("--backend", choices=["reqwest", "toolkit-http"], default="reqwest")
args = parser.parse_args()
tree = subprocess.check_output(["cargo", "tree", "--locked", "--target", args.target,
                                "--no-default-features", "--features", args.backend + "-client", "--edges", "normal,build", "--prefix", "none", "--format", "{p}"], text=True)
forbidden = {"aws-lc-fips-sys", "openssl", "openssl-sys", "libdbus-sys"}
if args.backend == "reqwest":
    forbidden |= {"aws-lc-rs", "aws-lc-sys"}
found = sorted({line.split()[0] for line in tree.splitlines() if line.split() and line.split()[0] in forbidden})
print(json.dumps({"target": args.target, "backend": args.backend, "forbidden_dependencies": found,
                  "ordinary_aws_lc": "aws-lc-rs " in tree, "reqwest_linked": "reqwest " in tree}))
raise SystemExit(bool(found))
