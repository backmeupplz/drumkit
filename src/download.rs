use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Normalize user input into `owner/repo` format.
///
/// Handles URLs like `https://github.com/owner/repo`, `github.com/owner/repo/`,
/// `www.github.com/owner/repo`, or plain `owner/repo`. Strips trailing slashes.
/// Returns `None` if the input doesn't contain a valid `owner/repo` pair.
pub fn normalize_repo_input(raw: &str) -> Option<String> {
    let mut s = raw.trim().to_string();

    // Strip protocol
    for prefix in &["https://", "http://"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
        }
    }

    // Strip www.
    if let Some(rest) = s.strip_prefix("www.") {
        s = rest.to_string();
    }

    // Strip github.com/
    if let Some(rest) = s.strip_prefix("github.com/") {
        s = rest.to_string();
    }

    // Strip trailing slashes
    let s = s.trim_end_matches('/');

    // Must be exactly owner/repo (two non-empty parts)
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        Some(format!("{}/{}", parts[0], parts[1]))
    } else {
        None
    }
}

/// A kit available for download from a remote repository.
pub struct RemoteKit {
    pub name: String,
    pub repo: String,
    pub file_count: usize,
    pub total_bytes: u64,
    pub installed: bool,
}

/// A row in the kit store list — either a repo header or a selectable kit.
pub enum StoreRow {
    RepoHeader(String),
    Kit(usize),
}

/// Build a flat list of `StoreRow`s from kits, inserting repo headers when the repo changes.
pub fn build_store_rows(kits: &[RemoteKit]) -> Vec<StoreRow> {
    let mut rows = Vec::new();
    let mut last_repo: Option<&str> = None;
    for (i, kit) in kits.iter().enumerate() {
        if last_repo != Some(&kit.repo) {
            rows.push(StoreRow::RepoHeader(kit.repo.clone()));
            last_repo = Some(&kit.repo);
        }
        rows.push(StoreRow::Kit(i));
    }
    rows
}

/// Information about a single file in the remote tree.
struct TreeBlob {
    path: String,
    #[allow(dead_code)]
    size: u64,
}

fn tree_url(repo: &str) -> String {
    format!(
        "https://api.github.com/repos/{}/git/trees/main?recursive=1",
        repo
    )
}

fn raw_base(repo: &str) -> String {
    format!("https://raw.githubusercontent.com/{}/main", repo)
}

fn fetch_tree(repo: &str) -> Result<serde_json::Value> {
    let url = tree_url(repo);
    let resp: serde_json::Value = ureq::get(&url)
        .set("User-Agent", "drumkit")
        .call()
        .with_context(|| format!("Failed to fetch tree from {}", repo))?
        .into_json()
        .context("Failed to parse GitHub tree response")?;
    Ok(resp)
}

/// Fetch the list of kits available across all configured repositories.
///
/// Uses a single GitHub Trees API call per repo and groups blobs by top-level directory.
pub fn fetch_kit_list(repos: &[String], local_dirs: &[PathBuf]) -> Result<Vec<RemoteKit>> {
    let mut all_kits: Vec<RemoteKit> = Vec::new();

    for repo in repos {
        match fetch_kit_list_single(repo, local_dirs) {
            Ok(kits) => all_kits.extend(kits),
            Err(e) => {
                // Include repo name in error but don't fail the whole list
                all_kits.push(RemoteKit {
                    name: format!("[error: {}]", e),
                    repo: repo.clone(),
                    file_count: 0,
                    total_bytes: 0,
                    installed: false,
                });
            }
        }
    }

    all_kits.sort_by(|a, b| (&a.repo, &a.name).cmp(&(&b.repo, &b.name)));
    Ok(all_kits)
}

fn fetch_kit_list_single(repo: &str, local_dirs: &[PathBuf]) -> Result<Vec<RemoteKit>> {
    let resp = fetch_tree(repo)?;

    let tree = resp
        .get("tree")
        .and_then(|t: &serde_json::Value| t.as_array())
        .context("Missing 'tree' array in response")?;

    // Group blobs by top-level directory name
    let mut kits_map: HashMap<String, (usize, u64)> = HashMap::new();

    for entry in tree {
        let entry_type = entry
            .get("type")
            .and_then(|t: &serde_json::Value| t.as_str())
            .unwrap_or("");
        if entry_type != "blob" {
            continue;
        }

        let path = entry
            .get("path")
            .and_then(|p: &serde_json::Value| p.as_str())
            .unwrap_or("");
        let size = entry
            .get("size")
            .and_then(|s: &serde_json::Value| s.as_u64())
            .unwrap_or(0);

        // Only include files inside a top-level directory (skip root files like README)
        if let Some(slash_pos) = path.find('/') {
            let kit_name = &path[..slash_pos];
            let entry = kits_map.entry(kit_name.to_string()).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += size;
        }
    }

    let kits: Vec<RemoteKit> = kits_map
        .into_iter()
        .map(|(name, (file_count, total_bytes))| {
            let installed = is_kit_installed(&name, local_dirs);
            RemoteKit {
                name,
                repo: repo.to_string(),
                file_count,
                total_bytes,
                installed,
            }
        })
        .collect();

    Ok(kits)
}

