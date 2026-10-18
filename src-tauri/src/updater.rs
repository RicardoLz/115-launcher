//! 服务端更新：从 GitHub Release 拉最新服务端 zip，测速选镜像，下载后解压重排到
//! `<root>/server/work/dfo-lan/...`，全程保留 `runtime/storage/` 存档。
//!
//! 与旧 ps1 的契约一致：
//!   zip 根下有 server-bin/ configs/ server/testdata/ server/runtime/ client/
//! 部署映射：
//!   server-bin/*.exe            → server/work/dfo-lan/bin/
//!   configs/                    → server/work/dfo-lan/configs/
//!   server/testdata/            → server/work/dfo-lan/cmd/wireprobe/testdata/
//!   server/runtime/*.bin        → server/work/dfo-lan/runtime/（不碰 storage/）
//!   client/dinput8.dll 等       → 按 config::client_dir() 指向的客户端目录
//!
//! 本地已部署版本号记在 `<root>/.server-update-tag`（纯文本 tag_name）。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config;
use crate::logbus::Bus;
use crate::paths::Root;
use crate::procutil;
use crate::report::curl_exe;

const REPO_API: &str = "https://api.github.com/repos/RicardoLz/115";
const DOWNLOAD_CACHE_DIR: &str = "115us-server-updates";

/// 内置加速镜像池（按测速排序挑最快）。
const MIRRORS: &[&str] = &[
    "https://gh-proxy.com",
    "https://ghfast.top",
    "https://ghproxy.net",
    "https://ghproxy.com",
    "https://ghproxy.cc",
    "https://ghps.cc",
    "https://github.moeyy.xyz",
    "https://gh.ddlc.top",
    "https://gitproxy.click",
    "https://hub.gitmirror.com",
    "https://mirror.ghproxy.com",
    "https://gh.llkk.cc",
    "https://ghproxy.cn",
    "https://ghproxy.net.cn",
    "https://gh.api.99988866.xyz",
    "https://gh.xxpp.co",
    "https://ghp.ci",
];

#[derive(Serialize, Clone, Debug)]
pub struct UpdateCheck {
    /// 本机已部署的 tag；None = 从未部署过。
    pub current_tag: Option<String>,
    pub latest_tag: String,
    pub has_update: bool,
    pub asset_name: String,
    /// 字节数。
    pub asset_size: u64,
    pub download_url: String,
}

fn tag_path(root: &Root) -> PathBuf {
    root.path.join(".server-update-tag")
}

/// 发布包的落点。下载这一侧与界面进度监视认同一条路径，免得两处各拼各的。
pub fn zip_target(asset_name: &str) -> PathBuf {
    std::env::temp_dir().join(DOWNLOAD_CACHE_DIR).join(asset_name)
}

pub fn current_tag(root: &Root) -> Option<String> {
    std::fs::read_to_string(tag_path(root))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn write_current_tag(root: &Root, tag: &str) -> Result<(), String> {
    std::fs::write(tag_path(root), tag)
        .map_err(|err| format!("写入版本标记失败：{err}"))
}

/// 调 GitHub API 查最新 Release。限流/离线返回 Err，由调用方降级。
pub fn check(root: &Root) -> Result<UpdateCheck, String> {
    let url = format!("{REPO_API}/releases/latest");
    let out = curl_get_json(&url, Duration::from_secs(30))?;
    let latest_tag = out
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "GitHub 返回里没有 tag_name".to_string())?
        .to_string();
    let assets = out
        .get("assets")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "GitHub 返回里没有 assets".to_string())?;
    let asset = assets
        .iter()
        .find(|a| a.get("name").and_then(|n| n.as_str()).unwrap_or("").ends_with(".zip"))
        .ok_or_else(|| "最新 Release 没有 zip 资产".to_string())?;
    let asset_name = asset
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "zip 资产缺 name".to_string())?
        .to_string();
    let asset_size = asset
        .get("size")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "zip 资产缺 size".to_string())? as u64;
    let download_url = asset
        .get("browser_download_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "zip 资产缺 browser_download_url".to_string())?
        .to_string();

    let current = current_tag(root);
    let has_update = current.as_deref() != Some(latest_tag.as_str());
    Ok(UpdateCheck {
        current_tag: current,
        latest_tag,
        has_update,
        asset_name,
        asset_size,
        download_url,
    })
}

