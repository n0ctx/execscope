#!/usr/bin/env python3
"""Check the pinned Windows Node probe on a native x64 test machine."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


SHA256 = "86fc4785327db5f5c7558dc6156c543b433c313e813a222e088086222ab4afa6"


def check(binary, node, root):
    env = {**os.environ, "PATH": str(node.parent), "PATHEXT": ".EXE",
           "NODE_OPTIONS": "--require=./must-not-load.cjs"}
    proc = subprocess.run([str(binary), "inspect", "--scope", "system", "--cwd", str(root),
                           "--requires", "node", "--detail", "full"], env=env, cwd=root,
                          timeout=10, check=True, text=True, encoding="utf-8", capture_output=True)
    return json.loads(proc.stdout)["preflight"]["requirements"][0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("node", type=Path, help="Pinned Node installed in a protected Program Files/nodejs directory")
    args = parser.parse_args()
    if os.name != "nt":
        parser.error("Windows is required")
    binary = args.binary.resolve(strict=True)
    node = args.node.resolve(strict=True)
    with node.open("rb") as stream:
        assert hashlib.file_digest(stream, "sha256").hexdigest() == SHA256, "Not the pinned release binary"
    with tempfile.TemporaryDirectory(prefix="execscope Windows probe ") as tmp:
        cwd = Path(tmp)
        (cwd / "must-not-load.cjs").write_text("throw new Error('should never load');", encoding="utf-8")
        positive = check(binary, node, cwd)
        assert positive["outcome"] == "ready", positive
        assert positive["verified_version"] == "24.22.3", positive
        assert positive["source"] == "windows_digest_and_contained_identity", positive
        copy = cwd / "node.exe"
        shutil.copyfile(node, copy)
        negative = check(binary, copy, cwd)
        assert negative["outcome"] == "unverified", negative
        assert negative["reason_code"] == "outside_trusted_install_roots", negative
        assert "verified_version" not in negative, negative
    print("Pinned Node passed the contained version check; the same bytes in the project directory were not launched.")


if __name__ == "__main__":
    main()