/// Download a kit from a specific repository to the local kits directory.
///
/// Downloads files from raw.githubusercontent.com (no API rate limits).
/// Uses a temp directory + rename for atomicity.
/// Reports progress via atomic counters.
pub fn download_kit(
    repo: &str,
    name: &str,
    progress: &Arc<AtomicUsize>,
    total: &Arc<AtomicUsize>,
) -> Result<PathBuf> {
    let target_dir = default_kits_dir().context("Cannot determine kits directory")?;
    std::fs::create_dir_all(&target_dir).context("Failed to create kits directory")?;

    let final_path = target_dir.join(name);
    if final_path.exists() {
        return Ok(final_path);
    }

    // Fetch tree to get list of files for this kit
    let resp = fetch_tree(repo)?;

    let tree = resp
        .get("tree")
        .and_then(|t: &serde_json::Value| t.as_array())
        .context("Missing 'tree' array in response")?;

    let prefix = format!("{}/", name);
    let blobs: Vec<TreeBlob> = tree
        .iter()
        .filter_map(|entry: &serde_json::Value| {
            let entry_type = entry
                .get("type")
                .and_then(|t: &serde_json::Value| t.as_str())?;
            if entry_type != "blob" {
                return None;
            }
            let path = entry
                .get("path")
                .and_then(|p: &serde_json::Value| p.as_str())?;
            if !path.starts_with(&prefix) {
                return None;
            }
            let size = entry
                .get("size")
                .and_then(|s: &serde_json::Value| s.as_u64())
                .unwrap_or(0);
            Some(TreeBlob {
                path: path.to_string(),
                size,
            })
        })
        .collect();

    if blobs.is_empty() {
        anyhow::bail!("No files found for kit '{}'", name);
    }

    total.store(blobs.len(), Ordering::Relaxed);
    progress.store(0, Ordering::Relaxed);

    // Download to a temp directory, then rename for atomicity
    let tmp_path = target_dir.join(format!(".{}.tmp", name));
    if tmp_path.exists() {
        std::fs::remove_dir_all(&tmp_path)?;
    }
    std::fs::create_dir_all(&tmp_path)?;

    let base = raw_base(repo);
    for blob in &blobs {
        let relative = &blob.path[prefix.len()..];
        let dest = tmp_path.join(relative);

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let url = format!("{}/{}", base, blob.path);
        let response = ureq::get(&url)
            .set("User-Agent", "drumkit")
            .call()
            .with_context(|| format!("Failed to download {}", blob.path))?;

        let mut reader = response.into_reader();
        let mut file = std::fs::File::create(&dest)
            .with_context(|| format!("Failed to create {}", dest.display()))?;
        std::io::copy(&mut reader, &mut file)
            .with_context(|| format!("Failed to write {}", dest.display()))?;

        progress.fetch_add(1, Ordering::Relaxed);
    }

    // Atomic rename
    std::fs::rename(&tmp_path, &final_path).context("Failed to finalize kit download")?;

    Ok(final_path)
}

/// Return the default kits directory (`$XDG_DATA_HOME/drumkit/kits` or
/// `~/.local/share/drumkit/kits`).
pub fn default_kits_dir() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        Some(PathBuf::from(xdg).join("drumkit/kits"))
    } else if let Ok(home) = std::env::var("HOME") {
        Some(PathBuf::from(home).join(".local/share/drumkit/kits"))
    } else {
        None
    }
}

/// Check whether a kit with the given name exists in any of the local search directories.
pub fn is_kit_installed(name: &str, extra_dirs: &[PathBuf]) -> bool {
    // Check default search dirs
    if let Some(dir) = default_kits_dir() {
        if dir.join(name).is_dir() {
            return true;
        }
    }
    // Check extra dirs
    for dir in extra_dirs {
        if dir.join(name).is_dir() {
            return true;
        }
    }
    false
}

/// Format bytes as a human-readable size string.
pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_plain_owner_repo() {
        assert_eq!(normalize_repo_input("owner/repo"), Some("owner/repo".to_string()));
    }

    #[test]
    fn normalize_https_url() {
        assert_eq!(normalize_repo_input("https://github.com/owner/repo"), Some("owner/repo".to_string()));
    }

    #[test]
    fn normalize_http_url() {
        assert_eq!(normalize_repo_input("http://github.com/owner/repo"), Some("owner/repo".to_string()));
    }

    #[test]
    fn normalize_www_url() {
        assert_eq!(normalize_repo_input("https://www.github.com/owner/repo"), Some("owner/repo".to_string()));
    }

    #[test]
    fn normalize_trailing_slash() {
        assert_eq!(normalize_repo_input("owner/repo/"), Some("owner/repo".to_string()));
        assert_eq!(normalize_repo_input("https://github.com/owner/repo/"), Some("owner/repo".to_string()));
    }

    #[test]
    fn normalize_github_com_prefix_no_protocol() {
        assert_eq!(normalize_repo_input("github.com/owner/repo"), Some("owner/repo".to_string()));
        assert_eq!(normalize_repo_input("github.com/owner/repo/"), Some("owner/repo".to_string()));
    }

    #[test]
    fn normalize_with_whitespace() {
        assert_eq!(normalize_repo_input("  owner/repo  "), Some("owner/repo".to_string()));
    }

    #[test]
    fn normalize_rejects_empty() {
        assert_eq!(normalize_repo_input(""), None);
        assert_eq!(normalize_repo_input("   "), None);
    }

    #[test]
    fn normalize_rejects_no_slash() {
        assert_eq!(normalize_repo_input("just-a-name"), None);
    }

    #[test]
    fn normalize_rejects_too_many_parts() {
        // After stripping github.com/, "owner/repo/extra" has 3 parts
        assert_eq!(normalize_repo_input("owner/repo/extra"), None);
    }
}
