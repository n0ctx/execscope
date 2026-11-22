//! Declarative harness signatures.
//!
//! Each row states verified identity evidence (install paths, package names, bundle
//! identifiers, environment variables the product sets for its children), the
//! product's own configuration root, and which allowlisted keys/directories hold
//! model defaults and capability declarations. Rows contain no model, MCP, Skill or
//! plugin catalogs: names are discovered from the user's files at inspection time.
//! Versions are informational only; no row gates on a version range.
//!
//! Identity never comes from shared resources (`AGENTS.md`, `CLAUDE.md`,
//! `.agents/skills`, `.gemini/`, `.cursor/` imports) or from generic basenames such as
//! `agent`, `kimi`, `pi` or `code`, and never from `CLAUDE_*` variables, which other
//! harnesses also set. Per-product sources are listed in `tests/fixtures/harness/SOURCES.md`.
//!
//! Maintenance rule: a changed path or key needs a row edit plus one fixture; a new
//! product needs a new row. Hooks (`trust`, `special`) exist only for documented
//! semantics the shared fields cannot express.

use super::{
    Env, Extraction,
    config::{self, Entries, Format, Names, Read},
};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cap {
    Mcp,
    Skills,
    Subagents,
    Extensions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Relative to the product's configuration root.
    User,
    /// Relative to each project directory (the cwd up to its VCS root).
    Project,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Base {
    Home,
    /// `$XDG_CONFIG_HOME` or `~/.config` on every OS.
    XdgConfig,
}

pub(crate) struct Root {
    /// Documented override variables, first set wins: (variable, suffix appended to
    /// its absolute value; "" when the variable names the root itself).
    pub(crate) env: &'static [(&'static str, &'static str)],
    pub(crate) base: Base,
    pub(crate) path: &'static str,
    /// Windows default under `%LOCALAPPDATA%` when it differs from `base`/`path`.
    pub(crate) windows_local: Option<&'static str>,
    /// Documented profile selection that relocates the effective root.
    pub(crate) select: Option<fn(PathBuf, &Env) -> PathBuf>,
}

pub(crate) struct Fields {
    pub(crate) provider: &'static [&'static str],
    pub(crate) model: &'static [&'static str],
    pub(crate) thinking: &'static [&'static str],
    pub(crate) mcp: &'static [&'static str],
    pub(crate) agents: &'static [&'static str],
    pub(crate) extensions: &'static [&'static str],
    pub(crate) skills: &'static [&'static str],
    /// (key path, label): scalar values render as `label=value`, arrays as `label=count`.
    pub(crate) limits: &'static [(&'static str, &'static str)],
}

pub(crate) struct Source {
    pub(crate) scope: Scope,
    pub(crate) file: &'static str,
    pub(crate) format: Format,
    pub(crate) fields: &'static Fields,
}

pub(crate) struct Inventory {
    pub(crate) scope: Scope,
    pub(crate) dir: &'static str,
    pub(crate) cap: Cap,
    pub(crate) entries: Entries,
}

#[derive(Clone, Copy)]
pub(crate) enum Trust {
    NotApplicable,
    /// Some(true) trusted, Some(false) untrusted, None not determinable. Project-scope
    /// model settings are applied only when trusted.
    Check(fn(&mut Extraction<'_>) -> Option<bool>),
}

pub(crate) struct Signature {
    pub(crate) id: &'static str,
    pub(crate) surface: &'static str,
    /// Distinctive executable basenames (never generic names such as `agent` or `pi`).
    pub(crate) names: &'static [&'static str],
    /// Product-owned path fragments ('/' separated, lowercase on Windows) and the
    /// surface each implies.
    pub(crate) paths: &'static [(&'static str, &'static str)],
    /// argv tokens (after argv[0]) that select a surface, such as `acp`.
    pub(crate) arg_surfaces: &'static [(&'static str, &'static str)],
    /// Package names whose metadata confirms identity and supplies a version.
    pub(crate) packages: &'static [&'static str],
    /// macOS `CFBundleIdentifier` values and the surface each implies.
    pub(crate) bundles: &'static [(&'static str, &'static str)],
    /// Variables the product sets for child processes: (name, exact value or "" for
    /// any nonempty value).
    pub(crate) env: &'static [(&'static str, &'static str)],
    /// PATH commands whose resolved target is checked against `paths`/`packages`.
    pub(crate) commands: &'static [&'static str],
    /// Application bundle names checked in the platform application folders.
    pub(crate) apps: &'static [&'static str],
    pub(crate) root: Root,
    /// Product-specific files under `root` that establish `configured` presence.
    pub(crate) markers: &'static [&'static str],
    /// Installed-version metadata under `root`: (file, JSON key path or "" for text).
    pub(crate) version_file: Option<(&'static str, &'static str)>,
    /// Documented variables that override the configured model for new sessions.
    pub(crate) model_env: &'static [&'static str],
    /// In precedence order for the model field (highest first).
    pub(crate) sources: &'static [Source],
    pub(crate) inventories: &'static [Inventory],
    pub(crate) trust: Trust,
    pub(crate) special: Option<fn(&mut Extraction<'_>)>,
    /// A standing visibility limit (for example, state kept only in the cloud).
    pub(crate) visibility: Option<&'static str>,
}

pub(super) const NONE: Fields = Fields {
    provider: &[],
    model: &[],
    thinking: &[],
    mcp: &[],
    agents: &[],
    extensions: &[],
    skills: &[],
    limits: &[],
};

pub(super) const MCP: Fields = Fields {
    mcp: &["mcpServers"],
    ..NONE
};

const PI_SETTINGS: Fields = Fields {
    provider: &["defaultProvider"],
    model: &["defaultModel"],
    thinking: &["defaultThinkingLevel"],
    extensions: &["packages", "extensions"],
    ..NONE
};

// Claude Code reads the same keys at every settings layer.
const CLAUDE_SETTINGS: Fields = Fields {
    model: &["model"],
    thinking: &["effortLevel"],
    extensions: &["enabledPlugins"],
    limits: &[
        ("permissions.defaultMode", "permission_mode"),
        ("permissions.deny", "denied_tool_rules"),
    ],
    ..NONE
};

// Plugins are a user-level setting in Codex; project config shares the rest.
const CODEX_PROJECT: Fields = Fields {
    provider: &["model_provider"],
    model: &["model"],
    thinking: &["model_reasoning_effort"],
    mcp: &["mcp_servers"],
    limits: &[("sandbox_mode", "sandbox"), ("approval_policy", "approval")],
    ..NONE
};

