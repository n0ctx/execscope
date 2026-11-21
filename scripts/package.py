#!/usr/bin/env python3
"""Package an existing native release binary with its usage guide and checksum."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tarfile
import zipfile


TARGETS = {
    "x86_64-pc-windows-msvc", "x86_64-apple-darwin", "aarch64-apple-darwin",
    "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu",
}


def notices(target):
    manifest = Path(__file__).resolve().parents[1] / "Cargo.toml"
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", target,
        "--manifest-path", str(manifest),
    ]))
    root = metadata["resolve"]["root"]
    nodes = {node["id"]: node["deps"] for node in metadata["resolve"]["nodes"]}
    pending, closure = [root], set()
    while pending:
        package = pending.pop()
        if package in closure:
            continue
        closure.add(package)
        pending.extend(dep["pkg"] for dep in nodes[package] if any(k["kind"] != "dev" for k in dep["dep_kinds"]))
    lines = ["ExecScope third-party notices", f"Cargo dependencies for {target}", ""]
    for pkg in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if pkg["id"] == root or pkg["id"] not in closure:
            continue
        directory = Path(pkg["manifest_path"]).parent
        files = {p for p in directory.iterdir() if p.is_file() and p.name.upper().startswith(("LICENSE", "COPYING", "NOTICE", "COPYRIGHT", "UNLICENSE"))}
        if pkg["license_file"]:
            files.add(directory / pkg["license_file"])
        lines.extend(["=" * 72, f'{pkg["name"]} {pkg["version"]} — {pkg["license"] or "See bundled terms"}', pkg["repository"] or "", ""])
        for file in sorted(files):
            lines.extend([file.name, file.read_text(encoding="utf-8", errors="replace"), ""])
    return "\n".join(lines).encode("utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", choices=sorted(TARGETS))
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--out", type=Path, default=Path("dist"))
    args = parser.parse_args()
    windows = "windows" in args.target
    basename = "execscope.exe" if windows else "execscope"
    binary = args.binary or Path("target") / args.target / "release" / basename
    if not binary.is_file():
        parser.error("build the release binary for this target first")
    repo = Path(__file__).resolve().parents[1]
    docs = [repo / "README.md", *sorted((repo / "docs").glob("*.md"))]
    args.out.mkdir(parents=True, exist_ok=True)
    archive = args.out / f"execscope-{args.target}{'.zip' if windows else '.tar.gz'}"
    terms = notices(args.target)
    if windows:
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as out:
            out.write(binary, basename)
            for doc in docs:
                out.write(doc, doc.relative_to(repo).as_posix())
            out.writestr("THIRD-PARTY-NOTICES.txt", terms)
    else:
        with tarfile.open(archive, "w:gz") as out:
            out.add(binary, arcname=basename)
            for doc in docs:
                out.add(doc, arcname=doc.relative_to(repo).as_posix())
            info = tarfile.TarInfo("THIRD-PARTY-NOTICES.txt")
            info.size = len(terms)
            out.addfile(info, io.BytesIO(terms))
    checksum = hashlib.sha256()
    with archive.open("rb") as stream:
        for block in iter(lambda: stream.read(65536), b""):
            checksum.update(block)
    archive.with_name(archive.name + ".sha256").write_bytes(f"{checksum.hexdigest()}  {archive.name}\n".encode("ascii"))
    print(archive)


if __name__ == "__main__":
    main()
