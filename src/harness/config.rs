//! Shared, bounded readers for harness configuration.
//!
//! Every format is reduced to a `serde_json::Value` so extractors can read a few
//! allowlisted key paths uniformly. Unknown keys are ignored; a malformed file only
//! loses its own fields. Values are never executed or interpreted as instructions.

use crate::safe;
use serde_json::{Map, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) const MAX_FILE_BYTES: usize = 256 * 1024;
pub(crate) const MAX_DIR_ENTRIES: usize = 128;
pub(crate) const MAX_NAMES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    /// JSON, JSONC and the JSON5 subset used by agent configs (comments, trailing
    /// commas, unquoted keys, single-quoted strings).
    Json,
    Toml,
    /// Block-mapping YAML subset: nested maps, scalar values and simple lists.
    Yaml,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Read {
    Missing,
    /// Exists but could not be read within bounds or parsed.
    Unreadable,
    Parsed(Value),
}

pub(crate) fn load(path: &Path, format: Format, max: usize) -> Read {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Read::Missing,
        Err(_) => return Read::Unreadable,
        Ok(_) => {}
    }
    let Some(text) = safe::read(path, max) else {
        return Read::Unreadable;
    };
    parse(&text, format).map_or(Read::Unreadable, Read::Parsed)
}

pub(crate) fn parse(text: &str, format: Format) -> Option<Value> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    match format {
        Format::Json => serde_json::from_str(text)
            .ok()
            .or_else(|| serde_json::from_str(&relaxed_json(text)?).ok()),
        Format::Toml => toml::from_str::<toml::Table>(text)
            .ok()
            .and_then(|t| serde_json::to_value(t).ok()),
        Format::Yaml => yaml(text),
    }
}

/// Rewrites JSONC/JSON5 conveniences into strict JSON. Returns None on unterminated input.
fn relaxed_json(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    // Whether the next token is in object-key position.
    let mut key_position = false;
    let mut stack: Vec<char> = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' | '\'' => {
                let quote = c;
                out.push('"');
                i += 1;
                loop {
                    let ch = *chars.get(i)?;
                    i += 1;
                    match ch {
                        '\\' => {
                            let next = *chars.get(i)?;
                            i += 1;
                            // A backslash before a line break continues the string.
                            if next == '\r' && chars.get(i) == Some(&'\n') {
                                i += 1;
                            } else if next == '\n' || next == '\r' {
                            } else if next == '\'' {
                                out.push('\'');
                            } else {
                                out.push('\\');
                                out.push(next);
                            }
                        }
                        '"' if quote == '\'' => out.push_str("\\\""),
                        ch if ch == quote => break,
                        ch => out.push(ch),
                    }
                }
                out.push('"');
                key_position = false;
                continue;
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                if i + 1 >= chars.len() {
                    return None;
                }
                i += 2;
                continue;
            }
            ',' => {
                // Drop a trailing comma before a closing bracket.
                let mut j = i + 1;
                loop {
                    while j < chars.len() && chars[j].is_whitespace() {
                        j += 1;
                    }
                    if chars.get(j) == Some(&'/') && chars.get(j + 1) == Some(&'/') {
                        while j < chars.len() && chars[j] != '\n' {
                            j += 1;
                        }
                        continue;
                    }
                    if chars.get(j) == Some(&'/') && chars.get(j + 1) == Some(&'*') {
                        j += 2;
                        while j + 1 < chars.len() && !(chars[j] == '*' && chars[j + 1] == '/') {
                            j += 1;
                        }
                        j += 2;
                        continue;
                    }
                    break;
                }
                if !matches!(chars.get(j), Some('}' | ']')) {
                    out.push(',');
                }
                key_position = stack.last() == Some(&'{');
            }
            '{' | '[' => {
                stack.push(c);
                key_position = c == '{';
                out.push(c);
            }
            '}' | ']' => {
                stack.pop();
                key_position = false;
                out.push(c);
            }
            ':' => {
                key_position = false;
                out.push(c);
            }
            c if key_position && (c.is_alphabetic() || c == '_' || c == '$') => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$')
                {
                    i += 1;
                }
                out.push('"');
                out.extend(&chars[start..i]);
                out.push('"');
                key_position = false;
                continue;
            }
            c if !key_position && (c.is_ascii_digit() || matches!(c, '+' | '-' | '.')) => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || matches!(chars[i], '+' | '-' | '.'))
                {
                    i += 1;
                }
                let token: String = chars[start..i].iter().collect();
                out.push_str(&json5_number(&token).unwrap_or(token));
                continue;
            }
            c if !key_position && c.is_ascii_alphabetic() => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_alphanumeric() {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                // Strict JSON has no infinities or NaN; their values are never needed.
                out.push_str(if matches!(word.as_str(), "Infinity" | "NaN") {
                    "null"
                } else {
                    &word
                });
                continue;
            }
            c => out.push(c),
        }
        i += 1;
    }
    Some(out)
}