const CODEX_USER: Fields = Fields {
    extensions: &["plugins"],
    ..CODEX_PROJECT
};

const GEMINI_SETTINGS: Fields = Fields {
    model: &["model.name"],
    mcp: &["mcpServers"],
    limits: &[
        ("general.defaultApprovalMode", "approval"),
        ("tools.sandbox", "sandbox"),
    ],
    ..NONE
};

const ZCODE_MCP: Fields = Fields {
    mcp: &["mcp.servers"],
    ..NONE
};

const AMP_MCP: Fields = Fields {
    mcp: &["amp.mcpServers"],
    ..NONE
};

// V1 `agent`/`plugin` and V2 `agents`/`plugins`; MCP shapes differ (see `opencode_mcp`).
const OPENCODE: Fields = Fields {
    model: &["model"],
    agents: &["agent", "agents"],
    extensions: &["plugin", "plugins"],
    ..NONE
};

pub(super) const BASE: Signature = Signature {
    id: "",
    surface: "cli",
    names: &[],
    paths: &[],
    arg_surfaces: &[],
    packages: &[],
    bundles: &[],
    env: &[],
    commands: &[],
    apps: &[],
    root: home(&[], ""),
    markers: &[],
    version_file: None,
    model_env: &[],
    sources: &[],
    inventories: &[],
    trust: Trust::NotApplicable,
    special: None,
    visibility: None,
};

pub(super) const MD: Entries = Entries::Files(&["md"]);
pub(super) const SKILLS: Entries = Entries::SkillDirs;

pub(super) const fn user(file: &'static str, format: Format, fields: &'static Fields) -> Source {
    Source {
        scope: Scope::User,
        file,
        format,
        fields,
    }
}

pub(super) const fn project(file: &'static str, format: Format, fields: &'static Fields) -> Source {
    Source {
        scope: Scope::Project,
        file,
        format,
        fields,
    }
}

pub(super) const fn inv(scope: Scope, dir: &'static str, cap: Cap, entries: Entries) -> Inventory {
    Inventory {
        scope,
        dir,
        cap,
        entries,
    }
}

pub(super) const fn home(env: &'static [(&'static str, &'static str)], path: &'static str) -> Root {
    Root {
        env,
        base: Base::Home,
        path,
        windows_local: None,
        select: None,
    }
}

use Cap::{Extensions, Mcp, Skills, Subagents};
use Format::{Json, Toml, Yaml};
use Scope::{Project as P, User as U};

