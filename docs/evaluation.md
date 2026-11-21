# Agent evaluation protocol

This protocol tests whether ExecScope helps an agent choose the correct execution context with fewer calls. A successful MCP registration is not a result for this experiment.

## Cases and ground truth

Use a disposable fixture and record the tool order, selected version, blocker, and correct first command before each trial. Ask for a command recommendation; do not install software or launch an arbitrary build. Record the exact fixture, effective environment, and model identifier in the run log.

| Case | Fixture | Correct first action |
| --- | --- | --- |
| Package manager | A workspace containing a nested package with `packageManager: pnpm@…` and a conflicting workspace lockfile | Choose the declared manager; if it is blocked, name the blocker rather than substituting another manager |
| Tool conflict | Two Node candidates with the earlier candidate below the nested project's supported minimum | Identify the earlier candidate and the project constraint; do not call the alternate selected |
| Execution boundary | MCP server on the host, command tool in a Linux container or WSL | Inspect from inside the command environment before using a host-specific command |
| Build failure | A Rust project pin whose selected rustup component is missing | Identify the specific toolchain/component gap without running a repair or repeatedly probing the proxy |

## Trial design

Keep the same model, settings, harness version, prompt, filesystem fixture, and allowlisted command access. Start a clean session for each trial; randomize condition order and run each case at least five times per condition. Use the following conditions:

1. No ExecScope. The agent may use ordinary local shell discovery.
2. ExecScope available; no instruction snippet and no requirement to call it. This measures spontaneous tool choice.
3. ExecScope available with the optional [instruction snippet](agent-instructions.md). Compare tool choice against condition 2.
4. Same as condition 3, with the agent explicitly asked to check the known prerequisites using `requires`. Compare blocker discovery and follow-up calls against condition 3.

Include a Fastfetch baseline if available. Give it credit only for comparable OS, CPU, storage, or accelerator facts; it is not a project or readiness checker. Run Workcell/DevEnvInfo only if an isolated, read-only configuration can be verified. If a comparator is unavailable or outside the case's scope, record that instead of counting it as a wrong answer.

For each run record: inspection tool calls; elapsed time to the first correct environment-dependent recommendation; correct effective-selection flag; false-ready flag; exact blocker flag; unsupported assumptions; inspection result tokens; and total task input/output tokens as reported by the same harness. Use a named tokenizer if the harness does not supply token counts. Report denominators, including timeouts and cases in which no recommendation was offered.

No agent usefulness trials were run for this delivery. Host-only Fastfetch timings and deterministic fixture results are separate measurements in [verification](verification.md).
