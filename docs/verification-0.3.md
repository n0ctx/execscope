# ExecScope 0.3 harness-awareness verification

This record covers the harness-awareness and brief-budget changes on top of the [0.2 follow-up](verification-0.2.md). All checks ran on 2026-11-22. GitHub Actions was not run, and no release or tag was created.

## Runtime and build evidence

| Target | Checked | Still unverified |
| --- | --- | --- |
| Apple Silicon macOS (macOS 27.2) | `cargo fmt --check` and `cargo clippy -D warnings`. 83/83 Rust tests: 49 harness tests (43 fixture cases, 6 reader and ancestry tests), plus brief, parser and validation tests. Release build. `tests/protocol.py` on both MCP lifecycles. `tests/token_calibration.py` on real briefs. Live inspection under a real Claude Code session. | Other macOS versions |
| Windows x64 | `cargo clippy --all-targets -D warnings` cross-check for `x86_64-pc-windows-msvc`, including the Toolhelp/`QueryFullProcessImageNameW` ancestry code and tests | Execution on Windows: ancestry, PID-reuse guard, `%LOCALAPPDATA%` roots |
| Linux x64 | `cargo clippy --all-targets -D warnings` cross-check for `x86_64-unknown-linux-gnu`. The C dependency headers were stubbed for type-checking only, nothing was linked. The `/proc` stat and cmdline parsers are unit-tested on macOS. | Execution on Linux, containers, missing `/proc` |
| Intel macOS, Linux ARM64 | Not rebuilt for this change | Everything |

## Harness detection

The fixture suite (`src/harness/tests.rs`) runs the collector against a temporary home with an injected environment. It never reads real settings. Each of the fourteen products has:

- a positive identity case (process ancestor path, interpreter script, or application bundle identifier);
- at least one collision case (shared files, a same-named binary, a foreign bundle, imported resources);
- extraction of the fields that a local source provides.

Cross-cutting cases:

- version tolerance: a future version number, unknown keys, and new model, server and Skill names;
- field-level failure: a malformed file, a removed key, a `chmod 000` file, a relative root override;
- an authoritative replacement: `OPENCLAW_CONFIG_PATH`, a Codex project config over its profile;
- a breaking layout;
- add/remove inventory refresh;
- association rules: nested, environment-only, ambiguous, generic terminal, unavailable ancestry, MCP server context;
- auto mode with no link reads no unlinked configuration;
- candidate cap and determinism;
- hostile names and secret filtering;
- the 100/600/3000 brief budgets;
- regressions from an independent code review: PyYAML-style lists, YAML 1.1 booleans and flow maps, restrictions from an untrusted project, the home directory as a project, paths and SSH remotes in declared names, case-sensitive executable names on Windows, Windows trust-record keys, Gemini `TRUST_PARENT`/`DO_NOT_TRUST`, Claude trust on an enclosing project, and OpenClaw agent-list ids.

A test-only fictional signature (`a_new_product_needs_only_a_signature_row`) shows that a new product needs only a table row, with no new module, schema change or connector.

`tests/protocol.py` drives the release binary with a fixture home and Pi's child-process variable. It checks identity, the configured model and its source tag, declared MCP and Skills, a new Skill appearing after `refresh=true`, CLI/MCP agreement, and that an MCP `env` value is never printed.

Live check on this machine, as a Bash child of Claude Code 2.1.330 (personal Skill and subagent names replaced with placeholders):

```text
harness=claude-code process_linked(process_ancestor) surface=cli version=2.1.330
model_config=opus thinking=high source=user_config
declared_mcp=2 cua-driver,execscope
declared_skills=11 <skill-a>,<skill-b>,<skill-c>
declared_subagents=5 Explore,<agent-a>,<agent-b>
limits=permission_mode=auto
```

`--scope harness` also listed Codex, Copilot CLI, Grok Build, Hermes, Kimi Code, legacy kimi-cli, Pi and WorkBuddy as `configured`. These are real installs on this machine. Cursor, ZCode, OpenCode, OpenClaw, Gemini CLI, Antigravity and DeepSeek Harness are not installed here or on the second test Mac. Their rows rest on official documentation and released source, and are covered only by fixtures.

## Brief budget calibration

`tests/fixtures/token_calibration.in` holds 34 strings with reference counts from tiktoken 0.14: report lines, hashes, UUIDs, base64, CJK including rare blocks, Devanagari, Ethiopic, Thai, symbols, emoji and 300-character fields. The Rust test requires the estimate to be at or above both `o200k_base` and `cl100k_base` for every row.

Real briefs from the release binary, measured with tiktoken. The table shows the maximum of the two encodings.

| Scope | Budget 100 | Budget 600 | Budget 3000 |
| --- | --- | --- | --- |
| auto | 61 | 218 | 269 |
| harness | 61 | 284 | 284 |
| all (with `--requires git,node,cargo`) | 61 | 382 | 495 |

At the default budget a brief lists at most three declared names of each kind; from 1,200 tokens, or with `scope=harness`/`all`, it lists every bounded name. Briefs are not padded: budgets above the available evidence produce the same text.

## Performance and size (same machine, alternating runs, n=30, medians)

| Measurement | 0.2.0 baseline | 0.3.0 |
| --- | --- | --- |
| CLI cold `auto` brief | 9.74 / 9.97 ms | 10.09 / 10.09 ms |
| MCP cold `auto` brief | 6.42 / 6.41 ms | 6.44 / 6.48 ms |
| MCP warm `auto` brief | 2.23 / 2.23 ms | 2.35 / 2.33 ms |
| MCP `all` full, refresh | 65.3 / 65.2 ms | 66.7 / 66.8 ms |
| MCP `harness` brief | n/a | 3.38 / 3.31 ms |
| Release binary (aarch64, stripped) | 6,509,216 B | 6,742,256 B (+3.6%) |
| Peak RSS, CLI `auto` | 10.1 MB | 11.5 MB |

The incremental median for `auto` is under 0.4 ms, well inside the 100 ms target. The MCP `auto` brief grew from 86 to 207 o200k tokens on this machine because it now carries the linked harness. A full `all` report grew from 6,891 to 10,080 tokens. The tool definition stays below 2.6 KB of JSON, and the protocol test enforces that limit.

## Known limits

- A long-lived MCP server describes its own process. It cannot attribute a call to a client session or model.
- Configured models are defaults for new sessions. In-session changes such as `/model`, tools that extensions register at runtime, cloud agents, and remote gateway state are not observable from files.
- On Windows, ancestry provides executable paths but no arguments. Harnesses launched as `node <script>` there are recognized only through their child-process variables.
- Plist files in binary format cannot be read, so such bundles fall back to path markers.
- Rows for products without a local install follow their documentation. When a real install disagrees, the fix is a row edit plus one fixture.
