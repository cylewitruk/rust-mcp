use std::path::{Path, PathBuf};

use tracing::{debug, warn};

use crate::state::{AppState, OutboundSource};

/// Downloads and extracts a `.crate` tarball from `static.crates.io` into the
/// local source cache directory.
///
/// Returns the extracted directory path on success.
pub async fn download_and_extract_crate_source(
    state: &AppState,
    crate_name: &str,
    version: &str,
) -> Result<PathBuf, String> {
    let cache_dir = &state
        .config
        .crate_source_cache_dir;
    let dir_name = format!("{crate_name}-{version}");
    let target_dir = cache_dir.join(&dir_name);

    // Already extracted — skip download.
    if target_dir.is_dir() {
        debug!(%crate_name, %version, "crate source already cached on disk");
        return Ok(target_dir);
    }

    let url = format!("https://static.crates.io/crates/{crate_name}/{crate_name}-{version}.crate");
    debug!(%crate_name, %version, %url, "downloading crate source");

    state
        .acquire_outbound_slot(OutboundSource::CratesIo)
        .await;

    let response = state
        .http
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("crate source download failed for {crate_name}@{version}: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "crate source download failed for {crate_name}@{version}: HTTP {status}"
        ));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("failed to read crate source body for {crate_name}@{version}: {e}"))?;

    // Extract in a blocking task to avoid blocking the async runtime.
    let cache_dir_owned = cache_dir.clone();
    let crate_name_owned = crate_name.to_string();
    let version_owned = version.to_string();

    tokio::task::spawn_blocking(move || {
        extract_crate_tarball(&bytes, &cache_dir_owned, &crate_name_owned, &version_owned)
    })
    .await
    .map_err(|e| format!("crate source extraction task panicked: {e}"))?
}

fn extract_crate_tarball(
    bytes: &[u8],
    cache_dir: &Path,
    crate_name: &str,
    version: &str,
) -> Result<PathBuf, String> {
    let dir_name = format!("{crate_name}-{version}");
    let target_dir = cache_dir.join(&dir_name);

    // Double-check after acquiring the blocking slot.
    if target_dir.is_dir() {
        return Ok(target_dir);
    }

    std::fs::create_dir_all(cache_dir).map_err(|e| {
        format!("failed to create crate source cache directory {}: {e}", cache_dir.display())
    })?;

    // .crate files are gzipped tarballs. The tarball root is `{name}-{version}/`.
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);

    // Extract to the cache dir. The tarball root `{name}-{version}/` means
    // files land in `{cache_dir}/{name}-{version}/...`.
    archive
        .unpack(cache_dir)
        .map_err(|e| format!("failed to extract crate source for {crate_name}@{version}: {e}"))?;

    if !target_dir.is_dir() {
        warn!(
            %crate_name, %version,
            "crate tarball extracted but expected directory {} not found",
            target_dir.display()
        );
        return Err(format!(
            "crate source extraction produced unexpected layout for {crate_name}@{version}"
        ));
    }

    debug!(
        %crate_name, %version,
        path = %target_dir.display(),
        "crate source extracted successfully"
    );
    Ok(target_dir)
}

#[cfg(test)]
mod tests {
    use super::extract_crate_tarball;

    /// Creates a minimal gzipped tarball that mimics a `.crate` file layout:
    /// `{name}-{version}/Cargo.toml` and `{name}-{version}/src/lib.rs`.
    fn build_synthetic_crate_tarball(name: &str, version: &str, lib_content: &str) -> Vec<u8> {
        use std::io::Write as _;

        use flate2::Compression;
        use flate2::write::GzEncoder;

        let prefix = format!("{name}-{version}");

        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);

            // Add Cargo.toml.
            let cargo_toml = format!(
                "[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n"
            );
            let mut header = tar::Header::new_gnu();
            header
                .set_path(format!("{prefix}/Cargo.toml"))
                .unwrap();
            header.set_size(cargo_toml.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append(&header, cargo_toml.as_bytes())
                .unwrap();

            // Add src/lib.rs.
            let mut header = tar::Header::new_gnu();
            header
                .set_path(format!("{prefix}/src/lib.rs"))
                .unwrap();
            header.set_size(lib_content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append(&header, lib_content.as_bytes())
                .unwrap();

            builder.finish().unwrap();
        }

        // Gzip the tar.
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder
            .write_all(&tar_bytes)
            .unwrap();
        encoder.finish().unwrap()
    }

    fn make_temp_dir(label: &str) -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rust-mcp-test-{label}-{nanos}"));
        std::fs::create_dir_all(&dir).expect("failed to create temp dir");
        dir
    }

    #[test]
    fn extract_crate_tarball_creates_expected_directory_and_files() {
        let cache_dir = make_temp_dir("extract");
        let _cleanup = CleanupDir(cache_dir.clone());

        let lib_content = "pub fn hello() -> &'static str { \"world\" }";
        let tarball = build_synthetic_crate_tarball("tiny_crate", "0.1.0", lib_content);

        let result = extract_crate_tarball(&tarball, &cache_dir, "tiny_crate", "0.1.0");
        assert!(result.is_ok(), "extraction failed: {:?}", result.err());

        let extracted_dir = result.unwrap();
        assert!(extracted_dir.is_dir());
        assert_eq!(
            extracted_dir
                .file_name()
                .unwrap()
                .to_str()
                .unwrap(),
            "tiny_crate-0.1.0"
        );

        // Verify Cargo.toml exists.
        let cargo_toml = extracted_dir.join("Cargo.toml");
        assert!(cargo_toml.exists(), "Cargo.toml should exist");
        let toml_content = std::fs::read_to_string(&cargo_toml).unwrap();
        assert!(toml_content.contains("tiny_crate"));

        // Verify src/lib.rs exists with expected content.
        let lib_rs = extracted_dir.join("src/lib.rs");
        assert!(lib_rs.exists(), "src/lib.rs should exist");
        let actual = std::fs::read_to_string(&lib_rs).unwrap();
        assert_eq!(actual, lib_content);
    }

    #[test]
    fn extract_crate_tarball_is_idempotent() {
        let cache_dir = make_temp_dir("idempotent");
        let _cleanup = CleanupDir(cache_dir.clone());

        let tarball = build_synthetic_crate_tarball("demo", "1.0.0", "fn main() {}");

        // First extraction.
        let first = extract_crate_tarball(&tarball, &cache_dir, "demo", "1.0.0");
        assert!(first.is_ok());

        // Second extraction — should succeed (directory already exists).
        let second = extract_crate_tarball(&tarball, &cache_dir, "demo", "1.0.0");
        assert!(second.is_ok());
        assert_eq!(first.unwrap(), second.unwrap());
    }

    #[test]
    fn extract_crate_tarball_rejects_invalid_gzip() {
        let cache_dir = make_temp_dir("invalid");
        let _cleanup = CleanupDir(cache_dir.clone());
        let result = extract_crate_tarball(b"not a gzip", &cache_dir, "bad", "0.0.1");
        assert!(result.is_err());
    }

    struct CleanupDir(std::path::PathBuf);
    impl Drop for CleanupDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