/// JSON5 number spellings that strict JSON lacks: a leading `+`, a leading or
/// trailing `.`, hexadecimal, and signed infinities or NaN.
fn json5_number(token: &str) -> Option<String> {
    let (sign, body) = match token.as_bytes().first()? {
        b'+' => ("", &token[1..]),
        b'-' => ("-", &token[1..]),
        _ => ("", token),
    };
    if matches!(body, "Infinity" | "NaN") {
        return Some("null".into());
    }
    if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16)
            .ok()
            .map(|n| format!("{sign}{n}"));
    }
    let body = if body.starts_with('.') {
        format!("0{body}")
    } else {
        body.to_owned()
    };
    let body = match body.strip_suffix('.') {
        Some(whole) => whole.to_owned(),
        None => body.replace(".e", ".0e").replace(".E", ".0E"),
    };
    Some(format!("{sign}{body}"))
}

/// Minimal YAML reader for configuration metadata. Anything outside the subset
/// (anchors, flow mappings, multi-document streams) is skipped, not guessed.
fn yaml(text: &str) -> Option<Value> {
    struct Frame {
        indent: usize,
        value: Value,
        key: Option<String>,
    }
    fn scalar(raw: &str) -> Value {
        let raw = strip_comment(raw).trim();
        if raw.starts_with(['[', '{']) {
            // A flow collection that does not parse completely is not guessed.
            return flow(raw).unwrap_or(Value::Null);
        }
        if raw.starts_with(['"', '\'']) {
            return Value::String(unquote(raw));
        }
        plain(raw)
    }
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#') && *l != "---");
    if first.is_some_and(|l| l.starts_with(['{', '['])) {
        let body: Vec<&str> = text
            .lines()
            .filter(|l| !l.trim_start().starts_with('#') && l.trim() != "---")
            .collect();
        return flow(&body.join("\n"));
    }
    fn strip_comment(raw: &str) -> &str {
        let mut quote = None;
        for (i, c) in raw.char_indices() {
            match (quote, c) {
                (None, '"' | '\'') => quote = Some(c),
                (Some(q), c) if c == q => quote = None,
                (None, '#') if i == 0 || raw[..i].ends_with(' ') => return &raw[..i],
                _ => {}
            }
        }
        raw
    }
    fn unquote(raw: &str) -> String {
        let raw = raw.trim();
        if raw.len() >= 2
            && ((raw.starts_with('"') && raw.ends_with('"'))
                || (raw.starts_with('\'') && raw.ends_with('\'')))
        {
            raw[1..raw.len() - 1].to_owned()
        } else {
            raw.to_owned()
        }
    }
    fn attach(parent: &mut Frame, child: Value) {
        match &mut parent.value {
            Value::Object(map) => {
                if let Some(key) = parent.key.take() {
                    map.insert(key, child);
                }
            }
            Value::Array(items) => items.push(child),
            _ => {}
        }
    }
    let mut stack = vec![Frame {
        indent: 0,
        value: Value::Object(Map::new()),
        key: None,
    }];
    let mut block_indent: Option<usize> = None;
    for line in text.lines().take(20_000) {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if let Some(block) = block_indent {
            if trimmed.is_empty() || indent > block {
                continue;
            }
            block_indent = None;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed == "---" {
            continue;
        }
        if line.contains('\t') && indent > 0 && line[..indent].contains('\t') {
            return None;
        }
        // Close frames deeper than this line.
        while stack.len() > 1 && indent < stack.last()?.indent {
            let done = stack.pop()?;
            attach(stack.last_mut()?, done.value);
        }
        let (item, body) = if let Some(rest) = trimmed.strip_prefix("- ") {
            (true, rest.trim_start())
        } else if trimmed == "-" {
            (true, "")
        } else {
            (false, trimmed)
        };
        // Keys of a mapping inside a list item align with the item's first key.
        let child_indent = indent + (trimmed.len() - body.len()).max(2);
        // A document whose first entry is a list item is a root-level list.
        let root_only = stack.len() == 1;
        let top = stack.last_mut()?;
        if item
            && root_only
            && top.key.is_none()
            && top.value.as_object().is_some_and(Map::is_empty)
        {
            top.value = Value::Array(vec![]);
            top.indent = indent;
        }
        let top = stack.last_mut()?;
        if item {
            if top.indent != indent || !top.value.is_array() {
                // A list starts under the pending key.
                if top.value.is_object() && top.key.is_some() && indent >= top.indent {
                    stack.push(Frame {
                        indent,
                        value: Value::Array(vec![]),
                        key: None,
                    });
                } else {
                    continue;
                }
            }
            match split_key(body) {
                Some((key, rest)) if !rest.trim().is_empty() => {
                    let mut map = Map::new();
                    map.insert(key, scalar(rest));
                    stack.push(Frame {
                        indent: child_indent,
                        value: Value::Object(map),
                        key: None,
                    });
                }
                Some((key, _)) => {
                    stack.push(Frame {
                        indent: child_indent,
                        value: Value::Object(Map::new()),
                        key: Some(key),
                    });
                }
                None => {
                    let value = scalar(body);
                    if let Some(top) = stack.last_mut() {
                        attach(top, value);
                    }
                }
            }
            continue;
        }
        // A mapping line ends an "indentless" list written at its key's indent.
        while stack.len() > 1
            && stack
                .last()
                .is_some_and(|f| f.value.is_array() && indent <= f.indent)
        {
            let done = stack.pop()?;
            attach(stack.last_mut()?, done.value);
        }
        let top = stack.last_mut()?;
        let Some((key, rest)) = split_key(body) else {
            continue;
        };
        if top.value.is_array() {
            continue;
        }
        if top.indent != indent {
            if top.key.is_some() && indent > top.indent {
                stack.push(Frame {
                    indent,
                    value: Value::Object(Map::new()),
                    key: None,
                });
            } else if top.indent < indent {
                continue;
            }
        }
        let top = stack.last_mut()?;
        let rest = strip_comment(rest).trim();
        if rest.is_empty() {
            top.key = Some(key);
        } else if rest.starts_with('|') || rest.starts_with('>') {
            block_indent = Some(indent);
            if let Value::Object(map) = &mut top.value {
                map.insert(key, Value::Null);
            }
        } else if let Value::Object(map) = &mut top.value {
            map.insert(key, scalar(rest));
            top.key = None;
        }
    }
    while stack.len() > 1 {
        let done = stack.pop()?;
        attach(stack.last_mut()?, done.value);
    }
    // A key with no nested lines stays null.
    let mut root = stack.pop()?;
    if let (Value::Object(map), Some(key)) = (&mut root.value, root.key.take()) {
        map.entry(key).or_insert(Value::Null);
    }
    Some(root.value)
}

/// An unquoted YAML scalar: YAML 1.1 and 1.2 boolean and null spellings, else a string.
fn plain(raw: &str) -> Value {
    match raw.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" => Value::Bool(true),
        "false" | "no" | "off" => Value::Bool(false),
        "null" | "~" | "" => Value::Null,
        _ => Value::String(raw.to_owned()),
    }
}

