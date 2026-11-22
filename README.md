# ExecScope

ExecScope reports facts about the process that will inspect an environment. Use it before choosing a build command, package manager, or runtime. Its optional **Execution Preflight** checks named tools without attempting a build or installation.

The report describes the **local ExecScope process**, not the agent's live shell. Shell aliases, functions, and unexported version-manager state are not visible. Passing `cwd` changes project lookup and the directory of eligible probes; it does not change the process PATH or enter another OS. If you will run commands in WSL, a container, or an SSH session, run ExecScope inside that environment.

## Install and run

On macOS and Linux, the installer downloads the archive for this machine, checks its SHA-256, and puts the binary in `~/.local/bin`:

```sh
curl -fsSL https://raw.githubusercontent.com/n0ctx/execscope/main/scripts/install.sh | bash
# a specific release or destination
curl -fsSL https://raw.githubusercontent.com/n0ctx/execscope/main/scripts/install.sh | bash -s -- --version 0.2.0 --prefix /usr/local/bin
```

Or unpack the archive for your OS and architecture, then put `execscope` (`execscope.exe` on Windows) in a directory on PATH. Verify the adjacent SHA-256 checksum before use. On macOS/Linux: `shasum -a 256 -c <archive>.sha256`; on PowerShell: `Get-FileHash -Algorithm SHA256 <archive>` and compare the hash. The binary does not require Rust, Python, Node.js, or an MCP package. Linux GNU builds require a compatible system glibc; the release workflow uses Ubuntu 22.04. Windows targets x64 on a release with `IsWow64Process2` (Windows 10 version 1709 or newer). macOS artifacts are not signed or notarized; follow your machine's policy for local unsigned software.

```sh
execscope
execscope inspect --scope dev --detail full --cwd '/path/to/My App'
execscope inspect --cwd '/path/to/My App' --requires node,pnpm,git
execscope inspect --scope network --refresh
execscope --help
execscope inspect --help
```

No arguments and `inspect` use the same defaults (`auto`, `brief`). `cwd` defaults to ExecScope's own working directory; an explicit relative path is resolved against it. Invalid arguments and missing or non-directory paths fail without falling back to a different location. An inspection can succeed and report that a prerequisite is blocked; use `preflight.status` in full JSON rather than the CLI exit code to distinguish readiness.

| Scope | Coverage | Outbound diagnostics |
| --- | --- | --- |
| `auto` | Local process, project and relevant tools, access, effective hardware, passive network | No |
| `system` | OS, process and execution context, privileges and directory access estimate | No |
| `dev` | Project, relevant tools and process context | No |
| `hardware` | CPU, memory, mount free space and available accelerator indicators | No |
| `network` | Local proxy/DNS configuration and explicit fixed-target checks | Yes |
| `all` | All local modules and the complete supported tool list | No |

`--detail full` adds evidence; it never enables network diagnostics. `--refresh` bypasses caches only for requested modules. Each CLI invocation has no persistent cache.

## MCP stdio

Expose exactly one tool, `inspect_environment`, by launching `execscope mcp`. This generic example applies to clients that use an `mcpServers` object:

```json
{
  "mcpServers": {
    "execscope": {
      "command": "/absolute/path/to/execscope",
      "args": ["mcp"]
    }
  }
}
```

Use `C:\\Program Files\\ExecScope\\execscope.exe` as a Windows JSON example. Keep `mcp` in `args`; don't put quotes or arguments in `command`. [Client registration examples](docs/clients.md) and [optional agent instructions](docs/agent-instructions.md) are separate so that you can choose where to use them. ExecScope does not edit any client configuration or instructions.

The tool accepts optional `scope`, `detail`, `cwd`, `refresh`, and `requires`, with the same values as the CLI. Its result is one text content item: the report as compact text or full JSON. MCP stdout contains only JSON-RPC frames. If an IDE starts the server from a different directory or with a different PATH, pass `cwd` and review the process PATH evidence; do not assume the terminal uses the same tool. The server supports modern discovery (2026-07-28) and the legacy initialization lifecycle for compatible clients.

