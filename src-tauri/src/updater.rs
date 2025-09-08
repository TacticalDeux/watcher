use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_shell::ShellExt;

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

const GITHUB_REPO: &str = "TacticalDeux/watcher";

pub async fn check_for_updates(app_handle: AppHandle) {
    if let Err(e) = perform_update_check(&app_handle).await {
        eprintln!("Update check failed: {}", e);
    }
}

pub async fn perform_update_check(app_handle: &AppHandle) -> Result<(), anyhow::Error> {
    let current_version = app_handle.package_info().version.to_string();
    let current_version_semver = semver::Version::parse(&current_version)?;

    let client = reqwest::Client::new();
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );

    let response = client
        .get(&url)
        .header("User-Agent", "watcher-updater")
        .send()
        .await?
        .json::<Release>()
        .await?;

    let latest_version_str = response.tag_name.trim_start_matches('v');
    let latest_version_semver = semver::Version::parse(latest_version_str)?;

    if latest_version_semver > current_version_semver {
        if let Some(asset) = response
            .assets
            .iter()
            .find(|a| a.name.ends_with("_x64-setup.exe"))
        {
            app_handle.emit(
                "update-available",
                serde_json::json!({
                    "version": latest_version_str,
                    "url": asset.browser_download_url
                }),
            )?;
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn run_updater(app_handle: AppHandle, url: String) -> Result<(), String> {
    let temp_dir = std::env::temp_dir();
    let file_name = url.split('/').last().unwrap_or("watcher-update.exe");
    let file_path = temp_dir.join(file_name);

    let client = reqwest::Client::new();
    let response = client.get(&url).send().await.map_err(|e| e.to_string())?;
    let content = response.bytes().await.map_err(|e| e.to_string())?;

    tokio::fs::write(&file_path, &content)
        .await
        .map_err(|e| e.to_string())?;

    app_handle
        .shell()
        .command(file_path.to_str().unwrap())
        .spawn()
        .map_err(|e| e.to_string())?;

    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.close();
    }

    Ok(())
}