/// A complete YAML flow collection (`{a: 1, b: [x, "y,z"]}`), nested up to 32 levels.
fn flow(text: &str) -> Option<Value> {
    struct Parser<'a> {
        chars: std::iter::Peekable<std::str::Chars<'a>>,
        depth: usize,
    }
    impl Parser<'_> {
        /// Skips whitespace and `# comments`; a comment needs whitespace before it.
        fn skip_space(&mut self) {
            let mut spaced = true;
            loop {
                if self.chars.next_if(|c| c.is_whitespace()).is_some() {
                    spaced = true;
                } else if spaced && self.chars.next_if_eq(&'#').is_some() {
                    while self.chars.next_if(|c| *c != '\n').is_some() {}
                } else {
                    return;
                }
            }
        }
        fn value(&mut self) -> Option<Value> {
            self.skip_space();
            match self.chars.peek()? {
                '{' | '[' if self.depth < 32 => {
                    let open = self.chars.next()?;
                    self.depth += 1;
                    let value = if open == '{' { self.map() } else { self.list() };
                    self.depth -= 1;
                    value
                }
                '"' | '\'' => self.quoted().map(Value::String),
                _ => Some(plain(self.plain_text(false).trim())),
            }
        }
        fn quoted(&mut self) -> Option<String> {
            let quote = self.chars.next()?;
            let mut out = String::new();
            loop {
                match self.chars.next()? {
                    '\'' if quote == '\'' && self.chars.next_if_eq(&'\'').is_some() => {
                        out.push('\'')
                    }
                    '\\' if quote == '"' => out.push(self.chars.next()?),
                    c if c == quote => return Some(out),
                    c => out.push(c),
                }
            }
        }
        /// Plain text up to a flow indicator; a key also stops at `: `.
        fn plain_text(&mut self, key: bool) -> String {
            let mut out = String::new();
            while let Some(&c) = self.chars.peek() {
                if matches!(c, ',' | ']' | '}')
                    || (key && c == ':')
                    || (c == '#' && out.ends_with(char::is_whitespace))
                {
                    break;
                }
                out.push(c);
                self.chars.next();
            }
            out
        }
        fn list(&mut self) -> Option<Value> {
            let mut items = Vec::new();
            loop {
                self.skip_space();
                if self.chars.next_if_eq(&']').is_some() {
                    return Some(Value::Array(items));
                }
                items.push(self.value()?);
                self.skip_space();
                match self.chars.next()? {
                    ',' => {}
                    ']' => return Some(Value::Array(items)),
                    _ => return None,
                }
            }
        }
        fn map(&mut self) -> Option<Value> {
            let mut map = Map::new();
            loop {
                self.skip_space();
                if self.chars.next_if_eq(&'}').is_some() {
                    return Some(Value::Object(map));
                }
                let key = match self.chars.peek()? {
                    '"' | '\'' => self.quoted()?,
                    _ => self.plain_text(true).trim().to_owned(),
                };
                self.skip_space();
                let value = if self.chars.next_if_eq(&':').is_some() {
                    self.value()?
                } else {
                    Value::Null
                };
                map.insert(key, value);
                self.skip_space();
                match self.chars.next()? {
                    ',' => {}
                    '}' => return Some(Value::Object(map)),
                    _ => return None,
                }
            }
        }
    }
    let mut parser = Parser {
        chars: text.trim().chars().peekable(),
        depth: 0,
    };
    let value = parser.value()?;
    parser.skip_space();
    parser.chars.peek().is_none().then_some(value)
}

