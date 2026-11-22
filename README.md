# ExecScope

ExecScope reports facts about the process that will inspect an environment. Use it before choosing a build command, package manager, or runtime. Its optional **Execution Preflight** checks named tools without attempting a build or installation. **Harness awareness** names the agent harness (Claude Code, Codex, Pi and others) that launched the process, the model it is configured to use, the MCP servers, Skills, subagents and extensions it declares, and restrictions that may block the agent.

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
execscope inspect --scope harness                 # harness-focused brief
execscope inspect --scope harness --detail full   # every bounded candidate as JSON
execscope inspect --brief-tokens 100              # smallest brief (100 to 3000, default 600)
execscope inspect --scope network --refresh
execscope --help
execscope inspect --help
```

No arguments and `inspect` use the same defaults (`auto`, `brief`). `cwd` defaults to ExecScope's own working directory; an explicit relative path is resolved against it. Invalid arguments and missing or non-directory paths fail without falling back to a different location. An inspection can succeed and report that a prerequisite is blocked; use `preflight.status` in full JSON rather than the CLI exit code to distinguish readiness.

| Scope | Coverage | Outbound diagnostics |
| --- | --- | --- |
| `auto` | Local process, project and relevant tools plus git, node and python, access, effective hardware, passive network, and the harness linked to this process | No |
| `system` | OS, process and execution context, privileges and directory access estimate | No |
| `dev` | Project, relevant tools plus git, node and python, and process context | No |
| `hardware` | CPU, memory, mount free space and available accelerator indicators | No |
| `network` | Local proxy/DNS configuration and explicit fixed-target checks | Yes |
| `harness` | Process context plus every bounded harness candidate: linked, configured or installed | No |
| `all` | All local modules, the complete supported tool list and all harness candidates | No |

`--detail full` adds evidence; it never enables network diagnostics. `--refresh` bypasses caches only for requested modules. Each CLI invocation has no persistent cache. `--brief-tokens N` sets the brief's size limit (see [Brief size](#brief-size)); it is an error with `--detail full`.

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

The tool accepts optional `scope`, `detail`, `cwd`, `refresh`, `requires`, and `brief_tokens`, with the same values as the CLI. Calling it with no arguments returns the normal snapshot including the linked harness; no second call or harness selection is needed. Its result is one text content item: the report as compact text or full JSON. MCP stdout contains only JSON-RPC frames. If an IDE starts the server from a different directory or with a different PATH, pass `cwd` and review the process PATH evidence; do not assume the terminal uses the same tool. The server supports modern discovery (2026-07-28) and the legacy initialization lifecycle for compatible clients.

## Harness awareness

The harness section answers which agent harness this process belongs to and what it is configured with. It describes the **ExecScope process**, not necessarily the agent that is calling it. A long-lived MCP server cannot tell which client session or model sent a given call, so the report never claims a live session model.

| Presence | Meaning |
| --- | --- |
| `process_linked` | A product-specific process is among ExecScope's ancestors (`association: process_ancestor`), or the product's own child-process variable is set (`environment_hint`, weaker). |
| `configured` | The product's own configuration files exist; no process relation was established. |
| `installed` | A verified install was found on PATH or in an application folder; nothing stronger. |

`primary` is the nearest process-linked harness. It is `null` when there is no association, or when only environment hints exist for several products (`association: ambiguous`). Nested harnesses (for example Codex started from a Claude Code shell) report the nearest one and list the rest as `also_linked`.

Each candidate can carry these fields:

- `configured_model`: provider, model and thinking or reasoning level from configuration. Each value has a source tag: `user_config`, `project_config` or `environment_override`. This is a default for new sessions, not the model of a running session.
- `capabilities`: declared MCP servers, Skills, subagents and extensions or plugins, with counts, names, entries disabled in config, and source tags. A declaration does not mean the item loaded, connected, or is callable.
- `constraints`: restrictions such as `permission_mode=plan`, `sandbox=read-only`, `denied_tool_rules=2`, `terminal_backend=ssh`, `project_untrusted` or `project_trust_unverified`.
- `visibility`: sources that could not be read. A missing entry from an unread source is not proof of absence.

Project resources from an untrusted or unverified project are listed as declarations but do not change the configured model.

Identity comes from install paths, package metadata, application bundle identifiers, and environment variables a harness sets for its children. Shared files never establish identity: `AGENTS.md`, `CLAUDE.md`, `.agents/skills`, `~/.gemini/config/`, and resources one harness imports from another. Generic names such as `agent` or `pi` and `CLAUDE_*` variables, which several harnesses set, are not used either.

Recognized products: Claude Code, Codex CLI, Gemini CLI, Antigravity, Kimi Code (and legacy kimi-cli), Grok Build, WorkBuddy, DeepSeek Harness, Cursor (CLI, IDE, ACP), ZCode, Pi, Hermes Agent, OpenCode, OpenClaw, plus recognition-only rows for Qwen Code, GitHub Copilot CLI, Amp and Aider. [The source note](tests/fixtures/harness/SOURCES.md) lists the evidence and keys behind each one.

Names are read from your files at inspection time, so a new model, MCP server, Skill or plugin needs no ExecScope update, and neither does a new harness version with the same layout. Unknown keys are ignored. A malformed or unreadable source loses only its own fields. If a vendor changes its layout, the product is still recognized, and the fields that cannot be read are listed under `visibility` instead of being guessed.

Collection modes:

- **`auto` (cheap):** reads bounded ancestor identities and environment hints, then only the linked product's own config root and the current project.
- **`harness` and `all`:** also check each product's known roots, verified PATH commands and application folders. Candidates are capped at 12, and the rest are reported as `omitted_candidates`.

Neither mode searches the home directory recursively, runs a harness, connects to MCP servers, or reads sessions, transcripts, credentials, prompt bodies, hooks or scripts.

On Windows, ancestry provides executable paths but not command-line arguments, so a harness started as `node <script>` is recognized there only through its child-process variable.

Runtime state (the model chosen with `/model`, tools registered by extensions, cloud agents, remote gateways) is not observable from files. SSH sessions, containers and remote terminal backends are reported as boundaries. To learn about another machine, run ExecScope there.

## Brief size

The brief is limited by an approximate token budget: default 600, selectable from 100 to 3000. Facts are kept in this priority order, and the least important whole facts are dropped first:

1. Execution boundary and project path.
2. Preflight blockers and restrictions.
3. Harness identity or ambiguity.
4. Configured model.
5. Capability inventory.
6. Caveats.
7. Hardware and network context.

When facts are dropped, a final `omitted=N use_detail=full` line says so. The budget is a ceiling, not a target. Larger budgets add detail only when there is evidence to show; they never read more files.

The estimate is a fast, deterministic, deliberately conservative approximation, calibrated against the `o200k_base` and `cl100k_base` tokenizers on report-shaped text, CJK and long untrusted fields (`tests/token_calibration.py`). On real briefs it runs about twice the reference count, so a 600 budget holds roughly 300 reference tokens. Random letter strings, which real names rarely are, can count a little higher than estimated. Other model tokenizers can count differently. Text is never cut inside a UTF-8 character.

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

`schema_version` starts at `"1"`. Additive optional fields may appear in v1 (the `harness` section is one); incompatible interpretations require a new version. `sampled_at` is a UTC RFC3339 timestamp; a cached module retains its original sample time and reports `cached: true`. `cwd` contains `value`, `source` (`process` or `explicit`), and `status`. `environment_source` is always `local_process`. A report is `ok` or `partial` (`overall_status`); invalid input is a CLI or MCP tool error, not a fallback report. An omitted section was not requested, or could not complete within the budget (in which case `warnings` identifies it). Dates and runtime samples vary; maps and candidate iteration order are stable for the same evidence.

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

ExecScope reads bounded local metadata, not credentials, shell history, project scripts, or user documents. From harness configuration it reads only allowlisted keys: model defaults, declared names, disabled flags, restriction settings and trust records. Large state files contribute only those keys; from `~/.claude.json`, for example, only `mcpServers` and the current project's trust and MCP entries. Of the inherited environment it reads only each product's documented variables: child-process markers, config-root overrides (used to locate files, never printed) and model overrides (reported as the configured model with the `environment_override` tag). It never reads MCP `env` values, headers, commands, URLs or prompt bodies. Declared names that look unsafe (URLs, paths, remote or e-mail addresses, shell syntax, credential-like text, control characters) are counted but not printed. A model name starting with `~` is not printed either, because it cannot be told apart from a `~user/` home path. Process arguments are used only to match signatures and are never reported. It does not execute shell startup, package-manager activation, dependency installation, project hooks, Corepack shims, or arbitrary files from PATH. Paths are included only where they explain the inspected context or selection; common home prefixes and unsafe path text are redacted. Process/system proxy settings are reported by presence and source without their values. Child output, diagnostics, and inherited environment are not copied into reports.

Only `scope=network` intentionally performs outbound probes: a system resolver lookup for `example.com`, followed (if it returns a safe public address) by a direct TLS `HEAD` to `https://example.com/`. When a proxy is configured, ExecScope skips HTTPS rather than bypassing it or forwarding credentials. Otherwise the HTTPS client uses no application proxy, follows no redirects, and reads no response body. The resolver has a 700 ms wait limit and HTTPS has a 1,350 ms wait limit. DNS can continue inside the OS resolver after the wait expires. A system VPN, transparent proxy, or sandbox can affect either result. Reachability of one destination is not a measure of all internet access.

