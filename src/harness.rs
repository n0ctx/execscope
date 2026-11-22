//! Harness awareness: which agent harness launched this process, what it is
//! configured to use, and what may block the agent.
//!
//! Detection is driven by the small declarative table in `signatures`. Identity
//! comes from process ancestry, environment hints set by the harness, verified
//! install locations or bundle metadata, never from shared files such as
//! `AGENTS.md` alone. Configuration is read through the bounded readers in
//! `config` and reported as *configured* or *declared*, never as live state.

mod config;
mod process;
mod signatures;

use crate::{brief::Line, safe};
use config::{Format, Names, Read};
use serde::Serialize;
use signatures::{Base, Cap, Root, SIGNATURES, Scope, Signature, Source, Trust};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

const MAX_CANDIDATES: usize = 12;
/// Total configuration files and directories read in one inspection.
const MAX_READS: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Presence {
    ProcessLinked,
    Configured,
    Installed,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Version {
    value: String,
    source: &'static str,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct ConfiguredModel {
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<String>,
    source: &'static str,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Inventory {
    /// Distinct declared/discovered entries, including any not safe to name.
    count: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    names: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    disabled: Vec<String>,
    sources: Vec<&'static str>,
    /// A bounded read stopped early; the real number may be higher.
    #[serde(skip_serializing_if = "crate::model::is_false")]
    limited: bool,
    #[serde(skip)]
    all: Names,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Capabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    mcp: Option<Inventory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skills: Option<Inventory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subagents: Option<Inventory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extensions: Option<Inventory>,
}

impl Capabilities {
    fn slot(&mut self, cap: Cap) -> &mut Option<Inventory> {
        match cap {
            Cap::Mcp => &mut self.mcp,
            Cap::Skills => &mut self.skills,
            Cap::Subagents => &mut self.subagents,
            Cap::Extensions => &mut self.extensions,
        }
    }

    fn add(&mut self, cap: Cap, names: Names, source: &'static str) {
        if names.count() == 0 && !names.limited {
            return;
        }
        let slot = self.slot(cap).get_or_insert_with(Inventory::default);
        // Project configuration overrides the user's for the same name.
        slot.all
            .merge(names, if source == "project_config" { 2 } else { 1 });
        if !slot.sources.contains(&source) {
            slot.sources.push(source);
        }
        slot.count = slot.all.count();
        slot.names = slot
            .all
            .names
            .iter()
            .take(config::MAX_NAMES)
            .cloned()
            .collect();
        slot.disabled = slot.all.disabled.clone();
        slot.limited = slot.all.limited;
    }

    fn is_empty(&self) -> bool {
        self.mcp.is_none()
            && self.skills.is_none()
            && self.subagents.is_none()
            && self.extensions.is_none()
    }

    fn iter(&self) -> impl Iterator<Item = (&'static str, &Inventory)> {
        [
            ("mcp", &self.mcp),
            ("skills", &self.skills),
            ("subagents", &self.subagents),
            ("extensions", &self.extensions),
        ]
        .into_iter()
        .filter_map(|(k, v)| Some((k, v.as_ref()?)))
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Candidate {
    id: &'static str,
    surface: &'static str,
    presence: Presence,
    /// Source tags for the identity evidence, strongest first.
    evidence: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<Version>,
    #[serde(skip_serializing_if = "Option::is_none")]
    configured_model: Option<ConfiguredModel>,
    #[serde(skip_serializing_if = "Capabilities::is_empty")]
    capabilities: Capabilities,
    /// Actionable restrictions or eligibility caveats from configuration.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    constraints: Vec<String>,
    /// Sources that could not be observed; absence there is not proof of absence.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    visibility: Vec<&'static str>,
    #[serde(skip)]
    distance: Option<usize>,
    #[serde(skip)]
    root: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Harness {
    inspection_boundary: &'static str,
    primary: Option<&'static str>,
    association: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    candidates: Vec<Candidate>,
    #[serde(skip_serializing_if = "is_zero")]
    omitted_candidates: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<&'static str>,
    #[serde(skip)]
    focused: bool,
}

fn is_zero(v: &usize) -> bool {
    *v == 0
}

/// What the caller already knows about the host context.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Context {
    /// ExecScope runs as a (possibly long-lived) MCP server.
    pub(crate) mcp_server: bool,
    /// Focused `scope=harness`/`all`: enumerate installed and configured candidates too.
    pub(crate) focused: bool,
}

/// Resolves a command name to canonical executable targets.
pub(crate) type Resolver = Box<dyn Fn(&str) -> Vec<PathBuf>>;
/// Per-inspection cache: application bundle -> (identifier, short version).
type Bundles = HashMap<PathBuf, Option<(String, Option<String>)>>;

/// Environment the collector reads; injectable so fixtures never touch real settings.
pub(crate) struct Env {
    pub(crate) home: Option<PathBuf>,
    pub(crate) vars: HashMap<String, String>,
    pub(crate) ancestry: process::Ancestry,
    /// PATH resolution for install evidence: command name -> canonical targets.
    pub(crate) commands: Resolver,
    /// Absolute directories that may hold application bundles.
    pub(crate) app_dirs: Vec<PathBuf>,
}

impl Env {
    pub(crate) fn process(cwd: &Path) -> Self {
        let vars = std::env::vars_os()
            .take(4096)
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
            .filter(|(k, _)| k.len() <= 128)
            .collect();
        #[cfg(not(windows))]
        let home = std::env::var_os("HOME");
        #[cfg(windows)]
        let home = std::env::var_os("USERPROFILE");
        let home = home.map(PathBuf::from).filter(|p| p.is_absolute());
        let search = crate::lookup::Search::process();
        let cwd = cwd.to_owned();
        let mut app_dirs = vec![];
        if cfg!(target_os = "macos") {
            app_dirs.push(PathBuf::from("/Applications"));
            if let Some(home) = &home {
                app_dirs.push(home.join("Applications"));
            }
        }
        Self {
            home,
            vars,
            ancestry: process::ancestry(),
            commands: Box::new(move |name| {
                search
                    .find(name, &cwd)
                    .candidates
                    .into_iter()
                    .filter(|c| c.executable)
                    .filter_map(|c| c.target)
                    .take(4)
                    .collect()
            }),
            app_dirs,
        }
    }

    fn var(&self, name: &str) -> Option<&str> {
        self.vars
            .get(name)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }
}

pub(crate) fn inspect(cwd: &Path, context: Context) -> Harness {
    collect(cwd, context, &Env::process(cwd), SIGNATURES)
}

fn collect(cwd: &Path, context: Context, env: &Env, signatures: &[Signature]) -> Harness {
    let mut reads = MAX_READS;
    let mut bundles = HashMap::new();
    let mut found: Vec<Candidate> = Vec::new();
    // Resolve each ancestor's paths once, not once per signature.
    let ancestors: Vec<Resolved> = env.ancestry.chain.iter().map(resolve_ancestor).collect();
    for sig in signatures {
        let mut candidate: Option<Candidate> = None;
        // 1. Process ancestry: the nearest matching ancestor.
        for (distance, ancestor) in ancestors.iter().enumerate() {
            if let Some(hit) = identify(sig, ancestor, &mut bundles) {
                candidate = Some(new_candidate(
                    sig,
                    Presence::ProcessLinked,
                    hit.evidence,
                    hit.surface,
                ));
                let c = candidate.as_mut().expect("just set");
                c.distance = Some(distance);
                c.version = hit.version;
                break;
            }
        }
        // 2. Environment hints set by the harness for its children.
        if sig.env.iter().any(|(name, value)| {
            env.var(name)
                .is_some_and(|v| value.is_empty() || v == *value)
        }) {
            match &mut candidate {
                Some(c) => c.evidence.push("environment_hint"),
                None => {
                    candidate = Some(new_candidate(
                        sig,
                        Presence::ProcessLinked,
                        "environment_hint",
                        sig.surface,
                    ))
                }
            }
        }
        // Cheap auto mode touches no files of an unlinked product.
        if candidate.is_none() && !context.focused {
            continue;
        }
        let root = home_root(sig, env);
        // 3. Configured: product-specific files in the product's own root.
        let configured = root.as_ref().is_some_and(|r| {
            sig.markers.iter().any(|m| {
                reads = reads.saturating_sub(1);
                reads > 0 && r.join(m).exists()
            })
        });
        // 4. Installed: verified install identity on PATH or in application folders.
        let installed =
            if context.focused || candidate.as_ref().is_some_and(|c| c.version.is_none()) {
                installed(sig, env, &mut bundles, &mut reads)
            } else {
                None
            };
        let c = match candidate {
            Some(mut c) => {
                if configured {
                    c.evidence.push("user_config");
                }
                if let Some(hit) = installed {
                    c.evidence.push(hit.evidence);
                    c.version = c.version.or(hit.version);
                }
                c
            }
            None if configured => {
                let mut c = new_candidate(sig, Presence::Configured, "user_config", sig.surface);
                if let Some(hit) = installed {
                    c.evidence.push(hit.evidence);
                    c.version = hit.version;
                    c.surface = hit.surface;
                }
                c
            }
            None => match installed {
                Some(hit) => {
                    let mut c = new_candidate(sig, Presence::Installed, hit.evidence, hit.surface);
                    c.version = hit.version;
                    c
                }
                None => continue,
            },
        };
        let mut c = c;
        c.evidence.dedup();
        if c.version.is_none()
            && let (Some(root), Some((file, key))) = (&root, sig.version_file)
            && reads > 0
        {
            reads -= 1;
            c.version = installed_version(&root.join(file), key);
        }
        c.root = root;
        found.push(c);
    }
    // Deterministic priority: process-linked (nearest first), configured, installed, then id.
    found.sort_by(|a, b| {
        a.presence
            .cmp(&b.presence)
            .then(
                a.distance
                    .unwrap_or(usize::MAX)
                    .cmp(&b.distance.unwrap_or(usize::MAX)),
            )
            .then(a.id.cmp(b.id))
    });
    let omitted_candidates = found.len().saturating_sub(MAX_CANDIDATES);
    found.truncate(MAX_CANDIDATES);
    let linked: Vec<&Candidate> = found
        .iter()
        .filter(|c| c.presence == Presence::ProcessLinked)
        .collect();
    let nearest = linked
        .iter()
        .filter(|c| c.distance.is_some())
        .min_by_key(|c| c.distance);
    let (primary, association) = match (nearest, linked.len()) {
        (Some(c), _) => (Some(c.id), "process_ancestor"),
        (None, 1) => (Some(linked[0].id), "environment_hint"),
        (None, 0) => (None, "none"),
        (None, _) => (None, "ambiguous"),
    };
    let mut notes = Vec::new();
    if env.ancestry.unavailable {
        notes.push("process_ancestry_unavailable");
    } else if env.ancestry.incomplete {
        notes.push("process_ancestry_incomplete");
    }
    if association == "environment_hint" {
        // Such variables are inherited, so a long-lived daemon started from a harness
        // shell can still carry them after the harness exits.
        notes.push("association_from_inherited_environment_only");
    }
    if linked.len() > 1 {
        notes.push(if primary.is_some() {
            "nested_or_multiple_harness_context"
        } else {
            "ambiguous_harness_association"
        });
    }
    if context.mcp_server {
        notes.push("mcp_server_context_not_caller_session");
    }
    let project = config::project_dirs(cwd, env.home.as_deref());
    for c in &mut found {
        // Cheap auto mode extracts configuration only for the associated harness.
        if !context.focused && c.presence != Presence::ProcessLinked {
            continue;
        }
        let sig = signatures
            .iter()
            .find(|s| s.id == c.id)
            .expect("candidate ids come from signatures");
        extract(sig, c, &project, env, &mut reads);
    }
    if found.iter().any(|c| c.configured_model.is_some()) {
        notes.push("configured_model_is_not_live_session_model");
    }
    if found.iter().any(|c| !c.capabilities.is_empty()) {
        notes.push("declarations_are_not_loaded_tools");
    }
    if reads == 0 {
        notes.push("read_budget_exhausted");
    }
    Harness {
        inspection_boundary: "local_process",
        primary,
        association,
        candidates: found,
        omitted_candidates,
        notes,
        focused: context.focused,
    }
}

fn new_candidate(
    sig: &Signature,
    presence: Presence,
    evidence: &'static str,
    surface: &'static str,
) -> Candidate {
    Candidate {
        id: sig.id,
        surface,
        presence,
        evidence: vec![evidence],
        version: None,
        configured_model: None,
        capabilities: Capabilities::default(),
        constraints: vec![],
        visibility: vec![],
        distance: None,
        root: None,
    }
}

struct Hit {
    evidence: &'static str,
    surface: &'static str,
    version: Option<Version>,
}

fn normalized(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('\\', "/").to_ascii_lowercase()
    } else {
        text.into_owned()
    }
}

/// File name without a `.exe` suffix, case preserved: `Claude.exe` (Claude Desktop) is
/// not `claude` (Claude Code) on case-insensitive Windows either.
fn basename(path: &Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    Some(match name.len().checked_sub(4) {
        Some(cut) if name.is_char_boundary(cut) && name[cut..].eq_ignore_ascii_case(".exe") => {
            &name[..cut]
        }
        _ => name,
    })
}

const INTERPRETERS: &[&str] = &["node", "bun", "deno", "python", "python3", "ruby"];

fn is_interpreter(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    INTERPRETERS.contains(&name.as_str())
        || name
            .strip_prefix("python3.")
            .is_some_and(|v| v.bytes().all(|b| b.is_ascii_digit()))
}

/// An ancestor's executable and, for interpreter launches, its resolved script.
struct Resolved {
    exe: Option<PathBuf>,
    paths: Vec<PathBuf>,
    args: Vec<String>,
}

fn resolve_ancestor(ancestor: &process::Ancestor) -> Resolved {
    let exe = ancestor.exe.clone().or_else(|| {
        ancestor
            .args
            .first()
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    });
    let mut paths = Vec::new();
    if let Some(exe) = &exe {
        paths.push(exe.clone());
        if basename(exe).is_some_and(is_interpreter)
            && let Some(script) = ancestor
                .args
                .iter()
                .skip(1)
                .find(|a| !a.starts_with('-'))
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        {
            paths.push(std::fs::canonicalize(&script).unwrap_or(script));
        }
    }
    Resolved {
        exe,
        paths,
        args: ancestor.args.clone(),
    }
}

/// Matches one ancestor against a signature by product path markers or bundle
/// identifier, then by a distinctive executable name.
fn identify(sig: &Signature, ancestor: &Resolved, bundles: &mut Bundles) -> Option<Hit> {
    let surface = |default: &'static str| {
        sig.arg_surfaces
            .iter()
            .find(|(token, _)| ancestor.args.iter().skip(1).any(|a| a == token))
            .map_or(default, |(_, s)| s)
    };
    for path in &ancestor.paths {
        if let Some(hit) = by_path(sig, path, bundles) {
            return Some(Hit {
                evidence: "process_ancestor",
                surface: surface(hit.surface),
                ..hit
            });
        }
    }
    let name = basename(ancestor.exe.as_deref()?)?;
    sig.names.contains(&name).then(|| Hit {
        evidence: "process_ancestor",
        surface: surface(sig.surface),
        version: None,
    })
}

/// Product-specific path markers or a verified application bundle.
fn by_path(sig: &Signature, path: &Path, bundles: &mut Bundles) -> Option<Hit> {
    // Inside an application bundle the bundle identifier decides, so a renamed or
    // unrelated app cannot match by folder name; unreadable metadata falls back to paths.
    if let Some(app) = path
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        && let Some((id, version)) = bundles
            .entry(app.to_owned())
            .or_insert_with(|| bundle(app))
            .clone()
    {
        let (_, surface) = sig.bundles.iter().find(|(b, _)| *b == id)?;
        return Some(Hit {
            evidence: "bundle_identifier",
            surface,
            version: version.map(|value| Version {
                value,
                source: "bundle_metadata",
            }),
        });
    }
    let text = normalized(path);
    let (_, surface) = sig.paths.iter().find(|(m, _)| {
        if cfg!(windows) {
            text.contains(&m.to_ascii_lowercase())
        } else {
            text.contains(m)
        }
    })?;
    Some(Hit {
        evidence: "install_path",
        surface,
        version: version_near(sig, path),
    })
}

/// Reads `CFBundleIdentifier` and `CFBundleShortVersionString` from an XML Info.plist.
fn bundle(app: &Path) -> Option<(String, Option<String>)> {
    let text = safe::read(&app.join("Contents/Info.plist"), 128 * 1024)?;
    let value = |key: &str| {
        let marker = format!("<key>{key}</key>");
        let after = text[text.find(&marker)? + marker.len()..]
            .trim_start()
            .strip_prefix("<string>")?;
        safe::token(&after[..after.find("</string>")?], 128)
    };
    Some((
        value("CFBundleIdentifier")?,
        value("CFBundleShortVersionString"),
    ))
}

/// Version from package metadata next to the matched path, or a `versions/<v>` layout.
fn version_near(sig: &Signature, path: &Path) -> Option<Version> {
    for dir in path.ancestors().skip(1).take(6) {
        if let Some(Read::Parsed(pkg)) = Some(config::load(
            &dir.join("package.json"),
            Format::Json,
            64 * 1024,
        )) && let Some(name) = pkg.get("name").and_then(|n| n.as_str())
            && sig.packages.contains(&name)
        {
            return pkg
                .get("version")
                .and_then(|v| v.as_str())
                .and_then(|v| safe::token(v, 64))
                .map(|value| Version {
                    value,
                    source: "package_metadata",
                });
        }
    }
    let parent = path.parent()?;
    let name = path.file_name()?.to_str()?;
    (parent.file_name()? == "versions"
        && name.split('.').count() >= 2
        && name
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())))
    .then(|| Version {
        value: name.to_owned(),
        source: "install_path",
    })
}

