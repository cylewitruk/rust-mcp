//! Ripgrep wrapper for structured source-file content search.
//!
//! Encapsulates all `rg` command construction, execution, and JSON output
//! parsing so that tool handlers never interact with `std::process::Command`
//! or parse ripgrep JSON directly.

use std::fmt;
use std::path::Path;
use std::process::Command;

/// A single match returned by ripgrep.
#[derive(Debug, Clone)]
pub struct RipgrepMatch {
    /// File path relative to the search root directory.
    pub path: String,
    /// 1-based line number of the match.
    pub line_number: u32,
    /// The full text of the matched line (trimmed of trailing newline).
    pub line_content: String,
}

/// Search mode for ripgrep invocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RipgrepMode {
    /// Case-insensitive fixed-string search (`rg -F -i`).
    FixedString,
    /// Regex search (`rg`). The pattern is passed as-is.
    Regex,
}

/// Error type for ripgrep operations.
#[derive(Debug)]
pub enum RipgrepError {
    /// The `rg` binary could not be spawned (not installed or not in PATH).
    NotFound(std::io::Error),
    /// `rg` returned a non-zero exit code other than 1 (no matches).
    ExecutionFailed {
        /// Process exit code, if available.
        exit_code: Option<i32>,
        /// Captured stderr output from the `rg` process.
        stderr: String,
    },
    /// Failed to parse a JSON line from ripgrep output.
    ParseError(String),
}

impl fmt::Display for RipgrepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(e) => write!(f, "ripgrep binary not found: {e}"),
            Self::ExecutionFailed { exit_code, stderr } => {
                write!(f, "ripgrep failed (exit code {:?}): {}", exit_code, stderr.trim())
            }
            Self::ParseError(msg) => write!(f, "ripgrep output parse error: {msg}"),
        }
    }
}