pub(crate) static SIGNATURES: &[Signature] = &[
    // https://code.claude.com/docs/en/settings, /env-vars, /claude-directory
    Signature {
        id: "claude-code",
        names: &["claude"],
        paths: &[
            ("/.local/share/claude/versions/", "cli"),
            ("/node_modules/@anthropic-ai/claude-code/", "cli"),
        ],
        packages: &["@anthropic-ai/claude-code"],
        bundles: &[("com.anthropic.claude-code", "cli")],
        // Set for tool shells, hooks and stdio MCP servers; IDE terminals set it too.
        env: &[("CLAUDECODE", "1")],
        commands: &["claude"],
        root: home(&[("CLAUDE_CONFIG_DIR", "")], ".claude"),
        markers: &["settings.json", "agents", "plugins/installed_plugins.json"],
        model_env: &["ANTHROPIC_MODEL"],
        sources: &[
            project(".claude/settings.local.json", Json, &CLAUDE_SETTINGS),
            project(".claude/settings.json", Json, &CLAUDE_SETTINGS),
            user("settings.json", Json, &CLAUDE_SETTINGS),
            project(".mcp.json", Json, &MCP),
        ],
        inventories: &[
            inv(U, "skills", Skills, SKILLS),
            inv(P, ".claude/skills", Skills, SKILLS),
            inv(U, "agents", Subagents, MD),
            inv(P, ".claude/agents", Subagents, MD),
        ],
        trust: Trust::Check(claude_state),
        ..BASE
    },
    // https://developers.openai.com/codex/config-basic and config-reference;
    // openai/codex codex-rs/core/src/spawn.rs (child environment).
    Signature {
        id: "codex",
        names: &["codex"],
        paths: &[
            ("/.codex/packages/standalone/", "cli"),
            ("/node_modules/@openai/codex/", "cli"),
        ],
        packages: &["@openai/codex"],
        env: &[("CODEX_THREAD_ID", ""), ("CODEX_SANDBOX", "")],
        commands: &["codex"],
        root: home(&[("CODEX_HOME", "")], ".codex"),
        markers: &["config.toml"],
        version_file: Some(("packages/standalone/current/codex-package.json", "version")),
        sources: &[
            project(".codex/config.toml", Toml, &CODEX_PROJECT),
            user("config.toml", Toml, &CODEX_USER),
        ],
        inventories: &[
            inv(U, "skills", Skills, SKILLS),
            inv(P, ".codex/skills", Skills, SKILLS),
            inv(U, "agents", Subagents, Entries::Files(&["toml"])),
        ],
        trust: Trust::Check(codex_trust),
        special: Some(codex_profile),
        ..BASE
    },
    // https://geminicli.com/docs/reference/configuration/, /cli/skills, /core/subagents;
    // google-gemini/gemini-cli shellExecutionService.ts (GEMINI_CLI=1).
    Signature {
        id: "gemini-cli",
        names: &["gemini"],
        paths: &[
            ("/node_modules/@google/gemini-cli/", "cli"),
            ("/Cellar/gemini-cli/", "cli"),
        ],
        packages: &["@google/gemini-cli"],
        env: &[("GEMINI_CLI", "1")],
        commands: &["gemini"],
        root: home(&[("GEMINI_CLI_HOME", ".gemini")], ".gemini"),
        // `.gemini` is shared with Antigravity: only Gemini CLI's own files count.
        markers: &["settings.json", "trustedFolders.json", "extensions"],
        model_env: &["GEMINI_MODEL"],
        sources: &[
            project(".gemini/settings.json", Json, &GEMINI_SETTINGS),
            user("settings.json", Json, &GEMINI_SETTINGS),
        ],
        inventories: &[
            inv(U, "skills", Skills, SKILLS),
            inv(P, ".gemini/skills", Skills, SKILLS),
            inv(U, "agents", Subagents, MD),
            inv(P, ".gemini/agents", Subagents, MD),
            inv(U, "extensions", Extensions, Entries::DirsOrFiles(&[])),
        ],
        trust: Trust::Check(gemini_trust),
        ..BASE
    },
    // https://antigravity.google/docs/customization-discovery/, /settings/, /mcp/,
    // /skills/, /subagents/; Homebrew cask `antigravity` (bundle com.google.antigravity).
    Signature {
        id: "antigravity",
        surface: "desktop",
        names: &["agy"],
        paths: &[("/Antigravity.app/", "desktop")],
        bundles: &[("com.google.antigravity", "desktop")],
        commands: &["agy"],
        apps: &["Antigravity.app"],
        root: home(&[], ".gemini"),
        // `~/.gemini/config/` alone is not evidence: other tools write it.
        markers: &[
            "antigravity-cli/settings.json",
            "antigravity",
            "antigravity-ide",
        ],
        sources: &[
            user(
                "antigravity-cli/settings.json",
                Json,
                &Fields {
                    provider: &["modelProvider"],
                    model: &["model"],
                    limits: &[
                        ("agentMode", "agent_mode"),
                        ("toolPermission", "tool_permission"),
                        ("permissions.deny", "denied_tool_rules"),
                    ],
                    ..NONE
                },
            ),
            user("config/mcp_config.json", Json, &MCP),
            project(".agents/mcp_config.json", Json, &MCP),
        ],
        inventories: &[
            inv(U, "config/skills", Skills, SKILLS),
            inv(U, "antigravity-cli/skills", Skills, SKILLS),
            inv(P, ".agents/skills", Skills, SKILLS),
            inv(U, "config/agents", Subagents, Entries::DirsOrFiles(&["md"])),
            inv(
                P,
                ".agents/agents",
                Subagents,
                Entries::DirsOrFiles(&["md"]),
            ),
            inv(U, "config/plugins", Extensions, Entries::DirsOrFiles(&[])),
        ],
        trust: Trust::Check(antigravity_trust),
        ..BASE
    },
    // https://www.kimi.com/code/docs/en/kimi-code-cli/ (configuration, mcp, skills,
    // agents, data-locations, tools: KIMI_CODE_SESSION_ID in tool shells).
    Signature {
        id: "kimi-code",
        paths: &[
            ("/.kimi-code/bin/", "cli"),
            ("/node_modules/@moonshot-ai/kimi-code/", "cli"),
        ],
        arg_surfaces: &[("acp", "acp"), ("web", "web")],
        packages: &["@moonshot-ai/kimi-code"],
        env: &[("KIMI_CODE_SESSION_ID", "")],
        commands: &["kimi"],
        root: home(&[("KIMI_CODE_HOME", "")], ".kimi-code"),
        markers: &["config.toml", "session_index.jsonl"],
        version_file: Some(("updates/install.json", "lastSuccess.version")),
        sources: &[
            user(
                "config.toml",
                Toml,
                &Fields {
                    model: &["default_model"],
                    thinking: &["thinking.effort"],
                    limits: &[("default_permission_mode", "permission_mode")],
                    ..NONE
                },
            ),
            project(".kimi-code/mcp.json", Json, &MCP),
            user("mcp.json", Json, &MCP),
        ],
        inventories: &[
            inv(U, "skills", Skills, Entries::DirsOrFiles(&["md"])),
            inv(
                P,
                ".kimi-code/skills",
                Skills,
                Entries::DirsOrFiles(&["md"]),
            ),
            inv(U, "agents", Subagents, MD),
            inv(P, ".kimi-code/agents", Subagents, MD),
        ],
        // Project config is gated by workspace trust kept in undocumented state files.
        trust: Trust::Check(|_| None),
        special: Some(kimi_model),
        ..BASE
    },
    // Legacy Python kimi-cli 1.x (uv tool `kimi-cli`, root ~/.kimi via KIMI_SHARE_DIR).
    Signature {
        id: "kimi-cli-legacy",
        names: &["kimi-cli"],
        paths: &[
            ("/uv/tools/kimi-cli/", "cli"),
            ("/site-packages/kimi_cli/", "cli"),
        ],
        root: home(&[("KIMI_SHARE_DIR", "")], ".kimi"),
        markers: &["kimi.json"],
        sources: &[
            user(
                "config.toml",
                Toml,
                &Fields {
                    model: &["default_model"],
                    thinking: &["default_thinking"],
                    limits: &[("default_yolo", "yolo")],
                    ..NONE
                },
            ),
            user("mcp.json", Json, &MCP),
        ],
        inventories: &[inv(U, "skills", Skills, SKILLS)],
        special: Some(kimi_legacy),
        ..BASE
    },
    // Grok Build docs shipped in ~/.grok/docs/user-guide (05-configuration,
    // 08-skills, 16-subagents, 26-config-reference). `agent` is Grok's second name
    // and also Cursor's CLI name, so only resolved install paths identify it.
    Signature {
        id: "grok-build",
        paths: &[("/.grok/downloads/grok-", "cli"), ("/.grok/bin/", "cli")],
        arg_surfaces: &[("agent", "acp"), ("stdio", "acp")],
        commands: &["grok"],
        root: home(&[("GROK_HOME", "")], ".grok"),
        markers: &["config.toml", ".metadata_version"],
        version_file: Some((".metadata_version", "")),
        sources: &[
            project(
                ".grok/config.toml",
                Toml,
                &Fields {
                    mcp: &["mcp_servers"],
                    limits: &[("permission.deny", "denied_tool_rules")],
                    ..NONE
                },
            ),
            user(
                "config.toml",
                Toml,
                &Fields {
                    model: &["models.default"],
                    thinking: &["models.default_reasoning_effort"],
                    mcp: &["mcp_servers"],
                    limits: &[
                        ("ui.permission_mode", "permission_mode"),
                        ("permission.deny", "denied_tool_rules"),
                    ],
                    ..NONE
                },
            ),
        ],
        inventories: &[
            inv(U, "skills", Skills, SKILLS),
            inv(P, ".grok/skills", Skills, SKILLS),
            inv(U, "agents", Subagents, MD),
            inv(P, ".grok/agents", Subagents, MD),
            inv(U, "plugins", Extensions, Entries::DirsOrFiles(&[])),
        ],
        trust: Trust::Check(grok_trust),
        special: Some(grok_disabled_mcp),
        ..BASE
    },
    // WorkBuddy.app Info.plist (com.tencent.workbuddy.mac); bundled CLI product.json
    // (dataFolderName .workbuddy, WORKBUDDY_CONFIG_DIR); workbuddy.ai MCP guide.
    Signature {
        id: "workbuddy",
        surface: "desktop",
        paths: &[
            ("/WorkBuddy.app/", "desktop"),
            ("/.workbuddy/binaries/", "desktop"),
        ],
        bundles: &[("com.tencent.workbuddy.mac", "desktop")],
        apps: &["WorkBuddy.app"],
        root: home(&[("WORKBUDDY_CONFIG_DIR", "")], ".workbuddy"),
        markers: &["settings.json", "last-launch.json"],
        version_file: Some(("last-launch.json", "version")),
        sources: &[
            user(
                "settings.json",
                Json,
                &Fields {
                    extensions: &["enabledPlugins"],
                    ..NONE
                },
            ),
            user("mcp.json", Json, &MCP),
            project(".workbuddy/mcp.json", Json, &MCP),
        ],
        inventories: &[
            inv(U, "skills", Skills, SKILLS),
            inv(P, ".workbuddy/skills", Skills, SKILLS),
        ],
        visibility: Some("model_selection_not_in_local_config"),
        ..BASE
    },
    // github.com/deepseek-ai/deepseek-harness (apps/cli/package.json bin `dsh`,
    // packages/shell/shell-env: DSH_* for model shell calls, docs/config-catalog.md).
    Signature {
        id: "deepseek-harness",
        paths: &[("/node_modules/@deepseek-ai/dsh/", "cli")],
        arg_surfaces: &[("acp", "acp"), ("web", "web"), ("sdk", "sdk")],
        packages: &["@deepseek-ai/dsh"],
        env: &[("DSH_SESSION_ID", "")],
        commands: &["dsh"],
        root: home(&[("DSH_HOME", "")], ".dsh"),
        markers: &["cordis.patch.yml", "profiles"],
        inventories: &[
            inv(U, "skills", Skills, SKILLS),
            inv(P, ".dsh/skills", Skills, SKILLS),
            inv(U, "profiles", Extensions, Entries::DirsOrFiles(&[])),
        ],
        special: Some(dsh_patch),
        ..BASE
    },
    // https://cursor.com/docs (cli/reference/configuration, mcp, skills, subagents,
    // agent/tools/terminal: CURSOR_AGENT; enterprise/deployment-patterns: bundle ids).
    Signature {
        id: "cursor",
        paths: &[
            ("/.local/share/cursor-agent/versions/", "cli"),
            ("/Cursor.app/", "ide"),
            ("/programs/cursor/", "ide"),
        ],
        arg_surfaces: &[("acp", "acp")],
        bundles: &[
            ("com.todesktop.230313mzl4w4u92", "ide"),
            ("co.anysphere.cursor.nightly", "ide"),
        ],
        env: &[("CURSOR_AGENT", "")],
        commands: &["cursor-agent"],
        apps: &["Cursor.app"],
        root: home(&[("CURSOR_CONFIG_DIR", "")], ".cursor"),
        markers: &["cli-config.json", "mcp.json"],
        sources: &[
            project(
                ".cursor/cli.json",
                Json,
                &Fields {
                    limits: &[("permissions.deny", "denied_tool_rules")],
                    ..NONE
                },
            ),
            user(
                "cli-config.json",
                Json,
                &Fields {
                    model: &["model"],
                    limits: &[
                        ("approvalMode", "approval"),
                        ("sandbox.mode", "sandbox"),
                        ("permissions.deny", "denied_tool_rules"),
                    ],
                    ..NONE
                },
            ),
            project(".cursor/mcp.json", Json, &MCP),
            user("mcp.json", Json, &MCP),
        ],
        inventories: &[
            inv(U, "skills", Skills, SKILLS),
            inv(P, ".cursor/skills", Skills, SKILLS),
            inv(U, "agents", Subagents, MD),
            inv(P, ".cursor/agents", Subagents, MD),
            inv(U, "plugins/local", Extensions, Entries::DirsOrFiles(&[])),
        ],
        // MCP toggles and IDE model choice live in editor state, not these files.
        visibility: Some("ide_model_and_mcp_toggles_not_observable"),
        ..BASE
    },
    // github.com/zai-org/ZCode (desktop-product-identity.mjs, installer.mjs,
    // apps/zcode-cli/docs/usage/03-config.md, tool/handlers/bash.ts).
    Signature {
        id: "zcode",
        names: &["zcode", "zcode-preview"],
        paths: &[
            ("/.zcode/runtime/", "cli"),
            ("/ZCode.app/", "desktop"),
            ("/ZCode Preview.app/", "desktop"),
        ],
        arg_surfaces: &[("--web", "web")],
        packages: &["zcode-runtime"],
        bundles: &[
            ("dev.zcode.app", "desktop"),
            ("dev.zcode.app.preview", "desktop"),
        ],
        env: &[("ZCODE_SESSION_ID", "")],
        commands: &["zcode"],
        apps: &["ZCode.app", "ZCode Preview.app"],
        root: home(&[("ZCODE_DATA_BASE_DIR", ".zcode")], ".zcode"),
        markers: &["v2/provider_config.json", "cli/config.json"],
        sources: &[
            user(
                "v2/provider_config.json",
                Json,
                &Fields {
                    provider: &["config.defaultModelSelection.providerId"],
                    model: &["config.defaultModelSelection.modelId"],
                    ..NONE
                },
            ),
            user(
                "cli/config.json",
                Json,
                &Fields {
                    mcp: &["mcp.servers"],
                    extensions: &["plugins.enabledPlugins"],
                    limits: &[
                        ("permission.mode", "permission_mode"),
                        ("permission.disallowedTools", "denied_tools"),
                    ],
                    ..NONE
                },
            ),
            project("zcode.json", Json, &ZCODE_MCP),
            project(".zcode/config.json", Json, &ZCODE_MCP),
        ],
        inventories: &[
            inv(U, "skills", Skills, SKILLS),
            inv(P, ".zcode/skills", Skills, SKILLS),
            inv(P, ".zcode/agents", Subagents, MD),
        ],
        special: Some(zcode_provider_override),
        ..BASE
    },
    // Pi docs bundled with @earendil-works/pi-coding-agent (configuration.md,
    // settings.md, mcp.md, skills.md, security.md, environment-variables.md).
    Signature {
        id: "pi",
        paths: &[
            ("/node_modules/@earendil-works/pi-coding-agent/", "cli"),
            ("/node_modules/@mariozechner/pi-coding-agent/", "cli"),
            ("/.pi/agent/install/", "cli"),
            ("/.pi/agent/bin/", "cli"),
        ],
        arg_surfaces: &[("rpc", "rpc")],
        packages: &[
            "@earendil-works/pi-coding-agent",
            "@mariozechner/pi-coding-agent",
        ],
        // The CLI and RPC entry points export these to every child process.
        env: &[("PI_CODING_AGENT", "true"), ("AI_AGENT", "pi")],
        commands: &["pi"],
        root: home(&[("PI_CODING_AGENT_DIR", "")], ".pi/agent"),
        markers: &["settings.json", "trust.json"],
        version_file: Some(("install/current-version", "")),
        sources: &[
            project(".pi/settings.json", Json, &PI_SETTINGS),
            user("settings.json", Json, &PI_SETTINGS),
            project(".pi/mcp.json", Json, &MCP),
            user("mcp.json", Json, &MCP),
        ],
        inventories: &[
            inv(U, "skills", Skills, SKILLS),
            inv(P, ".pi/skills", Skills, SKILLS),
            inv(
                U,
                "extensions",
                Extensions,
                Entries::DirsOrFiles(&["ts", "js"]),
            ),
            inv(
                P,
                ".pi/extensions",
                Extensions,
                Entries::DirsOrFiles(&["ts", "js"]),
            ),
            // Subagents are not core Pi: definitions here serve subagent extensions.
            inv(U, "agents", Subagents, MD),
        ],
        trust: Trust::Check(pi_trust),
        visibility: Some("extensions_can_register_tools_at_runtime"),
        ..BASE
    },
    // NousResearch/hermes-agent docs (user-guide/configuration.md, desktop.md,
    // profiles.md, features/skills.md, reference/environment-variables.md,
    // reference/mcp-config-reference.md); apps/desktop/product-identity.cjs.
    Signature {
        id: "hermes",
        paths: &[
            ("/.hermes/tools/python-", "cli"),
            ("/.hermes/hermes-agent/", "cli"),
            ("/site-packages/hermes_cli/", "cli"),
            ("/Cellar/hermes-agent/", "cli"),
        ],
        arg_surfaces: &[
            ("gateway", "gateway"),
            ("serve", "server"),
            ("webapp", "server"),
            ("acp", "acp"),
        ],
        names: &["hermes-acp"],
        bundles: &[
            ("com.nousresearch.hermes", "desktop"),
            ("com.nousresearch.hermes-light", "desktop"),
            ("com.nousresearch.hermes-bundled", "desktop"),
        ],
        env: &[
            ("HERMES_SESSION_ID", ""),
            ("HERMES_AGENT", "true"),
            ("AI_AGENT", "hermes-agent"),
        ],
        commands: &["hermes"],
        root: Root {
            windows_local: Some("hermes"),
            select: Some(hermes_profile),
            ..home(&[("HERMES_HOME", "")], ".hermes")
        },
        markers: &["config.yaml", "SOUL.md"],
        version_file: Some(("hermes-agent/install-stamp.json", "baseVersion")),
        sources: &[user(
            "config.yaml",
            Yaml,
            &Fields {
                provider: &["model.provider"],
                model: &["model.default", "model"],
                thinking: &["agent.reasoning_effort"],
                mcp: &["mcp_servers"],
                extensions: &["plugins.enabled"],
                limits: &[
                    ("approvals.mode", "approval"),
                    ("terminal.backend", "terminal_backend"),
                ],
                ..NONE
            },
        )],
        inventories: &[
            inv(U, "skills", Skills, Entries::SkillTree),
            inv(P, ".hermes/skills", Skills, Entries::SkillTree),
            inv(U, "plugins", Extensions, Entries::DirsOrFiles(&[])),
        ],
        trust: Trust::Check(hermes_skill_trust),
        special: Some(hermes_backends),
        ..BASE
    },
    // opencode.ai/docs/config (V2) and anomalyco/opencode v1.19.6 config.mdx,
    // src/index.ts (OPENCODE=1), desktop electron-builder.config.ts (appId).
    Signature {
        id: "opencode",
        names: &["opencode", "opencode2"],
        paths: &[
            ("/node_modules/opencode-ai/", "cli"),
            ("/node_modules/@opencode/cli", "cli"),
            ("/Cellar/opencode/", "cli"),
            ("/OpenCode.app/", "desktop"),
        ],
        arg_surfaces: &[("serve", "server"), ("web", "web")],
        packages: &["opencode-ai", "@opencode/cli"],
        bundles: &[
            ("ai.opencode.desktop", "desktop"),
            ("ai.opencode.desktop.beta", "desktop"),
        ],
        env: &[("OPENCODE", "1"), ("OPENCODE_PID", "")],
        commands: &["opencode"],
        apps: &["OpenCode.app"],
        root: Root {
            base: Base::XdgConfig,
            ..home(&[], "opencode")
        },
        // A plugins directory alone is not evidence; other tools drop files there.
        markers: &["opencode.json", "opencode.jsonc", "config.json"],
        sources: &[
            project(".opencode/opencode.jsonc", Json, &OPENCODE),
            project(".opencode/opencode.json", Json, &OPENCODE),
            project("opencode.jsonc", Json, &OPENCODE),
            project("opencode.json", Json, &OPENCODE),
            user("opencode.jsonc", Json, &OPENCODE),
            user("opencode.json", Json, &OPENCODE),
            user("config.json", Json, &OPENCODE),
        ],
        inventories: &[
            inv(U, "skills", Skills, SKILLS),
            inv(P, ".opencode/skills", Skills, SKILLS),
            inv(U, "agents", Subagents, MD),
            inv(U, "agent", Subagents, MD),
            inv(P, ".opencode/agents", Subagents, MD),
            inv(
                U,
                "plugins",
                Extensions,
                Entries::DirsOrFiles(&["js", "ts"]),
            ),
            inv(
                P,
                ".opencode/plugins",
                Extensions,
                Entries::DirsOrFiles(&["js", "ts"]),
            ),
        ],
        special: Some(opencode_mcp),
        ..BASE
    },
    // docs.openclaw.ai (help/environment.md, gateway/config-extensions.md,
    // tools/skills.md); openclaw/openclaw apps/macos Info.plist (ai.openclaw.mac).
    Signature {
        id: "openclaw",
        names: &["openclaw"],
        paths: &[
            ("/node_modules/openclaw/", "cli"),
            ("/OpenClaw.app/", "desktop"),
        ],
        arg_surfaces: &[
            ("gateway", "gateway"),
            ("acp", "acp"),
            ("tui", "tui"),
            ("mcp", "mcp_server"),
        ],
        packages: &["openclaw"],
        bundles: &[("ai.openclaw.mac", "desktop")],
        env: &[("OPENCLAW_SHELL", ""), ("OPENCLAW_CLI", "1")],
        commands: &["openclaw"],
        apps: &["OpenClaw.app"],
        root: Root {
            select: Some(openclaw_profile),
            ..home(
                &[("OPENCLAW_STATE_DIR", ""), ("OPENCLAW_HOME", ".openclaw")],
                ".openclaw",
            )
        },
        markers: &["openclaw.json"],
        // Read by `openclaw_config` so that OPENCLAW_CONFIG_PATH replaces the default.
        inventories: &[
            inv(U, "skills", Skills, SKILLS),
            inv(U, "workspace/skills", Skills, SKILLS),
            inv(U, "extensions", Extensions, Entries::DirsOrFiles(&[])),
        ],
        special: Some(openclaw_config),
        visibility: Some("gateway_session_state_not_read"),
        ..BASE
    },
    // Recognition-only rows for further products: the same shared mechanism.
    // github.com/QwenLM/qwen-code docs/users/configuration/settings.md
    Signature {
        id: "qwen-code",
        paths: &[("/node_modules/@qwen-code/qwen-code/", "cli")],
        packages: &["@qwen-code/qwen-code"],
        commands: &["qwen"],
        root: home(&[("QWEN_HOME", "")], ".qwen"),
        markers: &["settings.json"],
        sources: &[
            project(".qwen/settings.json", Json, &MCP),
            user(
                "settings.json",
                Json,
                &Fields {
                    model: &["model.name"],
                    mcp: &["mcpServers"],
                    ..NONE
                },
            ),
        ],
        ..BASE
    },
    // github.com/github/copilot-cli README; docs.github.com Copilot CLI MCP page.
    Signature {
        id: "copilot-cli",
        paths: &[("/node_modules/@github/copilot/", "cli")],
        packages: &["@github/copilot"],
        commands: &["copilot"],
        root: home(&[("COPILOT_HOME", "")], ".copilot"),
        markers: &["config.json", "mcp-config.json"],
        sources: &[user("mcp-config.json", Json, &MCP)],
        ..BASE
    },
    // ampcode.com/docs (cli/settings, customize/mcp).
    Signature {
        id: "amp",
        paths: &[
            ("/node_modules/@ampcode/cli/", "cli"),
            ("/node_modules/@sourcegraph/amp/", "cli"),
        ],
        packages: &["@ampcode/cli", "@sourcegraph/amp"],
        commands: &["amp"],
        root: Root {
            base: Base::XdgConfig,
            ..home(&[], "amp")
        },
        markers: &["settings.json", "settings.jsonc"],
        sources: &[
            user("settings.json", Json, &AMP_MCP),
            project(".amp/settings.json", Json, &AMP_MCP),
        ],
        inventories: &[
            inv(U, "skills", Skills, SKILLS),
            inv(P, ".amp/skills", Skills, SKILLS),
        ],
        ..BASE
    },
    // github.com/Aider-AI/aider pyproject (`aider` = aider.main), aider.chat config docs.
    Signature {
        id: "aider",
        paths: &[("/aider-chat/", "cli"), ("/site-packages/aider/", "cli")],
        commands: &["aider"],
        root: home(&[], ""),
        markers: &[".aider.conf.yml"],
        sources: &[user(
            ".aider.conf.yml",
            Yaml,
            &Fields {
                model: &["model"],
                ..NONE
            },
        )],
        ..BASE
    },
];