/// Version recorded by the product's own installer (plain text or a JSON key).
fn installed_version(path: &Path, key: &str) -> Option<Version> {
    let text = safe::read(path, 64 * 1024)?;
    let value = if key.is_empty() {
        safe::token(text.trim(), 64)?
    } else {
        let json = config::parse(&text, Format::Json)?;
        safe::token(config::get(&json, key)?.as_str()?, 64)?
    };
    Some(Version {
        value,
        source: "install_metadata",
    })
}

fn installed(sig: &Signature, env: &Env, bundles: &mut Bundles, reads: &mut usize) -> Option<Hit> {
    for command in sig.commands {
        if *reads == 0 {
            return None;
        }
        *reads -= 1;
        for target in (env.commands)(command) {
            if let Some(hit) = by_path(sig, &target, bundles) {
                return Some(Hit {
                    evidence: "install_path",
                    ..hit
                });
            }
        }
    }
    for dir in &env.app_dirs {
        for app in sig.apps {
            if *reads == 0 {
                return None;
            }
            *reads -= 1;
            let path = dir.join(app);
            if !path.is_dir() {
                continue;
            }
            if let Some(hit) = by_path(sig, &path.join("Contents/MacOS/x"), bundles) {
                return Some(hit);
            }
        }
    }
    None
}