## Execution Preflight

CLI `--requires` is a comma-separated list with no spaces. MCP takes an array. Identifiers are lowercase, unique, and limited to 16: `git`, `python`, `pip`, `uv`, `node`, `npm`, `pnpm`, `yarn`, `bun`, `rustc`, `cargo`, `go`, `swift`, `java`, `javac`, `dotnet`, `gcc`, `clang`, `cmake`, `docker`. No user command is accepted. A preflight can be combined with any scope and does not authorize traffic on its own.

| Outcome | Meaning |
| --- | --- |
| `ready` | The selected invocation passed a narrow identity check and any supported applicable requirement matched. This does not validate other subcommands or network/service access. |
| `blocked` | Evidence establishes a blocker, such as a missing tool in the inspected search, an app alias without a runnable target, or a verified version mismatch. An alternate binary does not make the selected one ready. |
| `unverified` | Inspect the reason and evidence. A wrapper may be unsafe to start, the selection may involve a layer that is not observable, or the constraint syntax may be unsupported. |

Illustrative brief output on a project that selects an incompatible Node version:

```text
os=linux/x86_64 exec=native shell=unknown access=standard
cwd=/work/app source=explicit project=node manager=pnpm
preflight=blocked items=node:blocked(version_mismatch,18.20.0)[want=>=22],pnpm:unverified(managed_or_wrapper_not_verified)
```

The supported comparison subset consists of numeric major or major.minor pins, exact semantic versions, `.x`/`.*` prefix pins, numeric inequalities, a comma-separated intersection of up to four comparisons, and `^`/`~` with all three version components. A leading `v` is accepted in numeric versions. Unsupported expressions (for example, npm `||`, nonnumeric aliases, or full ecosystem solver expressions) do not imply a match or mismatch. Pre-release ranges are not evaluated. Installed versions and project declarations are distinct evidence.

## Full JSON and evidence

`schema_version` starts at `"1"`. Additive optional fields may appear in v1; incompatible interpretations require a new version. `sampled_at` is a UTC RFC3339 timestamp; a cached module retains its original sample time and reports `cached: true`. `cwd` contains `value`, `source` (`process` or `explicit`), and `status`. `environment_source` is always `local_process`. A report is `ok` or `partial` (`overall_status`); invalid input is a CLI or MCP tool error, not a fallback report. An omitted section was not requested, or could not complete within the budget (in which case `warnings` identifies it). Dates and runtime samples vary; maps and candidate iteration order are stable for the same evidence.

`available`, `not_found`, `unknown`, `timeout`, and `unsupported` describe the evidence boundary. For tools, `declared` is a project requirement, `discovered` a local path, `selected` a command resolution, and `verified` a successful identity check for the selected target. `tools` preserves ordered candidates, manager evidence, selected invocation, probe source, `reason_code`, declarations, and a version `comparison` (`no_constraint`, `match`, `mismatch`, `unverified`). A `discovered_version` comes from local metadata and is not a `verified_version`. Optional `preflight` adds `requested`, aggregate `status`, and ordered `requirements`; an explicitly empty list is ready with no checks. Untested alternatives remain alternatives. Machine/user PATH and project-local installs never replace the process PATH.

Example excerpt from a full report (unrelated fields omitted):

```json
{
  "schema_version": "1",
  "sampled_at": "2026-11-22T00:00:00Z",
  "scope": "dev",
  "detail": "full",
  "environment_source": "local_process",
  "cwd": {"value":"/work/app","source":"explicit","status":"available"},
  "overall_status": "ok",
  "tools": {
    "node": {
      "status": "available",
      "stage": "verified",
      "source": "verified_executable",
      "selection_context": "process_path",
      "verified_version": "18.20.0",
      "comparison": "mismatch",
      "reason_code": "version_mismatch"
    }
  }
}
```

