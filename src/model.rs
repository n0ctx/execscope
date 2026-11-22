use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Available,
    NotFound,
    Unknown,
    Timeout,
    Unsupported,
}

macro_rules! choices {
    ($name:ident, $first:ident=$first_text:literal $(, $variant:ident=$text:literal)* $(,)?) => {
        #[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
        #[serde(rename_all = "lowercase")]
        pub(crate) enum $name { #[default] $first, $($variant),* }
        impl $name {
            pub(crate) const VALUES: &'static [&'static str] = &[$first_text, $($text),*];
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(match self { Self::$first => $first_text, $(Self::$variant => $text),* })
            }
        }
    }
}
choices!(
    Scope,
    Auto = "auto",
    System = "system",
    Dev = "dev",
    Hardware = "hardware",
    Network = "network",
    Harness = "harness",
    All = "all"
);
choices!(Detail, Brief = "brief", Full = "full");

pub(crate) const MAX_REQUIRED: usize = 16;
pub(crate) const BRIEF_TOKENS: std::ops::RangeInclusive<u16> = 100..=3000;
pub(crate) const DEFAULT_BRIEF_TOKENS: u16 = 600;
pub(crate) const TOOLS: &[&str] = &[
    "git", "python", "pip", "uv", "node", "npm", "pnpm", "yarn", "bun", "rustc", "cargo", "go",
    "swift", "java", "javac", "dotnet", "gcc", "clang", "cmake", "docker",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Tool(pub(crate) &'static str);

impl FromStr for Tool {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        TOOLS
            .iter()
            .find(|name| **name == s)
            .map(|name| Self(name))
            .ok_or("unsupported prerequisite identifier")
    }
}

impl Serialize for Tool {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for Tool {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        name.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct Request {
    pub(crate) scope: Scope,
    pub(crate) detail: Detail,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) refresh: bool,
    pub(crate) requires: Option<Vec<Tool>>,
    pub(crate) brief_tokens: Option<u16>,
}

impl Request {
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if let Some(budget) = self.brief_tokens {
            if self.detail == Detail::Full {
                return Err("brief_tokens applies only to detail=brief");
            }
            if !BRIEF_TOKENS.contains(&budget) {
                return Err("brief_tokens must be an integer from 100 to 3000");
            }
        }
        if let Some(requires) = &self.requires {
            if requires.len() > MAX_REQUIRED {
                return Err("at most 16 prerequisites are allowed");
            }
            if requires
                .iter()
                .enumerate()
                .any(|(i, item)| requires[..i].contains(item))
            {
                return Err("duplicate prerequisite identifier");
            }
        }
        Ok(())
    }
}

pub(crate) fn now() -> String {
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("system time is representable as RFC 3339")
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Sample<T> {
    pub(crate) sampled_at: String,
    pub(crate) cached: bool,
    #[serde(flatten)]
    pub(crate) data: T,
}

impl<T> Sample<T> {
    pub(crate) fn new(data: T) -> Self {
        Self {
            sampled_at: now(),
            cached: false,
            data,
        }
    }
}

pub(crate) fn is_false(v: &bool) -> bool {
    !v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_unique_requirements() {
        assert!(serde_json::from_str::<Request>(r#"{"requires":["Node"]}"#).is_err());
        assert!(serde_json::from_str::<Request>(r#"{"other":true}"#).is_err());
        assert!(
            serde_json::from_str::<Request>(r#"{"requires":["node","node"]}"#)
                .unwrap()
                .validate()
                .is_err()
        );
        let too_many = Tool("node");
        assert!(
            Request {
                requires: Some(vec![too_many; 17]),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn brief_budget_bounds_and_detail() {
        let parse = |s: &str| serde_json::from_str::<Request>(s).map(|r| r.validate());
        for ok in ["100", "600", "3000", "1234"] {
            assert_eq!(
                parse(&format!(r#"{{"brief_tokens":{ok}}}"#)).unwrap(),
                Ok(())
            );
        }
        for out in ["99", "3001", "0"] {
            assert!(
                parse(&format!(r#"{{"brief_tokens":{out}}}"#))
                    .unwrap()
                    .is_err()
            );
        }
        for bad in [r#""600""#, "600.5", "-1", "70000", "true"] {
            assert!(
                parse(&format!(r#"{{"brief_tokens":{bad}}}"#)).is_err(),
                "{bad}"
            );
        }
        assert!(
            parse(r#"{"brief_tokens":600,"detail":"full"}"#)
                .unwrap()
                .is_err()
        );
        assert!(parse(r#"{"scope":"harness"}"#).unwrap().is_ok());
    }
}