// ---- Product-specific hooks -------------------------------------------------

/// Spellings under which harnesses record a project directory. On Windows the
/// canonical `\\?\C:\...` form is also offered as `C:\...` and `C:/...`.
fn project_keys(x: &Extraction<'_>) -> Vec<String> {
    let mut keys = Vec::new();
    for dir in x.project {
        let text = dir.to_string_lossy().into_owned();
        if cfg!(windows) {
            let plain = text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned();
            keys.push(plain.replace('\\', "/"));
            keys.push(plain);
        }
        keys.push(text);
    }
    keys
}

/// Claude Code keeps user/local MCP declarations and per-project trust in
/// `.claude.json` beside the root (inside it when `CLAUDE_CONFIG_DIR` is set).
fn claude_state(x: &mut Extraction<'_>) -> Option<bool> {
    let root = x.root?.to_owned();
    let path = if x.env.var("CLAUDE_CONFIG_DIR").is_some() {
        root.join(".claude.json")
    } else {
        root.parent()?.join(".claude.json")
    };
    let state = match x.load_max(&path, Json, 4 * 1024 * 1024) {
        Read::Parsed(v) => v,
        Read::Missing => return None,
        Read::Unreadable => {
            x.hidden("user_state_unreadable");
            return None;
        }
    };
    if let Some(servers) = state.get("mcpServers") {
        x.add(Mcp, config::declared(servers), "user_config");
    }
    let projects = state.get("projects").and_then(Value::as_object)?;
    let mut trusted = None;
    for key in project_keys(x) {
        let Some(entry) = projects.get(&key) else {
            continue;
        };
        if let Some(servers) = entry.get("mcpServers") {
            x.add(Mcp, config::declared(servers), "project_config");
        }
        // `.mcp.json` servers need explicit approval; list rejected ones as disabled.
        if let Some(rejected) = entry.get("disabledMcpjsonServers") {
            let mut names = config::declared(rejected);
            names.disabled = names.names.clone();
            x.mark_disabled(Mcp, &names.disabled);
        }
        if let Some(rejected) = entry.get("disabledMcpServers") {
            x.mark_disabled(Mcp, &config::declared(rejected).names);
        }
        // Trust accepted for an enclosing project covers this directory.
        match entry.get("hasTrustDialogAccepted").and_then(Value::as_bool) {
            Some(true) => trusted = Some(true),
            Some(false) if trusted.is_none() => trusted = Some(false),
            _ => {}
        }
    }
    trusted
}