The collection engine runs independent local modules in a bounded pool. Typical local collection waits at most 650 ms per parallel group; tool resolution waits up to 1,050 ms for brief and 1,850 ms for full/all. Identity probes have a 300 ms budget on Unix and 750 ms on Windows. Filesystem calls may finish in bounded workers after a response deadline. The server caches successful hardware (10 s), passive networking (4 s) and verified Unix executable identity (30 s); project, PATH and manager evidence is collected again for each request. The default `auto` and `dev` reports resolve node, whose `node --version` probe alone takes about 60 ms with recent Node.js releases, so a cold CLI call takes about 70 ms; warm MCP calls reuse the cached identity.

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
python3 tests/token_calibration.py target/release/execscope   # needs tiktoken
python3 scripts/package.py aarch64-apple-darwin --binary target/release/execscope
```

The independent Python client tests the actual binary on both MCP lifecycles. The optional developer dependency `tiktoken` counts `o200k_base` tokens for the benchmark. A live network smoke test can be run explicitly with `execscope inspect --scope network --detail full`. The release workflow targets five native runner families. It packages the binary, this guide, dependency notices and a separate checksum. Running the workflow requires Actions to be enabled; the public repository has not run hosted CI. A successful local build does not publish a release. See the [verification record](docs/verification.md), the [0.2 follow-up](docs/verification-0.2.md) and the [0.3 harness record](docs/verification-0.3.md) for the tests actually run.
