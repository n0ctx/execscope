//! Fixture-driven harness tests. Every fixture lives in a temporary home; the
//! collector receives an injected environment, so real user settings are never read.

use super::*;
use crate::brief;
use signatures::{BASE, Fields, MCP, NONE, SKILLS, home, inv, user};
use std::fs;

struct Fixture {
    _dir: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
    vars: HashMap<String, String>,
    chain: Vec<process::Ancestor>,
    commands: HashMap<String, Vec<PathBuf>>,
    unavailable: bool,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        // Canonical paths keep the project keys stable on macOS (/var -> /private/var).
        let base = fs::canonicalize(dir.path()).unwrap();
        let home = base.join("home");
        let project = home.join("work").join("app");
        fs::create_dir_all(project.join(".git")).unwrap();
        Self {
            _dir: dir,
            home,
            project,
            vars: HashMap::new(),
            chain: vec![],
            commands: HashMap::new(),
            unavailable: false,
        }
    }

    /// Writes a file relative to the fixture home and returns its path.
    fn file(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.home.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    fn project_file(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.project.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    fn skill(&self, dir: &str, name: &str) {
        self.file(&format!("{dir}/{name}/SKILL.md"), "---\nname: x\n---\nbody");
    }

    fn app(&self, name: &str, bundle_id: &str, version: &str) -> PathBuf {
        let app = self.home.join("Applications").join(name);
        self.file(
            &format!("Applications/{name}/Contents/Info.plist"),
            &format!(
                "<?xml version=\"1.0\"?><plist><dict>\n<key>CFBundleIdentifier</key>\n\t<string>{bundle_id}</string>\n<key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>"
            ),
        );
        app
    }

    /// Adds the next (more distant) ancestor; relative paths are under the fixture home.
    fn ancestor(&mut self, exe: &str, args: &[&str]) -> &mut Self {
        let exe = if Path::new(exe).is_absolute() {
            PathBuf::from(exe)
        } else {
            self.home.join(exe)
        };
        self.chain.push(process::Ancestor {
            exe: Some(exe),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
        });
        self
    }

    /// An interpreter ancestor running a script under the fixture home.
    fn script(&mut self, interpreter: &str, script: &str, extra: &[&str]) -> &mut Self {
        let script = self.home.join(script);
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(&script, "// launcher fixture; never executed").unwrap();
        let mut args = vec![
            interpreter.to_owned(),
            script.to_string_lossy().into_owned(),
        ];
        args.extend(extra.iter().map(|a| (*a).to_owned()));
        self.chain.push(process::Ancestor {
            exe: Some(PathBuf::from(interpreter)),
            args,
        });
        self
    }

    fn var(&mut self, key: &str, value: &str) -> &mut Self {
        self.vars.insert(key.into(), value.into());
        self
    }

    fn env(&self) -> Env {
        let commands = self.commands.clone();
        Env {
            home: Some(self.home.clone()),
            vars: self.vars.clone(),
            ancestry: process::Ancestry {
                chain: self.chain.clone(),
                unavailable: self.unavailable,
            },
            commands: Box::new(move |name| commands.get(name).cloned().unwrap_or_default()),
            app_dirs: vec![self.home.join("Applications")],
        }
    }

    fn run(&self, focused: bool) -> Harness {
        self.run_with(focused, SIGNATURES)
    }

    fn run_with(&self, focused: bool, signatures: &[Signature]) -> Harness {
        collect(
            &self.project,
            Context {
                mcp_server: false,
                focused,
            },
            &self.env(),
            signatures,
        )
    }
}

fn find<'a>(h: &'a Harness, id: &str) -> &'a Candidate {
    h.candidates
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("{id} missing: {:?}", ids(h)))
}

fn ids(h: &Harness) -> Vec<&'static str> {
    h.candidates.iter().map(|c| c.id).collect()
}

fn names(c: &Candidate, cap: Cap) -> Vec<String> {
    let mut caps = c.capabilities.clone();
    caps.slot(cap)
        .as_ref()
        .map(|i| i.all.names.clone())
        .unwrap_or_default()
}

fn model(c: &Candidate) -> (Option<&str>, Option<&str>, Option<&str>, &'static str) {
    let m = c.configured_model.as_ref().expect("configured model");
    (
        m.provider.as_deref(),
        m.model.as_deref(),
        m.thinking.as_deref(),
        m.source,
    )
}

fn json(h: &Harness) -> String {
    serde_json::to_string(h).unwrap()
}

fn rendered(h: &Harness, budget: usize) -> String {
    brief::render(h.brief(true), budget)
}

const SECRET: &str = "SECRET_VALUE_0123";

// ---- Fourteen recognized products: positive identity, collisions, fields ----