fn codex_trust(x: &mut Extraction<'_>) -> Option<bool> {
    let root = x.root?.to_owned();
    let Read::Parsed(config) = x.load(&root.join("config.toml"), Toml) else {
        return None;
    };
    let projects = config.get("projects").and_then(Value::as_object)?;
    project_keys(x).iter().find_map(|key| {
        match projects.get(key)?.get("trust_level")?.as_str()? {
            "trusted" => Some(true),
            "untrusted" => Some(false),
            _ => None,
        }
    })
}

/// `profile = "<name>"` selects `[profiles.<name>]` (or `<root>/<name>.config.toml`)
/// over top-level model keys; `[agents.<name>]` tables declare subagents.
fn codex_profile(x: &mut Extraction<'_>) {
    let Some(root) = x.root.map(Path::to_owned) else {
        return;
    };
    let Read::Parsed(config) = x.load(&root.join("config.toml"), Toml) else {
        return;
    };
    if let Some(agents) = config.get("agents").and_then(Value::as_object) {
        let tables = Value::Object(
            agents
                .iter()
                .filter(|(_, v)| v.is_object())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );
        x.add(Subagents, config::declared(&tables), "user_config");
    }
    let Some(profile) = config.get("profile").and_then(config::scalar) else {
        return;
    };
    // A trusted project's `.codex/config.toml` outranks the selected profile.
    if x.model_source() == Some("project_config") {
        return;
    }
    let table = config
        .get("profiles")
        .and_then(|p| p.get(&profile))
        .cloned()
        .or_else(
            || match x.load(&root.join(format!("{profile}.config.toml")), Toml) {
                Read::Parsed(v) => Some(v),
                _ => None,
            },
        );
    match table {
        Some(table) => x.override_model(
            table.get("model_provider").and_then(config::scalar),
            table.get("model").and_then(config::scalar),
            table.get("model_reasoning_effort").and_then(config::scalar),
            "user_config",
        ),
        None => x.hidden("selected_profile_not_found"),
    }
}

