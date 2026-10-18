//! 背景接口：主页的壁纸与动态壁纸（视频）由玩家自己选。
//!
//! 素材留在玩家选的原始位置，启动器不搬也不复制，只做两件事：把路径记进
//! `launcher.settings.json` 的 `ui.background`，以及在运行时把**当前选中的那一个文件**
//! 放进 asset 协议白名单。白名单默认是空的，往里的每一条都是玩家自己用文件框点出来的
//! 单个文件（从不放目录），重启即清零，所以不会因为一个 URL 就读到盘上别的文件。

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::config;
use crate::paths::Root;

/// 位图四种加 gif：静态图当壁纸，动图 WebView 自己会动。
pub const IMAGE_EXTS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];
/// 视频只收 WebView2 原生认的容器/编码。`.mkv`、`.avi` 不收 —— 选了也是黑屏，不如不选。
pub const VIDEO_EXTS: [&str; 3] = ["mp4", "webm", "mov"];

#[derive(Serialize, Clone, Debug)]
pub struct BackgroundState {
    /// 玩家选中的绝对路径；没选过是空串。
    pub path: String,
    /// `"none"` | `"image"` | `"video"`：界面按它决定不画、画 `<img>` 还是画 `<video>`。
    pub kind: String,
    /// 路径记着但文件已经不在了（被搬走或删掉）：主页退回纯色，设置页照实说。
    pub exists: bool,
    pub note: String,
    /// 壁纸主题色 `#rrggbb`；空串 = 取不到（没背景、视频、坏图、整张灰阶）。
    /// 界面拿到空串就退回自己的默认色，绝不因为取色失败少画一块。
    pub accent: String,
}

/// 主题色只定「这屏壁纸给人什么色调」，96px 缩略图足够，再大只是白花时间。
const ACCENT_MAX_SIDE: u32 = 96;
/// 超过这个体积的图不取色：解码一张超大图的时间不该摊到界面首屏上。
const ACCENT_MAX_BYTES: u64 = 40 * 1024 * 1024;

/// 饱和度（HSL 口径，0..1）。灰阶为 0，越纯越大。
fn saturation(r: f64, g: f64, b: f64) -> f64 {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let light = (max + min) / 2.0;
    let denom = 255.0 - (2.0 * light - 255.0).abs();
    if denom <= 1.0 {
        0.0
    } else {
        (max - min) / denom
    }
}

/// 从壁纸里挑一个主题色：按 4bit/通道分桶，桶分 = 出现次数 × 饱和度^1.2，取最高分桶的均值色。
/// 接近黑、接近白、以及近似灰阶的像素先筛掉 —— 那种「主题色」只会把界面弄脏。
fn vibrant(pixels: &image::RgbImage) -> Option<(u8, u8, u8)> {
    let mut buckets: std::collections::BTreeMap<(u8, u8, u8), (u64, u64, u64, u64)> =
        std::collections::BTreeMap::new();
    for px in pixels.pixels() {
        let (r, g, b) = (f64::from(px[0]), f64::from(px[1]), f64::from(px[2]));
        let light = (r.max(g).max(b) + r.min(g).min(b)) / 2.0;
        if light < 46.0 || light > 219.0 {
            continue;
        }
        if saturation(r, g, b) < 0.18 {
            continue;
        }
        let entry = buckets
            .entry((px[0] >> 4, px[1] >> 4, px[2] >> 4))
            .or_insert((0, 0, 0, 0));
        entry.0 += 1;
        entry.1 += u64::from(px[0]);
        entry.2 += u64::from(px[1]);
        entry.3 += u64::from(px[2]);
    }
    let mut best: Option<(f64, (u8, u8, u8))> = None;
    for (_, (count, rs, gs, bs)) in buckets {
        if count == 0 {
            continue;
        }
        let avg = ((rs / count) as u8, (gs / count) as u8, (bs / count) as u8);
        let score = count as f64 * saturation(f64::from(avg.0), f64::from(avg.1), f64::from(avg.2)).powf(1.2);
        if best.map(|(prev, _)| score > prev).unwrap_or(true) {
            best = Some((score, avg));
        }
    }
    best.map(|(_, color)| color)
}

