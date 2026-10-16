//! 启动器自身更新：检查 RicardoLz/115-launcher 的 Release，下载新 exe 替换自己。

use serde::Deserialize;
use std::time::Duration;

use crate::report::curl_exe;
use crate::procutil;

const REPO_API: &str = "https://api.github.com/repos/RicardoLz/115-launcher/releases/latest";
const APP_EXE_NAME: &str = "dfo-launcher.exe";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SelfUpdateCheck {
    pub current_version: String,
    pub latest_version: String,
    pub has_update: bool,
    pub asset_name: String,
    pub asset_size: u64,
    pub download_url: String,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

pub fn current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

fn curl_json(url: &str, timeout: Duration) -> Result<serde_json::Value, String> {
    let output = procutil::command(&curl_exe())
        .arg("-sSL")
        .arg("--max-time")
        .arg(timeout.as_secs().to_string())
        .arg("-H")
        .arg("User-Agent: dfo-launcher-self-update")
        .arg(url)
        .output()
        .map_err(|e| format!("启动 curl 失败：{e}"))?;
    if !output.status.success() {
        return Err("curl 请求失败".to_string());
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("解析 JSON 失败：{e}"))
}

pub fn check() -> Result<SelfUpdateCheck, String> {
    let v = curl_json(REPO_API, Duration::from_secs(15))?;
    let release: GhRelease = serde_json::from_value(v).map_err(|e| format!("解析 Release：{e}"))?;
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == APP_EXE_NAME)
        .ok_or("Release 里没有 dfo-launcher.exe")?;

    let current = current_version();
    // Release tag: v20261016-1732-630b807
    // Cargo 版本: 20261016.1732.0
    // 取 tag 前两段（日期.时间）做比较
    let latest = release.tag_name.trim_start_matches('v').to_string();
    let parts: Vec<&str> = latest.split('-').collect();
    let latest_norm = if parts.len() >= 2 {
        format!("{}.{}.0", parts[0], parts[1])
    } else {
        latest.clone()
    };
    let has_update = latest_norm != current;

    Ok(SelfUpdateCheck {
        current_version: current,
        latest_version: latest,
        has_update,
        asset_name: asset.name.clone(),
        asset_size: asset.size,
        download_url: asset.browser_download_url.clone(),
    })
}

pub fn apply_self_update<F: Fn(&str)>(info: &SelfUpdateCheck, log: F) -> Result<String, String> {
    let exe_path = std::env::current_exe().map_err(|e| format!("找不到当前 exe：{e}"))?;
    let root_dir = exe_path.parent().ok_or("exe 无父目录")?;

    let tmp_dir = std::env::temp_dir().join("115us-self-update");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("创建临时目录失败：{e}"))?;
    let new_exe = tmp_dir.join(APP_EXE_NAME);
    if new_exe.is_file() {
        let _ = std::fs::remove_file(&new_exe);
    }

    // 所有镜像源
    let all_mirrors: &[&str] = &[
        "",
        "https://gh-proxy.com/",
        "https://ghfast.top/",
        "https://ghproxy.net/",
        "https://ghproxy.cc/",
        "https://ghps.cc/",
        "https://github.moeyy.xyz/",
        "https://gh.llkk.cc/",
        "https://ghproxy.cn/",
        "https://ghp.ci/",
    ];

    // 读镜像缓存，优先用上次测出来最快的源
    let mut ordered: Vec<String> = Vec::new();
    let cache_path = root_dir.join(".mirror-cache.json");
    if let Ok(text) = std::fs::read_to_string(&cache_path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(prefix) = v.get("prefix").and_then(|p| p.as_str()) {
                if !prefix.is_empty() {
                    ordered.push(prefix.to_string());
                    log(&format!("[自更新] 使用缓存的最快源：{}", prefix));
                }
            }
        }
    }
    // 剩余的源按顺序追加
    for m in all_mirrors {
        if !ordered.contains(&m.to_string()) {
            ordered.push(m.to_string());
        }
    }

    let mut last_code = -1;
    for (i, prefix) in ordered.iter().enumerate() {
        let url = format!("{}{}", prefix, info.download_url);
        let label = if prefix.is_empty() { "直连 GitHub" } else { prefix.as_str() };
        log(&format!("[自更新] 尝试源 {}：{}", i + 1, label));
        // --speed-limit 1024: 速度低于 1KB/s 持续 --speed-time 10 秒就中止，切下一个源
        let code = procutil::command(&curl_exe())
            .arg("-sSL")
            .arg("-o")
            .arg(&new_exe)
            .arg("--max-time")
            .arg("120")
            .arg("--speed-limit")
            .arg("1024")
            .arg("--speed-time")
            .arg("10")
            .arg("-H")
            .arg("User-Agent: dfo-launcher-self-update")
            .arg(&url)
            .status()
            .map_err(|e| format!("启动 curl 失败：{e}"))?;
        last_code = code.code().unwrap_or(-1);
        if code.success() && new_exe.is_file() && new_exe.metadata().map(|m| m.len()).unwrap_or(0) > 1_000_000 {
            let size = new_exe.metadata().map(|m| m.len() / 1024 / 1024).unwrap_or(0);
            log(&format!("[自更新] 下载完成（{} MB）", size));
            // 成功后更新镜像缓存
            let cache = serde_json::json!({ "prefix": prefix, "last_kbps": 0 });
            let _ = std::fs::write(&cache_path, cache.to_string());
            break;
        }
        log(&format!("[自更新] 源 {} 失败（curl exit {}）", i + 1, last_code));
        let _ = std::fs::remove_file(&new_exe);
    }

    if !new_exe.is_file() || new_exe.metadata().map(|m| m.len()).unwrap_or(0) < 1_000_000 {
        return Err(format!("所有源都失败（最后 curl exit {last_code}）"));
    }

    let target_exe = exe_path.to_string_lossy().replace('/', "\\");
    let new_exe_str = new_exe.to_string_lossy().replace('/', "\\");
    let bat_path = tmp_dir.join("update.bat");

    let bat = format!(
        "@echo off\r\n\
         :waitloop\r\n\
         timeout /t 1 /nobreak >nul\r\n\
         copy /y \"{new_exe}\" \"{target_exe}\" >nul 2>&1\r\n\
         if errorlevel 1 goto waitloop\r\n\
         start \"\" \"{target_exe}\"\r\n\
         del \"%~f0\"\r\n",
        new_exe = new_exe_str,
        target_exe = target_exe,
    );
    std::fs::write(&bat_path, bat).map_err(|e| format!("写更新脚本失败：{e}"))?;

    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        std::process::Command::new("cmd")
            .args(["/C", bat_path.to_string_lossy().as_ref()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(crate::procutil::CREATE_NO_WINDOW | DETACHED_PROCESS)
            .spawn()
            .map_err(|e| format!("启动更新脚本失败：{e}"))?;
    }

    Ok(format!("已下载 v{}，正在替换并重启…", info.latest_version))
}

pub fn cleanup_self_update_cache() {
    let dir = std::env::temp_dir().join("115us-self-update");
    if dir.is_dir() {
        let _ = std::fs::remove_dir_all(&dir);
    }
}