/// Search a directory tree for a pattern, returning structured matches.
///
/// Invokes `rg --json` with mode-appropriate flags. Parses the JSON Lines
/// output and returns up to `max_results` matches.
///
/// Returns an empty vec (not an error) when there are no matches.
pub fn search(
    dir: &Path,
    pattern: &str,
    mode: RipgrepMode,
    path_glob: Option<&str>,
    max_results: usize,
) -> Result<Vec<RipgrepMatch>, RipgrepError> {
    let mut cmd = Command::new("rg");
    cmd.arg("--json")
        .arg("--no-heading");

    match mode {
        RipgrepMode::FixedString => {
            cmd.arg("-F").arg("-i");
        }
        RipgrepMode::Regex => {
            // ripgrep defaults to regex mode; add case-insensitive for
            // consistency with the prior SQL `~*` operator.
            cmd.arg("-i");
        }
    }

    if let Some(glob) = path_glob {
        cmd.arg("--glob").arg(glob);
    }

    cmd.arg("--")
        .arg(pattern)
        .arg(dir);

    let output = cmd
        .output()
        .map_err(RipgrepError::NotFound)?;

    // Exit code 1 means "no matches found" — not an error.
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(RipgrepError::ExecutionFailed {
            exit_code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_json_output(&stdout, dir, max_results)
}

/// Search for a pattern across multiple directory trees, tagging each result
/// with a caller-provided value.
///
/// Runs [`search`] against each directory. Stops early once `max_total`
/// results have been collected across all directories.
pub fn search_multiple<T: Clone>(
    dirs: &[(impl AsRef<Path>, T)],
    pattern: &str,
    mode: RipgrepMode,
    path_glob: Option<&str>,
    max_per_dir: usize,
    max_total: usize,
) -> Result<Vec<(T, RipgrepMatch)>, RipgrepError> {
    let mut results = Vec::new();
    for (dir, tag) in dirs {
        if results.len() >= max_total {
            break;
        }
        let remaining = max_total.saturating_sub(results.len());
        let limit = max_per_dir.min(remaining);
        let matches = search(dir.as_ref(), pattern, mode, path_glob, limit)?;
        for m in matches {
            results.push((tag.clone(), m));
            if results.len() >= max_total {
                break;
            }
        }
    }
    Ok(results)
}

/// Parse ripgrep `--json` output into structured matches.
fn parse_json_output(
    stdout: &str,
    search_root: &Path,
    max_results: usize,
) -> Result<Vec<RipgrepMatch>, RipgrepError> {
    let mut matches = Vec::new();
    let prefix = search_root.to_string_lossy();

    for line in stdout.lines() {
        if matches.len() >= max_results {
            break;
        }

        let value: serde_json::Value =
            serde_json::from_str(line).map_err(|e| RipgrepError::ParseError(e.to_string()))?;

        let Some(msg_type) = value
            .get("type")
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        if msg_type != "match" {
            continue;
        }

        let Some(data) = value.get("data") else {
            continue;
        };

        let raw_path = data
            .pointer("/path/text")
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        // Make the path relative to the search root.
        let relative_path = raw_path
            .strip_prefix(prefix.as_ref())
            .unwrap_or(raw_path)
            .trim_start_matches('/')
            .trim_start_matches('\\')
            .to_string();

        let line_number = data
            .get("line_number")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        let line_content = data
            .pointer("/lines/text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_string();

        matches.push(RipgrepMatch {
            path: relative_path,
            line_number,
            line_content,
        });
    }

    Ok(matches)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    fn create_temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rust_mcp_rg_test_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("failed to create temp dir");
        dir
    }

    fn write_file(dir: &Path, relative_path: &str, content: &str) {
        let full_path = dir.join(relative_path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).expect("failed to create parent dir");
        }
        fs::write(&full_path, content).expect("failed to write file");
    }

    #[test]
    fn search_fixed_string_finds_matches() {
        let dir = create_temp_dir("fixed_string");

        write_file(&dir, "src/lib.rs", "pub fn hello() {\n    println!(\"hello world\");\n}\n");
        write_file(&dir, "src/util.rs", "fn helper() {}\nfn hello_again() {}\n");
        write_file(&dir, "src/other.rs", "fn nothing_here() {}\n");

        let results = search(&dir, "hello", RipgrepMode::FixedString, None, 100).unwrap();

        // Should find "hello" in lib.rs (lines 1 and 2) and util.rs (line 2).
        assert!(results.len() >= 2, "expected at least 2 matches, got {}", results.len());
        assert!(
            results
                .iter()
                .any(|m| m.path.contains("lib.rs"))
        );
        assert!(
            results
                .iter()
                .any(|m| m.path.contains("util.rs"))
        );

        // Verify line numbers are present.
        for m in &results {
            assert!(m.line_number > 0, "line number should be > 0");
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_regex_finds_pattern() {
        let dir = create_temp_dir("regex");

        write_file(
            &dir,
            "src/lib.rs",
            "pub fn compute(x: i32) -> i32 {\n    x + 1\n}\npub fn transform() -> bool {\n    \
             true\n}\n",
        );

        let results = search(&dir, r"fn\s+\w+\(", RipgrepMode::Regex, None, 100).unwrap();

        assert!(results.len() >= 2, "expected at least 2 function matches, got {}", results.len());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_with_path_glob_filters_files() {
        let dir = create_temp_dir("glob");

        write_file(&dir, "src/lib.rs", "fn target() {}\n");
        write_file(&dir, "Cargo.toml", "[package]\nname = \"target\"\n");

        let results = search(&dir, "target", RipgrepMode::FixedString, Some("*.rs"), 100).unwrap();

        assert!(!results.is_empty(), "expected matches in .rs files");
        assert!(
            results
                .iter()
                .all(|m| m.path.ends_with(".rs")),
            "all matches should be in .rs files"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_returns_empty_on_no_match() {
        let dir = create_temp_dir("no_match");

        write_file(&dir, "src/lib.rs", "pub fn hello() {}\n");

        let results =
            search(&dir, "zzz_nonexistent_pattern_zzz", RipgrepMode::FixedString, None, 100)
                .unwrap();

        assert!(results.is_empty(), "expected no matches");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_caps_at_max_results() {
        let dir = create_temp_dir("max_results");

        let mut content = String::new();
        for i in 0..20 {
            content.push_str(&format!("fn func_{i}() {{ let x = target; }}\n"));
        }
        write_file(&dir, "src/lib.rs", &content);

        let results = search(&dir, "target", RipgrepMode::FixedString, None, 3).unwrap();

        assert_eq!(results.len(), 3, "expected exactly 3 results (capped by max_results)");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_multiple_tags_results_correctly() {
        let dir_a = create_temp_dir("multi_a");
        let dir_b = create_temp_dir("multi_b");

        write_file(&dir_a, "src/lib.rs", "use tokio;\nfn a_func() {}\n");
        write_file(&dir_b, "src/lib.rs", "use tokio;\nfn b_func() {}\n");

        let dirs: Vec<(&Path, &str)> = vec![(&dir_a, "crate_a"), (&dir_b, "crate_b")];

        let results =
            search_multiple(&dirs, "tokio", RipgrepMode::FixedString, None, 10, 100).unwrap();

        assert!(results.len() >= 2, "expected matches from both directories");
        assert!(
            results
                .iter()
                .any(|(tag, _)| *tag == "crate_a")
        );
        assert!(
            results
                .iter()
                .any(|(tag, _)| *tag == "crate_b")
        );

        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }

    #[test]
    fn search_handles_invalid_regex_gracefully() {
        let dir = create_temp_dir("bad_regex");

        write_file(&dir, "src/lib.rs", "fn hello() {}\n");

        let result = search(&dir, "[invalid(regex", RipgrepMode::Regex, None, 100);

        assert!(result.is_err(), "expected an error for invalid regex");
        match result.unwrap_err() {
            RipgrepError::ExecutionFailed { .. } => {}
            other => panic!("expected ExecutionFailed, got: {other}"),
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