/// 取当前壁纸的主题色。任何一步不成都返回空串：取色是锦上添花，不许把背景层带崩。
pub fn accent_of(path: &Path, kind: &str, exists: bool) -> String {
    if !exists || kind != "image" || path.as_os_str().is_empty() {
        return String::new();
    }
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX);
    if len > ACCENT_MAX_BYTES {
        return String::new();
    }
    let Ok(reader) = image::ImageReader::open(path).and_then(|r| r.with_guessed_format()) else {
        return String::new();
    };
    let Ok(decoded) = reader.decode() else {
        return String::new();
    };
    let small = image::imageops::thumbnail(&decoded.to_rgb8(), ACCENT_MAX_SIDE, ACCENT_MAX_SIDE);
    match vibrant(&small) {
        Some((r, g, b)) => format!("#{r:02x}{g:02x}{b:02x}"),
        None => String::new(),
    }
}

/// 按扩展名定性；不认的一律 `None`，不收进设置。
pub fn kind_of(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if IMAGE_EXTS.contains(&ext.as_str()) {
        Some("image")
    } else if VIDEO_EXTS.contains(&ext.as_str()) {
        Some("video")
    } else {
        None
    }
}

fn saved(root: &Root) -> PathBuf {
    let raw = config::settings(root).background;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        PathBuf::new()
    } else {
        PathBuf::from(trimmed)
    }
}

