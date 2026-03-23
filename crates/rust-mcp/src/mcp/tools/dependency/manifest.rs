//! Shared Cargo.toml manifest parsing using the `cargo_toml` crate.

use std::collections::BTreeMap;

use cargo_toml::{Dependency, DepsSet, Inheritable, Manifest};

/// A dependency extracted from a Cargo.toml manifest.
#[derive(Debug, Clone)]
pub struct ParsedDependency {
    /// Crate name as declared in the manifest (the dependency key).
    pub name: String,
    /// Semver version requirement, if one was specified (e.g. `"1.0"`).
    pub requirement: Option<String>,
}

/// Parsed manifest metadata and dependencies.
#[derive(Debug)]
pub struct ParsedManifest {
    /// Value of `[package] name`, if present.
    pub package_name: Option<String>,
    /// Value of `[package] rust-version` (MSRV), if explicitly set.
    pub package_rust_version: Option<String>,
    /// Deduplicated dependencies collected from all manifest sections.
    pub dependencies: Vec<ParsedDependency>,
}

/// Parse a Cargo.toml manifest string and extract package metadata and all
/// dependencies (regular, dev, build, and target-specific).
///
/// Dependencies appearing in multiple sections are deduplicated by name,
/// preferring entries that have an explicit version requirement.
pub fn parse_manifest(manifest: &str) -> Result<ParsedManifest, String> {
    let parsed = Manifest::from_slice(manifest.as_bytes())
        .map_err(|e| format!("invalid Cargo.toml: {e}"))?;

    let package_name = parsed
        .package
        .as_ref()
        .map(|p| p.name.clone());

    let package_rust_version = parsed
        .package
        .as_ref()
        .and_then(|p| p.rust_version.as_ref())
        .and_then(|v| match v {
            Inheritable::Set(s) => Some(s.clone()),
            Inheritable::Inherited => None,
        });

    let mut merged = BTreeMap::<String, ParsedDependency>::new();

    collect_deps_set(&parsed.dependencies, &mut merged);
    collect_deps_set(&parsed.dev_dependencies, &mut merged);
    collect_deps_set(&parsed.build_dependencies, &mut merged);

    for target in parsed.target.values() {
        collect_deps_set(&target.dependencies, &mut merged);
        collect_deps_set(&target.dev_dependencies, &mut merged);
        collect_deps_set(&target.build_dependencies, &mut merged);
    }

    Ok(ParsedManifest {
        package_name,
        package_rust_version,
        dependencies: merged.into_values().collect(),
    })
}

fn collect_deps_set(deps: &DepsSet, merged: &mut BTreeMap<String, ParsedDependency>) {
    for (name, dep) in deps {
        let requirement = version_req(dep);
        let parsed = ParsedDependency {
            name: name.clone(),
            requirement,
        };

        let key = name.to_ascii_lowercase();
        match merged.get(&key) {
            None => {
                merged.insert(key, parsed);
            }
            Some(existing) if existing.requirement.is_none() && parsed.requirement.is_some() => {
                merged.insert(key, parsed);
            }
            _ => {}
        }
    }
}

fn version_req(dep: &Dependency) -> Option<String> {
    dep.try_req()
        .ok()
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty() && r != "*")
}

#[cfg(test)]
mod tests {
    use super::parse_manifest;

    #[test]
    fn parses_root_and_target_dependencies() {
        let manifest = r#"
            [package]
            name = "demo"
            rust-version = "1.70"

            [dependencies]
            serde = "1"

            [dev-dependencies]
            tokio = { version = "1" }

            [build-dependencies]
            cc = "1.0"

            [target.'cfg(unix)'.dependencies]
            libc = "0.2"
        "#;

        let parsed = parse_manifest(manifest).expect("manifest parses");
        let names: Vec<_> = parsed
            .dependencies
            .iter()
            .map(|d| d.name.as_str())
            .collect();

        assert!(names.contains(&"serde"));
        assert!(names.contains(&"tokio"));
        assert!(names.contains(&"cc"));
        assert!(names.contains(&"libc"));
        assert_eq!(parsed.package_name.as_deref(), Some("demo"));
        assert_eq!(
            parsed
                .package_rust_version
                .as_deref(),
            Some("1.70")
        );
    }

    #[test]
    fn deduplicates_preferring_versioned() {
        let manifest = r#"
            [dependencies]
            serde = { path = "../serde" }

            [target.'cfg(windows)'.dependencies]
            serde = "1.0"
        "#;

        let parsed = parse_manifest(manifest).expect("manifest parses");
        let serde = parsed
            .dependencies
            .iter()
            .find(|d| d.name == "serde")
            .expect("serde should be present");
        assert!(serde.requirement.is_some(), "should prefer the entry with a version requirement");
    }

    #[test]
    fn handles_workspace_deps_gracefully() {
        // workspace deps without a workspace root will have no version req
        let manifest = r#"
            [package]
            name = "child"

            [dependencies]
            tokio = { workspace = true }
        "#;

        let parsed = parse_manifest(manifest).expect("manifest parses");
        assert_eq!(parsed.dependencies.len(), 1);
        assert_eq!(parsed.dependencies[0].name, "tokio");
    }

    #[test]
    fn handles_renamed_packages() {
        let manifest = r#"
            [package]
            name = "demo"

            [dependencies]
            my_serde = { package = "serde", version = "1" }
        "#;

        let parsed = parse_manifest(manifest).expect("manifest parses");
        // cargo_toml uses the key name, not the package name
        assert_eq!(parsed.dependencies.len(), 1);
    }

    #[test]
    fn empty_manifest() {
        let parsed = parse_manifest("").expect("empty manifest parses");
        assert!(parsed.dependencies.is_empty());
        assert!(parsed.package_name.is_none());
    }
}
