//! Public GitHub catalog reads. Every asset is read from one immutable commit;
//! this module never executes content or writes to either local library.

use opm_store::{PipelineDoc, ToolDefinition, ToolSource, FORMAT_VERSION};
use std::time::Duration;

pub const DEFAULT_REPOSITORY: &str = "ximocm/opm-registry";
pub const DEFAULT_REF: &str = "dev";

#[derive(Clone)]
pub enum Asset {
    Tool(ToolDefinition),
    Pipeline(PipelineDoc),
}

#[derive(Clone)]
pub struct CatalogItem {
    pub path: String,
    pub repository: String,
    pub revision: String,
    pub asset: Asset,
}

impl CatalogItem {
    pub fn name(&self) -> &str {
        match &self.asset {
            Asset::Tool(tool) => &tool.name,
            Asset::Pipeline(doc) => &doc.name,
        }
    }
    pub fn description(&self) -> &str {
        match &self.asset {
            Asset::Tool(tool) => &tool.description,
            Asset::Pipeline(doc) => &doc.description,
        }
    }
    pub fn kind(&self) -> &'static str {
        match self.asset {
            Asset::Tool(_) => "Tool",
            Asset::Pipeline(_) => "Pipeline",
        }
    }
    pub fn stem(&self) -> &str {
        self.path
            .rsplit('/')
            .next()
            .unwrap()
            .rsplit_once('.')
            .unwrap()
            .0
    }
}

pub fn validate_repository(repository: &str) -> Result<(), String> {
    let parts: Vec<_> = repository.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|s| {
            s.is_empty()
                || *s == "."
                || *s == ".."
                || s.len() > 100
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
    {
        return Err("Enter a public GitHub repository as owner/name".to_owned());
    }
    Ok(())
}

fn encode(segment: &str) -> String {
    segment
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn get(agent: &ureq::Agent, url: &str, limit: u64) -> Result<String, String> {
    agent
        .get(url)
        .header(
            "User-Agent",
            concat!("OpenPipelineManager/", env!("CARGO_PKG_VERSION")),
        )
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("Cannot read catalog: {e}"))?
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_string()
        .map_err(|e| format!("Cannot read catalog content: {e}"))
}

/// Only immediate tool JSON and native pipeline files can become installable
/// assets. README files, scripts, symlinks and paths escaping those folders are excluded.
pub fn asset_path(path: &str) -> bool {
    let parts: Vec<_> = path.split('/').collect();
    parts.len() == 2
        && !parts[1].starts_with('.')
        && !parts[1].contains('\\')
        && ((parts[0] == "tools" && parts[1].ends_with(".json"))
            || (parts[0] == "pipelines" && parts[1].ends_with(".pipeline")))
        && parts[1]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

pub fn decode_asset(
    repository: &str,
    revision: &str,
    path: &str,
    text: &str,
) -> Result<CatalogItem, String> {
    if !asset_path(path) {
        return Err("Unsupported catalog path".to_owned());
    }
    let asset = if path.starts_with("tools/") {
        let mut tool = ToolDefinition::from_json(text).map_err(|e| e.to_string())?;
        if path != format!("tools/{}.json", tool.id) {
            return Err("Tool id does not match catalog filename".to_owned());
        }
        tool.source = Some(ToolSource {
            repository: repository.to_owned(),
            revision: revision.to_owned(),
            path: path.to_owned(),
        });
        Asset::Tool(tool)
    } else {
        let doc: PipelineDoc =
            serde_json::from_str(text).map_err(|e| format!("Invalid pipeline: {e}"))?;
        if doc.format_version != FORMAT_VERSION || doc.name.trim().is_empty() {
            return Err("Unsupported pipeline format or empty pipeline name".to_owned());
        }
        Asset::Pipeline(doc)
    };
    Ok(CatalogItem {
        path: path.to_owned(),
        repository: repository.to_owned(),
        revision: revision.to_owned(),
        asset,
    })
}

pub fn fetch(repository: &str, reference: &str) -> Result<Vec<CatalogItem>, String> {
    validate_repository(repository)?;
    if reference.is_empty() || reference.len() > 200 {
        return Err("Enter a branch, tag or commit".to_owned());
    }
    let config = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(Duration::from_secs(20)))
        .max_redirects(3)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let commit: serde_json::Value = serde_json::from_str(&get(
        &agent,
        &format!(
            "https://api.github.com/repos/{repository}/commits/{}",
            encode(reference)
        ),
        1024 * 1024,
    )?)
    .map_err(|e| e.to_string())?;
    let revision = commit["sha"]
        .as_str()
        .filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("Catalog returned an invalid commit")?;
    let tree: serde_json::Value = serde_json::from_str(&get(
        &agent,
        &format!("https://api.github.com/repos/{repository}/git/trees/{revision}?recursive=1"),
        4 * 1024 * 1024,
    )?)
    .map_err(|e| e.to_string())?;
    if tree["truncated"].as_bool() == Some(true) {
        return Err("Catalog is too large to browse completely".to_owned());
    }
    let entries = tree["tree"].as_array().ok_or("Catalog tree is missing")?;
    let paths: Vec<_> = entries
        .iter()
        .filter(|v| v["type"] == "blob" && v["mode"] != "120000")
        .filter_map(|v| v["path"].as_str())
        .filter(|p| asset_path(p))
        .collect();
    if paths.len() > 128 {
        return Err("Catalog contains more than 128 assets".to_owned());
    }
    let mut items = Vec::new();
    for path in paths {
        let text = get(
            &agent,
            &format!("https://raw.githubusercontent.com/{repository}/{revision}/{path}"),
            1024 * 1024,
        )?;
        items.push(
            decode_asset(repository, revision, path, &text).map_err(|e| format!("{path}: {e}"))?,
        );
    }
    items.sort_by_key(|item| {
        (
            matches!(item.asset, Asset::Pipeline(_)),
            item.name().to_lowercase(),
        )
    });
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repository_and_catalog_paths_cannot_escape_github_or_asset_folders() {
        for repository in [
            "https://evil.test/x",
            "../repo",
            "owner/repo/other",
            "owner/a?b",
            "owner/..",
        ] {
            assert!(validate_repository(repository).is_err());
        }
        assert!(validate_repository(DEFAULT_REPOSITORY).is_ok());
        for path in [
            "tools/../x.json",
            "tools/.hidden.json",
            "tools/run.py",
            "pipelines/a.json",
            "tools/a\\b.json",
            "tools/a.json?x",
        ] {
            assert!(!asset_path(path), "{path}");
        }
        assert!(asset_path("tools/cutadapt-single.json"));
        assert!(asset_path("pipelines/metagenomics-paired-qc.pipeline"));
    }

    #[test]
    #[ignore = "requires public GitHub network access"]
    fn live_registry_contains_tools_and_native_pipelines() {
        let items = fetch(DEFAULT_REPOSITORY, DEFAULT_REF).unwrap();
        assert!(items
            .iter()
            .any(|item| matches!(item.asset, Asset::Tool(_))));
        assert!(items
            .iter()
            .any(|item| matches!(item.asset, Asset::Pipeline(_))));
        for item in items {
            assert_eq!(item.revision.len(), 40);
        }
    }
}
