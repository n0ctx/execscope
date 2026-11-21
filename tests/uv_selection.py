#!/usr/bin/env python3
"""Compare the reported project environment with a real offline uv run."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--uv", default="uv")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="execscope uv smoke ") as tmp:
        root = Path(tmp).resolve()
        app = root / "workspace"
        app.mkdir()
        (app / "pyproject.toml").write_text('[project]\nname = "execscope-uv-fixture"\nversion = "0.0.0"\nrequires-python = ">=3.10"\n', encoding="utf-8")
        subdir = app / "nested"
        subdir.mkdir()
        env = {k: v for k, v in os.environ.items() if not k.upper().startswith(("UV_", "PYTHON", "CONDA_", "VIRTUAL_ENV"))}
        env.update(UV_OFFLINE="1", UV_PYTHON_DOWNLOADS="never", UV_PROJECT_ENVIRONMENT=".uv-fixture", UV_CACHE_DIR=str(root / "uv-cache"))

        def run(*cmd, cwd=app):
            return subprocess.run(cmd, cwd=cwd, env=env, check=True, capture_output=True, text=True, encoding="utf-8", timeout=20).stdout.strip()

        run(args.uv, "venv", "--no-config", "--offline", "--no-python-downloads", "--python", sys.executable, ".uv-fixture")
        actual = run(args.uv, "run", "--no-sync", "--offline", "--no-python-downloads", "python", "-I", "-c", "import sys; print(sys.executable)", cwd=subdir)
        data = json.loads(run(str(binary), "inspect", "--scope", "dev", "--detail", "full", "--requires", "python", "--cwd", str(subdir)))
        python = data["preflight"]["requirements"][0]
        context = python["managed"]["uv_project"]
        def normalized(path):
            # Compare the invocation paths without resolving the venv's python symlink.
            return os.path.normcase(os.path.abspath(str(path).removeprefix("\\\\?\\")))
        assert context["state"] == "existing_project_interpreter_candidate", context
        assert normalized(context["executable"]) == normalized(actual), (context, actual)
        assert normalized(python["selected_invocation"]) != normalized(actual), python
    print("ExecScope and real offline uv chose the same project environment; process PATH remained separate.")


if __name__ == "__main__":
    main()
