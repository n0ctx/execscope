# MCP client examples

Replace `/absolute/path/to/execscope` with the actual binary path. Keep the binary path separate from the `mcp` argument. These examples are opt-in; nothing here needs to be installed by the program. Pi, Hermes, and Claude Code connected using isolated profiles. Codex parsed the isolated TOML and resolved the stdio command; an actual Codex model session was not run.

| Client | Configuration reference | Check |
| --- | --- | --- |
| Generic stdio | [README example](../README.md#mcp-stdio); property names vary by client | List tools; expect exactly `inspect_environment` |
| Pi 1.6.0 | Installed `docs/mcp.md`; project `.pi/mcp.json` uses `mcpServers` | `pi mcp list` |
| Hermes Agent 0.23.1 | [Hermes MCP configuration](https://hermes-agent.nousresearch.com/docs/user-guide/features/mcp) | `hermes mcp test execscope` |
| Claude Code 2.1.325 | [Claude Code MCP](https://code.claude.com/docs/en/mcp) | `claude mcp get execscope` |
| Codex CLI 0.167.0 | [Codex MCP](https://developers.openai.com/codex/mcp) | `codex mcp get execscope` |

Pi project registration requires project trust:

```sh
pi mcp add --local execscope -- '/absolute/path/to/execscope' mcp
pi mcp list
```

Hermes: add the entry under `mcp_servers` in the selected profile's `config.yaml`:

```yaml
mcp_servers:
  execscope:
    command: /absolute/path/to/execscope
    args: [mcp]
```

Claude Code: add a local entry scoped to the current project for this user:

```sh
claude mcp add --scope local --transport stdio execscope -- '/absolute/path/to/execscope' mcp
claude mcp get execscope
```

Codex CLI: add this to the chosen `config.toml`:

```toml
[mcp_servers.execscope]
command = "/absolute/path/to/execscope"
args = ["mcp"]
```

Alternatively, `codex mcp add execscope -- '/absolute/path/to/execscope' mcp` writes the entry to the active Codex home. For a Windows path in TOML, use a literal string such as `command = 'C:\Program Files\ExecScope\execscope.exe'`.

If a client does not support local stdio or starts the server outside the target environment, use `execscope inspect --cwd <directory> --requires node,pnpm,git` from the environment in which commands will run.