fn home_root(sig: &Signature, env: &Env) -> Option<PathBuf> {
    let root = resolve(&sig.root, env)?;
    Some(match sig.root.select {
        Some(select) => select(root, env),
        None => root,
    })
}

fn resolve(root: &Root, env: &Env) -> Option<PathBuf> {
    if let Some((name, suffix)) = root.env.iter().find(|(name, _)| env.var(name).is_some()) {
        // An override that is not absolute is not guessed at.
        let path = PathBuf::from(env.var(name)?);
        return path.is_absolute().then(|| path.join(suffix));
    }
    if cfg!(windows)
        && let Some(local) = root.windows_local
    {
        return Some(PathBuf::from(env.var("LOCALAPPDATA")?).join(local));
    }
    let home = env.home.as_ref()?;
    let xdg = || {
        env.var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".config"))
    };
    Some(match root.base {
        Base::Home => home.join(root.path),
        Base::XdgConfig => xdg().join(root.path),
    })
}

/// Everything a product-specific hook may read or adjust.
pub(crate) struct Extraction<'a> {
    pub(crate) root: Option<&'a Path>,
    pub(crate) project: &'a [PathBuf],
    pub(crate) env: &'a Env,
    candidate: &'a mut Candidate,
    reads: &'a mut usize,
    /// Whether project trust allows project-scope model settings.
    project_eligible: bool,
    /// Some project-scope source or directory exists.
    project_found: bool,
    model: Option<ConfiguredModel>,
    /// Disable markers applied after every source has been read.
    disabled: Vec<(Cap, String)>,
    /// Restriction settings by label: the first scalar wins (sources arrive in
    /// precedence order); rule lists from several layers add up.
    limits: Vec<(&'static str, Limit)>,
}