fn split_key(body: &str) -> Option<(String, &str)> {
    let (key, rest) = if body.starts_with('"') || body.starts_with('\'') {
        let quote = body.chars().next()?;
        let end = body[1..].find(quote)? + 1;
        let rest = body[end + 1..].strip_prefix(':')?;
        (body[1..end].to_owned(), rest)
    } else {
        let colon = body
            .find(": ")
            .into_iter()
            .chain(body.find(":\t"))
            .min()
            .or_else(|| body.strip_suffix(':').map(str::len))?;
        let key = &body[..colon];
        if key.contains(['{', '[', '#']) {
            return None;
        }
        (key.trim().to_owned(), body.get(colon + 1..).unwrap_or(""))
    };
    (!key.is_empty()).then_some((key, rest))
}

/// Dot-separated lookup. A literal key containing dots (for example `amp.mcpServers`)
/// is preferred over descending.
pub(crate) fn get<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let Value::Object(map) = value else {
        return None;
    };
    if let Some(found) = map.get(path) {
        return Some(found);
    }
    let (head, rest) = path.split_once('.')?;
    get(map.get(head)?, rest)
}

/// A short scalar suitable for a report: provider/model/mode identifiers only.
pub(crate) fn scalar(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return None,
    };
    safe::identifier(&text, 96)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Names {
    pub(crate) names: Vec<String>,
    pub(crate) disabled: Vec<String>,
    /// Entries that exist but whose names are not safe to print.
    pub(crate) unnamed: usize,
    pub(crate) limited: bool,
    /// Precedence of the layer that last set each name's state, parallel to `names`.
    rank: Vec<u8>,
}