fn note_for(path: &Path, kind: &str, exists: bool) -> String {
    // 从没选过 ≠ 选过又不在了：这两种必须分开说，否则第一次打开设置页就是一条无端的故障提示。
    if path.as_os_str().is_empty() {
        return "没用背景（主页走纯色主题）".to_string();
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if !exists {
        return format!("记着的素材不在了：{}（主页先退回纯色背景）", path.display());
    }
    match kind {
        "image" => format!("壁纸：{name}"),
        "video" => format!("动态壁纸：{name}（主页循环播放，不带声音）"),
        _ => "没用背景（主页走纯色主题）".to_string(),
    }
}

pub fn state(root: &Root) -> BackgroundState {
    let path = saved(root);
    if path.as_os_str().is_empty() {
        return BackgroundState {
            path: String::new(),
            kind: "none".to_string(),
            exists: false,
            note: note_for(&path, "none", false),
            accent: String::new(),
        };
    }
    let exists = path.is_file();
    let kind = kind_of(&path).unwrap_or("none").to_string();
    BackgroundState {
        path: path.display().to_string(),
        note: note_for(&path, &kind, exists),
        accent: accent_of(&path, &kind, exists),
        kind,
        exists,
    }
}

/// 记下新素材：先验（在场、是文件、扩展名收得下），验不过一个字节都不写。
pub fn install(root: &Root, candidate: &Path) -> Result<BackgroundState, String> {
    if !candidate.is_file() {
        return Err(format!("{} 不是一个文件", candidate.display()));
    }
    let Some(kind) = kind_of(candidate) else {
        let ext = candidate
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_else(|| "没有扩展名".to_string());
        return Err(format!(
            "背景素材只收 {}，收到的是 .{ext}（WebView 播不了的容器不收，选了也只是黑屏）",
            IMAGE_EXTS
                .iter()
                .chain(VIDEO_EXTS.iter())
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join(" / ")
        ));
    };
    let path = crate::paths::squash(candidate);
    let raw = path.display().to_string();
    config::set_section(root, "ui", |ui| {
        ui.insert(
            "background".to_string(),
            serde_json::Value::String(raw.clone()),
        );
    })?;
    let after = state(root);
    // 记下来的必须就是刚验过的那一条：类型对不上等于写歪了，不能装作成功。
    if after.kind != kind {
        return Err(format!(
            "背景设置写进去的是 {}，与选中的 {kind} 不符",
            after.kind
        ));
    }
    Ok(after)
}

/// 清空背景选择，只删设置里那一行，素材文件本身绝不碰。
pub fn clear(root: &Root) -> Result<BackgroundState, String> {
    config::set_section(root, "ui", |ui| {
        ui.insert("background".to_string(), serde_json::Value::String(String::new()));
    })?;
    Ok(state(root))
}

/// 把当前记着的素材放进 asset 白名单。启动时与每次改动后都调它。
///
/// 这里只放不收。tauri 的 `forbid_file` 不可撤销：`is_allowed` 先查禁用表，而 `allow_file`
/// 移不掉里面的条目，所以换素材时顺手禁掉上一条，等于把玩家之后每次「选回刚才那个文件」
/// 都变成永久 403（2026-10-17 实机就是这个：换个素材再换回来，背景再也播不出来）。
pub fn apply_scope(app: &AppHandle, root: &Root) -> Result<(), String> {
    let current = saved(root);
    if current.as_os_str().is_empty() || !current.is_file() {
        return Ok(());
    }
    let scope = app.asset_protocol_scope();
    scope
        .allow_file(&current)
        .map_err(|err| format!("背景文件放行失败，主页会退回纯色背景：{err}"))?;
    // 调过放行不等于放行生效，直接问一次 scope：不生效时界面只会看到一句 403，
    // 没有这一行的话日志里连个方向都没有。
    if !scope.is_allowed(&current) {
        return Err(format!(
            "背景文件放行了但还是读不到，主页会退回纯色背景：{}",
            current.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> (Root, PathBuf) {
        let dir = std::env::temp_dir().join(format!("dfo-bg-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("临时目录");
        (Root { path: dir.clone(), source: "test" }, dir)
    }

    /// 收哪些扩展名是「WebView 能不能播」这件事的边界，不收的必须在门禁就挡掉。
    #[test]
    fn kind_of_accepts_only_webview_playable_extensions() {
        for name in ["a.png", "a.jpg", "a.JPEG", "a.webp", "a.gif"] {
            assert_eq!(kind_of(Path::new(name)), Some("image"), "{name}");
        }
        for name in ["a.mp4", "a.webm", "a.mov"] {
            assert_eq!(kind_of(Path::new(name)), Some("video"), "{name}");
        }
        for name in ["a.mkv", "a.avi", "a.txt", "noext"] {
            assert_eq!(kind_of(Path::new(name)), None, "{name}");
        }
    }

    /// 选不存在的路径或收不下的容器：一个字节都不许写进设置。
    #[test]
    fn install_rejects_missing_files_and_unplayable_containers() {
        let (root, dir) = temp_root("reject");
        let ghost = dir.join("ghost.png");
        let err = install(&root, &ghost).unwrap_err();
        assert!(err.contains("不是一个文件"), "{err}");
        assert_eq!(state(&root).kind, "none");

        let movie = dir.join("clip.mkv");
        std::fs::write(&movie, b"x").expect("造 mkv");
        let err = install(&root, &movie).unwrap_err();
        assert!(err.contains("只收"), "{err}");
        assert_eq!(state(&root).kind, "none", "验不过时不能已经把路径记下了");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 记下了就得能被读回来，类型按扩展名定性。
    #[test]
    fn install_records_the_path_and_state_reports_the_kind() {
        let (root, dir) = temp_root("ok");
        let wall = dir.join("wall.PNG");
        std::fs::write(&wall, b"x").expect("造壁纸");
        let after = install(&root, &wall).expect("该收下的");
        assert_eq!(after.kind, "image");
        assert!(after.exists);
        assert!(after.note.contains("壁纸：wall.PNG"), "{}", after.note);
        assert_eq!(state(&root).path, wall.display().to_string());

        let clip = dir.join("loop.mp4");
        std::fs::write(&clip, b"x").expect("造视频");
        let after = install(&root, &clip).expect("视频也该收下");
        assert_eq!(after.kind, "video");
        assert!(after.note.contains("不带声音"), "{}", after.note);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 玩家把素材搬走之后：照实报「不在了」，好让界面退回纯色而不是渲染一个坏 URL。
    #[test]
    fn state_says_so_when_the_recorded_file_was_moved_away() {
        let (root, dir) = temp_root("moved");
        let wall = dir.join("wall.png");
        std::fs::write(&wall, b"x").expect("造壁纸");
        install(&root, &wall).expect("先记下");
        std::fs::remove_file(&wall).expect("搬走");
        let view = state(&root);
        assert_eq!(view.kind, "image", "类型仍按扩展名报，界面才知道该画什么");
        assert!(!view.exists);
        assert!(view.note.contains("不在了"), "{}", view.note);

        let cleared = clear(&root).expect("清除该成行");
        assert_eq!(cleared.kind, "none");
        assert!(cleared.path.is_empty());
        assert!(cleared.note.contains("没用背景"), "{}", cleared.note);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 从没选过背景 = 正常态，措辞不能和「选过又不在了」那条故障混在一起。
    #[test]
    fn state_names_a_never_picked_background_as_unused() {
        let (root, dir) = temp_root("fresh");
        let view = state(&root);
        assert_eq!(view.kind, "none");
        assert!(view.path.is_empty());
        assert!(!view.exists);
        assert!(view.note.contains("没用背景"), "{}", view.note);
        assert!(!view.note.contains("不在了"), "{}", view.note);
        assert!(view.accent.is_empty(), "{}", view.accent);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn write_wall(dir: &Path, name: &str, paint: impl Fn(u32, u32) -> [u8; 3]) -> PathBuf {
        let mut img = image::RgbImage::new(240, 150);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = image::Rgb(paint(x, y));
        }
        let path = dir.join(name);
        img.save(&path).expect("造壁纸");
        path
    }

    fn hex_rgb(raw: &str) -> (u8, u8, u8) {
        let b = &raw[1..];
        (
            u8::from_str_radix(&b[0..2], 16).unwrap(),
            u8::from_str_radix(&b[2..4], 16).unwrap(),
            u8::from_str_radix(&b[4..6], 16).unwrap(),
        )
    }

    fn near(a: (u8, u8, u8), b: (u8, u8, u8), tol: i16) -> bool {
        let d = |x: u8, y: u8| (i16::from(x) - i16::from(y)).abs();
        d(a.0, b.0) <= tol && d(a.1, b.1) <= tol && d(a.2, b.2) <= tol
    }

    /// 主题色要的是「气质」不是「多数」：大片灰底里那一小块高饱和色必须被挑出来，
    /// 否则壁纸是暗色调时界面永远拿不到一个能用的边框色。
    #[test]
    fn accent_picks_the_saturated_region_not_the_majority() {
        let (_root, dir) = temp_root("accent");
        // 80% 中性灰 + 20% 品红：按计数会选灰，按饱和度加权要选品红。
        let wall = write_wall(&dir, "magenta.png", |x, _| {
            if x < 192 {
                [122, 126, 130]
            } else {
                [214, 32, 158]
            }
        });
        let raw = accent_of(&wall, "image", true);
        assert!(!raw.is_empty(), "灰底里的品红该被挑出来");
        assert!(
            near(hex_rgb(&raw), (214, 32, 158), 26),
            "取到的色偏离品红太远：{raw}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 整张灰阶（黑白壁纸）取不到主题色：必须回空串，让界面退回默认色，而不是报个错或硬给一个灰。
    #[test]
    fn accent_is_empty_for_a_desaturated_wall() {
        let (_root, dir) = temp_root("gray");
        let wall = write_wall(&dir, "gray.png", |_, y| {
            let v = (y as u8) / 8 * 8;
            [v, v, v]
        });
        assert_eq!(accent_of(&wall, "image", true), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 视频、不存在的文件、以及「记着但已被搬走」的图：一律空串，且不许 panic。
    #[test]
    fn accent_is_empty_for_video_and_missing_files() {
        let (_root, dir) = temp_root("noaccent");
        assert_eq!(accent_of(Path::new(""), "none", false), "");
        assert_eq!(accent_of(&dir.join("ghost.png"), "image", false), "");
        let clip = dir.join("loop.mp4");
        std::fs::write(&clip, b"x").expect("造视频");
        assert_eq!(accent_of(&clip, "video", true), "");
        // 扩展名是图、内容不是图（玩家手改后缀）：解不开也只回空串。
        let fake = dir.join("fake.png");
        std::fs::write(&fake, b"not really a png").expect("造坏图");
        assert_eq!(accent_of(&fake, "image", true), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 选完壁纸后 `state()` 要把主题色一起带出去，界面只读这一个结构。
    #[test]
    fn state_carries_the_accent_of_the_picked_wallpaper() {
        let (root, dir) = temp_root("state-accent");
        let wall = write_wall(&dir, "teal.png", |_, _| [12, 140, 146]);
        install(&root, &wall).expect("该收下的");
        let view = state(&root);
        assert_eq!(view.kind, "image");
        assert!(near(hex_rgb(&view.accent), (12, 140, 146), 26), "{}", view.accent);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