enum Limit {
    Value(String),
    Count(usize),
}

impl Extraction<'_> {
    pub(crate) fn load(&mut self, path: &Path, format: Format) -> Read {
        self.load_max(path, format, config::MAX_FILE_BYTES)
    }

    pub(crate) fn load_max(&mut self, path: &Path, format: Format, max: usize) -> Read {
        if *self.reads == 0 {
            self.hidden("read_budget_exhausted");
            return Read::Unreadable;
        }
        *self.reads -= 1;
        config::load(path, format, max)
    }

    pub(crate) fn constraint(&mut self, text: impl Into<String>) {
        let text = text.into();
        if !self.candidate.constraints.contains(&text) {
            self.candidate.constraints.push(text);
        }
    }

    pub(crate) fn hidden(&mut self, what: &'static str) {
        if !self.candidate.visibility.contains(&what) {
            self.candidate.visibility.push(what);
        }
    }

    pub(crate) fn add(&mut self, cap: Cap, names: Names, source: &'static str) {
        self.candidate.capabilities.add(cap, names, source);
    }

    pub(crate) fn mark_disabled(&mut self, cap: Cap, names: &[String]) {
        self.disabled.extend(names.iter().map(|n| (cap, n.clone())));
    }

    fn apply_disabled(&mut self) {
        for (cap, name) in std::mem::take(&mut self.disabled) {
            if let Some(inv) = self.candidate.capabilities.slot(cap)
                && inv.all.names.contains(&name)
                && !inv.all.disabled.contains(&name)
            {
                inv.all.disabled.push(name);
                inv.disabled = inv.all.disabled.clone();
            }
        }
    }

    pub(crate) fn model_source(&self) -> Option<&'static str> {
        self.model.as_ref().map(|m| m.source)
    }

    /// A documented selector (profile, alias, override file) replaces the model
    /// found through the plain key paths.
    pub(crate) fn override_model(
        &mut self,
        provider: Option<String>,
        model: Option<String>,
        thinking: Option<String>,
        source: &'static str,
    ) {
        if provider.is_none() && model.is_none() {
            if let (Some(t), Some(m)) = (thinking, &mut self.model) {
                m.thinking = Some(t);
            }
            return;
        }
        let previous = self.model.take();
        self.model = Some(ConfiguredModel {
            provider: provider.or_else(|| {
                previous
                    .as_ref()
                    .filter(|p| model.is_none() || p.source == source)
                    .and_then(|p| p.provider.clone())
            }),
            model,
            thinking: thinking.or_else(|| previous.and_then(|p| p.thinking)),
            source,
        });
    }

    /// Reads one source file through the shared allowlisted fields.
    fn apply(&mut self, path: &Path, source: &Source) {
        let tag = match source.scope {
            Scope::User => "user_config",
            Scope::Project => "project_config",
        };
        let value = match self.load(path, source.format) {
            Read::Missing => return,
            Read::Unreadable => {
                return self.hidden(match source.scope {
                    Scope::User => "user_config_unreadable",
                    Scope::Project => "project_config_unreadable",
                });
            }
            Read::Parsed(v) => v,
        };
        if source.scope == Scope::Project {
            self.project_found = true;
        }
        let f = source.fields;
        let pick = |keys: &[&str]| {
            keys.iter()
                .find_map(|k| config::get(&value, k).and_then(config::scalar))
        };
        let found = ConfiguredModel {
            provider: pick(f.provider),
            model: pick(f.model),
            thinking: pick(f.thinking),
            source: tag,
        };
        if source.scope == Scope::User || self.project_eligible {
            // Sources arrive in precedence order and merge key by key, as the
            // products' layered configs do: fill only what is still missing.
            match &mut self.model {
                None if found.model.is_some() || found.provider.is_some() => {
                    self.model = Some(found)
                }
                Some(m) => {
                    m.provider = m.provider.take().or(found.provider);
                    m.thinking = m.thinking.take().or(found.thinking);
                }
                None => {}
            }
        }
        for (cap, keys) in [
            (Cap::Mcp, f.mcp),
            (Cap::Subagents, f.agents),
            (Cap::Extensions, f.extensions),
            (Cap::Skills, f.skills),
        ] {
            for key in keys {
                if let Some(v) = config::get(&value, key) {
                    self.add(cap, config::declared(v), tag);
                }
            }
        }
        // An ineligible (untrusted or unverified) project's restrictions do not apply.
        if source.scope == Scope::Project && !self.project_eligible {
            return;
        }
        for (key, label) in f.limits {
            let found = match config::get(&value, key) {
                Some(serde_json::Value::Array(a)) if !a.is_empty() => Limit::Count(a.len()),
                Some(v) => match config::scalar(v) {
                    Some(s) => Limit::Value(s),
                    None => continue,
                },
                None => continue,
            };
            match (self.limits.iter_mut().find(|(l, _)| l == label), found) {
                (None, found) => self.limits.push((label, found)),
                (Some((_, Limit::Count(n))), Limit::Count(more)) => *n += more,
                (Some(_), _) => {}
            }
        }
    }
}

