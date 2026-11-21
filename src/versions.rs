//! A deliberately small comparison language. Unsupported is not a mismatch.
use semver::Version;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Comparison {
    NoConstraint,
    Match,
    Mismatch,
    Unverified,
}

#[derive(Clone, Copy)]
pub(crate) enum Syntax {
    Pin,
    Npm,
    Python,
}

pub(crate) fn compare(
    installed: Option<&str>,
    requirement: Option<&str>,
    syntax: Syntax,
) -> Comparison {
    let Some(requirement) = requirement else {
        return Comparison::NoConstraint;
    };
    let Some(installed) = installed.and_then(normalized) else {
        return Comparison::Unverified;
    };
    let mut matches = true;
    let parts = requirement.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.len() > 4 {
        return Comparison::Unverified;
    }
    for clause in parts {
        let (op, value) = [">=", "<=", "==", ">", "<", "=", "~", "^"]
            .iter()
            .find_map(|op| clause.strip_prefix(op).map(|v| (*op, v)))
            .unwrap_or(("=", clause));
        let value = value.trim();
        let wildcard = value.ends_with(".*") || value.ends_with(".x");
        let value = if wildcard {
            &value[..value.len() - 2]
        } else {
            value
        };
        if wildcard && !matches!(op, "=" | "==") {
            return Comparison::Unverified;
        }
        let Some(spec) = normalized(value) else {
            return Comparison::Unverified;
        };
        let count = value
            .split(['-', '+'])
            .next()
            .unwrap_or_default()
            .split('.')
            .count();
        let precedence = installed.cmp_precedence(&spec);
        let prefix_order = if count == 1 {
            installed.major.cmp(&spec.major)
        } else {
            (installed.major, installed.minor).cmp(&(spec.major, spec.minor))
        };
        let partial_npm = matches!(syntax, Syntax::Npm) && count < 3;
        let accepted = match op {
            ">=" => precedence.is_ge(),
            "<=" if partial_npm => prefix_order.is_le(),
            ">" if partial_npm => prefix_order.is_gt(),
            "<=" => precedence.is_le(),
            ">" => precedence.is_gt(),
            "<" => precedence.is_lt(),
            "=" | "==" if (count < 3 && !matches!(syntax, Syntax::Python)) || wildcard => {
                installed.major == spec.major
                    && (count == 1 || installed.minor == spec.minor)
                    && (count != 3 || installed.patch == spec.patch)
            }
            "=" | "==" => precedence.is_eq(),
            "~" if count == 3 => {
                precedence.is_ge() && installed.major == spec.major && installed.minor == spec.minor
            }
            "^" if count == 3 => {
                precedence.is_ge()
                    && installed.major == spec.major
                    && (spec.major != 0 || installed.minor == spec.minor)
                    && (spec.major != 0 || spec.minor != 0 || installed.patch == spec.patch)
            }
            _ => return Comparison::Unverified,
        };
        // Package and interpreter ranges differ for pre-release selection. Only
        // compare pre-releases when the declaration itself is an exact version.
        if !installed.pre.is_empty() && (!matches!(op, "=" | "==") || count != 3 || wildcard) {
            return Comparison::Unverified;
        }
        matches &= accepted;
    }
    if matches {
        Comparison::Match
    } else {
        Comparison::Mismatch
    }
}

fn normalized(value: &str) -> Option<Version> {
    let value = value.strip_prefix('v').unwrap_or(value);
    if let Ok(version) = Version::parse(value) {
        return Some(version);
    }
    if !value.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    match value.split('.').count() {
        1 => Version::parse(&format!("{value}.0.0")).ok(),
        2 => Version::parse(&format!("{value}.0")).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ecosystems_handle_partial_versions_differently() {
        assert_eq!(
            compare(Some("22.3.0"), Some("<=22"), Syntax::Npm),
            Comparison::Match
        );
        assert_eq!(
            compare(Some("22.3.0"), Some(">22"), Syntax::Npm),
            Comparison::Mismatch
        );
        assert_eq!(
            compare(Some("3.12.5"), Some("==3.12"), Syntax::Python),
            Comparison::Mismatch
        );
        assert_eq!(
            compare(Some("3.12.5"), Some("==3.12.*"), Syntax::Python),
            Comparison::Match
        );
    }

    #[test]
    fn bounded_ranges_never_guess() {
        for (v, r, expected) in [
            ("24.1.0", ">=22", Comparison::Match),
            ("18.10.0", ">=22", Comparison::Mismatch),
            ("3.12.6", ">=3.10,<3.13", Comparison::Match),
            ("3.13.0", "3.12.x", Comparison::Mismatch),
            ("9.2.1", "9.2", Comparison::Match),
            ("7.0.0", "^7.0.0", Comparison::Match),
            ("7.0.0", "10 || 9", Comparison::Unverified),
            ("7.0.0", ">=8,garbage", Comparison::Unverified),
            ("1.2.3-beta", ">=1.0.0", Comparison::Unverified),
            ("1.2.3-beta", "1.2.3-beta", Comparison::Match),
        ] {
            assert_eq!(
                compare(Some(v), Some(r), Syntax::Pin),
                expected,
                "{v} vs {r}"
            );
        }
    }
}