impl Names {
    /// Within one layer the first declaration of a name wins.
    fn add(&mut self, name: &str, disabled: bool) {
        match safe::identifier(name, 64) {
            Some(name) if self.names.contains(&name) => {}
            Some(name) => {
                if disabled {
                    self.disabled.push(name.clone());
                }
                self.names.push(name);
                self.rank.push(0);
            }
            None => self.unnamed += 1,
        }
    }

    /// Adds another layer's names. A layer of higher `rank` (project over user)
    /// decides whether a name is disabled, whatever order the layers are read in.
    pub(crate) fn merge(&mut self, other: Names, rank: u8) {
        for name in &other.names {
            let disabled = other.disabled.contains(name);
            match self.names.iter().position(|n| n == name) {
                Some(i) if rank > self.rank[i] => {
                    self.rank[i] = rank;
                    self.disabled.retain(|d| d != name);
                    if disabled {
                        self.disabled.push(name.clone());
                    }
                }
                Some(_) => {}
                None => {
                    if disabled {
                        self.disabled.push(name.clone());
                    }
                    self.names.push(name.clone());
                    self.rank.push(rank);
                }
            }
        }
        self.unnamed += other.unnamed;
        self.limited |= other.limited;
    }

    pub(crate) fn count(&self) -> usize {
        self.names.len() + self.unnamed
    }
}

fn is_disabled(value: &Value) -> bool {
    *value == Value::Bool(false)
        || get(value, "disabled") == Some(&Value::Bool(true))
        || get(value, "enabled") == Some(&Value::Bool(false))
}

/// Names declared under a key: object keys, string arrays, or arrays of `{name: ...}`.
pub(crate) fn declared(value: &Value) -> Names {
    let mut names = Names::default();
    match value {
        Value::Object(map) => {
            for (key, entry) in map.iter().take(MAX_DIR_ENTRIES) {
                names.add(key, is_disabled(entry));
            }
            names.limited = map.len() > MAX_DIR_ENTRIES;
        }
        Value::Array(items) => {
            for item in items.iter().take(MAX_DIR_ENTRIES) {
                match item {
                    Value::String(s) => names.add(s, false),
                    Value::Object(_) => match ["name", "id", "source", "package"]
                        .iter()
                        .find_map(|k| get(item, k).and_then(Value::as_str))
                    {
                        Some(name) => names.add(name, is_disabled(item)),
                        None => names.unnamed += 1,
                    },
                    _ => {}
                }
            }
            names.limited = items.len() > MAX_DIR_ENTRIES;
        }
        _ => {}
    }
    names
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Entries {
    /// Child directories that contain `SKILL.md`.
    SkillDirs,
    /// `SkillDirs`, plus one level of category directories (`<category>/<name>/SKILL.md`).
    SkillTree,
    /// Regular files with one of these extensions (name = file stem).
    Files(&'static [&'static str]),
    /// Child directories, plus files with these extensions.
    DirsOrFiles(&'static [&'static str]),
}