/// 执行下载 + 解压 + 部署。全程把进度写到 bus。
pub fn apply(root: &Root, info: &UpdateCheck, bus: &Bus) -> Result<String, String> {
    // 服务端在跑时不许覆盖二进制——会把网关打成半死。
    if crate::ports::listening(crate::paths::GATEWAY_PORT) {
        return Err("服务端网关端口 7001 正在监听，请先停止服务端再更新".to_string());
    }

    let cache_dir = std::env::temp_dir().join(DOWNLOAD_CACHE_DIR);
    std::fs::create_dir_all(&cache_dir).map_err(|e| format!("创建下载缓存目录失败：{e}"))?;
    let zip_path = zip_target(&info.asset_name);

    // 已缓存且大小一致则跳过下载。
    let cached_ok = zip_path.is_file()
        && std::fs::metadata(&zip_path)
            .map(|m| m.len() == info.asset_size)
            .unwrap_or(false);
    if cached_ok {
        bus.info(format!("[更新] 使用已缓存的发布包：{}", zip_path.display()));
    } else {
        download_zip(&zip_path, info, root, bus)?;
    }

    let stage = std::env::temp_dir().join(format!(
        "115us-stage-{}",
        uuid_like()
    ));
    std::fs::create_dir_all(&stage).map_err(|e| format!("创建解压目录失败：{e}"))?;
    let result = deploy_zip(&zip_path, &stage, root, bus);
    let _ = std::fs::remove_dir_all(&stage);
    result?;

    write_current_tag(root, &info.latest_tag)?;
    let msg = format!("服务端已更新到 {}", info.latest_tag);
    bus.ok(msg.clone());
    Ok(msg)
}

// ---------------------------------------------------------------- 下载

/// 镜像测速缓存：把上次最快的镜像前缀记下来，下次直接用；
/// 只有上次实际下载速度 < 100 KB/s 才重新测速。
#[derive(Serialize, Deserialize, Default)]
struct MirrorCache {
    /// 上次选中的镜像前缀，如 "https://gh-proxy.com"。空串 = 直连 GitHub。
    prefix: String,
    /// 上次实际下载平均速度 KB/s。
    last_kbps: u64,
}

fn mirror_cache_path(root: &Root) -> PathBuf {
    root.path.join(".mirror-cache.json")
}

