//! The optional `config.toml`. Sakshi runs the same without it.
//!
//! ```toml
//! [trust]
//! # Extra paths to flag when an agent touches them. Severity defaults to "warn".
//! sensitive_paths = [
//!   { pattern = "src/ledger/**", severity = "alert" },
//!   { pattern = "**/kyc/**" },
//! ]
//! # Paths never flagged as sensitive, not even by the built-in rules.
//! ignore_paths = ["fixtures/**"]
//! ```

use std::path::Path;

use anyhow::{bail, Context, Result};
use sakshi_core::{PathRule, Severity, TrustConfig};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    trust: TrustSection,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustSection {
    #[serde(default)]
    sensitive_paths: Vec<RuleEntry>,
    #[serde(default)]
    ignore_paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleEntry {
    pattern: String,
    #[serde(default = "default_severity")]
    severity: String,
}

fn default_severity() -> String {
    "warn".to_string()
}

/// Parse a config file's contents.
pub fn parse(text: &str) -> Result<TrustConfig> {
    let file: File = toml::from_str(text)?;
    let mut sensitive_paths = Vec::new();
    for entry in file.trust.sensitive_paths {
        if entry.pattern.trim().is_empty() {
            bail!("trust.sensitive_paths: a pattern is empty");
        }
        let severity: Severity = entry.severity.parse().map_err(|_| {
            anyhow::anyhow!(
                "trust.sensitive_paths: severity {:?} for {:?} must be \"info\", \"warn\" or \"alert\"",
                entry.severity,
                entry.pattern
            )
        })?;
        sensitive_paths.push(PathRule {
            pattern: entry.pattern,
            severity,
        });
    }
    Ok(TrustConfig {
        sensitive_paths,
        ignore_paths: file.trust.ignore_paths,
    })
}

/// Load the config at `path`. A missing file means the defaults; a broken
/// one is an error, so a typo never silently switches a rule off.
pub fn load(path: Option<&Path>) -> Result<TrustConfig> {
    let Some(path) = path else {
        return Ok(TrustConfig::default());
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TrustConfig::default());
        }
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    parse(&text).with_context(|| format!("invalid config in {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_is_the_default() {
        assert_eq!(parse("").unwrap(), TrustConfig::default());
    }

    #[test]
    fn parses_rules_with_and_without_severity() {
        let config = parse(
            r#"
            [trust]
            sensitive_paths = [
              { pattern = "src/ledger/**", severity = "alert" },
              { pattern = "**/kyc/**" },
            ]
            ignore_paths = ["fixtures/**"]
            "#,
        )
        .unwrap();
        assert_eq!(
            config.sensitive_paths,
            vec![
                PathRule {
                    pattern: "src/ledger/**".into(),
                    severity: Severity::Alert
                },
                PathRule {
                    pattern: "**/kyc/**".into(),
                    severity: Severity::Warn
                },
            ]
        );
        assert_eq!(config.ignore_paths, vec!["fixtures/**".to_string()]);
    }

    #[test]
    fn typos_and_bad_values_are_errors() {
        assert!(parse("[trust]\nsensitive_path = []").is_err());
        assert!(
            parse("[trust]\nsensitive_paths = [{ pattern = \"x\", severity = \"high\" }]").is_err()
        );
        assert!(parse("[trust]\nsensitive_paths = [{ pattern = \" \" }]").is_err());
        assert!(parse("not toml").is_err());
    }

    #[test]
    fn a_missing_file_is_the_default() {
        let path = std::env::temp_dir().join("sakshi-no-such-config.toml");
        assert_eq!(load(Some(&path)).unwrap(), TrustConfig::default());
        assert_eq!(load(None).unwrap(), TrustConfig::default());
    }
}