fn extract(sig: &Signature, c: &mut Candidate, project: &[PathBuf], env: &Env, reads: &mut usize) {
    let root = c.root.clone();
    let mut x = Extraction {
        root: root.as_deref(),
        project,
        env,
        candidate: c,
        reads,
        project_eligible: true,
        project_found: false,
        model: None,
        disabled: vec![],
        limits: vec![],
    };
    let trust = match sig.trust {
        Trust::NotApplicable => None,
        Trust::Check(check) => Some(check(&mut x)),
    };
    x.project_eligible = trust.is_none_or(|t| t == Some(true));
    // Project layers override user layers; within a scope the table order holds.
    let mut sources: Vec<&Source> = sig.sources.iter().collect();
    sources.sort_by_key(|s| s.scope != Scope::Project);
    for source in sources {
        let paths: Vec<PathBuf> = match source.scope {
            Scope::User => x
                .root
                .map(|r| vec![r.join(source.file)])
                .unwrap_or_default(),
            Scope::Project => project.iter().map(|d| d.join(source.file)).collect(),
        };
        for path in paths {
            x.apply(&path, source);
        }
    }
    for inv in sig.inventories {
        let (dirs, tag): (Vec<PathBuf>, _) = match inv.scope {
            Scope::User => (
                x.root.map(|r| vec![r.join(inv.dir)]).unwrap_or_default(),
                "user_config",
            ),
            Scope::Project => (
                project.iter().map(|d| d.join(inv.dir)).collect(),
                "project_config",
            ),
        };
        for dir in dirs {
            if *x.reads == 0 {
                x.hidden("read_budget_exhausted");
                break;
            }
            *x.reads -= 1;
            match config::entries(&dir, inv.entries) {
                None => {}
                Some(Err(())) => x.hidden("directory_unreadable"),
                Some(Ok(names)) => {
                    x.project_found |= inv.scope == Scope::Project;
                    x.add(inv.cap, names, tag);
                }
            }
        }
    }
    if let Some(special) = sig.special {
        special(&mut x);
    }
    x.apply_disabled();
    for (label, limit) in std::mem::take(&mut x.limits) {
        x.constraint(match limit {
            Limit::Value(v) => format!("{label}={v}"),
            Limit::Count(n) => format!("{label}={n}"),
        });
    }
    // A documented environment override wins over files for new sessions.
    if let Some(value) = sig
        .model_env
        .iter()
        .find_map(|k| env.var(k))
        .and_then(|v| safe::identifier(v, 96))
    {
        x.override_model(None, Some(value), None, "environment_override");
    }
    // Trust only matters when the project actually carries resources for this harness.
    if x.project_found {
        match trust {
            Some(Some(false)) => x.constraint("project_untrusted"),
            Some(None) => x.constraint("project_trust_unverified"),
            _ => {}
        }
    }
    if let Some(note) = sig.visibility {
        x.hidden(note);
    }
    if let Some(model) = x.model.take() {
        x.candidate.configured_model = Some(model);
    }
}

