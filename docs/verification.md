# 0.1 baseline verification

This is the historical 0.1 acceptance record. See [0.2 follow-up](verification-0.2.md) for the current status.

## Build and runtime evidence

Test host: Apple M4, macOS 27.2.1, 10 logical CPUs, 16 GiB visible RAM; Rust 1.100.0. All 25 Rust tests passed. The project started as an empty local directory, with no Git metadata or remote. No commit, push, tag, release, or CI run took place. No existing harness profile was edited. ExecScope does not include code copied from the reference projects; the archives include applicable Cargo dependency notices. No public license has been applied to the original ExecScope source.

| Target | Local evidence | Not established |
| --- | --- | --- |
| `aarch64-apple-darwin` | Native release build, CLI and both stdio lifecycles, archive/checksum | Other macOS versions |
| `x86_64-apple-darwin` | Cross-built Mach-O, archive/checksum | Native execution; Rosetta is unavailable on the test host |
| `x86_64-unknown-linux-gnu` | Zig cross-built ELF, cross-Clippy, archive/checksum | Native Linux execution |
| `aarch64-unknown-linux-gnu` | Zig cross-built ELF, cross-Clippy, archive/checksum | Native Linux execution; the local cross-linker omitted a Cortex-A53 workaround it does not implement |
| `x86_64-pc-windows-msvc` | Cross-`cargo check` and cross-Clippy | MSVC link, archive, or native execution |

Local commands:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo build --locked --release
python3 tests/protocol.py target/release/execscope
actionlint .github/workflows/ci.yml
```

The independent Python client tested startup, discovery, calls, error results, parity, cancellation tolerance, and clean EOF across [2025-11-25 initialization](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) and [2026-07-28 discovery](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning). In a real Node/Git preflight, Node was runnable but blocked by `>=100`, while Git was ready. MCP Inspector 2.16.0 independently listed the tool, called it, and observed `isError: true` for a bad scope. Pi, Hermes, and Claude Code connected with temporary profiles. Codex parsed the example config and resolved the stdio command; no Codex session was run. No client/model usefulness trial was run.

The explicit `network` smoke used the system resolver. It did not get a public address for `example.com`, so ExecScope did not attempt HTTPS. Fake-transport tests establish that a configured proxy skips HTTPS, failure of the resolver stops HTTPS, and a returned HTTP response is reported without a general internet claim. Native HTTPS success was not observed.

## Timing and model-facing output

Each row contains 30 sequential calls in a small Node project declaring `node >=22` and `pnpm@10.2.0`. The MCP cold timing starts after negotiation. Warm conditions prime the requested modules and confirm a usable local environment on the same server. Full output uses `refresh=true`. Tokens count report text only, using tiktoken 0.14.0 `o200k_base`; they do not count MCP schema, request, or model overhead.

| Scenario | Median ms | p95 ms | p99 ms | Bytes / characters | Tokens |
| --- | ---: | ---: | ---: | ---: | ---: |
| CLI cold `auto/brief` | 80.50 | 82.16 | 82.19 | 304 | 98 |
| MCP cold `auto/brief` | 74.90 | 77.12 | 77.64 | 304 | 98 |
| MCP warm `auto/brief` | 2.53 | 3.16 | 3.18 | 304 | 98 |
| MCP uncached `all/full` | 78.79 | 85.14 | 89.15 | 28,775 | 7,245 |
| MCP warm Preflight (`node,git`) | 2.34 | 2.95 | 3.32 | 340 | 109 |
| Fastfetch 2.71.0 OS/CPU/RAM/disk/GPU JSON | 13.43 | 15.04 | 17.30 | 2,120 | 659 |

To reproduce, create a disposable directory with the manifest `{"engines":{"node":">=22"},"packageManager":"pnpm@10.2.0"}`, then run `python3 tests/protocol.py target/release/execscope --repeat 30 --tokenizer --fastfetch --cwd <fixture>`. Omit `--fastfetch` if it is not installed. Fastfetch took less time for its narrower host inventory; it did not inspect project or prerequisite selection. Workcell/DevEnvInfo were not run. [Agent evaluation protocol](evaluation.md) defines controlled follow-up trials without treating host inventory as equivalent to ExecScope.

## Acceptance matrix

### Interface and local behavior

| IDs | Status | Evidence or gap |
| --- | --- | --- |
| A01, A02, A03 | Passed | CLI entry points, no-argument reports, and exactly one advertised MCP tool |
| A04, A06, A07 | Passed | Invalid arguments, both rendering modes, and semantic parity against the built binary |
| A05, A08 | Passed | Scope matrix, including absence of active diagnostics from passive reports; local process, shell, and accelerator provenance |
| A09 | Passed | Nearest package/workspace/Git roots, nested pins, Git worktree marker, malformed manifests, Unicode/space and symlink cwd |
| A10 | Failed | Unix real-tool identity and separate states work. Safe Windows runnable identity and identity on all listed manager paths are incomplete. |
| A11 | Passed | File and manager edits observed in the same MCP process; refresh restricts collection to requested modules |
| A12 | Passed | Bounded modules and Unix child-group timeout fixtures; MCP remains usable after cancellation |
| A13 | Passed | Explicit scope gate, no active section for passive scopes, and bounded fake transport tests. No OS packet trace was taken. |
| A14, A15 | Passed | Hostile PATH/project fixture not executed; secret/ANSI/oversize tests; probe denial/DNS failure stays partial |
| A16 | Passed | Independent stdio client and Inspector evidence above |

### Delivery, readiness, and edge cases

| IDs | Status | Evidence or gap |
| --- | --- | --- |
| A17 | Blocked | Four local builds (one native); Windows cross-check only. GitHub runner matrix was configured but not executed. |
| A18 | Passed locally | Archives inspected, SHA-256 verified, and `actionlint` passed; tag-triggered attachment was not run. |
| A19 | Passed | Thirty runs per scenario; cold and warm p95 below specification goals on the measured host |
| A20 | Passed locally | Versioned tokenizer and fixture measurements; model-task token comparisons not run |
| A21, A22 | Passed | English docs/examples, format/lint/test/release build, and filesystem audit |
| A23, A24 | Passed locally | Allowlist, limits, aggregate/status rules and same API in both interfaces. Windows positive identity remains a limitation under A10. |
| A25, A26 | Failed | Bounded enrichment covers common conflicts; user-managed installs are often statically identified only. Dynamic mise/plugin selection, full uv interpreter selection, and some toolchain precedence cases are incomplete. |
| A27 | Failed | PATHEXT/alias/secondary PATH fixtures and cross-Clippy pass; Windows registry/job behavior and runnable identity were not exercised natively. |
| A28 | Blocked | Native macOS plus deterministic WSL/cgroup/CLT fixtures; no native WSL, Linux container, or translated macOS execution was available. |
| A29, A30 | Passed locally | Candidate/selection/version evidence is separate; hostile Corepack fixture never starts and active probes are scope-gated. |
| A31 | Passed locally | PATH ordering, changed permissions, manifest edits and local cache changes are re-evaluated; only unchanged successful probe results are cached. |
| A32 | Not tested end to end | 6/6 black-box manager states matched ground truth, zero false-ready, three intentionally unverified despite static version evidence. No agent calls or task-selection rate were measured. |

The local binaries are usable for inspection, but the failed and blocked rows prevent a claim that the full specification passed. To exercise the real target matrix, run `.github/workflows/ci.yml` on an authorized repository. Run `python3 tests/protocol.py <binary>` on each native platform, and test explicit outbound diagnostics only where permitted.
