# ExecScope 0.2 follow-up verification

This record updates the [0.1 acceptance matrix](verification.md). It distinguishes an executed test from a cross-build. The GitHub repository is public; GitHub Actions is disabled and no hosted runs or releases were started.

## Runtime and build evidence

| Target | Checked | Still unverified |
| --- | --- | --- |
| Apple Silicon macOS | Release build, 30/30 Rust tests, both MCP lifecycles, real offline uv selection and a real mise selector comparison | Other macOS versions |
| Intel macOS | Cross-built x64 Mach-O and local archive | Native Intel/Rosetta execution |
| Linux ARM64 | GNU release binary ran with Ubuntu 22.04 userspace (glibc 2.35) in an Apple Virtualization VM on ARM64. 30/30 Linux Rust tests ran using a musl test binary with the same source. The GNU binary reported the expected PID/mount namespace and cgroup v2 limits (0.5 CPU, 128 MiB). | WSL, Docker runtime, other distributions |
| Linux x64 | Cross-built x64 GNU ELF and local archive | Native x64 execution |
| Windows x64 | `cargo clippy` cross-check, including tests, for `x86_64-pc-windows-msvc` | MSVC link, successful probe, ACL decisions, runtime mitigation and cleanup on a real Windows system |

Cross-linked Linux GNU binaries were built with a glibc 2.35 baseline using Zig 0.15.2. The ARM64 GNU cross-linker did not support the Cortex-A53 workaround flag; the repository's native toolchain settings are unchanged. The native ARM64 VM uses an Alpine Linux kernel and tested both Alpine and Ubuntu userspaces. Neither proves Windows, WSL, or Intel behavior.

## Windows probe boundary

The only executable launch currently supported is the exact upstream x64 Node.js 24.22.3 image in the standard protected install directory. ExecScope checks the native PE header, protected owners/ACLs and locked install path, then the digest published in [Node.js SHASUMS256](https://nodejs.org/dist/v24.22.3/SHASUMS256.txt). It requires NTFS, locks file and ancestor names against ordinary writes or replacement while it checks and launches, and rejects untrusted write or rename grants in the checked directories. The locked handle is not passed to the process API: the design relies on the verified NTFS sharing and ACL boundaries, and does not defend against a compromised kernel or administrator. It uses a System32 working directory, a minimal environment and a Windows Job configured at creation. A positive result requires the expected version output from the real process. Other Node releases, alternate installation layouts, other tools, and access conditions outside the policy remain unverified. This deliberately narrow probe is not a general Windows execution sandbox.

On a disposable, authorized Windows x64 host, install or stage the pinned upstream executable under the protected standard directory using the system administrator's normal procedure. Verify that its SHA-256 matches the published `win-x64/node.exe` digest; an MSI install is suitable only if it contains the same image. Build with native MSVC and run:

```powershell
cargo test --locked --all-targets --all-features
cargo build --locked --release
python -X utf8 tests/protocol.py target/release/execscope.exe
python -X utf8 tests/windows_identity.py target/release/execscope.exe "$env:ProgramFiles\nodejs\node.exe"
```

The Rust suite covers denial of ordinary rename/write attempts while handles are held. The last test requires `ready` for the protected pinned release and `unverified` for identical bytes in a project directory. It supplies a Node startup option that the probe must not inherit. Do not loosen installation ACLs to make the positive case pass. The test was authored and syntax-checked on macOS, not run on Windows.

## Manager resolution

- uv: an existing project environment is located using the Python project/workspace root. Relative `UV_PROJECT_ENVIRONMENT` binds to that root, not the inspected subdirectory or another language workspace. `UV_NO_PROJECT` disables that lookup. This candidate is never promoted to process PATH or treated as runnable. `tests/uv_selection.py` compared it with a real uv 0.13.6 offline `run --no-sync` in a temporary project.
- mise: the common project, environment, local and global override tiers are inspected without loading plugins. A unique installed match for a numeric pin is shown as evidence, not as executed or confirmed shim dispatch. A real mise 2026.11.12 `which --version node` agreed with ExecScope's static evidence in an isolated exact-pin fixture. Ambiguous installations, discovery overrides, explicit backends, templates, and dynamic options remain unverified.
- ASDF: a configured versions filename and ordered installed-version fallbacks are read locally; `system` stops the installed-version search rather than pretending to be a store release. Rustup: the nearest override is recognized, and an absolute `path` is distinguished from an installed channel without running the proxy. Component or target reconciliation is observed but not simulated.

## Acceptance changes and remaining work

| IDs | Current status |
| --- | --- |
| A10, A24, A27 | Improved but not fully accepted: Windows policy is cross-checked, and the native positive and refusal smoke still needs a Windows x64 host. |
| A17, A18 | Five-target workflow passes local lint; four local checksummed archives. No hosted job, native Windows package, or release attachment. |
| A25, A26 | More selector cases and observed real uv/mise agreement. Dynamic plugin/config execution and some ecosystem-specific selection remain unsupported. |
| A28 | Native Linux ARM64 namespace and cgroup v2 case passed; WSL, Docker, Rosetta, Intel macOS, and native Linux x64 remain unverified. |
| A32 | No live agent usefulness experiment was run. The [evaluation protocol](evaluation.md) remains available. |

Use `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets --all-features -- -D warnings`, `cargo test --locked --all-targets --all-features`, `cargo build --locked --release`, `python3 tests/protocol.py target/release/execscope`, `python3 tests/uv_selection.py target/release/execscope`, `shellcheck -s sh tests/linux_isolation.sh`, and `actionlint .github/workflows/ci.yml` to reproduce the local checks. Run `tests/linux_isolation.sh` only inside a disposable Linux host with writable cgroup v2 and delegated CPU and memory controllers.