/// Enumerates one directory level. Returns None when the directory does not exist.
pub(crate) fn entries(dir: &Path, kind: Entries) -> Option<Result<Names, ()>> {
    let reader = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) if e.kind() == std::io::ErrorKind::NotADirectory => return None,
        Err(_) => return Some(Err(())),
    };
    let mut found: Vec<(String, bool)> = Vec::new();
    let mut limited = false;
    for (n, entry) in reader.enumerate() {
        if n >= MAX_DIR_ENTRIES {
            limited = true;
            break;
        }
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            found.push((String::new(), false));
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        // Follow symlinks: user-managed skill links are common.
        let path = entry.path();
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        let file_stem = |exts: &[&str]| {
            meta.is_file()
                .then(|| {
                    exts.iter().find_map(|ext| {
                        name.strip_suffix(ext)
                            .and_then(|stem| stem.strip_suffix('.'))
                            .filter(|s| !s.is_empty())
                    })
                })
                .flatten()
        };
        let item = match kind {
            Entries::SkillDirs => {
                (meta.is_dir() && path.join("SKILL.md").is_file()).then_some(name)
            }
            Entries::SkillTree if meta.is_dir() && !path.join("SKILL.md").is_file() => {
                // Category folders share the one directory-entry budget.
                if found.len() >= MAX_DIR_ENTRIES {
                    limited = true;
                    break;
                }
                if let Some(Ok(nested)) = entries(&path, Entries::SkillDirs) {
                    limited |= nested.limited;
                    found.extend(nested.names.into_iter().map(|n| (n, false)));
                    for _ in 0..nested.unnamed {
                        found.push((String::new(), false));
                    }
                }
                None
            }
            Entries::SkillTree => meta.is_dir().then_some(name),
            Entries::Files(exts) => file_stem(exts),
            Entries::DirsOrFiles(exts) => {
                if meta.is_dir() {
                    Some(name)
                } else {
                    file_stem(exts)
                }
            }
        };
        if let Some(item) = item {
            found.push((item.to_owned(), false));
        }
    }
    // Directory order is not stable across filesystems.
    found.sort();
    let mut names = Names {
        limited,
        ..Names::default()
    };
    for (name, disabled) in found {
        if name.is_empty() {
            names.unnamed += 1;
        } else {
            names.add(&name, disabled);
        }
    }
    Some(Ok(names))
}

