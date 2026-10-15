use serde::{Deserialize, Serialize};

const GITHUB_REPO_OWNER: &str = "BuildWithAIs";
const GITHUB_REPO_NAME: &str = "voicekey";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub has_update: bool,
    pub latest_version: String,
    pub release_url: String,
    pub release_notes: String,
    pub error: Option<String>,
}

fn default_release_url() -> String {
    format!("https://github.com/{GITHUB_REPO_OWNER}/{GITHUB_REPO_NAME}/releases")
}

fn sanitize_release_url(url: Option<&str>) -> String {
    let Some(raw) = url else {
        return default_release_url();
    };
    let Ok(parsed) = url::Url::parse(raw) else {
        return default_release_url();
    };
    let allowed = format!("/{GITHUB_REPO_OWNER}/{GITHUB_REPO_NAME}/releases");
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("github.com")
        || !parsed.path().starts_with(&allowed)
    {
        return default_release_url();
    }
    parsed.to_string()
}

pub async fn check_for_updates() -> UpdateInfo {
    let url = format!(
        "https://api.github.com/repos/{GITHUB_REPO_OWNER}/{GITHUB_REPO_NAME}/releases/latest"
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .expect("client");

    match client
        .get(&url)
        .header("Accept", "application/vnd.github.v3+json")
        .header("User-Agent", "Voice-Key-App")
        .send()
        .await
    {
        Ok(response) => {
            let value: serde_json::Value = response.json().await.unwrap_or_default();
            let tag = value
                .get("tag_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let html = value.get("html_url").and_then(|v| v.as_str());
            let notes = value
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let current = env!("CARGO_PKG_VERSION").to_string();
            let has_update = version_gt(&tag, &current);
            UpdateInfo {
                has_update,
                latest_version: tag.trim_start_matches('v').to_string(),
                release_url: sanitize_release_url(html),
                release_notes: notes,
                error: None,
            }
        }
        Err(e) => UpdateInfo {
            has_update: false,
            latest_version: String::new(),
            release_url: default_release_url(),
            release_notes: String::new(),
            error: Some(e.to_string()),
        },
    }
}

fn version_gt(latest: &str, current: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        v.trim_start_matches('v')
            .split(['.', '-'])
            .filter_map(|p| p.parse::<u64>().ok())
            .collect()
    }
    let l = parts(latest);
    let c = parts(current);
    for i in 0..3 {
        let lv = l.get(i).copied().unwrap_or(0);
        let cv = c.get(i).copied().unwrap_or(0);
        if lv != cv {
            return lv > cv;
        }
    }
    false
}

pub fn sanitize_external_url(url: &str) -> Result<String, String> {
    let parsed = url::Url::parse(url).map_err(|e| e.to_string())?;
    let allowed = format!("/{GITHUB_REPO_OWNER}/{GITHUB_REPO_NAME}/releases");
    if parsed.scheme() == "https"
        && parsed.host_str() == Some("github.com")
        && parsed.path().starts_with(&allowed)
    {
        Ok(parsed.to_string())
    } else {
        Err("URL is not an allowed GitHub release link".into())
    }
}