/// `TRUST_FOLDER` trusts a directory and everything below it, `TRUST_PARENT` trusts
/// the entry's parent, and an exact `DO_NOT_TRUST` entry distrusts the directory.
fn gemini_trust(x: &mut Extraction<'_>) -> Option<bool> {
    let root = x.root?.to_owned();
    let path = x
        .env
        .var("GEMINI_CLI_TRUSTED_FOLDERS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("trustedFolders.json"));
    let Read::Parsed(Value::Object(folders)) = x.load(&path, Json) else {
        return None;
    };
    let keys = project_keys(x);
    // The first key is the cwd in the platform's plain spelling.
    let cwd = PathBuf::from(keys.first()?);
    let mut trusted = None;
    for (entry, value) in folders.iter().take(config::MAX_DIR_ENTRIES * 4) {
        let entry = Path::new(entry);
        match value.as_str() {
            Some("DO_NOT_TRUST") if keys.iter().any(|k| Path::new(k) == entry) => {
                return Some(false);
            }
            Some("TRUST_FOLDER") if cwd.starts_with(entry) => trusted = Some(true),
            Some("TRUST_PARENT") if entry.parent().is_some_and(|p| cwd.starts_with(p)) => {
                trusted = Some(true)
            }
            _ => {}
        }
    }
    trusted
}

fn antigravity_trust(x: &mut Extraction<'_>) -> Option<bool> {
    let root = x.root?.to_owned();
    let Read::Parsed(settings) = x.load(&root.join("antigravity-cli/settings.json"), Json) else {
        return None;
    };
    let trusted = settings.get("trustedWorkspaces")?.as_array()?;
    let keys = project_keys(x);
    // Absence from the list is not documented as distrust.
    trusted
        .iter()
        .filter_map(Value::as_str)
        .any(|t| keys.iter().any(|k| k == t))
        .then_some(true)
}

fn grok_trust(x: &mut Extraction<'_>) -> Option<bool> {
    let root = x.root?.to_owned();
    let Read::Parsed(trust) = x.load(&root.join("trusted_folders.toml"), Toml) else {
        return None;
    };
    let folders = trust.get("folders").and_then(Value::as_object)?;
    project_keys(x)
        .iter()
        .find_map(|k| folders.get(k)?.get("trusted")?.as_bool())
}

fn grok_disabled_mcp(x: &mut Extraction<'_>) {
    let Some(root) = x.root.map(Path::to_owned) else {
        return;
    };
    if let Read::Parsed(config) = x.load(&root.join("config.toml"), Toml)
        && let Some(disabled) = config.get("disabled_mcp_servers")
    {
        x.mark_disabled(Mcp, &config::declared(disabled).names);
    }
}

/// `default_model` names an alias in `[models.<alias>]` with `provider` and `model`.
fn kimi_model(x: &mut Extraction<'_>) {
    let Some(root) = x.root.map(Path::to_owned) else {
        return;
    };
    let Read::Parsed(config) = x.load(&root.join("config.toml"), Toml) else {
        return;
    };
    let Some(alias) = config.get("default_model").and_then(Value::as_str) else {
        return;
    };
    let Some(entry) = config.get("models").and_then(|m| m.get(alias)) else {
        return;
    };
    let thinking = match config.get("thinking").and_then(|t| t.get("enabled")) {
        Some(Value::Bool(false)) => Some("disabled".to_owned()),
        _ => config
            .get("thinking")
            .and_then(|t| t.get("effort"))
            .or_else(|| entry.get("default_effort"))
            .or_else(|| config.get("default_thinking"))
            .and_then(config::scalar),
    };
    x.override_model(
        entry.get("provider").and_then(config::scalar),
        entry.get("model").and_then(config::scalar),
        thinking,
        "user_config",
    );
}

fn kimi_legacy(x: &mut Extraction<'_>) {
    kimi_model(x);
    // Written by the Kimi Code migrator; the legacy root is kept, not removed.
    if x.root
        .is_some_and(|r| r.join(".migrated-to-kimi-code").exists())
    {
        x.constraint("migrated_to_kimi_code");
    }
}

/// Cordis patch layers: plugin entries `{name, config}` nested under list operations.
fn dsh_patch(x: &mut Extraction<'_>) {
    let Some(root) = x.root.map(Path::to_owned) else {
        return;
    };
    let patch = match x.load(&root.join("cordis.patch.yml"), Yaml) {
        Read::Parsed(v) => v,
        Read::Missing => return,
        Read::Unreadable => return x.hidden("user_config_unreadable"),
    };
    let mut entries = Vec::new();
    collect_plugins(&patch, &mut entries, 0);
    let mut servers = Names::default();
    for entry in entries {
        let name = entry.get("name").and_then(Value::as_str).unwrap_or("");
        let cfg = entry.get("config");
        let field = |k: &str| cfg.and_then(|c| c.get(k)).and_then(config::scalar);
        match name {
            "@deepseek-ai/dsh-agent-default-model" => x.override_model(
                field("provider"),
                field("model"),
                field("reasoningEffort"),
                "user_config",
            ),
            "@deepseek-ai/dsh-mcp-client" => {
                if let Some(server) = cfg.and_then(|c| c.get("serverName")) {
                    servers.merge(config::declared(&Value::Array(vec![server.clone()])));
                }
            }
            _ => {}
        }
    }
    x.add(Mcp, servers, "user_config");
    x.hidden("profile_layers_not_resolved");
}

fn collect_plugins<'a>(value: &'a Value, out: &mut Vec<&'a Value>, depth: usize) {
    if depth > 6 || out.len() > 256 {
        return;
    }
    match value {
        Value::Array(items) => items
            .iter()
            .for_each(|v| collect_plugins(v, out, depth + 1)),
        Value::Object(map) => {
            if map.get("name").is_some_and(Value::is_string) {
                out.push(value);
            }
            map.values()
                .filter(|v| v.is_array() || v.is_object())
                .for_each(|v| collect_plugins(v, out, depth + 1));
        }
        _ => {}
    }
}