Hardware capacities are in bytes and CPUs; a CPU quota can be fractional. Effective capacity is an upper bound, not guaranteed free CPU or memory. On Linux the collector considers visible capacity, affinity, cpuset, and finite cgroup quotas that it can read in the process hierarchy. Resource access in a container does not establish what the host has installed or available. A missing GPU utility is not proof of an absent GPU. A directory permission estimate does not prove that the sandbox or filesystem will permit a write.

## Privacy and failure boundaries

ExecScope reads bounded local metadata, not credentials, shell history, project scripts, or user documents. It does not execute shell startup, package-manager activation, dependency installation, project hooks, Corepack shims, or arbitrary files from PATH. Paths are included only where they explain the inspected context or selection; common home prefixes and unsafe path text are redacted. Process/system proxy settings are reported by presence and source without their values. Child output, diagnostics, and inherited environment are not copied into reports.

Only `scope=network` intentionally performs outbound probes: a system resolver lookup for `example.com`, followed (if it returns a safe public address) by a direct TLS `HEAD` to `https://example.com/`. When a proxy is configured, ExecScope skips HTTPS rather than bypassing it or forwarding credentials. Otherwise the HTTPS client uses no application proxy, follows no redirects, and reads no response body. The resolver has a 700 ms wait limit and HTTPS has a 1,350 ms wait limit. DNS can continue inside the OS resolver after the wait expires. A system VPN, transparent proxy, or sandbox can affect either result. Reachability of one destination is not a measure of all internet access.

The collection engine runs independent local modules in a bounded pool. Typical local collection waits at most 650 ms per parallel group; tool resolution waits up to 1,050 ms for brief and 1,850 ms for full/all. Identity probes have a 300 ms budget on Unix and 750 ms on Windows. Filesystem calls may finish in bounded workers after a response deadline. The server caches successful hardware (10 s), passive networking (4 s) and verified Unix executable identity (30 s); project, PATH and manager evidence is collected again for each request.

On Windows, runnable identity is deliberately limited to the pinned upstream x64 Node.js 24.22.3 binary at `Program Files\nodejs\node.exe`. ExecScope verifies the SHA-256 and protected installation ACLs, checks the file type and architecture, and launches only `node.exe --version` with a clean environment, a System32 working directory and a restrictive Windows job. The probe does not use the project directory or launch wrappers. Other versions, locations and tools get PE metadata where available, not a `ready` verdict. UNC and mapped network drives are not supported as Windows project directories. On Unix, probes are restricted to allowlisted arguments and trusted install roots, including eligible Homebrew bottles. This is not a general sandbox.

For version managers, ExecScope reads static selectors and installed paths instead of running manager hooks. It can show ASDF fallback lists, common numeric mise selectors and config tiers, rustup channel and path overrides, and the existing uv project environment separately from the interpreter on process PATH. A derived candidate does not prove it would run. Custom mise backends and templates, unknown config discovery rules, ASDF `system` dispatch, and uv environment creation or replacement are not simulated.

## Build, test, and package

The CI toolchain is Rust 1.100.0. From the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo build --locked --release
python3 tests/protocol.py target/release/execscope
python3 tests/protocol.py target/release/execscope --repeat 30 --tokenizer
python3 scripts/package.py aarch64-apple-darwin --binary target/release/execscope
```

The independent Python client tests the actual binary on both MCP lifecycles. The optional developer dependency `tiktoken` counts `o200k_base` tokens for the benchmark. A live network smoke test can be run explicitly with `execscope inspect --scope network --detail full`. The release workflow targets five native runner families. It packages the binary, this guide, dependency notices and a separate checksum. Running the workflow requires Actions to be enabled; the public repository has not run hosted CI. A successful local build does not publish a release. See the [verification record](docs/verification.md) and [0.2 follow-up](docs/verification-0.2.md) for the tests actually run.