#[test]
fn claude_code_native_install_settings_state_and_trust() {
    let mut f = Fixture::new();
    f.ancestor(".local/share/claude/versions/2.1.330", &["claude"]);
    f.file(
        ".claude/settings.json",
        &format!(
            r#"{{"model":"opus","effortLevel":"high","permissions":{{"defaultMode":"plan","deny":["Bash(rm *)"]}},
               "enabledPlugins":{{"fmt@market":true,"old@market":false}},"env":{{"TOKEN":"{SECRET}"}},"futureKey":{{"x":1}}}}"#
        ),
    );
    let project = f.project.to_string_lossy().into_owned();
    f.file(
        ".claude.json",
        &serde_json::json!({
            "mcpServers": {"docs": {"command": "x", "env": {"API_KEY": SECRET}}},
            "projects": {project: {"hasTrustDialogAccepted": false, "mcpServers": {"local-db": {}},
                "disabledMcpjsonServers": ["shared"]}}
        })
        .to_string(),
    );
    f.skill(".claude/skills", "review");
    f.file(".claude/agents/explorer.md", "---\nname: explorer\n---\n");
    f.project_file(".mcp.json", r#"{"mcpServers":{"shared":{"command":"y"}}}"#);
    f.project_file(".claude/settings.json", r#"{"model":"haiku"}"#);
    let h = f.run(false);
    assert_eq!(
        (h.primary, h.association),
        (Some("claude-code"), "process_ancestor")
    );
    let c = find(&h, "claude-code");
    assert_eq!(c.version.as_ref().unwrap().value, "2.1.330");
    // Untrusted project settings do not change the configured model.
    assert_eq!(model(c), (None, Some("opus"), Some("high"), "user_config"));
    assert_eq!(names(c, Cap::Mcp), ["docs", "local-db", "shared"]);
    assert_eq!(c.capabilities.mcp.as_ref().unwrap().disabled, ["shared"]);
    assert_eq!(names(c, Cap::Skills), ["review"]);
    assert_eq!(names(c, Cap::Subagents), ["explorer"]);
    assert_eq!(
        c.capabilities.extensions.as_ref().unwrap().disabled,
        ["old@market"]
    );
    for constraint in [
        "permission_mode=plan",
        "denied_tool_rules=1",
        "project_untrusted",
    ] {
        assert!(
            c.constraints.iter().any(|x| x == constraint),
            "{:?}",
            c.constraints
        );
    }
    assert!(!json(&h).contains(SECRET));
}

#[test]
fn claude_code_trusted_project_model_and_env_override() {
    let mut f = Fixture::new();
    f.ancestor(".local/share/claude/versions/2.1.330", &[]);
    f.file(".claude/settings.json", r#"{"model":"opus"}"#);
    let project = f.project.to_string_lossy().into_owned();
    f.file(
        ".claude.json",
        &serde_json::json!({"projects": {project: {"hasTrustDialogAccepted": true}}}).to_string(),
    );
    f.project_file(".claude/settings.local.json", r#"{"model":"sonnet"}"#);
    let c = find(&f.run(false), "claude-code").clone();
    assert_eq!(model(&c).1, Some("sonnet"));
    assert_eq!(model(&c).3, "project_config");
    f.var("ANTHROPIC_MODEL", "custom-model-2031");
    let c = find(&f.run(false), "claude-code").clone();
    assert_eq!(model(&c).1, Some("custom-model-2031"));
    assert_eq!(model(&c).3, "environment_override");
}

#[test]
fn claude_desktop_and_shared_claude_files_are_not_claude_code() {
    let mut f = Fixture::new();
    f.app("Claude.app", "com.anthropic.claudefordesktop", "1.0");
    f.ancestor("Applications/Claude.app/Contents/MacOS/Claude", &[]);
    f.var("CLAUDE_PROJECT_DIR", "/x")
        .var("CLAUDE_CODE_SESSION_ID", "1");
    f.project_file("CLAUDE.md", "x");
    f.project_file("AGENTS.md", "x");
    f.project_file(".claude/skills/a/SKILL.md", "x");
    f.project_file(".agents/skills/b/SKILL.md", "x");
    let h = f.run(true);
    assert_eq!(h.primary, None);
    assert!(h.candidates.is_empty(), "{:?}", ids(&h));
}

#[test]
fn codex_npm_launcher_profile_trust_and_project_precedence() {
    let mut f = Fixture::new();
    let pkg = ".npm-global/lib/node_modules/@openai/codex";
    f.file(
        &format!("{pkg}/package.json"),
        r#"{"name":"@openai/codex","version":"0.169.0"}"#,
    );
    f.script("/usr/local/bin/node", &format!("{pkg}/bin/codex.js"), &[]);
    let project = f.project.to_string_lossy().into_owned();
    f.file(
        ".codex/config.toml",
        &format!(
            "model = \"gpt-base\"\nmodel_reasoning_effort = \"high\"\nprofile = \"fast\"\nsandbox_mode = \"read-only\"\nunknown_future = 3\n\
             [profiles.fast]\nmodel = \"gpt-fast\"\n\
             [mcp_servers.computer]\ncommand = \"x\"\nenabled = false\n[mcp_servers.docs]\nurl = \"https://h/mcp\"\nbearer_token_env_var = \"{SECRET}\"\n\
             [model_providers.local]\nexperimental_bearer_token = \"{SECRET}\"\n\
             [agents]\nmax_concurrent_threads_per_session = 4\n[agents.reviewer]\ndescription = \"r\"\n\
             [projects.\"{project}\"]\ntrust_level = \"trusted\"\n"
        ),
    );
    f.file(".codex/agents/default.toml", "name = \"default\"\n");
    f.skill(".codex/skills", "lint");
    let h = f.run(false);
    let c = find(&h, "codex");
    assert_eq!(c.version.as_ref().unwrap().value, "0.169.0");
    assert_eq!(c.version.as_ref().unwrap().source, "package_metadata");
    assert_eq!(
        model(c),
        (None, Some("gpt-fast"), Some("high"), "user_config")
    );
    assert_eq!(names(c, Cap::Mcp), ["computer", "docs"]);
    assert_eq!(c.capabilities.mcp.as_ref().unwrap().disabled, ["computer"]);
    assert_eq!(names(c, Cap::Subagents), ["default", "reviewer"]);
    assert!(c.constraints.contains(&"sandbox=read-only".to_owned()));
    assert!(!json(&h).contains(SECRET));
    // A trusted project config outranks the profile.
    f.project_file(".codex/config.toml", "model = \"gpt-project\"\n");
    let c = find(&f.run(false), "codex").clone();
    assert_eq!(model(&c).1, Some("gpt-project"));
    assert!(!c.constraints.iter().any(|x| x.starts_with("project_")));
}

#[test]
fn codex_untrusted_project_is_declared_only() {
    let mut f = Fixture::new();
    f.ancestor(
        ".codex/packages/standalone/releases/0.170.0/bin/codex",
        &["codex"],
    );
    let project = f.project.to_string_lossy().into_owned();
    f.file(
        ".codex/config.toml",
        &format!("model = \"gpt-user\"\n[projects.\"{project}\"]\ntrust_level = \"untrusted\"\n"),
    );
    f.file(
        ".codex/packages/standalone/current/codex-package.json",
        r#"{"version":"0.170.0","target":"aarch64-apple-darwin"}"#,
    );
    f.project_file(
        ".codex/config.toml",
        "model = \"gpt-project\"\n[mcp_servers.proj]\ncommand=\"x\"\n",
    );
    let c = find(&f.run(false), "codex").clone();
    assert_eq!(model(&c).1, Some("gpt-user"));
    assert_eq!(c.version.as_ref().unwrap().source, "install_metadata");
    assert_eq!(names(&c, Cap::Mcp), ["proj"]);
    assert!(c.constraints.contains(&"project_untrusted".to_owned()));
}

#[test]
fn codex_home_without_its_config_is_not_configured() {
    let f = Fixture::new();
    f.file(".codex/AGENTS.md", "written by another tool");
    assert!(f.run(true).candidates.is_empty());
}

#[test]
fn gemini_cli_identity_trust_and_shared_gemini_folder() {
    let mut f = Fixture::new();
    let pkg = ".npm/lib/node_modules/@google/gemini-cli";
    f.file(
        &format!("{pkg}/package.json"),
        r#"{"name":"@google/gemini-cli","version":"0.70.0"}"#,
    );
    f.script(
        "/opt/homebrew/bin/node",
        &format!("{pkg}/bundle/gemini.js"),
        &[],
    );
    f.file(
        ".gemini/settings.json",
        r#"{"model":{"name":"gemini-9-pro"},"general":{"defaultApprovalMode":"plan"},"mcpServers":{"fs":{"command":"x"}}}"#,
    );
    let project = f.project.to_string_lossy().into_owned();
    f.file(
        ".gemini/trustedFolders.json",
        &serde_json::json!({ project: "DO_NOT_TRUST" }).to_string(),
    );
    f.project_file(
        ".gemini/settings.json",
        r#"{"model":{"name":"project-model"}}"#,
    );
    f.file(".gemini/extensions/ext-one/gemini-extension.json", "{}");
    let h = f.run(false);
    let c = find(&h, "gemini-cli");
    assert_eq!(c.version.as_ref().unwrap().value, "0.70.0");
    assert_eq!(model(c).1, Some("gemini-9-pro"));
    assert_eq!(names(c, Cap::Extensions), ["ext-one"]);
    assert!(c.constraints.contains(&"approval=plan".to_owned()));
    assert!(c.constraints.contains(&"project_untrusted".to_owned()));
    assert!(!ids(&h).contains(&"antigravity"));
}

#[test]
fn shared_gemini_config_folder_identifies_nothing() {
    let f = Fixture::new();
    f.file(".gemini/GEMINI.md", "<!-- written by another tool -->");
    f.file(".gemini/config/mcp_config.json", r#"{"mcpServers":{}}"#);
    f.skill(".gemini/config/skills", "shared");
    assert!(f.run(true).candidates.is_empty());
}

#[test]
fn antigravity_bundle_identity_and_settings() {
    let mut f = Fixture::new();
    f.app("Antigravity.app", "com.google.antigravity", "2.34.0");
    f.ancestor("Applications/Antigravity.app/Contents/MacOS/Electron", &[]);
    let project = f.project.to_string_lossy().into_owned();
    f.file(
        ".gemini/antigravity-cli/settings.json",
        &serde_json::json!({"model": "gemini-x", "modelProvider": "gemini", "agentMode": "plan",
            "trustedWorkspaces": [project]})
        .to_string(),
    );
    f.file(
        ".gemini/config/mcp_config.json",
        r#"{"mcpServers":{"maps":{"serverUrl":"https://h","disabled":true},"local":{"command":"x"}}}"#,
    );
    f.skill(".gemini/config/skills", "draw");
    let h = f.run(false);
    let c = find(&h, "antigravity");
    assert_eq!((c.surface, c.evidence[0]), ("desktop", "process_ancestor"));
    assert_eq!(c.version.as_ref().unwrap().source, "bundle_metadata");
    assert_eq!(
        model(c),
        (Some("gemini"), Some("gemini-x"), None, "user_config")
    );
    assert_eq!(c.capabilities.mcp.as_ref().unwrap().disabled, ["maps"]);
    assert!(c.constraints.contains(&"agent_mode=plan".to_owned()));
    assert!(!ids(&h).contains(&"gemini-cli"));
}

#[test]
fn renamed_app_with_foreign_bundle_id_does_not_match_by_folder_name() {
    let mut f = Fixture::new();
    f.app("Cursor.app", "com.example.not-cursor", "1.0");
    f.ancestor("Applications/Cursor.app/Contents/MacOS/Cursor", &[]);
    assert_eq!(f.run(false).primary, None);
}

#[test]
fn kimi_code_current_vs_legacy_and_chat_app() {
    let mut f = Fixture::new();
    f.ancestor(".kimi-code/bin/kimi", &["kimi", "acp"]);
    f.file(
        ".kimi-code/config.toml",
        &format!(
            "default_model = \"kimi-code/k2\"\ndefault_permission_mode = \"yolo\"\n[thinking]\nenabled = true\neffort = \"max\"\n\
             [models.\"kimi-code/k2\"]\nprovider = \"managed:kimi-code\"\nmodel = \"kimi-for-coding\"\n\
             [providers.\"managed:kimi-code\"]\napi_key = \"{SECRET}\"\n"
        ),
    );
    f.file(
        ".kimi-code/mcp.json",
        r#"{"mcpServers":{"a":{"command":"x"},"b":{"url":"https://h","enabled":false}}}"#,
    );
    f.file(
        ".kimi-code/updates/install.json",
        r#"{"lastSuccess":{"version":"2.5.2"}}"#,
    );
    f.file(".kimi/kimi.json", r#"{"work_dirs":[]}"#);
    f.file(".kimi/.migrated-to-kimi-code", "{}");
    let h = f.run(true);
    let c = find(&h, "kimi-code");
    assert_eq!((c.presence, c.surface), (Presence::ProcessLinked, "acp"));
    assert_eq!(c.version.as_ref().unwrap().value, "2.5.2");
    assert_eq!(
        model(c),
        (
            Some("managed:kimi-code"),
            Some("kimi-for-coding"),
            Some("max"),
            "user_config"
        )
    );
    assert_eq!(c.capabilities.mcp.as_ref().unwrap().disabled, ["b"]);
    assert!(c.constraints.contains(&"permission_mode=yolo".to_owned()));
    let legacy = find(&h, "kimi-cli-legacy");
    assert_eq!(legacy.presence, Presence::Configured);
    assert!(
        legacy
            .constraints
            .contains(&"migrated_to_kimi_code".to_owned())
    );
    assert!(!json(&h).contains(SECRET));

    let mut chat = Fixture::new();
    chat.app("Kimi.app", "com.moonshot.kimichat", "3.2.27");
    chat.ancestor("Applications/Kimi.app/Contents/MacOS/Kimi", &[]);
    assert_eq!(chat.run(false).primary, None);

    let mut legacy = Fixture::new();
    legacy.ancestor(".local/share/uv/tools/kimi-cli/bin/python3.13", &[]);
    legacy.script(
        ".local/share/uv/tools/kimi-cli/bin/python3.13",
        ".local/share/uv/tools/kimi-cli/bin/kimi",
        &[],
    );
    assert_eq!(legacy.run(false).primary, Some("kimi-cli-legacy"));
}

#[test]
fn grok_build_paths_acp_surface_and_agent_name_collision() {
    let mut f = Fixture::new();
    f.ancestor(
        ".grok/downloads/grok-1.0.77-macos-aarch64",
        &["agent", "stdio"],
    );
    f.file(".grok/.metadata_version", "1.0.77\n");
    f.file(
        ".grok/config.toml",
        "disabled_mcp_servers = [\"web\"]\n[models]\ndefault = \"grok-5\"\ndefault_reasoning_effort = \"medium\"\n\
         [ui]\npermission_mode = \"plan\"\n[mcp_servers.web]\nurl = \"https://h\"\n[mcp_servers.fs]\ncommand = \"x\"\n\
         [compat.claude]\nskills = true\n",
    );
    let project = f.project.to_string_lossy().into_owned();
    f.file(
        ".grok/trusted_folders.toml",
        &format!("[folders.\"{project}\"]\ntrusted = true\n"),
    );
    f.project_file(".grok/config.toml", "[mcp_servers.proj]\ncommand = \"y\"\n");
    // Grok imports Claude/Cursor resources; that must not make them linked.
    f.file(".claude/settings.json", r#"{"model":"opus"}"#);
    f.file(".cursor/mcp.json", r#"{"mcpServers":{"c":{}}}"#);
    let h = f.run(false);
    assert_eq!(h.primary, Some("grok-build"));
    let c = find(&h, "grok-build");
    assert_eq!(
        (c.surface, c.version.as_ref().unwrap().value.as_str()),
        ("acp", "1.0.77")
    );
    assert_eq!(
        model(c),
        (None, Some("grok-5"), Some("medium"), "user_config")
    );
    assert_eq!(names(c, Cap::Mcp), ["proj", "fs", "web"]);
    assert_eq!(c.capabilities.mcp.as_ref().unwrap().disabled, ["web"]);
    assert_eq!(ids(&h), ["grok-build"]);
}

#[test]
fn cursor_cli_ide_bundle_and_grok_bot_collision() {
    let mut f = Fixture::new();
    let dir = ".local/share/cursor-agent/versions/2026.11.22-e9042b2";
    f.script(
        &f.home.join(dir).join("node").to_string_lossy(),
        &format!("{dir}/index.js"),
        &["acp"],
    );
    f.file(
        ".cursor/cli-config.json",
        r#"{"version":1,"model":"composer-2","approvalMode":"allowlist","permissions":{"allow":[],"deny":["Shell(rm)"]}}"#,
    );
    f.file(
        ".cursor/mcp.json",
        &format!(r#"{{"mcpServers":{{"remote":{{"url":"https://h","headers":{{"Authorization":"Bearer {SECRET}"}}}}}}}}"#),
    );
    f.project_file(
        ".cursor/mcp.json",
        r#"{"mcpServers":{"proj":{"command":"x"}}}"#,
    );
    let h = f.run(false);
    let c = find(&h, "cursor");
    assert_eq!(c.surface, "acp");
    assert_eq!(model(c).1, Some("composer-2"));
    assert_eq!(names(c, Cap::Mcp), ["proj", "remote"]);
    assert!(c.constraints.contains(&"denied_tool_rules=1".to_owned()));
    assert!(
        c.visibility
            .contains(&"ide_model_and_mcp_toggles_not_observable")
    );
    assert!(!json(&h).contains(SECRET));

    let mut ide = Fixture::new();
    ide.app("Cursor.app", "com.todesktop.230313mzl4w4u92", "3.1.0");
    ide.ancestor("Applications/Cursor.app/Contents/Frameworks/Cursor Helper.app/Contents/MacOS/Cursor Helper", &[]);
    let h = ide.run(false);
    assert_eq!(find(&h, "cursor").surface, "ide");

    let mut bot = Fixture::new();
    bot.app("Grok Bot.app", "com.anysphere.sand", "0.75.8");
    bot.ancestor("Applications/Grok Bot.app/Contents/MacOS/Grok Bot", &[]);
    bot.ancestor(".local/bin/agent", &["agent"]);
    let h = bot.run(false);
    assert_eq!(h.primary, None, "{:?}", ids(&h));
}

#[test]
fn workbuddy_bundle_and_codebuddy_collision() {
    let mut f = Fixture::new();
    f.app("WorkBuddy.app", "com.tencent.workbuddy.mac", "5.11.3");
    f.ancestor("Applications/WorkBuddy.app/Contents/MacOS/Electron", &[]);
    f.file(
        ".workbuddy/settings.json",
        &format!(r#"{{"enabledPlugins":{{"docs@builtin":true,"ui@experts":false}},"claw":{{"token":"{SECRET}"}}}}"#),
    );
    f.file(
        ".workbuddy/mcp.json",
        r#"{"mcpServers":{"demo":{"type":"stdio","command":"x"}}}"#,
    );
    f.skill(".workbuddy/skills", "slides");
    let h = f.run(false);
    let c = find(&h, "workbuddy");
    assert_eq!(
        (c.surface, c.version.as_ref().unwrap().value.as_str()),
        ("desktop", "5.11.3")
    );
    assert_eq!(names(c, Cap::Mcp), ["demo"]);
    assert_eq!(
        c.capabilities.extensions.as_ref().unwrap().disabled,
        ["ui@experts"]
    );
    assert!(c.configured_model.is_none());
    assert!(
        c.visibility
            .contains(&"model_selection_not_in_local_config")
    );
    assert!(!json(&h).contains(SECRET));

    let other = Fixture::new();
    other.file(".codebuddy/settings.json", "{}");
    other.file(".codebuddy/logs/x", "");
    assert!(other.run(true).candidates.is_empty());
}

#[test]
fn deepseek_harness_npm_patch_layers_and_name_collision() {
    let mut f = Fixture::new();
    let pkg = ".npm/lib/node_modules/@deepseek-ai/dsh";
    f.file(
        &format!("{pkg}/package.json"),
        r#"{"name":"@deepseek-ai/dsh","version":"0.2.3-alpha.1"}"#,
    );
    f.script("/usr/bin/node", &format!("{pkg}/lib/bin.js"), &["acp"]);
    f.file(
        ".dsh/cordis.patch.yml",
        &format!(
            "- insert:\n    - id: default-model\n      name: '@deepseek-ai/dsh-agent-default-model'\n      config:\n        provider: deepseek\n        model: deepseek-v9\n        reasoningEffort: high\n    - id: mcp-demo\n      name: '@deepseek-ai/dsh-mcp-client'\n      config:\n        serverName: demo\n        transport: stdio\n        env:\n          KEY: {SECRET}\n"
        ),
    );
    f.skill(".dsh/skills", "plan");
    let h = f.run(false);
    let c = find(&h, "deepseek-harness");
    assert_eq!(
        (c.surface, c.version.as_ref().unwrap().value.as_str()),
        ("acp", "0.2.3-alpha.1")
    );
    assert_eq!(
        model(c),
        (
            Some("deepseek"),
            Some("deepseek-v9"),
            Some("high"),
            "user_config"
        )
    );
    assert_eq!(names(c, Cap::Mcp), ["demo"]);
    assert!(!json(&h).contains(SECRET));

    let mut other = Fixture::new();
    other.ancestor("/usr/bin/dsh", &["dsh"]);
    other.var("DEEPSEEK_API_KEY", SECRET);
    other.file(".config/other/config.json", r#"{"provider":"deepseek"}"#);
    assert_eq!(other.run(false).primary, None);
}

#[test]
fn zcode_runtime_provider_config_and_zed_collision() {
    let mut f = Fixture::new();
    let release = ".zcode/runtime/releases/3.18.2";
    f.file(
        &format!("{release}/package.json"),
        r#"{"name":"zcode-runtime","version":"3.18.2"}"#,
    );
    f.script(
        "/usr/local/bin/node",
        &format!("{release}/bin/zcode.mjs"),
        &["--web"],
    );
    f.file(
        ".zcode/v2/provider_config.json",
        &format!(
            r#"{{"schemaVersion":1,"config":{{"providerConfigRules":{{"providerRules":[{{"providerId":"zai","config":{{"access":{{"apiKey":"{SECRET}"}}}}}}]}},
               "defaultModelSelection":{{"providerId":"zai","modelId":"glm-9","options":{{}}}}}}}}"#
        ),
    );
    f.file(
        ".zcode/cli/config.json",
        r#"{"permission":{"mode":"plan"},"mcp":{"servers":{"ex":{"command":"x","enabled":false}}}}"#,
    );
    let h = f.run(false);
    let c = find(&h, "zcode");
    assert_eq!(
        (c.surface, c.version.as_ref().unwrap().value.as_str()),
        ("web", "3.18.2")
    );
    assert_eq!(model(c), (Some("zai"), Some("glm-9"), None, "user_config"));
    assert_eq!(c.capabilities.mcp.as_ref().unwrap().disabled, ["ex"]);
    assert!(c.constraints.contains(&"permission_mode=plan".to_owned()));
    assert!(!json(&h).contains(SECRET));

    let mut zed = Fixture::new();
    zed.app("Zed.app", "dev.zed.Zed", "0.200.0");
    zed.ancestor("Applications/Zed.app/Contents/MacOS/zed", &[]);
    assert_eq!(zed.run(false).primary, None);
}

#[test]
fn pi_managed_install_trust_and_extension_caveat() {
    let mut f = Fixture::new();
    let pkg = ".pi/agent/install/releases/1.6.0/node_modules/@earendil-works/pi-coding-agent";
    f.file(
        &format!("{pkg}/package.json"),
        r#"{"name":"@earendil-works/pi-coding-agent","version":"1.6.0"}"#,
    );
    f.script(
        "/usr/local/bin/node",
        &format!("{pkg}/dist/bundle/cli.js"),
        &[],
    );
    f.file(
        ".pi/agent/settings.json",
        r#"{"defaultProvider":"local-qwen","defaultModel":"qwen3.8-27b","defaultThinkingLevel":"xhigh","packages":["npm:pi-subagent"]}"#,
    );
    f.file(
        ".pi/agent/models.json",
        &format!(r#"{{"providers":{{"local-qwen":{{"apiKey":"{SECRET}"}}}}}}"#),
    );
    f.file(
        ".pi/agent/mcp.json",
        r#"{"mcpServers":{"docs":{"command":"x","enabled":false}}}"#,
    );
    f.file(".pi/agent/extensions/pi-subagent.ts", "export default {}");
    f.file(".pi/agent/agents/explorer.md", "---\nname: explorer\n---");
    let parent = f.project.parent().unwrap().to_string_lossy().into_owned();
    f.file(
        ".pi/agent/trust.json",
        &serde_json::json!({ parent: true }).to_string(),
    );
    f.project_file(".pi/settings.json", r#"{"defaultModel":"qwen-project"}"#);
    let h = f.run(false);
    let c = find(&h, "pi");
    assert_eq!(c.version.as_ref().unwrap().value, "1.6.0");
    // Trusted via the closest ancestor decision; project settings override the user's.
    assert_eq!(
        model(c),
        (
            Some("local-qwen"),
            Some("qwen-project"),
            Some("xhigh"),
            "project_config"
        )
    );
    assert_eq!(
        names(c, Cap::Extensions),
        ["npm:pi-subagent", "pi-subagent"]
    );
    assert_eq!(names(c, Cap::Subagents), ["explorer"]);
    assert!(
        c.visibility
            .contains(&"extensions_can_register_tools_at_runtime")
    );
    assert!(!json(&h).contains(SECRET));
    let text = rendered(&h, 600);
    assert!(
        text.contains("harness=pi process_linked(process_ancestor)"),
        "{text}"
    );
    assert!(
        text.contains("model_config=local-qwen/qwen-project thinking=xhigh"),
        "{text}"
    );

    let mut other = Fixture::new();
    other.ancestor("/usr/local/bin/pi", &["pi"]);
    assert_eq!(other.run(false).primary, None);
}

#[test]
fn hermes_bundled_python_gateway_profiles_and_remote_backend() {
    let mut f = Fixture::new();
    f.ancestor(
        ".hermes/tools/python-3.14.9+20261101-darwin-arm64/bin/python3",
        &[
            "python3",
            "-I",
            "-c",
            "from hermes_cli.main import main",
            "gateway",
        ],
    );
    f.file(
        ".hermes/config.yaml",
        &format!(
            "model:\n  default: hermes-5\n  provider: nous\n  api_key: {SECRET}\nagent:\n  reasoning_effort: high\nmcp_servers:\n  docs:\n    command: npx\n    env:\n      KEY: \"{SECRET}\"\n    enabled: false\n  web:\n    url: https://h\napprovals:\n  mode: \"off\"\nterminal:\n  backend: ssh\nplugins:\n  enabled: [rtk-rewrite]\nskills:\n  trusted_project_dirs: []\n"
        ),
    );
    f.file(
        ".hermes/hermes-agent/install-stamp.json",
        r#"{"baseVersion":"0.23.3","displayVersion":"0.23.3+4.gabc"}"#,
    );
    f.file(".hermes/skills/software/review/SKILL.md", "x");
    f.file(".hermes/skills/writing/notes/SKILL.md", "x");
    f.file(".hermes/desktop-ssh/gateway-1.json", "{}");
    f.project_file(".hermes/skills/proj/SKILL.md", "x");
    let h = f.run(false);
    let c = find(&h, "hermes");
    assert_eq!(
        (c.surface, c.version.as_ref().unwrap().value.as_str()),
        ("gateway", "0.23.3")
    );
    assert_eq!(
        model(c),
        (Some("nous"), Some("hermes-5"), Some("high"), "user_config")
    );
    assert_eq!(c.capabilities.mcp.as_ref().unwrap().disabled, ["docs"]);
    assert_eq!(names(c, Cap::Skills), ["notes", "review", "proj"]);
    for constraint in [
        "approval=off",
        "terminal_backend=ssh",
        "remote_ssh_gateways_declared",
        "project_untrusted",
    ] {
        assert!(
            c.constraints.iter().any(|x| x == constraint),
            "{:?}",
            c.constraints
        );
    }
    assert!(!json(&h).contains(SECRET));

    // A sticky profile relocates the effective home; HERMES_HOME wins over it.
    f.file(".hermes/active_profile", "work\n");
    f.file(
        ".hermes/profiles/work/config.yaml",
        "model:\n  default: work-model\n",
    );
    assert_eq!(model(find(&f.run(false), "hermes")).1, Some("work-model"));
    let alt = f.file("alt-home/config.yaml", "model: plain-model\n");
    f.var("HERMES_HOME", &alt.parent().unwrap().to_string_lossy());
    assert_eq!(model(find(&f.run(false), "hermes")).1, Some("plain-model"));

    let mut engine = Fixture::new();
    engine.ancestor("/opt/homebrew/bin/hermes", &["hermes", "bundle.js"]);
    engine.app("Hermes.app", "com.nousresearch.hermes.setup", "0.0.1");
    engine.ancestor("Applications/Hermes.app/Contents/MacOS/Hermes-Setup", &[]);
    assert_eq!(engine.run(false).primary, None);
}

#[test]
fn opencode_v1_and_v2_shapes_and_plugin_dir_collision() {
    let mut f = Fixture::new();
    f.ancestor(".opencode/bin/opencode", &["opencode", "serve"]);
    f.file(
        ".config/opencode/opencode.json",
        &format!(
            r#"{{ // JSONC
              "model": "anthropic/claude-x", "provider": {{"a": {{"options": {{"apiKey": "{SECRET}"}}}}}},
              "mcp": {{"v1": {{"type":"local","command":["x"],"enabled":false}}}},
              "agent": {{"review": {{"mode":"subagent"}}}}, "plugin": ["opencode-foo"],
            }}"#
        ),
    );
    f.project_file(
        "opencode.json",
        r#"{"model":"openai/project","mcp":{"timeout":5,"servers":{"v2":{"type":"remote","url":"https://h","disabled":true}}}}"#,
    );
    let h = f.run(false);
    let c = find(&h, "opencode");
    assert_eq!(c.surface, "server");
    assert_eq!(model(c).1, Some("openai/project"));
    assert_eq!(names(c, Cap::Mcp), ["v1", "v2"]);
    assert_eq!(c.capabilities.mcp.as_ref().unwrap().disabled, ["v1", "v2"]);
    assert_eq!(names(c, Cap::Subagents), ["review"]);
    assert!(!json(&h).contains(SECRET));

    let other = Fixture::new();
    other.file(".config/opencode/plugins/terminal-activity.js", "x");
    assert!(other.run(true).candidates.is_empty());
}

#[test]
fn openclaw_json5_gateway_and_config_path_override() {
    let mut f = Fixture::new();
    let pkg = ".npm/lib/node_modules/openclaw";
    f.file(
        &format!("{pkg}/package.json"),
        r#"{"name":"openclaw","version":"2026.11.6"}"#,
    );
    f.script(
        "/usr/local/bin/node",
        &format!("{pkg}/openclaw.mjs"),
        &["gateway", "run"],
    );
    f.file(
        ".openclaw/openclaw.json",
        &format!(
            "{{\n  // JSON5\n  agents: {{ defaults: {{ model: {{ primary: 'anthropic/claude-x' }}, thinkingDefault: 'low', sandbox: {{ mode: 'non-main' }} }},\n    entries: {{ main: {{}}, helper: {{}} }} }},\n  models: {{ providers: {{ p: {{ apiKey: '{SECRET}' }} }} }},\n  mcp: {{ servers: {{ docs: {{ command: 'x', enabled: false }}, }} }},\n  tools: {{ deny: ['browser'] }},\n}}\n"
        ),
    );
    f.file(".openclaw/workspace/SOUL.md", "shared name; not identity");
    let h = f.run(false);
    let c = find(&h, "openclaw");
    assert_eq!(
        (c.surface, c.version.as_ref().unwrap().value.as_str()),
        ("gateway", "2026.11.6")
    );
    assert_eq!(
        model(c),
        (None, Some("anthropic/claude-x"), Some("low"), "user_config")
    );
    assert_eq!(names(c, Cap::Subagents), ["helper", "main"]);
    assert!(c.constraints.contains(&"denied_tools=1".to_owned()));
    assert!(c.constraints.contains(&"sandbox=non-main".to_owned()));
    assert!(!json(&h).contains(SECRET));
    // The override replaces the default file instead of merging with stale content.
    let alt = f.file(
        "elsewhere/openclaw.json",
        "{agents:{defaults:{model:'openai/other'}}}",
    );
    f.var("OPENCLAW_CONFIG_PATH", &alt.to_string_lossy());
    let c = find(&f.run(false), "openclaw").clone();
    assert_eq!(model(&c).1, Some("openai/other"));
    assert!(c.capabilities.mcp.is_none());
}

// ---- Association rules ------------------------------------------------------

#[test]
fn generic_terminal_and_unavailable_ancestry_attribute_nothing() {
    let mut f = Fixture::new();
    f.ancestor("/bin/zsh", &["-zsh"])
        .ancestor("/usr/bin/login", &[])
        .ancestor(
            "/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal",
            &[],
        );
    f.var("TERM_PROGRAM", "vscode")
        .var("AI_AGENT", "unknown-agent_1");
    let h = f.run(false);
    assert_eq!((h.primary, h.association), (None, "none"));
    assert!(h.candidates.is_empty());
    assert!(rendered(&h, 600).contains("harness=none_process_linked"));
    f.chain.clear();
    f.unavailable = true;
    assert!(f.run(false).notes.contains(&"process_ancestry_unavailable"));
}

#[test]
fn auto_mode_does_not_scan_unlinked_configs() {
    let f = Fixture::new();
    f.file(".codex/config.toml", "model = \"x\"\n");
    f.file(".grok/config.toml", "[models]\ndefault = \"y\"\n");
    f.file(".pi/agent/settings.json", "{}");
    let auto = f.run(false);
    assert!(auto.candidates.is_empty(), "{:?}", ids(&auto));
    let focused = f.run(true);
    assert_eq!(ids(&focused), ["codex", "grok-build", "pi"]);
    assert!(
        focused
            .candidates
            .iter()
            .all(|c| c.presence == Presence::Configured)
    );
    assert_eq!(focused.primary, None);
    assert!(rendered(&focused, 600).contains("other_harnesses=codex(configured)"));
}

#[test]
fn nested_harnesses_prefer_the_nearest_ancestor() {
    let mut f = Fixture::new();
    let pkg = ".npm/lib/node_modules/@openai/codex";
    f.script("/usr/local/bin/node", &format!("{pkg}/bin/codex.js"), &[]);
    f.ancestor(".local/share/claude/versions/2.1.330", &[]);
    f.var("CLAUDECODE", "1");
    let h = f.run(false);
    assert_eq!(
        (h.primary, h.association),
        (Some("codex"), "process_ancestor")
    );
    assert!(h.notes.contains(&"nested_or_multiple_harness_context"));
    assert_eq!(find(&h, "claude-code").presence, Presence::ProcessLinked);
    assert!(rendered(&h, 600).contains("also_linked=claude-code"));
}

#[test]
fn environment_hints_alone_are_weak_and_can_be_ambiguous() {
    let mut f = Fixture::new();
    f.var("PI_CODING_AGENT", "true");
    let h = f.run(false);
    assert_eq!((h.primary, h.association), (Some("pi"), "environment_hint"));
    f.var("OPENCODE", "1");
    let h = f.run(false);
    assert_eq!((h.primary, h.association), (None, "ambiguous"));
    assert!(h.notes.contains(&"ambiguous_harness_association"));
    assert!(rendered(&h, 600).contains("harness=ambiguous candidates=opencode,pi"));
}

#[test]
fn mcp_server_context_is_noted() {
    let mut f = Fixture::new();
    f.ancestor(".local/share/claude/versions/2.1.330", &[]);
    let h = collect(
        &f.project,
        Context {
            mcp_server: true,
            focused: false,
        },
        &f.env(),
        SIGNATURES,
    );
    assert!(h.notes.contains(&"mcp_server_context_not_caller_session"));
}

#[test]
fn installed_evidence_requires_a_verified_target() {
    let mut f = Fixture::new();
    let target = f.file(".local/share/claude/versions/2.2.0", "binary");
    f.commands.insert("claude".into(), vec![target]);
    // A same-named command elsewhere is not an install of the product.
    f.commands
        .insert("pi".into(), vec![PathBuf::from("/usr/local/bin/pi")]);
    let h = f.run(true);
    assert_eq!(ids(&h), ["claude-code"]);
    let c = find(&h, "claude-code");
    assert_eq!(
        (c.presence, c.version.as_ref().unwrap().value.as_str()),
        (Presence::Installed, "2.2.0")
    );
}

// ---- Version tolerance, field-level failure and inventory refresh ------------

#[test]
fn newer_versions_unknown_keys_and_new_names_need_no_code_change() {
    let mut f = Fixture::new();
    f.ancestor(".local/share/claude/versions/99.12.0", &[]);
    f.file(
        ".claude/settings.json",
        r#"{"model":"brand-new-model-2031","effortLevel":"ultra","newSection":{"deep":{"nested":[1,2]}},"anotherFlag":true}"#,
    );
    f.file(
        ".claude.json",
        r#"{"mcpServers":{"zz-new-server":{"transport":"quantum"}},"futureState":{"a":1}}"#,
    );
    f.skill(".claude/skills", "skill-invented-later");
    let c = find(&f.run(false), "claude-code").clone();
    assert_eq!(c.version.as_ref().unwrap().value, "99.12.0");
    assert_eq!(
        model(&c),
        (
            None,
            Some("brand-new-model-2031"),
            Some("ultra"),
            "user_config"
        )
    );
    assert_eq!(names(&c, Cap::Mcp), ["zz-new-server"]);
    assert_eq!(names(&c, Cap::Skills), ["skill-invented-later"]);
}

#[test]
fn malformed_and_missing_sources_only_lose_their_own_fields() {
    let mut f = Fixture::new();
    f.ancestor(".local/share/claude/versions/2.1.330", &[]);
    f.file(".claude/settings.json", r#"{"model":"opus"}"#);
    f.file(".claude.json", "{ not json");
    f.skill(".claude/skills", "kept");
    f.project_file(".mcp.json", "[[[");
    let c = find(&f.run(false), "claude-code").clone();
    assert_eq!(model(&c).1, Some("opus"));
    assert_eq!(names(&c, Cap::Skills), ["kept"]);
    assert!(c.visibility.contains(&"user_state_unreadable"));
    assert!(c.visibility.contains(&"project_config_unreadable"));
    assert!(c.capabilities.mcp.is_none());
    // Removing the optional key loses only the model.
    f.file(".claude/settings.json", r#"{"permissions":{}}"#);
    let c = find(&f.run(false), "claude-code").clone();
    assert!(c.configured_model.is_none());
    assert_eq!(names(&c, Cap::Skills), ["kept"]);
}

#[cfg(unix)]
#[test]
fn inaccessible_file_is_a_visibility_limit_not_an_absence() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        return; // root can read anything
    }
    let mut f = Fixture::new();
    f.ancestor(".grok/downloads/grok-1.0.77-macos-aarch64", &[]);
    let config = f.file(".grok/config.toml", "[models]\ndefault = \"grok-5\"\n");
    f.skill(".grok/skills", "still-visible");
    fs::set_permissions(&config, fs::Permissions::from_mode(0o000)).unwrap();
    let c = find(&f.run(false), "grok-build").clone();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(c.configured_model.is_none());
    assert!(c.visibility.contains(&"user_config_unreadable"));
    assert_eq!(names(&c, Cap::Skills), ["still-visible"]);
}

#[test]
fn breaking_layout_keeps_identity_without_fabricated_fields() {
    let mut f = Fixture::new();
    f.ancestor(".grok/downloads/grok-3.0.0-macos-aarch64", &[]);
    // A future release moved everything to a new unknown file.
    f.file(".grok/settings.v3.db.json", r#"{"models":{"default":"x"}}"#);
    let h = f.run(false);
    let c = find(&h, "grok-build");
    assert_eq!(c.presence, Presence::ProcessLinked);
    assert!(c.configured_model.is_none() && c.capabilities.is_empty());
}

#[test]
fn root_overrides_are_honored_and_relative_ones_ignored() {
    let mut f = Fixture::new();
    f.ancestor(".codex/packages/standalone/releases/1/bin/codex", &[]);
    f.file(".codex/config.toml", "model = \"default-home\"\n");
    let alt = f.file("alt-codex/config.toml", "model = \"alt-home\"\n");
    f.var("CODEX_HOME", &alt.parent().unwrap().to_string_lossy());
    assert_eq!(model(find(&f.run(false), "codex")).1, Some("alt-home"));
    f.var("CODEX_HOME", "relative/dir");
    let c = find(&f.run(false), "codex").clone();
    assert!(c.configured_model.is_none());
    let mut g = Fixture::new();
    g.ancestor("/x/node_modules/@google/gemini-cli/bundle/gemini.js", &[]);
    let alt = g.file(
        "gemini-home/.gemini/settings.json",
        r#"{"model":{"name":"alt"}}"#,
    );
    let base = alt
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    g.var("GEMINI_CLI_HOME", &base);
    assert_eq!(model(find(&g.run(false), "gemini-cli")).1, Some("alt"));
}

#[test]
fn inventory_changes_appear_on_the_next_inspection() {
    let mut f = Fixture::new();
    f.ancestor(".kimi-code/bin/kimi", &[]);
    f.skill(".kimi-code/skills", "first");
    let count = |f: &Fixture| names(find(&f.run(false), "kimi-code"), Cap::Skills);
    assert_eq!(count(&f), ["first"]);
    f.skill(".kimi-code/skills", "second");
    f.file(".kimi-code/mcp.json", r#"{"mcpServers":{"added":{}}}"#);
    assert_eq!(count(&f), ["first", "second"]);
    assert_eq!(names(find(&f.run(false), "kimi-code"), Cap::Mcp), ["added"]);
    fs::remove_dir_all(f.home.join(".kimi-code/skills/first")).unwrap();
    assert_eq!(count(&f), ["second"]);
}

#[test]
fn a_new_product_needs_only_a_signature_row() {
    static FICTIONAL: &[Signature] = &[Signature {
        id: "acme-agent",
        paths: &[("/opt/acme/bin/", "cli")],
        env: &[("ACME_AGENT", "1")],
        root: home(&[("ACME_HOME", "")], ".acme"),
        markers: &["settings.json"],
        sources: &[
            user(
                "settings.json",
                Format::Json,
                &Fields {
                    model: &["model"],
                    ..NONE
                },
            ),
            user("mcp.json", Format::Json, &MCP),
        ],
        inventories: &[inv(Scope::User, "skills", Cap::Skills, SKILLS)],
        ..BASE
    }];
    let mut f = Fixture::new();
    f.ancestor("/opt/acme/bin/acme", &[]);
    f.file(".acme/settings.json", r#"{"model":"acme-1"}"#);
    f.file(".acme/mcp.json", r#"{"mcpServers":{"tools":{}}}"#);
    f.skill(".acme/skills", "deploy");
    let h = f.run_with(false, FICTIONAL);
    let c = find(&h, "acme-agent");
    assert_eq!(model(c).1, Some("acme-1"));
    assert_eq!(names(c, Cap::Mcp), ["tools"]);
    assert_eq!(names(c, Cap::Skills), ["deploy"]);
    assert!(SIGNATURES.iter().all(|s| s.id != "acme-agent"));
}

#[test]
fn candidate_list_is_bounded_and_deterministic() {
    let f = Fixture::new();
    for sig in SIGNATURES {
        if let Some(marker) = sig.markers.first()
            && let Some(root) = resolve(&sig.root, &f.env())
        {
            let path = root.join(marker);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "{}").unwrap();
        }
    }
    let a = f.run(true);
    let b = f.run(true);
    assert!(a.candidates.len() <= MAX_CANDIDATES);
    assert_eq!(
        a.omitted_candidates,
        SIGNATURES.len().saturating_sub(MAX_CANDIDATES)
    );
    assert_eq!(ids(&a), ids(&b));
    assert_eq!(json(&a), json(&b));
}

#[test]
fn hostile_names_are_counted_not_printed() {
    let mut f = Fixture::new();
    f.ancestor(".kimi-code/bin/kimi", &[]);
    f.file(
        ".kimi-code/mcp.json",
        &format!(
            r#"{{"mcpServers":{{"ok":{{}},"$(rm -rf ~)":{{}},"https://user:{SECRET}@h":{{}},"{}":{{}},"{}evil":{{}}}}}}"#,
            "x".repeat(500),
            '\u{202e}'
        ),
    );
    let h = f.run(false);
    let mcp = find(&h, "kimi-code").capabilities.mcp.clone().unwrap();
    assert_eq!(
        (mcp.count, mcp.names.as_slice()),
        (5, ["ok".to_owned()].as_slice())
    );
    let out = json(&h);
    assert!(!out.contains(SECRET) && !out.contains("rm -rf") && !out.contains("xxxxxxxx"));
}

#[test]
fn brief_budgets_keep_identity_first() {
    let mut f = Fixture::new();
    f.ancestor(".local/share/claude/versions/2.1.330", &[]);
    f.file(
        ".claude/settings.json",
        r#"{"model":"opus","permissions":{"defaultMode":"plan"}}"#,
    );
    for i in 0..40 {
        f.skill(
            ".claude/skills",
            &format!("skill-number-{i:02}-with-a-long-name"),
        );
    }
    let h = f.run(false);
    let small = rendered(&h, 100);
    let large = rendered(&h, 3000);
    assert!(brief::estimate(&small) <= 100, "{small}");
    assert!(small.contains("harness=claude-code"), "{small}");
    assert!(small.contains("permission_mode=plan"), "{small}");
    assert!(large.contains("declared_skills=40+"), "{large}");
    assert!(large.len() > small.len());
    let compact = brief::render(h.brief(false), 600);
    assert!(compact.contains("declared_skills=40+ skill-number-00-with-a-long-name,skill-number-01-with-a-long-name,skill-number-02-with-a-long-name\n"), "{compact}");
}