fn read_mirror_cache(root: &Root) -> MirrorCache {
    std::fs::read_to_string(mirror_cache_path(root))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_mirror_cache(root: &Root, cache: &MirrorCache) {
    if let Ok(text) = serde_json::to_string(cache) {
        let _ = std::fs::write(mirror_cache_path(root), text);
    }
}

fn clear_mirror_cache(root: &Root) {
    let _ = std::fs::remove_file(mirror_cache_path(root));
}

fn download_zip(zip_path: &Path, info: &UpdateCheck, root: &Root, bus: &Bus) -> Result<(), String> {
    // 候选源：直连 GitHub + 全部镜像。
    let mut candidates: Vec<(String, String)> = Vec::new();
    candidates.push(("直连 GitHub".to_string(), info.download_url.clone()));
    for m in MIRRORS {
        let url = format!("{}/{}", m.trim_end_matches('/'), info.download_url);
        candidates.push((m.to_string(), url));
    }

    // 有缓存的镜像：直接用，不跑全量测速；下完测实际速度，太慢就清缓存下次重测。
    let cached = read_mirror_cache(root);
    if !cached.prefix.is_empty() && cached.last_kbps > 0 {
        let label = format!("缓存镜像 {}", cached.prefix);
        let url = format!("{}/{}", cached.prefix.trim_end_matches('/'), info.download_url);
        bus.info(format!(
            "[更新] 使用上次缓存的镜像：{}（上次 {} KB/s）",
            cached.prefix, cached.last_kbps
        ));
        if let Ok(kbps) = try_download_with_speed(&label, &url, zip_path, info, bus) {
            if kbps >= 100 {
                // 速度够，更新一下缓存里的实际速度。
                write_mirror_cache(root, &MirrorCache { prefix: cached.prefix.clone(), last_kbps: kbps });
                return Ok(());
            }
            bus.warn(format!("[更新] 缓存镜像实际速度只有 {} KB/s（< 100），清除缓存并重新测速", kbps));
            clear_mirror_cache(root);
            let _ = std::fs::remove_file(zip_path);
        } else {
            bus.warn("[更新] 缓存镜像下载失败，清除缓存并重新测速");
            clear_mirror_cache(root);
            let _ = std::fs::remove_file(zip_path);
        }
    }

    // 全量测速。
    bus.info("[更新] 正在对下载源测速（前 512KB）…");
    let mut usable: Vec<(String, String, u64)> = Vec::new();
    for (label, url) in &candidates {
        let rate = probe_speed(url);
        bus.info(format!("  {label:<32} {rate} KB/s"));
        if rate > 0 {
            usable.push((label.clone(), url.clone(), rate));
        }
    }
    if usable.is_empty() {
        return Err("所有下载源均不可达，请检查网络后重试".to_string());
    }
    usable.sort_by(|a, b| b.2.cmp(&a.2));
    let best = &usable[0];
    bus.ok(format!(
        "[更新] 最快源：{}（{} KB/s，预计 {:.1} 分钟）",
        best.0,
        best.2,
        info.asset_size as f64 / 1024.0 / best.2 as f64 / 60.0
    ));

    // 把最快的镜像前缀写缓存（直连 GitHub 不缓存，因为它本来就是 fallback）。
    if !best.0.starts_with("直连") {
        write_mirror_cache(root, &MirrorCache { prefix: best.0.clone(), last_kbps: best.2 });
    }

    for (label, url, _) in &usable {
        if let Ok(kbps) = try_download_with_speed(label, url, zip_path, info, bus) {
            // 更新缓存为本次实际最快的。
            if !label.starts_with("直连") {
                write_mirror_cache(root, &MirrorCache { prefix: label.clone(), last_kbps: kbps });
            }
            return Ok(());
        }
        let _ = std::fs::remove_file(zip_path);
    }
    Err("全部下载源均未成功".to_string())
}

/// 用指定 url 下载完整 zip，返回实际平均速度 KB/s。
fn try_download_with_speed(
    label: &str,
    url: &str,
    zip_path: &Path,
    info: &UpdateCheck,
    bus: &Bus,
) -> Result<u64, String> {
    for attempt in 1..=3 {
        bus.info(format!("[更新] 下载 {label}（第 {attempt}/3 次，断点续传）…"));
        let sw = Instant::now();
        let code = curl_download_into(url, zip_path);
        let elapsed = sw.elapsed().as_secs_f64();
        if code == 0 {
            let size = std::fs::metadata(zip_path).map(|m| m.len()).unwrap_or(0);
            if size == info.asset_size {
                let kbps = if elapsed > 0.0 {
                    (size as f64 / 1024.0 / elapsed) as u64
                } else {
                    0
                };
                bus.ok(format!(
                    "[更新] 下载完成：{:.1} MB（{label}，平均 {} KB/s，耗时 {:.0} 秒）",
                    size as f64 / 1_048_576.0, kbps, elapsed
                ));
                return Ok(kbps);
            }
            bus.warn(format!("[更新] 大小不符：{} / 期望 {}，重试…", size, info.asset_size));
        } else {
            bus.warn(format!("[更新] {label} 下载失败（curl exit {code}）"));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    Err(format!("{label} 三次尝试均失败"))
}

/// 下前 512KB 计时，返回 KB/s（0 = 不可用）。
fn probe_speed(url: &str) -> u64 {
    let seg = std::env::temp_dir().join(format!("115us-spd-{}.bin", uuid_like()));
    let sw = Instant::now();
    let code = curl(&[
        "-s", "-L", "-r", "0-524287", "--max-time", "20", "-o",
        &seg.to_string_lossy(), url,
    ]);
    let elapsed = sw.elapsed().as_secs_f64();
    let size = std::fs::metadata(&seg).map(|m| m.len()).unwrap_or(0);
    let _ = std::fs::remove_file(&seg);
    if code == 0 && size == 524_288 && elapsed > 0.0 {
        (size as f64 / 1024.0 / elapsed) as u64
    } else {
        0
    }
}

fn curl_download_into(url: &str, out: &Path) -> i32 {
    curl(&[
        "-s", "-L", "-C", "-", "--retry", "1", "--connect-timeout", "30",
        "--max-time", "900", "-o", &out.to_string_lossy(), url,
    ])
}

/// 同步跑一次 curl，返回 exit code。所有子进程都隐藏窗口。
fn curl(args: &[&str]) -> i32 {
    let mut cmd = procutil::command(&curl_exe());
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    match cmd.status() {
        Ok(status) => status.code().unwrap_or(-1),
        Err(_) => -1,
    }
}

// ---------------------------------------------------------------- 解压部署

fn deploy_zip(zip_path: &Path, stage: &Path, root: &Root, bus: &Bus) -> Result<(), String> {
    bus.info("[更新] 解压发布包…");
    // Windows 10 1803+ 自带 bsdtar(tar.exe)，直接支持 zip。
    let code = procutil::command_str("tar")
        .args(["-xf", &zip_path.to_string_lossy(), "-C", &stage.to_string_lossy()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("无法启动 tar.exe：{e}"))?;
    if !code.success() {
        return Err(format!("解压 zip 失败（tar exit {:?}）", code.code()));
    }

    let bin_src     = stage.join("server-bin");
    let configs_src = stage.join("configs");
    let testdata_src= stage.join("server").join("testdata");
    let runtime_src = stage.join("server").join("runtime");
    let client_src  = stage.join("client");
    for p in [&bin_src, &configs_src, &testdata_src, &runtime_src] {
        if !p.is_dir() {
            return Err(format!("发布包缺少部件：{}", p.display()));
        }
    }

    let module_bin    = root.path.join("server").join("work").join("dfo-lan").join("bin");
    let probe_dest    = root.path.join("server").join("work").join("dfo_probe_tools");
    let configs_dest  = root.path.join("server").join("work").join("dfo-lan").join("configs");
    let testdata_dest = root.path.join("server").join("work").join("dfo-lan").join("cmd").join("wireprobe").join("testdata");
    let runtime_dest  = root.path.join("server").join("work").join("dfo-lan").join("runtime");
    for d in [&module_bin, &probe_dest, &configs_dest, &testdata_dest, &runtime_dest] {
        std::fs::create_dir_all(d).map_err(|e| format!("创建 {} 失败：{e}", d.display()))?;
    }

    bus.info("[更新] 部署服务端程序…");
    copy_exes(&bin_src, &module_bin)?;
    // pvf-default.json 的 binary 指向 wireprobe-handoff-source.exe，发布包只带 wireprobe-pvf.exe。
    let pvf = module_bin.join("wireprobe-pvf.exe");
    if pvf.is_file() {
        std::fs::copy(&pvf, module_bin.join("wireprobe-handoff-source.exe"))
            .map_err(|e| format!("补齐 wireprobe-handoff-source.exe 失败：{e}"))?;
    }
    let probe_src = module_bin.join("probe.exe");
    if probe_src.is_file() {
        std::fs::copy(&probe_src, probe_dest.join("probe.exe"))
            .map_err(|e| format!("部署 probe.exe 失败：{e}"))?;
    }

    bus.info("[更新] 部署配置与测试数据…");
    copy_dir_recursive(&configs_src, &configs_dest)?;
    copy_dir_recursive(&testdata_src, &testdata_dest)?;

    // runtime：只合并 *.bin，绝不碰 runtime/storage/。
    bus.info("[更新] 部署 runtime 标记文件（保留 storage/ 存档）…");
    if let Ok(entries) = std::fs::read_dir(&runtime_src) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file() && p.extension().map(|x| x == "bin").unwrap_or(false) {
                let _ = std::fs::copy(&p, runtime_dest.join(e.file_name()));
            }
        }
    }

    // 客户端组件：按 GUI 已选的 client_dir 同步。
    if client_src.is_dir() {
        sync_client_components(&client_src, root, bus)?;
    }

    // 运行时标记文件：发布包不带，wireprobe 首次启动前必须存在，否则直接退出。
    write_runtime_markers(&runtime_dest, bus)?;

    // 关键文件校验
    for required in [
        module_bin.join("dfolauncher.exe"),
        module_bin.join("wireprobe-handoff-source.exe"),
        module_bin.join("wireprobe-pvf.exe"),
        probe_dest.join("probe.exe"),
        configs_dest.join("pvf-default.json"),
    ] {
        if !required.is_file() {
            return Err(format!("部署后缺少关键文件：{}", required.display()));
        }
    }
    bus.ok("[更新] 服务端部署完成。");
    Ok(())
}

fn copy_exes(src: &Path, dest: &Path) -> Result<(), String> {
    for e in std::fs::read_dir(src).map_err(|e| format!("读取 {} 失败：{e}", src.display()))? {
        let e = e.map_err(|e| e.to_string())?;
        let p = e.path();
        if p.is_file() && p.extension().map(|x| x == "exe").unwrap_or(false) {
            std::fs::copy(&p, dest.join(e.file_name()))
                .map_err(|e| format!("复制 {} 失败：{e}", p.display()))?;
        }
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("创建 {} 失败：{e}", dest.display()))?;
    for e in std::fs::read_dir(src).map_err(|e| format!("读取 {} 失败：{e}", src.display()))? {
        let e = e.map_err(|e| e.to_string())?;
        let from = e.path();
        let to = dest.join(e.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if from.is_file() {
            std::fs::copy(&from, &to).map_err(|e| format!("复制 {} 失败：{e}", from.display()))?;
        }
    }
    Ok(())
}

/// 把发布包里的 client/ 内容同步到 GUI 已配置的客户端目录。
/// 首次运行还没选客户端时只警告，不报错。
fn sync_client_components(client_src: &Path, root: &Root, bus: &Bus) -> Result<(), String> {
    let client_dir = config::client_dir(root);
    if !client_dir.is_dir() {
        bus.warn(format!(
            "[更新] 客户端目录不存在（{}），跳过 DLL 同步；请先在主界面选择 DFO.exe 所在目录。",
            client_dir.display()
        ));
        return Ok(());
    }
    bus.info(format!("[更新] 同步客户端组件 → {}", client_dir.display()));
    for name in ["dinput8.dll", "ChineseLocalization.dll"] {
        let src = client_src.join(name);
        if src.is_file() {
            std::fs::copy(&src, client_dir.join(name))
                .map_err(|e| format!("复制 {name} 失败：{e}"))?;
        }
    }
    let mods_src = client_src.join(".115us-mods");
    if mods_src.is_dir() {
        let mods_dest = client_dir.join(".115us-mods");
        std::fs::create_dir_all(&mods_dest).map_err(|e| format!("创建 .115us-mods 失败：{e}"))?;
        copy_dir_recursive(&mods_src, &mods_dest)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- 工具

fn curl_get_json(url: &str, timeout: Duration) -> Result<serde_json::Value, String> {
    let out = std::env::temp_dir().join(format!("115us-api-{}.json", uuid_like()));
    let code = procutil::command(&curl_exe())
        .args([
            "-s", "--max-time", &timeout.as_secs().to_string(),
            "-H", "User-Agent: doubao-115us-launcher",
            "-H", "Accept: application/vnd.github+json",
            "-o", &out.to_string_lossy(),
            url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("启动 curl 失败：{e}"))?;
    if !code.success() {
        let _ = std::fs::remove_file(&out);
        return Err("无法连接 GitHub（可能被限流或离线）".to_string());
    }
    let text = std::fs::read_to_string(&out).unwrap_or_default();
    let _ = std::fs::remove_file(&out);
    serde_json::from_str(&text).map_err(|e| format!("GitHub 返回不是 JSON：{e}"))
}

/// 不引 uuid crate：用进程 ID + 当前纳秒拼一个临时名。
fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}", std::process::id(), nanos)
}

/// 游戏成功启动后调用：删掉下载的 zip 缓存，避免占磁盘。
pub fn cleanup_download_cache() {
    let dir = std::env::temp_dir().join(DOWNLOAD_CACHE_DIR);
    if dir.is_dir() {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 写出 wireprobe 启动所需的标记文件（发布包不带，缺了直接退出）。
fn write_runtime_markers(runtime_dest: &Path, bus: &Bus) -> Result<(), String> {
    std::fs::create_dir_all(runtime_dest).map_err(|e| format!("创建 runtime 目录失败：{e}"))?;
    let markers: &[(&str, &[u8])] = &[
        ("characters_ok.bin", &[
            0x00, 0x02, 0x00, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xA6,
            0x00, 0x00, 0x00, 0x00, 0xA4, 0x92, 0x97, 0xD3, 0x26, 0xA4, 0x9C, 0xBA,
            0x94, 0xA5, 0x31, 0xA6, 0xDE, 0x13, 0xE9, 0x46, 0xEE, 0x0E, 0xB6, 0xAD,
            0x38, 0xE6, 0xB7, 0x72, 0x1E, 0xC4, 0xDB, 0xFC, 0x16, 0xAC, 0xD9, 0x34,
        ]),
        ("login_ok.bin", &[
            0x01, 0x01, 0x00, 0x38, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xBF,
            0x00, 0x00, 0x00, 0x00, 0xDA, 0x4A, 0xDB, 0x8E, 0x29, 0xC2, 0xC0, 0xB1,
            0x26, 0x56, 0xE4, 0x3A, 0x5E, 0xFE, 0xA0, 0x5C, 0x72, 0xCA, 0xD0, 0xFB,
            0x48, 0x87, 0x18, 0x56, 0x72, 0xCA, 0xD0, 0xFB, 0x48, 0x87, 0x18, 0x56,
            0x72, 0xCA, 0xD0, 0xFB, 0x48, 0x87, 0x18, 0x56,
        ]),
        ("name_ok.bin", &[
            0x01, 0xAC, 0x02, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xA6,
            0x00, 0x00, 0x00, 0x00, 0x3B, 0x70, 0x32, 0x29, 0xE2, 0xC9, 0x27, 0x68,
            0xAB, 0x22, 0x54, 0xEC, 0xD6, 0x45, 0x79, 0xE0,
        ]),
        ("precheck_ok.bin", &[
            0x01, 0x12, 0x06, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x64,
            0x00, 0x00, 0x00, 0x00, 0xFB, 0x0A, 0xDB, 0x0F, 0x3B, 0xF8, 0x57, 0xB5,
            0x4F, 0x56, 0xDA, 0x4B, 0x6D, 0x93, 0x0A, 0x74,
        ]),
    ];
    let mut written = 0;
    for (name, bytes) in markers {
        let path = runtime_dest.join(name);
        if !path.is_file() {
            std::fs::write(&path, bytes).map_err(|e| format!("写出 {} 失败：{e}", path.display()))?;
            written += 1;
        }
    }
    if written > 0 {
        bus.info(format!("[更新] 已写出 {} 个运行时标记文件", written));
    }
    Ok(())
}