fn zcode_provider_override(x: &mut Extraction<'_>) {
    let Some(path) = x
        .env
        .var("ZCODE_PERSONAL_PROVIDER_CONFIG_FILE")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    else {
        return;
    };
    if let Read::Parsed(v) = x.load(&path, Json) {
        let pick = |k: &str| config::get(&v, k).and_then(config::scalar);
        x.override_model(
            pick("config.defaultModelSelection.providerId"),
            pick("config.defaultModelSelection.modelId"),
            None,
            "environment_override",
        );
    }
}

/// trust.json maps directories to decisions; the closest ancestor decision wins,
/// then `defaultProjectTrust` (`always`/`never`) from the user settings.
fn pi_trust(x: &mut Extraction<'_>) -> Option<bool> {
    let root = x.root?.to_owned();
    if let Read::Parsed(Value::Object(map)) = x.load(&root.join("trust.json"), Json) {
        let cwd = x.project.first()?.clone();
        for dir in cwd.ancestors().take(64) {
            if let Some(decision) = map.get(&*dir.to_string_lossy()).and_then(Value::as_bool) {
                return Some(decision);
            }
        }
    }
    match x.load(&root.join("settings.json"), Json) {
        Read::Parsed(v) => match v.get("defaultProjectTrust").and_then(Value::as_str) {
            Some("always") => Some(true),
            Some("never") => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// `active_profile` selects `profiles/<name>/` as the effective home unless
/// `HERMES_HOME` already names one.
fn hermes_profile(root: PathBuf, env: &Env) -> PathBuf {
    if env.var("HERMES_HOME").is_some() {
        return root;
    }
    crate::safe::read(&root.join("active_profile"), 256)
        .and_then(|name| crate::safe::identifier(&name, 64))
        .filter(|name| !name.contains(['/', '.']) && name != "default")
        .map(|name| root.join("profiles").join(name))
        .filter(|dir| dir.is_dir())
        .unwrap_or(root)
}

/// Project skills load only after `hermes skills trust` records the project.
fn hermes_skill_trust(x: &mut Extraction<'_>) -> Option<bool> {
    let root = x.root?.to_owned();
    let Read::Parsed(config) = x.load(&root.join("config.yaml"), Yaml) else {
        return None;
    };
    let trusted = config::get(&config, "skills.trusted_project_dirs")?.as_array()?;
    let keys = project_keys(x);
    Some(
        trusted
            .iter()
            .filter_map(Value::as_str)
            .any(|t| keys.iter().any(|k| k == t)),
    )
}

/// Desktop SSH gateways run Hermes on another machine; only their presence is noted.
fn hermes_backends(x: &mut Extraction<'_>) {
    let Some(root) = x.root.map(Path::to_owned) else {
        return;
    };
    if let Some(Ok(names)) =
        config::entries(&root.join("desktop-ssh"), Entries::DirsOrFiles(&["json"]))
        && names.count() > 0
    {
        x.constraint("remote_ssh_gateways_declared");
    }
}

/// OpenCode V1 declares `mcp.<name>`; V2 declares `mcp.servers.<name>` beside
/// scalar settings such as `mcp.timeout`.
fn opencode_mcp(x: &mut Extraction<'_>) {
    let user: Vec<PathBuf> = x
        .root
        .map(|r| {
            ["opencode.jsonc", "opencode.json", "config.json"]
                .map(|f| r.join(f))
                .to_vec()
        })
        .unwrap_or_default();
    let project: Vec<PathBuf> = x
        .project
        .iter()
        .flat_map(|d| {
            [
                "opencode.json",
                "opencode.jsonc",
                ".opencode/opencode.json",
                ".opencode/opencode.jsonc",
            ]
            .map(|f| d.join(f))
        })
        .collect();
    for (paths, tag) in [(user, "user_config"), (project, "project_config")] {
        for path in paths {
            let Read::Parsed(v) = x.load(&path, Json) else {
                continue;
            };
            let Some(mcp) = v.get("mcp").and_then(Value::as_object) else {
                continue;
            };
            let servers = match mcp.get("servers") {
                Some(servers) => servers.clone(),
                None => Value::Object(
                    mcp.iter()
                        .filter(|(_, v)| v.is_object())
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                ),
            };
            x.add(Mcp, config::declared(&servers), tag);
        }
    }
}

/// `OPENCLAW_PROFILE` moves the default state dir to `~/.openclaw-<profile>`.
fn openclaw_profile(root: PathBuf, env: &Env) -> PathBuf {
    if env.var("OPENCLAW_STATE_DIR").is_some() || env.var("OPENCLAW_HOME").is_some() {
        return root;
    }
    match env
        .var("OPENCLAW_PROFILE")
        .and_then(|p| crate::safe::identifier(p, 64))
    {
        Some(profile) if !profile.contains(['/', '.']) => {
            root.with_file_name(format!(".openclaw-{profile}"))
        }
        _ => root,
    }
}

const OPENCLAW: Source = user(
    "openclaw.json",
    Json,
    &Fields {
        model: &["agents.defaults.model.primary", "agents.defaults.model"],
        thinking: &["agents.defaults.thinkingDefault"],
        mcp: &["mcp.servers"],
        agents: &["agents.entries", "agents.list"],
        extensions: &["plugins.entries"],
        limits: &[
            ("tools.deny", "denied_tools"),
            ("tools.profile", "tool_profile"),
            ("agents.defaults.sandbox.mode", "sandbox"),
        ],
        ..NONE
    },
);

/// `OPENCLAW_CONFIG_PATH` names the active config file; the default location is then
/// not consulted, so a stale copy is never reported as effective.
fn openclaw_config(x: &mut Extraction<'_>) {
    let path = match x.env.var("OPENCLAW_CONFIG_PATH").map(PathBuf::from) {
        Some(p) if p.is_absolute() => p,
        Some(_) => return x.hidden("config_path_override_not_absolute"),
        None => match x.root {
            Some(root) => root.join(OPENCLAW.file),
            None => return,
        },
    };
    x.apply(&path, &OPENCLAW);
}