impl Harness {
    pub(crate) fn note(&mut self, note: &'static str) {
        if !self.notes.contains(&note) {
            self.notes.push(note);
        }
    }

    /// Brief lines in priority order (see `brief` for the selection rules).
    /// `all_names` lists every known declared name instead of the first three per kind.
    pub(crate) fn brief(&self, all_names: bool) -> Vec<Line> {
        let mut lines = Vec::new();
        let primary = self
            .primary
            .and_then(|id| self.candidates.iter().find(|c| c.id == id));
        let linked: Vec<&Candidate> = self
            .candidates
            .iter()
            .filter(|c| c.presence == Presence::ProcessLinked)
            .collect();
        let mut head = match (primary, self.association) {
            (Some(c), association) => {
                let mut line =
                    Line::new(3, format!("harness={} process_linked({association})", c.id));
                line.push(4, format!("surface={}", c.surface));
                if let Some(v) = &c.version {
                    line.push(6, format!("version={}", v.value));
                }
                line
            }
            (None, "ambiguous") => Line::new(
                3,
                format!(
                    "harness=ambiguous candidates={}",
                    linked.iter().map(|c| c.id).collect::<Vec<_>>().join(",")
                ),
            ),
            (None, _) => Line::new(3, "harness=none_process_linked"),
        };
        let others: Vec<&str> = self
            .candidates
            .iter()
            .filter(|c| Some(c.id) != self.primary)
            .map(|c| c.id)
            .collect();
        if primary.is_some() && linked.len() > 1 {
            head.push(
                3,
                format!(
                    "also_linked={}",
                    linked
                        .iter()
                        .filter(|c| Some(c.id) != self.primary)
                        .map(|c| c.id)
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            );
        }
        lines.push(head);
        let detailed: Vec<&Candidate> = match primary {
            Some(c) => vec![c],
            None if self.focused => self.candidates.iter().take(3).collect(),
            None => vec![],
        };
        for c in &detailed {
            let label = if primary.is_some() {
                String::new()
            } else {
                format!("[{}:{}] ", c.id, presence_label(c.presence))
            };
            if let Some(m) = &c.configured_model {
                let mut text = format!("{label}model_config=");
                match (&m.provider, &m.model) {
                    (Some(p), Some(model)) if !model.starts_with(&format!("{p}/")) => {
                        text.push_str(&format!("{p}/{model}"))
                    }
                    (_, Some(model)) => text.push_str(model),
                    (Some(p), None) => text.push_str(&format!("{p}/default")),
                    (None, None) => text.push_str("default"),
                }
                let mut line = Line::new(4, text);
                if let Some(t) = &m.thinking {
                    line.push(4, format!("thinking={t}"));
                }
                line.push(6, format!("source={}", m.source));
                lines.push(line);
            }
            // One line per kind: `declared_mcp=3 a,b,c(off)`; enabled names first.
            for (kind, inv) in c.capabilities.iter() {
                let more = if inv.limited { "+" } else { "" };
                let mut line = Line::new(5, format!("{label}declared_{kind}={}{more}", inv.count))
                    .joined(" ", ",");
                let (on, off): (Vec<&String>, Vec<&String>) =
                    inv.names.iter().partition(|n| !inv.disabled.contains(n));
                let shown = if all_names { usize::MAX } else { 3 };
                for (i, name) in on.into_iter().enumerate().take(shown) {
                    line.push(if i < 3 { 5 } else { 7 }, name.clone());
                }
                for name in off.into_iter().take(if all_names { usize::MAX } else { 0 }) {
                    line.push(7, format!("{name}(off)"));
                }
                lines.push(line);
            }
            if !c.constraints.is_empty() || !c.visibility.is_empty() {
                let mut line = Line::list(format!("{label}limits="), ",");
                for constraint in &c.constraints {
                    line.push(2, constraint.clone());
                }
                for hidden in &c.visibility {
                    line.push(6, *hidden);
                }
                lines.push(line);
            }
        }
        if primary.is_none() && !others.is_empty() {
            let mut line = Line::list("other_harnesses=", ",");
            for c in self
                .candidates
                .iter()
                .filter(|c| Some(c.id) != self.primary)
            {
                line.push(6, format!("{}({})", c.id, presence_label(c.presence)));
            }
            lines.push(line);
        } else if primary.is_some() && self.focused {
            let mut line = Line::list("other_harnesses=", ",");
            for c in self
                .candidates
                .iter()
                .filter(|c| Some(c.id) != self.primary && c.presence != Presence::ProcessLinked)
            {
                line.push(7, format!("{}({})", c.id, presence_label(c.presence)));
            }
            lines.push(line);
        }
        if self.omitted_candidates > 0 {
            lines.push(Line::new(
                7,
                format!("harness_candidates_omitted={}", self.omitted_candidates),
            ));
        }
        if !self.notes.is_empty() {
            let mut line = Line::list("harness_notes=", ",");
            for note in &self.notes {
                let priority = match *note {
                    "ambiguous_harness_association" | "harness_facts_describe_this_host_only" => 3,
                    "mcp_server_context_not_caller_session" => 4,
                    _ => 6,
                };
                line.push(priority, *note);
            }
            lines.push(line);
        }
        lines
    }
}

fn presence_label(p: Presence) -> &'static str {
    match p {
        Presence::ProcessLinked => "process_linked",
        Presence::Configured => "configured",
        Presence::Installed => "installed",
    }
}

#[cfg(test)]
mod tests;
