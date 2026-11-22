#!/usr/bin/env python3
"""Independent, black-box MCP stdio client and repeatable timing harness."""
import argparse
import contextlib
import json
import os
from pathlib import Path
from queue import Queue
import statistics
import subprocess
import tempfile
import threading
import time


class Client:
    def __init__(self, binary, version, env=None):
        self.process = subprocess.Popen(
            [str(binary), "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, encoding="utf-8", bufsize=1, env=env,
        )
        self.pending = Queue()
        self.version = version
        self.id = 0
        self.cancelled = set()
        threading.Thread(target=self.read, daemon=True).start()

    def read(self):
        try:
            for line in self.process.stdout:
                self.pending.put(json.loads(line))
        except Exception as exc:
            self.pending.put(exc)
        finally:
            self.pending.put(None)

    def send(self, message):
        self.process.stdin.write(json.dumps({"jsonrpc": "2.0", **message}) + "\n")
        self.process.stdin.flush()

    def submit(self, method, params=None):
        self.id += 1
        params = dict(params or {})
        if self.version == "2026-07-28":
            params["_meta"] = {
                "io.modelcontextprotocol/protocolVersion": self.version,
                "io.modelcontextprotocol/clientInfo": {"name": "execscope-test", "version": "1"},
                "io.modelcontextprotocol/clientCapabilities": {},
            }
        self.send({"id": self.id, "method": method, "params": params})
        return self.id

    def reply(self, ident):
        while True:
            reply = self.pending.get(timeout=8)
            assert isinstance(reply, dict) and reply.get("jsonrpc") == "2.0", reply
            if reply.get("id") in self.cancelled:
                continue
            assert reply.get("id") == ident, reply
            return reply

    def request(self, method, params=None):
        return self.reply(self.submit(method, params))

    def start(self):
        if self.version == "2025-11-25":
            info = self.request("initialize", {
                "protocolVersion": self.version, "capabilities": {},
                "clientInfo": {"name": "execscope-test", "version": "1"},
            })["result"]
            assert info["protocolVersion"] == self.version, info
            self.send({"method": "notifications/initialized"})
        else:
            info = self.request("server/discover")["result"]
            assert self.version in info["supportedVersions"], info
            assert info["resultType"] == "complete", info
        assert "tools" in info["capabilities"], info
        assert not {"prompts", "resources"} & info["capabilities"].keys(), info

    def call(self, arguments=None, error=False):
        params = {"name": "inspect_environment"}
        if arguments is not None:
            params["arguments"] = arguments
        result = self.request("tools/call", params)["result"]
        assert result.get("isError", False) == error, result
        assert "structuredContent" not in result, result
        assert len(result["content"]) == 1 and result["content"][0]["type"] == "text", result
        if self.version == "2026-07-28":
            assert result["resultType"] == "complete", result
        else:
            assert "resultType" not in result, result
        return result["content"][0]["text"]

    def close(self, check):
        self.process.stdin.close()
        try:
            code = self.process.wait(timeout=5)
            stderr = self.process.stderr.read()
            leftover = self.pending.get(timeout=2)
            while isinstance(leftover, dict) and leftover.get("id") in self.cancelled:
                leftover = self.pending.get(timeout=2)
            if check:
                assert code == 0 and stderr == "" and leftover is None, (code, stderr, leftover)
        finally:
            if self.process.poll() is None:
                self.process.kill()
                self.process.wait(timeout=5)
            self.process.stdout.close()
            self.process.stderr.close()


@contextlib.contextmanager
def session(binary, version="2026-07-28", env=None):
    client = Client(binary, version, env=env)
    ok = False
    try:
        client.start()
        yield client
        ok = True
    finally:
        client.close(ok)


def run(binary, *args, check=True, env=None):
    return subprocess.run([str(binary), *args], capture_output=True, text=True, encoding="utf-8", check=check, timeout=8, env=env)


def meaningful(value):
    if isinstance(value, dict):
        return {k: meaningful(v) for k, v in value.items() if k not in {"cached", "probe_cached", "sampled_at", "probe_sampled_at"}}
    if isinstance(value, list):
        return [meaningful(v) for v in value]
    return value


def smoke(binary):
    assert run(binary, "--help").returncode == 0
    assert run(binary, "--version").stdout.startswith("execscope ")
    assert run(binary).stdout == run(binary, "inspect").stdout
    with tempfile.TemporaryDirectory(prefix="execscope space ") as tmp:
        cwd = Path(tmp) / "App 测试"
        cwd.mkdir()
        (cwd / "package.json").write_text('{"engines":{"node":">=99"},"packageManager":"pnpm@10.2.0"}', encoding="utf-8")
        for version in ("2025-11-25", "2026-07-28"):
            with session(binary, version) as c:
                tools = c.request("tools/list")["result"]["tools"]
                assert [t["name"] for t in tools] == ["inspect_environment"], tools
                assert tools[0]["inputSchema"]["additionalProperties"] is False
                assert "cwd=" in c.call()
                assert "preflight=" not in c.call({})
                params = {"scope": "dev", "detail": "full", "cwd": str(cwd), "requires": ["node", "pnpm", "git"]}
                full = json.loads(c.call(params))
                assert full["schema_version"] == "1" and full["environment_source"] == "local_process", full
                assert full["preflight"]["requested"] == params["requires"], full
                assert full["cwd"]["source"] == "explicit" and full["cwd"]["status"] == "available", full
                assert "测试" in full["cwd"]["value"], full
                # The first cold probe on a slow host can exhaust the tools budget; warm the
                # same evidence the server already holds before comparing the two paths.
                parity = ["inspect", "--scope", "dev", "--detail", "full", "--cwd", str(cwd), "--requires", "node,pnpm,git"]
                run(binary, *parity, check=False)
                direct = run(binary, *parity)
                assert meaningful(json.loads(direct.stdout)) == meaningful(full), (direct.stdout, full)
                for invalid in ({"extra": True}, {"scope": "bad"}, {"detail": None}, {"cwd": str(cwd / "missing")}, {"cwd": str(cwd / "package.json")}, {"requires": ["git", "git"]}, {"requires": ["NODE"]}):
                    c.call(invalid, error=True)
                assert "error" in c.request("tools/call", {"name": "not_a_tool"})
                if version == "2025-11-25":
                    ident = c.submit("tools/call", {"name": "inspect_environment", "arguments": {"scope": "all", "cwd": str(cwd), "refresh": True}})
                    c.cancelled.add(ident)
                    c.send({"method": "notifications/cancelled", "params": {"requestId": ident, "reason": "test cancellation"}})
                    # Completion may win the race; a cancelled request need not reply.
                    after = c.request("tools/list")
                    assert len(after["result"]["tools"]) == 1, after
                for scope in ("auto", "dev", "system", "hardware", "all"):
                    report = json.loads(c.call({"scope": scope, "detail": "full", "cwd": str(cwd), "refresh": True}))
                    assert "active_network" not in report and "preflight" not in report, report
                    assert ("system" in report) == (scope != "hardware"), report
                    assert ("project" in report) == (scope in ("auto", "dev", "all")), report
                    assert ("network" in report) == (scope in ("auto", "all")), report
                    assert ("hardware" in report) == (scope in ("auto", "hardware", "all")), report
                assert json.loads(c.call({"requires": [], "detail": "full"}))["preflight"]["status"] == "ready"
                (cwd / "package.json").write_text('{"engines":{"node":"22"}}', encoding="utf-8")
                changed = json.loads(c.call(params))
                assert changed["project"] != full["project"] and "pnpm@10.2.0" not in json.dumps(changed), changed
                (cwd / "package.json").write_text('{"engines":{"node":">=99"},"packageManager":"pnpm@10.2.0"}', encoding="utf-8")
        if os.name != "nt":
            link = cwd.parent / "linked project"
            link.symlink_to(cwd, target_is_directory=True)
            linked = json.loads(run(binary, "inspect", "--scope", "system", "--detail", "full", "--cwd", str(link)).stdout)
            assert "测试" in linked["cwd"]["value"] and "linked project" not in linked["cwd"]["value"], linked
        bad = run(binary, "inspect", "--cwd", str(cwd / "token=not-for-output"), check=False)
        assert bad.returncode != 0 and not bad.stdout and "not-for-output" not in bad.stderr, bad
        private = {**os.environ, "HTTPS_PROXY": "https://private:secret-test-pass@example.org/?token=never-output", "API_TOKEN": "never-output"}
        for detail in ("brief", "full"):
            report = run(binary, "inspect", "--cwd", str(cwd), "--detail", detail, env=private)
            assert "never-output" not in report.stdout and "secret-test-pass" not in report.stdout, report
    print("Passed independent stdio discovery, default and explicit calls, schema/error, scope, cache change, CLI parity, privacy and clean EOF checks (2025-11-25 and 2026-07-28).")
    evidence_cases(binary)


def evidence_cases(binary):
    states = []
    with tempfile.TemporaryDirectory(prefix="execscope fixtures ") as tmp:
        root = Path(tmp)
        cwd = root / "project"
        cwd.mkdir()
        (cwd / ".git").write_text("gitdir: elsewhere\n", encoding="utf-8")
        shell = '.cmd' if os.name == "nt" else ''
        native = '.exe' if os.name == "nt" else ''
        hostile = root / "node_modules" / "corepack" / "dist"
        hostile.mkdir(parents=True)
        proxy = hostile / f"pnpm{shell}"
        proxy.write_text("@echo write>should-not-exist\n" if shell else "#!/bin/sh\ntouch should-not-exist\n", encoding="utf-8")
        proxy.chmod(0o755)
        rustbin = root / ".cargo" / "bin"
        rustbin.mkdir(parents=True)
        rustproxy = rustbin / f"cargo{native}"
        rustproxy.write_bytes(b"untrusted toolchain proxy fixture")
        rustproxy.chmod(0o755)
        store = root / "rustup" / "toolchains"
        store.mkdir(parents=True)
        (store.parent / "settings.toml").write_text('default_host_triple="test-host"\n', encoding="utf-8")
        cache = root / "cache"
        cache.mkdir()
        (cwd / "package.json").write_text('{"packageManager":"pnpm@10.2.0"}', encoding="utf-8")
        (cwd / "rust-toolchain").write_text("1.92\n", encoding="utf-8")
        env = {**os.environ, "PATH": os.pathsep.join([str(hostile), str(rustbin)]), "RUSTUP_HOME": str(store.parent),
               "RUSTUP_TOOLCHAIN": "1.92", "COREPACK_HOME": str(cache), "PATHEXT": ".CMD;.EXE"}
        args = {"cwd": str(cwd), "detail": "full", "scope": "dev", "requires": ["pnpm", "cargo"]}

        def check(client, expected):
            full = json.loads(client.call(args))
            brief = client.call({**args, "detail": "brief"})
            assert full["preflight"]["status"] != "ready", full
            for item, (outcome, reason) in zip(full["preflight"]["requirements"], expected, strict=True):
                assert item["outcome"] == outcome and item["reason_code"] == reason, item
                assert f'{item["name"]}:{outcome}({reason}' in brief, brief
                assert item["stage"] == "selected" and item["selected_invocation"], item
                states.append(outcome)
            return full

        with session(binary, env=env) as client:
            first = check(client, [("blocked", "corepack_not_cached"), ("blocked", "required_runtime_unavailable")])
            assert first["preflight"]["requirements"][0]["selection_context"] == "corepack_static_cache"
            hydration = cache / "v1" / "pnpm" / "10.2.0"
            hydration.mkdir(parents=True)
            (hydration / ".corepack").write_text('{"locator":{"name":"pnpm","reference":"10.2.0"}}', encoding="utf-8")
            (store / "1.92-test-host").mkdir()
            second = check(client, [("unverified", "managed_or_wrapper_not_verified"), ("blocked", "required_toolchain_component_unavailable")])
            assert second["preflight"]["requirements"][0]["managed"]["discovered_version"] == "10.2.0"
            assert second["preflight"]["requirements"][0]["discovered_version"] == "10.2.0"
        assert not (cwd / "should-not-exist").exists()
        assert not (hostile / "should-not-exist").exists()

        nvm = root / "nvm"
        node = nvm / "versions" / "node" / "v20.9.0" / "bin" / f"node{native}"
        node.parent.mkdir(parents=True)
        node.write_bytes(b"fixture executable must not be launched")
        node.chmod(0o755)
        venv = cwd / ".venv"
        python = venv / ("Scripts" if os.name == "nt" else "bin") / f"python{native}"
        python.parent.mkdir(parents=True)
        python.write_bytes(b"fixture interpreter must not be launched")
        python.chmod(0o755)
        (venv / "pyvenv.cfg").write_text("version = 3.12.1\n", encoding="utf-8")
        manager_env = {**env, "PATH": os.pathsep.join([str(node.parent), str(python.parent)]),
                       "NVM_DIR": str(nvm), "VIRTUAL_ENV": str(venv)}
        with session(binary, env=manager_env) as client:
            opts = {"cwd": str(cwd), "scope": "dev", "requires": ["node", "python"]}
            full = json.loads(client.call({**opts, "detail": "full"}))
            for report, (name, version) in zip(full["preflight"]["requirements"], [("node", "20.9.0"), ("python", "3.12.1")], strict=True):
                assert report["name"] == name and report["outcome"] == "unverified", report
                assert report["discovered_version"] == version and "verified_version" not in report, report
                states.append(report["outcome"])
            brief = client.call(opts)
            assert "observed=20.9.0" in brief and "observed=3.12.1" in brief, brief

        bad = run(binary, "inspect", "--scope", "fixture-token=do-not-echo", check=False)
        assert bad.returncode == 2 and not bad.stdout and "do-not-echo" not in bad.stderr
        assert run(binary, "inspect", "--requires", "node,node", check=False).returncode == 2

    print(f"Resolved {len(states)}/{len(states)} deterministic preflight states; "
          f"false_ready=0; unverified_after_static_evidence={states.count('unverified')}; "
          "Corepack and rustup cache/selection changes surfaced in the same MCP session.")


def benchmark(binary, n, cwd, tokenizer, fastfetch):
    rows = {}

    def measure(name, call):
        times = []
        for _ in range(n):
            start = time.perf_counter()
            report = call()
            times.append((time.perf_counter() - start) * 1000)
        ordered = sorted(times)
        rows[name] = {
            "n": n, "median_ms": round(statistics.median(times), 3),
            "p95_ms": round(ordered[(95*n + 99)//100 - 1], 3),
            "p99_ms": round(ordered[(99*n + 99)//100 - 1], 3),
            "characters": len(report), "bytes": len(report.encode("utf-8")),
        }
        if tokenizer:
            import tiktoken
            rows[name]["tokens"] = len(tiktoken.get_encoding("o200k_base").encode(report))
            rows[name]["tokenizer"] = f"tiktoken {tiktoken.__version__} / o200k_base"

    def cold(args):
        # Exclude startup/negotiation from MCP cold timings.
        times = []
        for _ in range(n):
            with session(binary) as c:
                started = time.perf_counter()
                report = c.call(args)
                times.append((time.perf_counter() - started)*1000)
        ordered = sorted(times)
        rows["mcp_cold_auto_brief"] = {"n": n, "median_ms": round(statistics.median(times), 3),
            "p95_ms": round(ordered[(95*n + 99)//100 - 1], 3), "p99_ms": round(ordered[(99*n + 99)//100 - 1], 3),
            "characters": len(report), "bytes": len(report.encode("utf-8"))}
        if tokenizer:
            import tiktoken
            rows["mcp_cold_auto_brief"]["tokens"] = len(tiktoken.get_encoding("o200k_base").encode(report))
            rows["mcp_cold_auto_brief"]["tokenizer"] = f"tiktoken {tiktoken.__version__} / o200k_base"

    base = {"cwd": str(cwd)}
    measure("cli_cold_auto_brief", lambda: run(binary, "inspect", "--cwd", str(cwd)).stdout.removesuffix("\n"))
    cold(base)
    with session(binary) as c:
        c.call(base)
        measure("mcp_warm_auto_brief", lambda: c.call(base))
        measure("mcp_refresh_all_full", lambda: c.call({**base, "scope": "all", "detail": "full", "refresh": True}))
        c.call({**base, "requires": ["node", "git"]})
        measure("mcp_warm_preflight", lambda: c.call({**base, "requires": ["node", "git"]}))
    if fastfetch:
        measure("fastfetch_host_only", lambda: subprocess.run(["fastfetch", "--pipe", "--structure", "OS:CPU:Memory:Disk:GPU", "--format", "json"], capture_output=True, text=True, encoding="utf-8", check=True, timeout=8).stdout.removesuffix("\n"))
    print(json.dumps(rows, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--repeat", type=int, default=0, help="benchmark instead of smoke-testing")
    parser.add_argument("--cwd", type=Path, default=Path.cwd())
    parser.add_argument("--tokenizer", action="store_true", help="measure with developer dependency tiktoken")
    parser.add_argument("--fastfetch", action="store_true", help="measure the installed Fastfetch host-only baseline")
    args = parser.parse_args()
    if args.repeat > 0:
        benchmark(args.binary.resolve(), args.repeat, args.cwd.resolve(), args.tokenizer, args.fastfetch)
    else:
        smoke(args.binary.resolve())


if __name__ == "__main__":
    main()
