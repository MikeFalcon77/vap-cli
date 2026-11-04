"""Reproduce/audit embedded metadata from official API description exports. No credentials are read."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path


def methods(path):
    result = {}

    def walk(node):
        if node.get("link"):
            result[node["link"].lower()] = node
        for child in node.get("children", []):
            walk(child)

    for node in json.loads(path.read_text(encoding="utf-8"))["data"]:
        walk(node)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--public", type=Path, required=True)
    parser.add_argument("--extended", type=Path, required=True)
    parser.add_argument("--version", default="8.15")
    parser.add_argument("--write", type=Path, help="Explicitly regenerate a gzip catalog")
    parser.add_argument("--catalog", type=Path, default=Path(__file__).resolve().parents[1] / "assets/catalog.json.gz")
    args = parser.parse_args()
    public, extended = methods(args.public), methods(args.extended)
    normalized = []
    for path, node in sorted((public | extended).items()):
        parts = path.split("/")
        normalized.append({
            "path": "/".join(parts[2:]), "namespace": parts[2], "service": parts[3],
            "name": parts[-1], "public": path in public, "doc_url": node.get("docLink"),
            "parameters": [{"name": p["name"], "kind": p.get("type", "String"),
                            "required": not p.get("isOptional", True), "aliases": p.get("altName", [])}
                           for p in node.get("parameters", [])],
        })
    catalog = {"format_version": 1, "platform_version": args.version,
               "source": f"Installed official Jelastic CLI {args.version}: api.json + private_api.json",
               "methods": normalized}
    encoded = json.dumps(catalog, ensure_ascii=False, separators=(",", ":")).encode()
    if args.write:
        args.write.write_bytes(gzip.compress(encoded, mtime=0))
    expected = json.loads(gzip.decompress(args.catalog.read_bytes()))
    identical = expected == catalog
    print(json.dumps({"identical": identical, "methods": len(normalized), "public_methods": len(public),
                      "sha256": {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                                 for p in [args.public, args.extended]}}, indent=2))
    if not identical:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