/// Project roots consulted for project-level config: the directory itself and its
/// ancestors up to the nearest VCS root (bounded).
///
/// The home directory is never a project root: its dot-folders are user config, and
/// reading them twice would also report a spurious project-trust state.
pub(crate) fn project_dirs(cwd: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut rooted = false;
    for dir in cwd.ancestors().take(8) {
        if Some(dir) == home {
            break;
        }
        dirs.push(dir.to_owned());
        if dir.join(".git").exists() {
            rooted = true;
            break;
        }
    }
    if !rooted {
        // Without a VCS root only the directory itself is a project root.
        dirs.truncate(1);
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn relaxed_json_accepts_jsonc_and_json5_subset() {
        let text = r#"{
            // comment
            model: 'anthropic/x', /* block */
            "mcpServers": { 'a-b': {enabled: false,}, c: {}, },
            list: ["x", 'y',],
            url: "http://h//not-a-comment",
        }"#;
        let v = parse(text, Format::Json).unwrap();
        assert_eq!(v["model"], "anthropic/x");
        assert_eq!(v["url"], "http://h//not-a-comment");
        let names = declared(&v["mcpServers"]);
        assert_eq!(names.names, ["a-b", "c"]);
        assert_eq!(names.disabled, ["a-b"]);
        assert_eq!(parse("{\"a\": 1", Format::Json), None);
        assert_eq!(parse("{'a': \"it's\"}", Format::Json).unwrap()["a"], "it's");
    }

    #[test]
    fn yaml_subset_reads_nested_maps_and_lists() {
        let text = "# c\nmodel:\n  default: \"hermes-4\"   # note\n  provider: nous\nmcp_servers:\n  docs:\n    command: npx\n    enabled: false\n  web: {}\ntoolsets:\n  - web\n  - terminal\nprompt: |\n  ignore me: true\n  - item\nagents:\n  - name: helper\n    model: m\nflags: [a, 'b']\n";
        let v = parse(text, Format::Yaml).unwrap();
        assert_eq!(v["model"]["default"], "hermes-4");
        assert_eq!(v["model"]["provider"], "nous");
        assert_eq!(declared(&v["mcp_servers"]).names, ["docs", "web"]);
        assert_eq!(declared(&v["mcp_servers"]).disabled, ["docs"]);
        assert_eq!(v["toolsets"], json!(["web", "terminal"]));
        assert_eq!(v["prompt"], Value::Null);
        assert_eq!(declared(&v["agents"]).names, ["helper"]);
        assert_eq!(v["flags"], json!(["a", "b"]));
        let list = parse(
            "- insert:\n    - id: a\n      config:\n        model: m\n    - id: b\n- other: 1\n",
            Format::Yaml,
        )
        .unwrap();
        assert_eq!(list[0]["insert"][0]["config"]["model"], "m");
        assert_eq!(list[0]["insert"][1]["id"], "b");
        assert_eq!(list[1]["other"], "1");
        // PyYAML's default style puts list items at the key's indent.
        let pyyaml = "toolsets:\n- web\n- terminal\nmcp_servers:\n  docs:\n    args:\n    - -y\n    - pkg\n    enabled: false\n  b: {command: x, enabled: Off}\n  c:\n    enabled: NO\napprovals:\n  mode: manual\n";
        let v = parse(pyyaml, Format::Yaml).unwrap();
        assert_eq!(v["toolsets"], json!(["web", "terminal"]));
        assert_eq!(v["mcp_servers"]["docs"]["args"], json!(["-y", "pkg"]));
        assert_eq!(declared(&v["mcp_servers"]).disabled, ["b", "c", "docs"]);
        assert_eq!(v["approvals"]["mode"], "manual");
    }

    #[test]
    fn yaml_flow_collections_and_spacing_variants() {
        let text = "servers:\n  a: {command: \"x,y\", enabled: false}\n  b: {command: x, args: [a, 'b, c'], env: {K: v}, enabled: off}\n  c: {}\nagents:\n-   name: helper\n    enabled: false\n-   name: other\nmodel:\tm\n";
        let v = parse(text, Format::Yaml).unwrap();
        assert_eq!(v["servers"]["a"]["command"], "x,y");
        assert_eq!(v["servers"]["b"]["args"], json!(["a", "b, c"]));
        assert_eq!(declared(&v["servers"]).disabled, ["a", "b"]);
        assert_eq!(v["agents"][0]["enabled"], false);
        assert_eq!(declared(&v["agents"]).names, ["helper", "other"]);
        assert_eq!(v["model"], "m");
        let top = parse("# c\n{a: 1, b: [x, {c: on}]}\n", Format::Yaml).unwrap();
        assert_eq!(top, json!({"a": "1", "b": ["x", {"c": true}]}));
        let commented = parse("{a: 1, # c\n b: [x, # d\n y]} # e\n", Format::Yaml).unwrap();
        assert_eq!(commented, json!({"a": "1", "b": ["x", "y"]}));
        let inline = parse("s: {u: 'a#b', v: c#d} # e\n", Format::Yaml).unwrap();
        assert_eq!(inline["s"], json!({"u": "a#b", "v": "c#d"}));
        // Unbalanced flow syntax is not guessed.
        assert_eq!(
            parse("a: {b: [1}\n", Format::Yaml).unwrap()["a"],
            Value::Null
        );
    }

    #[test]
    fn json5_numbers_and_line_continuations() {
        let v = parse(
            "{a: +1, b: .5, c: 5., d: 0x10, e: -Infinity, f: NaN, g: 'one \\\ntwo', h: [1e3, -2.5E-1]}",
            Format::Json,
        )
        .unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], 0.5);
        assert_eq!(v["c"], 5);
        assert_eq!(v["d"], 16);
        assert_eq!(v["e"], Value::Null);
        assert_eq!(v["f"], Value::Null);
        assert_eq!(v["g"], "one two");
        assert_eq!(v["h"], json!([1000.0, -0.25]));
    }

    #[test]
    fn higher_layer_decides_disabled_state_in_any_order() {
        let layer = |entries: &[(&str, bool)]| {
            let mut n = Names::default();
            entries.iter().for_each(|(name, off)| n.add(name, *off));
            n
        };
        let user = || layer(&[("docs", true), ("web", true), ("fs", false)]);
        let project = || layer(&[("docs", false), ("fs", true)]);
        for user_first in [true, false] {
            let mut all = Names::default();
            if user_first {
                all.merge(user(), 1);
                all.merge(project(), 2);
            } else {
                all.merge(project(), 2);
                all.merge(user(), 1);
            }
            let mut disabled = all.disabled.clone();
            disabled.sort();
            assert_eq!(disabled, ["fs", "web"], "user_first={user_first}");
            assert_eq!(all.count(), 3);
        }
    }

    #[test]
    fn toml_and_lookup() {
        let v = parse(
            "model = \"gpt-x\"\n[mcp_servers.docs]\ncommand = \"x\"\n[profiles.fast]\nmodel = \"m\"\n",
            Format::Toml,
        )
        .unwrap();
        assert_eq!(
            get(&v, "profiles.fast.model").and_then(scalar).as_deref(),
            Some("m")
        );
        assert_eq!(declared(&v["mcp_servers"]).names, ["docs"]);
        assert_eq!(get(&v, "missing.key"), None);
        let dotted = json!({"amp.mcpServers": {"a": {}}, "p": {"x": false}});
        assert_eq!(
            declared(get(&dotted, "amp.mcpServers").unwrap()).names,
            ["a"]
        );
        assert_eq!(declared(&dotted["p"]).disabled, ["x"]);
        assert_eq!(scalar(&json!("has space and $(cmd)")), None);
    }

    #[test]
    fn directory_inventory_is_bounded_and_sorted() {
        let d = tempfile::tempdir().unwrap();
        for name in ["zeta", "alpha", "no-skill"] {
            fs::create_dir(d.path().join(name)).unwrap();
        }
        fs::write(d.path().join("zeta/SKILL.md"), "x").unwrap();
        fs::write(d.path().join("alpha/SKILL.md"), "x").unwrap();
        fs::write(d.path().join("helper.md"), "x").unwrap();
        fs::write(d.path().join(".hidden.md"), "x").unwrap();
        let skills = entries(d.path(), Entries::SkillDirs).unwrap().unwrap();
        assert_eq!(skills.names, ["alpha", "zeta"]);
        let agents = entries(d.path(), Entries::Files(&["md"])).unwrap().unwrap();
        assert_eq!(agents.names, ["helper"]);
        assert!(entries(&d.path().join("missing"), Entries::SkillDirs).is_none());
        for i in 0..MAX_DIR_ENTRIES + 5 {
            fs::write(d.path().join(format!("f{i}.md")), "").unwrap();
        }
        let many = entries(d.path(), Entries::Files(&["md"])).unwrap().unwrap();
        assert!(many.limited && many.count() <= MAX_DIR_ENTRIES);
    }
}
