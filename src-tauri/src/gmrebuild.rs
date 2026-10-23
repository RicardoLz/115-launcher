//! GM 产物的同源校验与一键重建。
//!
//! 契约没变：启动器运行期**仍然不读 PVF**。`gmbuild` 的 `build` feature（PVF 内核、
//! flate2/rsa 那一批）不会因为这一层链进启动器 —— 重建走的是「叫外部 `gmbuild.exe`」这条路，
//! 与 `wireprobe-pvf.exe` 同一个先例：子进程干活，启动器只负责调度、校验与落盘。
//!
//! 为什么需要这一层：换 PVF 之后旧产物不会自己失效（运行期只读 JSON），于是会出现
//! 「查询照开、发放照写、养成和任务却被指纹门整条拒」这种半边能用半边不能用的状态。
//! 这里把「哪几件同源、差在哪、怎么一次补齐」摆到界面上，省得靠人记五条 CLI。
//!
//! 两条硬判据（判据本身在各自的写链路里，这里只是**提前**把它们摊开）：
//!
//! 1. `items.index.json` / `gm-growth.json` / `gm-quests.json` 的归档校验必须相等
//!    —— 它们由同一次 `gmbuild` 构建从同一份 PVF 烘出来；
//! 2. `inventory.current37.json` 的 `source` 必须等于 `equipment.current37.json` 的
//!    `source.checksum`（`gmgrant::EquipCatalog::from_json` 那条同源闸）。
//!
//! 这两组现在一次点齐：`build-equipment` 直读 PVF 全量枚举出装备目录，`refresh-bag`
//! 只把背包规则那份的代际戳换掉、策略字段逐键不动。唯一不进重建的是
//! `experience.compat90.json`——它整份文件里没有来源戳（只有 `reference_sha256`
//! 这类人工口径），换 PVF 不会让它失效，重建也就无从校验它。
//!
//! 进度：子命令的输出是流式收的（`procutil::run_with_lines`），`进度 n/m` 那种行
//! 换算成步内百分比，再按各步实测秒数摊进总进度，走 `bus.progress` 单独一条事件
//! 发给界面——不挤日志环形，也不落盘。

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

use gmbuild::gmindex;

use crate::config;
use crate::logbus::Bus;
use crate::paths::Root;
use crate::procutil;

/// 重建的一步。
///
/// `weight` 是这一步的实测秒数（2026-10-22 那一次整轮 90s 起的头），只用来把总进度条
/// 切成七段：某步跑得快慢不影响任何判定，进度条提前/滞后走完都比「卡在 0% 九十分钟」好读。
#[derive(Clone, Copy)]
struct StepSpec {
    cmd: &'static str,
    outputs: &'static [&'static str],
    timeout: u64,
    /// 需要把「旧产物目录」当第 4 个参数传进去（`refresh-bag` 从那里读旧背包规则）。
    needs_src: bool,
    weight: u64,
}

/// 重建要跑的子命令、每步该产出的文件、超时（秒）。顺序照依赖摆：
/// `build-slots` 会读同名旧表来保留历史条目，所以它得在物品库之后；
/// `refresh-bag` 换的是装备目录那一份戳，排在 `build-equipment` 之后，界面上一眼能看出先后。
fn build_steps() -> Vec<StepSpec> {
    vec![
        StepSpec { cmd: "build-names", outputs: &["names.client.json"], timeout: 300, needs_src: false, weight: 14 },
        StepSpec { cmd: "build-items", outputs: &["items.index.json"], timeout: 3600, needs_src: false, weight: 23 },
        StepSpec {
            cmd: "build-slots",
            outputs: &["equipment.slots.json", "equipment.jobs.json", "item.rarity.json"],
            timeout: 3600,
            needs_src: false,
            weight: 28,
        },
        StepSpec { cmd: "build-growth", outputs: &["gm-growth.json"], timeout: 1200, needs_src: false, weight: 13 },
        StepSpec { cmd: "build-quests", outputs: &["gm-quests.json"], timeout: 1800, needs_src: false, weight: 13 },
        StepSpec { cmd: "build-equipment", outputs: &["equipment.current37.json"], timeout: 3600, needs_src: false, weight: 20 },
        StepSpec { cmd: "refresh-bag", outputs: &["inventory.current37.json"], timeout: 120, needs_src: true, weight: 8 },
    ]
}

/// 产物清单：文件、给界面看的中文说明。
const ARTIFACTS: [(&str, &str); 10] = [
    ("items.index.json", "物品库 · gmbuild"),
    ("gm-growth.json", "成长表 · gmbuild"),
    ("gm-quests.json", "任务目录 · gmbuild"),
    ("inventory.current37.json", "背包规则 · gmbuild（只换戳）"),
    ("equipment.current37.json", "装备目录 · gmbuild"),
    ("experience.compat90.json", "经验规则 · 人工策略，不重建"),
    ("names.client.json", "名字表 · gmbuild"),
    ("equipment.slots.json", "部位表 · gmbuild"),
    ("equipment.jobs.json", "职业限制表 · gmbuild"),
    ("item.rarity.json", "品级表 · gmbuild"),
];


// ---------------------------------------------------------------------------
// 视图
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct ArtifactView {
    pub file: String,
    pub role: String,
    pub present: bool,
    /// 产物里的代际指纹：PVF 归档校验或服务端导出的来源校验。
    /// 旁表（名字/部位/职业/品级）与经验规则没有这一项，是空串。
    pub checksum: String,
    /// 产物里记的来源名（`Script.pvf` / `runtime/pvf_source/Script.inner.pvf`）。
    pub path: String,
    pub built_at: String,
    /// 产物记录的那份来源文件多大（字节）。0 = 没记。
    pub size: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct IssueView {
    /// `ok` / `warn` / `err`。err = 相关写链路会整条拒绝；warn = 能用但代际漂移。
    pub tone: &'static str,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct PvfView {
    pub path: String,
    pub size: i64,
    pub modified: String,
    /// 与产物记录的大小是否一致。只做「像不像换过」的粗判，不是哈希。
    pub size_matches: bool,
    pub newer_than_build: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct AuditView {
    pub dir: String,
    /// 这次比较的是哪一份客户端 PVF（可能不存在）。
    pub pvf_path: String,
    pub pvf: Option<PvfView>,
    pub artifacts: Vec<ArtifactView>,
    pub issues: Vec<IssueView>,
    /// 三件 gmbuild 产物是否同一次构建。
    pub consistent: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct StepView {
    pub step: String,
    pub ok: bool,
    pub seconds: f64,
    pub detail: String,
}

/// 重建进度：只走 `progress` 事件，不进日志环形、不落盘。
///
/// `percent` 是整轮的总进度（0~100），不是单步的：界面只需要一根条，
/// 单步信息由 `step` / `step_index` 表达，细账在跑完后照 `RebuildView.steps` 摊开。
#[derive(Clone, Debug, Serialize)]
pub struct ProgressView {
    pub step: String,
    /// 1 起算；第 N 步 = `step_total`。
    pub step_index: usize,
    pub step_total: usize,
    pub percent: u32,
    /// 界面直接显示的那句话。
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RebuildView {
    pub dir: String,
    pub pvf: String,
    /// 新产物共用的归档校验（GM 三件与装备目录、背包规则戳现在同一个值）。
    pub checksum: String,
    pub steps: Vec<StepView>,
    /// 被替换的旧产物备份目录（空串 = 原来没有可备份的）。
    pub backup: String,
    pub elapsed: f64,
    pub audit: AuditView,
}

// ---------------------------------------------------------------------------
// 产物里那几行元数据：不整份解析
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct Snapshot {
    checksum: String,
    path: String,
    built_at: String,
    size: i64,
}

fn is_hex64(v: &str) -> bool {
    v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit())
}

fn short(v: &str) -> String {
    if v.is_empty() {
        "（空）".to_string()
    } else {
        format!("{}…", &v[..v.len().min(12)])
    }
}

/// 从 `at` 往后取第一段字符串值：跳过空白与冒号，读到收尾引号。
fn string_at(text: &str, at: usize) -> Option<String> {
    let bytes = text.as_bytes();
    let mut i = at;
    while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b':') {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'"' {
        return None;
    }
    i += 1;
    let start = i;
    while i < bytes.len() && bytes[i] != b'"' {
        if bytes[i] == b'\\' {
            return None;
        }
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    Some(text[start..i].to_string())
}

/// `key` 之后第一段字符串值。
fn string_after(text: &str, key: &str) -> Option<String> {
    let at = text.find(key)? + key.len();
    string_at(text, at)
}

/// `key` 之后第一段整数。
fn number_after(text: &str, key: &str) -> Option<i64> {
    let at = text.find(key)? + key.len();
    let bytes = text.as_bytes();
    let mut i = at;
    while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b':') {
        i += 1;
    }
    let start = i;
    if i < bytes.len() && bytes[i] == b'-' {
        i += 1;
    }
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == start {
        return None;
    }
    text[start..i].parse().ok()
}

/// 扫 `key` 之后所有「恰好 64 位十六进制」的字符串，取最后一个。
///
/// 不做完整反序列化：`items.index.json` 是一行几十 MB 的大 JSON，`gm-growth.json` 的
/// `source` 后面还挂着整棵成长树，为四个字段把整份读进内存不值得。键在这些产物里各自
/// 只出现一次；物品名里出现 `"source"` 这类字样时，靠「必须是 64 位 hex」筛掉。
fn hex64_after(text: &str, key: &str) -> Option<String> {
    let mut from = 0;
    let mut found = None;
    while let Some(pos) = text[from..].find(key) {
        let start = from + pos;
        let after = start + key.len();
        if let Some(v) = string_at(text, after) {
            if is_hex64(&v) {
                found = Some(v);
            }
        }
        from = after;
    }
    found
}

fn snapshot_of(text: &str, key: &str) -> Snapshot {
    Snapshot {
        checksum: hex64_after(text, key).unwrap_or_default(),
        path: string_after(text, "\"path\"").unwrap_or_default(),
        built_at: string_after(text, "\"built_at\"").unwrap_or_default(),
        size: number_after(text, "\"size\"").unwrap_or(0),
    }
}

fn read_head(path: &Path, cap: u64) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("读取 {}：{e}", path.display()))?;
    let mut buf = Vec::with_capacity(cap as usize);
    file.take(cap)
        .read_to_end(&mut buf)
        .map_err(|e| format!("读取 {}：{e}", path.display()))?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn read_tail(path: &Path, cap: u64) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("读取 {}：{e}", path.display()))?;
    let len = file
        .metadata()
        .map_err(|e| format!("读取 {}：{e}", path.display()))?
        .len();
    file.seek(SeekFrom::Start(len.saturating_sub(cap)))
        .map_err(|e| format!("读取 {}：{e}", path.display()))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|e| format!("读取 {}：{e}", path.display()))?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// 读一件产物的元数据。大文件只读头或尾；`names.client.json` 是扁平 map，没有元数据。
fn load_artifact(dir: &Path, file: &'static str, role: &'static str) -> ArtifactView {
    let path = dir.join(file);
    let mut view = ArtifactView {
        file: file.to_string(),
        role: role.to_string(),
        present: path.is_file(),
        checksum: String::new(),
        path: String::new(),
        built_at: String::new(),
        size: 0,
    };
    if !view.present {
        return view;
    }
    let snap = match file {
        // 物品库的 `source` 在整份 JSON 的尾巴上。
        "items.index.json" => read_tail(&path, 8192).ok().map(|text| Snapshot {
            checksum: hex64_after(&text, "\"source\"").unwrap_or_default(),
            ..Default::default()
        }),
        // 这三份把 `source` 对象摆在最前面。
        "gm-growth.json" | "gm-quests.json" | "equipment.current37.json" => {
            read_head(&path, 8192).ok().map(|text| snapshot_of(&text, "\"checksum\""))
        }
        "inventory.current37.json" | "experience.compat90.json" => std::fs::read_to_string(&path)
            .ok()
            .map(|text| snapshot_of(&text, "\"source\"")),
        // 旁表只有 `source`（一句中文说明）与 `built_at`。
        "equipment.slots.json" | "equipment.jobs.json" | "item.rarity.json" => {
            read_head(&path, 8192).ok().map(|text| Snapshot {
                built_at: string_after(&text, "\"built_at\"").unwrap_or_default(),
                ..Default::default()
            })
        }
        _ => None,
    };
    if let Some(snap) = snap {
        view.checksum = snap.checksum;
        view.path = snap.path;
        view.built_at = snap.built_at;
        view.size = snap.size;
    }
    view
}

// ---------------------------------------------------------------------------
// 校验
// ---------------------------------------------------------------------------

/// 按配置解出来的客户端目录找 PVF，再做一次同源校验。
pub fn audit(root: &Root) -> Result<AuditView, String> {
    let dir = gmindex::find_configs(&root.path).ok_or_else(|| {
        "未找到 GM 产物目录（`<整合包>/gm-tool/configs` 或 `<整合包>/源码/*/gm-tool/configs`）"
            .to_string()
    })?;
    let pvf = config::client_dir(root).join("Script.pvf");
    Ok(audit_in(&dir, Some(pvf.as_path())))
}

fn pvf_view(path: &Path, recorded_size: i64, built_at: &str) -> Option<PvfView> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let modified = meta
        .modified()
        .ok()
        .map(|t| {
            chrono::DateTime::<chrono::Local>::from(t)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_default();
    let newer = meta
        .modified()
        .ok()
        .and_then(|t| {
            chrono::DateTime::parse_from_rfc3339(built_at)
                .ok()
                .map(|b| chrono::DateTime::<chrono::Local>::from(t) > b)
        })
        .unwrap_or(false);
    Some(PvfView {
        path: path.display().to_string(),
        size: meta.len() as i64,
        modified,
        size_matches: recorded_size > 0 && meta.len() as i64 == recorded_size,
        newer_than_build: newer,
    })
}

fn audit_in(dir: &Path, pvf: Option<&Path>) -> AuditView {
    let artifacts: Vec<ArtifactView> = ARTIFACTS
        .iter()
        .map(|(file, role)| load_artifact(dir, file, role))
        .collect();
    let find = |file: &str| {
        artifacts
            .iter()
            .find(|a| a.file == file)
            .expect("ARTIFACTS 与查找表同源")
    };

    let items = find("items.index.json");
    let growth = find("gm-growth.json");
    let quests = find("gm-quests.json");
    let inv = find("inventory.current37.json");
    let equip = find("equipment.current37.json");

    let mut issues: Vec<IssueView> = Vec::new();
    let err = |text: String| IssueView { tone: "err", text };
    let warn = |text: String| IssueView { tone: "warn", text };

    for a in &artifacts {
        if !a.present {
            issues.push(err(format!(
                "缺少 {}（{}）：用到它的链路会整条拒绝。",
                a.file, a.role
            )));
        }
    }

    // 判据 1：三件 gmbuild 产物必须同一次构建。
    let mut consistent = false;
    if items.present && growth.present && quests.present {
        if !is_hex64(&items.checksum) || !is_hex64(&growth.checksum) || !is_hex64(&quests.checksum) {
            issues.push(err(
                "GM 产物的归档校验读不出来：文件可能不是 gmbuild 写的，或写到一半断了。"
                    .to_string(),
            ));
        } else if items.checksum == growth.checksum && growth.checksum == quests.checksum {
            consistent = true;
        } else {
            issues.push(err(format!(
                "三件 GM 产物不是同一次构建（物品库 {} / 成长表 {} / 任务目录 {}）：养成与任务的写侧会被指纹门整条拒，换过 PVF 就点「一键重建」。",
                short(&items.checksum),
                short(&growth.checksum),
                short(&quests.checksum)
            )));
        }
    }

    // 判据 2：装备目录与背包规则同源（发装备那条闸）。
    if inv.present && equip.present {
        if !is_hex64(&inv.checksum) || inv.checksum != equip.checksum {
            issues.push(err(format!(
                "装备目录与背包规则不同源（背包 {} / 装备 {}）：发装备会被拒「装备目录与背包规则不同源」。这两件现在都由一键重建产出（装备目录全量枚举、背包规则换代际戳），点一次就补齐。",
                short(&inv.checksum),
                short(&equip.checksum)
            )));
        } else if consistent && inv.checksum != items.checksum {
            // 不是错：发放链路只比上面那一对。但两套产物的代际差会体现在分类与耐久上。
            issues.push(warn(format!(
                "装备目录与背包规则（{}）跟 GM 三件（{}）不是同一次构建：发放按前者的分类与耐久，养成与任务按后者判身份。现在一次「一键重建」就把两组一起换到当前 PVF。",
                short(&inv.checksum),
                short(&items.checksum)
            )));
        }
    }

    // 当前 PVF 与产物记录的那份比一比：大小不同 / 比产物新，都说明「换过」。
    let pvf_path = pvf
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "（未配置客户端目录）".to_string());
    let view = pvf.and_then(|p| pvf_view(p, growth.size, &growth.built_at));
    if let Some(v) = &view {
        if !v.size_matches && growth.size > 0 {
            issues.push(warn(format!(
                "当前 Script.pvf 是 {} 字节，产物构建时记的是 {} 字节：像换过 PVF，先「一键重建」再发放。",
                v.size, growth.size
            )));
        }
        if v.newer_than_build {
            issues.push(warn(format!(
                "当前 Script.pvf 比产物新（{}，产物建于 {}）：产物可能落后于客户端。",
                v.modified, growth.built_at
            )));
        }
    } else if pvf.is_some() {
        issues.push(warn(format!(
            "没找到当前客户端 PVF（{}）：重建前请确认客户端目录，或手动「选择 PVF…」。",
            pvf_path
        )));
    }

    if issues.iter().all(|i| i.tone != "err" && i.tone != "warn") {
        issues.push(IssueView {
            tone: "ok",
            text: format!(
                "三件 GM 产物同源（{}），装备目录与背包规则也同源。",
                short(&items.checksum)
            ),
        });
    }

    AuditView {
        dir: dir.display().to_string(),
        pvf_path,
        pvf: view,
        artifacts,
        issues,
        consistent,
    }
}

// ---------------------------------------------------------------------------
// 一键重建
// ---------------------------------------------------------------------------

static REBUILDING: AtomicBool = AtomicBool::new(false);

struct Busy;

impl Drop for Busy {
    fn drop(&mut self) {
        REBUILDING.store(false, Ordering::SeqCst);
    }
}

/// 找 `gmbuild.exe`：环境变量 > 启动器旁边 > 整合包里 > 开发时的 target。
pub fn find_exe(root: &Root) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(value) = std::env::var("DFO_GMBUILD") {
        if !value.trim().is_empty() {
            candidates.push(PathBuf::from(value));
        }
    }
    if let Some(exe) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        candidates.push(exe.join("gmbuild.exe"));
        candidates.push(exe.join("bin").join("gmbuild.exe"));
    }
    candidates.push(root.path.join("gm-tool").join("bin").join("gmbuild.exe"));
    candidates.push(root.work_dir().join("bin").join("gmbuild.exe"));
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("gmbuild").join("target").join("release").join("gmbuild.exe"));
        candidates.push(cwd.join("gmbuild").join("target").join("debug").join("gmbuild.exe"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn last_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .last()
        .unwrap_or("")
        .to_string()
}

/// 失败时给一段尾巴：gmbuild 的 panic 与解密报错都是多行的，只留最后一行
/// （往往是 `note: run with RUST_BACKTRACE=1`）等于什么也没说。
fn tail_text(text: &str, cap: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= cap {
        return trimmed.to_string();
    }
    let skip = trimmed.chars().count() - cap;
    let tail: String = trimmed.chars().skip(skip).collect();
    format!("…{tail}")
}

/// 头部那串十进制数：返回 `(值, 剩下的部分)`。
fn number_head(s: &str) -> Option<(u64, &str)> {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    s[..end].parse().ok().map(|v| (v, &s[end..]))
}

/// 从子进程的一行输出里取出 `进度 n/m` 并换算成 0.0~1.0。
///
/// 只认「进度」后面第一段 `数字/数字`：物品库、旁表、装备目录三条打印的都是这个形状，
/// 措辞改了不用跟着改这里。名字表/养成/任务不打印分数，返回 None，进度照步权重跳格。
fn fraction_of(line: &str) -> Option<f64> {
    let at = line.find("进度")? + "进度".len();
    let (done, rest) = number_head(line[at..].trim_start())?;
    let (total, _) = number_head(rest.strip_prefix('/')?)?;
    if total == 0 {
        return None;
    }
    Some((done as f64 / total as f64).clamp(0.0, 1.0))
}

/// 直读 PVF 重烘一组产物，校验通过后替换进产物目录。
///
/// 失败时**不替换**：先全部写在 `.gmbuild-staging` 里，七件校验同源才搬过去；
/// 搬之前把旧产物备份到 `.backup-<时戳>`，搬完删 staging。
pub fn rebuild(root: &Root, bus: &Bus, pvf: Option<PathBuf>) -> Result<RebuildView, String> {
    if REBUILDING.swap(true, Ordering::SeqCst) {
        return Err("已经有一次重建在跑，等它结束再点".to_string());
    }
    let _busy = Busy;
    let started = Instant::now();

    let dir = gmindex::find_configs(&root.path).ok_or_else(|| {
        "未找到 GM 产物目录（`<整合包>/gm-tool/configs` 或 `<整合包>/源码/*/gm-tool/configs`）"
            .to_string()
    })?;
    let pvf = pvf.unwrap_or_else(|| config::client_dir(root).join("Script.pvf"));
    if !pvf.is_file() {
        return Err(format!(
            "找不到 PVF：{}。先在首页确认客户端目录，或点「选择 PVF…」手动指一次。",
            pvf.display()
        ));
    }
    let exe = find_exe(root).ok_or_else(|| {
        format!(
            "找不到 gmbuild.exe。可放在 {}、启动器旁边，或用环境变量 DFO_GMBUILD 指过去（开发时就是 gmbuild/target/release/gmbuild.exe）。",
            root.path.join("gm-tool").join("bin").display()
        )
    })?;

    bus.info(format!(
        "GM 产物重建开始：{}（gmbuild={}） -> {}",
        pvf.display(),
        exe.display(),
        dir.display()
    ));

    let staging = dir.join(".gmbuild-staging");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("建暂存目录失败：{e}"))?;
    // build-slots 会把「现有表里有、这次没扫到」的条目保留下来；不给它旧表就丢掉那批历史条目。
    let old_slots = dir.join("equipment.slots.json");
    if old_slots.is_file() {
        let _ = std::fs::copy(&old_slots, staging.join("equipment.slots.json"));
    }

    let pvf_arg = pvf.display().to_string();
    let out_arg = staging.display().to_string();
    let dir_arg = dir.display().to_string();
    let specs = build_steps();
    let total_weight: u64 = specs.iter().map(|s| s.weight).sum();
    let mut steps: Vec<StepView> = Vec::new();
    let mut done_weight = 0u64;

    for (index, spec) in specs.iter().enumerate() {
        let cmd = spec.cmd;
        let outputs = spec.outputs;
        let tick = Instant::now();
        // 这一步在总进度里的起点与跨度（按实测秒数分摊，不是等分）。
        let base = done_weight as f64 * 100.0 / total_weight as f64;
        let span = spec.weight as f64 * 100.0 / total_weight as f64;
        let emit = |percent: u32, text: String| {
            bus.progress(&ProgressView {
                step: cmd.to_string(),
                step_index: index + 1,
                step_total: specs.len(),
                percent,
                text,
            });
        };
        bus.info(format!("{cmd} 开始（超时 {} 分钟）", spec.timeout / 60));
        emit(
            base as u32,
            format!("第 {}/{} 步：{}", index + 1, specs.len(), cmd),
        );
        let mut step_cmd = procutil::command(&exe);
        step_cmd.args([cmd, pvf_arg.as_str(), out_arg.as_str()]);
        if spec.needs_src {
            // refresh-bag 读旧背包规则是从**产物目录**读的，不是暂存目录：
            // 暂存里那份要么还没有，要么正是这次要换的。
            step_cmd.arg(dir_arg.as_str());
        }
        let mut shown = base as u32;
        let out = procutil::run_with_lines(step_cmd, Duration::from_secs(spec.timeout), |line| {
            let Some(frac) = fraction_of(line) else {
                return;
            };
            let percent = (base + span * frac).clamp(0.0, 100.0).round() as u32;
            if percent <= shown {
                return;
            }
            shown = percent;
            emit(percent, line.trim().to_string());
        });
        let seconds = tick.elapsed().as_secs_f64();
        let missing: Vec<&str> = outputs
            .iter()
            .copied()
            .filter(|f| !staging.join(f).is_file())
            .collect();
        let (ok, detail) = match &out {
            Err(text) => (false, text.clone()),
            Ok((false, text)) => (
                false,
                format!("{cmd} 退出码非零：{}", tail_text(text, 800)),
            ),
            Ok((true, text)) if !missing.is_empty() => (
                false,
                format!("{} 没写出：{}；输出尾巴：{}", cmd, missing.join("、"), tail_text(text, 400)),
            ),
            Ok((true, text)) => (true, last_line(text)),
        };
        steps.push(StepView {
            step: cmd.to_string(),
            ok,
            seconds,
            detail: detail.clone(),
        });
        if ok {
            done_weight += spec.weight;
            emit(
                (base + span).clamp(0.0, 100.0).round() as u32,
                format!("第 {}/{} 步完成：{}", index + 1, specs.len(), cmd),
            );
            bus.ok(format!("{cmd} 完成（{seconds:.1}s）：{detail}"));
        } else {
            bus.error(format!("{cmd} 失败：{detail}"));
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!(
                "重建中止在 {cmd}：{detail}
旧产物没有动（暂存目录已清掉）。"
            ));
        }
    }

    // 五件带戳的必须同源：gmbuild 一次跑出来的产物本来就应该相等，这里防的是「构建中途 PVF 被换掉」
    // 这类外因，以及将来某个子命令只重建其中一件。
    let c_items = read_tail(&staging.join("items.index.json"), 8192)
        .ok()
        .and_then(|t| hex64_after(&t, "\"source\""))
        .unwrap_or_default();
    let c_growth = read_head(&staging.join("gm-growth.json"), 8192)
        .ok()
        .and_then(|t| hex64_after(&t, "\"checksum\""))
        .unwrap_or_default();
    let c_quests = read_head(&staging.join("gm-quests.json"), 8192)
        .ok()
        .and_then(|t| hex64_after(&t, "\"checksum\""))
        .unwrap_or_default();
    let c_equip = read_head(&staging.join("equipment.current37.json"), 8192)
        .ok()
        .and_then(|t| hex64_after(&t, "\"checksum\""))
        .unwrap_or_default();
    let c_inv = std::fs::read_to_string(staging.join("inventory.current37.json"))
        .ok()
        .and_then(|t| hex64_after(&t, "\"source\""))
        .unwrap_or_default();
    if !is_hex64(&c_items)
        || c_items != c_growth
        || c_items != c_quests
        || c_items != c_equip
        || c_items != c_inv
    {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!(
            "新产物之间不同源（物品库 {} / 成长表 {} / 任务目录 {} / 装备目录 {} / 背包规则 {}），已放弃替换；旧产物没动。",
            short(&c_items),
            short(&c_growth),
            short(&c_quests),
            short(&c_equip),
            short(&c_inv)
        ));
    }

    // 备份 + 替换。备份只存这次会被覆盖的那几件（旁表也算，方便整套回退）。
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let done = deploy_artifacts(&staging, &dir, &stamp)?;
    let _ = std::fs::remove_dir_all(&staging);

    let view = RebuildView {
        dir: dir.display().to_string(),
        pvf: pvf.display().to_string(),
        checksum: c_items.clone(),
        steps,
        backup: if done.backed_up == 0 {
            String::new()
        } else {
            done.backup.display().to_string()
        },
        elapsed: started.elapsed().as_secs_f64(),
        audit: audit_in(&dir, Some(pvf.as_path())),
    };
    bus.ok(format!(
        "GM 产物已换新：{} 件写入，归档校验 {}，用时 {:.1}s",
        done.deployed,
        short(&c_items),
        view.elapsed
    ));
    Ok(view)
}

/// 替换结果。
#[derive(Debug)]
struct Deployed {
    backup: PathBuf,
    backed_up: usize,
    deployed: usize,
}

/// 把暂存里的产物搬进产物目录：先备份旧的，再逐个替换。
///
/// 中途任何一步失败都**尽力回滚**已替换的那几件——否则「旧产物没动」就成了谎话。
/// 一件都没备份上（目录原本是空的）就把空备份目录删掉，免得界面上写一个不存在的回退点。
fn deploy_artifacts(staging: &Path, dir: &Path, tag: &str) -> Result<Deployed, String> {
    let backup = dir.join(format!(".backup-{tag}"));
    std::fs::create_dir_all(&backup).map_err(|e| format!("建备份目录失败：{e}"))?;
    let mut backed_up: Vec<&'static str> = Vec::new();
    let mut added: Vec<&'static str> = Vec::new();
    let mut deployed = 0usize;

    for spec in build_steps() {
        for file in spec.outputs {
            let src = staging.join(file);
            let dst = dir.join(file);
            let attempt = (|| -> Result<(), String> {
                if dst.is_file() {
                    std::fs::copy(&dst, backup.join(file))
                        .map_err(|e| format!("备份 {} 失败：{e}", dst.display()))?;
                }
                std::fs::copy(&src, &dst)
                    .map_err(|e| format!("替换 {} 失败：{e}", dst.display()))?;
                Ok(())
            })();
            match attempt {
                Ok(()) => {
                    if backup.join(file).is_file() {
                        if !backed_up.contains(file) {
                            backed_up.push(file);
                        }
                    } else if !added.contains(&file) {
                        added.push(file);
                    }
                    deployed += 1;
                }
                Err(err) => {
                    // 已替换的回滚到备份；原本没有的件（新增件）删掉，别留下半套。
                    for done in &backed_up {
                        let _ = std::fs::copy(backup.join(done), dir.join(done));
                    }
                    for done in &added {
                        let _ = std::fs::remove_file(dir.join(done));
                    }
                    let _ = std::fs::remove_dir_all(&backup);
                    return Err(format!("{err}（已尽力回滚，旧产物未变）"));
                }
            }
        }
    }

    if backed_up.is_empty() {
        let _ = std::fs::remove_dir_all(&backup);
    }
    Ok(Deployed {
        backup,
        backed_up: backed_up.len(),
        deployed,
    })
}

// ---------------------------------------------------------------------------
// 命令层
// ---------------------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
pub async fn gm_audit(
    state: tauri::State<'_, crate::commands::AppState>,
) -> Result<AuditView, String> {
    let root = state.root.clone();
    crate::commands::join(move || audit(&root)).await
}

/// 重建七步产物（含装备目录全量枚举与背包规则换戳），成功即重载物品库。`pvf` 为空时用当前客户端目录里的那份。
#[tauri::command(rename_all = "snake_case")]
pub async fn gm_rebuild(
    pvf: Option<String>,
    state: tauri::State<'_, crate::commands::AppState>,
) -> Result<RebuildView, String> {
    let root = state.root.clone();
    let bus = state.session.bus();
    let shared = state.gm.clone();
    crate::commands::join(move || {
        let picked = pvf
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty());
        let view = rebuild(&root, &bus, picked)?;
        shared.reload(&root)?;
        Ok(view)
    })
    .await
}

/// 手动指一份 PVF（换包、或客户端目录还没指对时用）。取消返回 None。
#[tauri::command]
pub async fn gm_pick_pvf(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::{DialogExt, FilePath};
    let picked = crate::commands::block(move || {
        app.dialog()
            .file()
            .add_filter("PVF", &["pvf"])
            .blocking_pick_file()
    })
    .await?;
    match picked {
        None => Ok(None),
        Some(FilePath::Path(path)) => Ok(Some(path.display().to_string())),
        Some(FilePath::Url(url)) => Err(format!("只能选本地文件，收到的是：{url}")),
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);

    impl Tmp {
        fn new(tag: &str) -> Tmp {
            let path = std::env::temp_dir().join(format!(
                "dfo-gmrebuild-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("建临时目录");
            Tmp(path)
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const S: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    fn configs(tmp: &Tmp, items: &str, growth: &str, quests: &str, inv: &str, equip: &str) -> PathBuf {
        let dir = tmp.0.join("gm-tool").join("configs");
        std::fs::create_dir_all(&dir).expect("建产物目录");
        std::fs::write(
            dir.join("items.index.json"),
            format!(r#"{{"count":0,"items":[],"source":"{items}"}}"#),
        )
        .unwrap();
        std::fs::write(
            dir.join("gm-growth.json"),
            format!(
                r#"{{"source":{{"format":"dfo_20260901","path":"Script.pvf","size":10,"checksum":"{growth}","built_at":"2026-10-21T05:17:55+08:00"}},"thresholds":[],"professions":{{}}}}"#
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("gm-quests.json"),
            format!(
                r#"{{"source":{{"format":"dfo_20260901","path":"Script.pvf","size":10,"checksum":"{quests}","built_at":"2026-10-21T05:37:09+08:00"}},"index_sha256":"{A}","quests":{{}}}}"#
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("inventory.current37.json"),
            format!(r#"{{"model":"reference90-bag-v1","source":"{inv}","slots":{{}},"missing_stack_limit":1000}}"#),
        )
        .unwrap();
        std::fs::write(
            dir.join("equipment.current37.json"),
            format!(r#"{{"source":{{"format":"dfo_20260901_inner","path":"Script.inner.pvf","size":10,"checksum":"{equip}"}},"rows":[]}}"#),
        )
        .unwrap();
        std::fs::write(
            dir.join("experience.compat90.json"),
            format!(
                r#"{{"model":"reference90-solo-v1","reference_sha256":"{A}","level_cap":115,"named_multiplier":3,"monster_minus_character_level_rates":{{"0":1}}}}"#
            ),
        )
        .unwrap();
        std::fs::write(dir.join("names.client.json"), "{}").unwrap();
        for table in ["equipment.slots.json", "equipment.jobs.json", "item.rarity.json"] {
            std::fs::write(
                dir.join(table),
                r#"{"source":"Script.pvf 全量导出","built_at":"2026-10-20T23:13:48+08:00","slots":{}}"#,
            )
            .unwrap();
        }
        dir
    }

    #[test]
    fn reads_the_three_metadata_shapes() {
        // 成长/任务：source 在最前面。
        let growth = format!(
            r#"{{"source":{{"path":"Script.pvf","size":865854418,"checksum":"{A}","built_at":"2026-10-21T05:17:55+08:00"}},"x":1}}"#
        );
        assert_eq!(hex64_after(&growth, "\"checksum\"").as_deref(), Some(A));
        assert_eq!(number_after(&growth, "\"size\""), Some(865854418));
        assert_eq!(string_after(&growth, "\"built_at\"").as_deref(), Some("2026-10-21T05:17:55+08:00"));

        // 物品库：source 在尾巴上，而且前面可能有名字里带 source 的物品。
        let items = format!(
            r#"{{"count":2,"items":[{{"id":1,"name":"source"}},{{"id":2,"name":"source of truth"}}],"source":"{B}"}}"#
        );
        assert_eq!(hex64_after(&items, "\"source\"").as_deref(), Some(B));

        // 背包规则：pretty-print，冒号后面有空格。
        let inv = format!("{{\n  \"model\": \"reference90-bag-v1\",\n  \"source\": \"{S}\",\n  \"slots\": {{}}\n}}");
        assert_eq!(hex64_after(&inv, "\"source\"").as_deref(), Some(S));
    }

    #[test]
    fn audit_accepts_a_same_source_set() {
        let tmp = Tmp::new("ok");
        let dir = configs(&tmp, A, A, A, A, A);
        let view = audit_in(&dir, None);
        assert!(view.consistent);
        assert_eq!(view.artifacts.iter().filter(|a| a.present).count(), ARTIFACTS.len());
        assert!(
            view.issues.iter().all(|i| i.tone == "ok"),
            "{:?}",
            view.issues
        );
    }

    #[test]
    fn audit_flags_a_stale_pvf_group_and_a_cross_generation_equipment_pair() {
        let tmp = Tmp::new("stale");
        // 物品库与成长表同源、任务目录是另一次构建；装备那一对又是另一代。
        let dir = configs(&tmp, A, A, B, S, S);
        let view = audit_in(&dir, None);
        assert!(!view.consistent);
        let errs: Vec<&str> = view
            .issues
            .iter()
            .filter(|i| i.tone == "err")
            .map(|i| i.text.as_str())
            .collect();
        assert!(
            errs.iter().any(|t| t.contains("不是同一次构建")),
            "{errs:?}"
        );

        // 同源的三件 + 装备那一对不同源：只该是 warn，不是 err。
        let tmp2 = Tmp::new("cross");
        let dir2 = configs(&tmp2, A, A, A, S, S);
        let view2 = audit_in(&dir2, None);
        assert!(view2.consistent);
        assert!(
            view2
                .issues
                .iter()
                .any(|i| i.tone == "warn" && i.text.contains("不是同一次构建")),
            "{:?}",
            view2.issues
        );
        assert!(view2.issues.iter().all(|i| i.tone != "err"));
    }

    #[test]
    fn audit_flags_the_equipment_bag_same_source_gate() {
        let tmp = Tmp::new("equip");
        let dir = configs(&tmp, A, A, A, S, B);
        let view = audit_in(&dir, None);
        assert!(view
            .issues
            .iter()
            .any(|i| i.tone == "err" && i.text.contains("不同源")));
    }

    #[test]
    fn audit_notices_a_swapped_pvf_by_size() {
        let tmp = Tmp::new("pvf");
        let dir = configs(&tmp, A, A, A, S, S);
        // 产物记的 10 字节，现场这份不是。
        let pvf = tmp.0.join("Script.pvf");
        std::fs::write(&pvf, b"0123456789abcdef").unwrap();
        let view = audit_in(&dir, Some(pvf.as_path()));
        assert!(view.pvf.as_ref().map(|p| !p.size_matches).unwrap_or(false));
        assert!(view
            .issues
            .iter()
            .any(|i| i.tone == "warn" && i.text.contains("像换过 PVF")));
    }

    #[test]
    fn audit_reports_missing_artifacts_one_by_one() {
        let tmp = Tmp::new("missing");
        let dir = tmp.0.join("gm-tool").join("configs");
        std::fs::create_dir_all(&dir).unwrap();
        let view = audit_in(&dir, None);
        assert!(!view.consistent);
        let missing: usize = view.artifacts.iter().filter(|a| !a.present).count();
        assert_eq!(missing, ARTIFACTS.len());
        assert_eq!(
            view.issues.iter().filter(|i| i.tone == "err").count(),
            ARTIFACTS.len()
        );
    }

    #[test]
    fn build_steps_cover_every_artifact_they_replace() {
        let produced: usize = build_steps().iter().map(|s| s.outputs.len()).sum();
        assert_eq!(produced, 9, "七条子命令应产出九件（部位/职业/品级三件同一步）");
        for spec in build_steps() {
            assert!(spec.timeout >= 60, "{} 超时太短", spec.cmd);
            assert!(!spec.outputs.is_empty(), "{} 没有产出文件", spec.cmd);
            assert!(spec.weight > 0, "{} 没有权重，进度条会漏这一段", spec.cmd);
        }
        // 只有换戳那一步要读旧产物目录；其余各步只认 PVF 与暂存目录。
        let needs_src: Vec<&str> = build_steps()
            .iter()
            .filter(|s| s.needs_src)
            .map(|s| s.cmd)
            .collect();
        assert_eq!(needs_src, vec!["refresh-bag"]);
    }

    /// `进度 n/m` 那三条打印形状都要认得；不打印分数的那些返回 None。
    #[test]
    fn progress_lines_become_fractions() {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(close(
            fraction_of("  items.index 进度 100000/5752281/eq/list/equipment.lst").unwrap(),
            100000.0 / 5752281.0
        ));
        assert!(close(
            fraction_of("  旁表进度 3/5752281 read=1.2").unwrap(),
            3.0 / 5752281.0
        ));
        assert!(close(
            fraction_of("  equipment.current37 进度 560000/5752281 候选 4213").unwrap(),
            560000.0 / 5752281.0
        ));
        // 收尾那行没有分数：不能把它读成 100%。
        assert!(fraction_of("equipment.current37.json 写出 106254 行").is_none());
        assert!(fraction_of("  客户端文本表 string/UI.str 新增 12 条").is_none());
        // 总数 0 / 缺分母 / 根本没有「进度」二字。
        assert!(fraction_of("进度 5/0").is_none());
        assert!(fraction_of("进度 5").is_none());
        assert!(fraction_of("nothing here").is_none());
        // 越界的分子按这一步走完算，不让总条超过 100。
        assert!(close(fraction_of("进度 900/100").unwrap(), 1.0));
    }

    /// 替换只动 build_steps 列出的那九件；旧的进备份，新的进目录。
    #[test]
    fn deploy_backs_up_and_replaces_only_the_built_artifacts() {
        let tmp = Tmp::new("deploy");
        let dir = tmp.0.join("configs");
        let staging = tmp.0.join("staging");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        // 旧目录里只有三件被覆盖（其余件原本不存在）。
        for file in ["items.index.json", "gm-growth.json", "equipment.slots.json"] {
            std::fs::write(dir.join(file), "old").unwrap();
        }
        std::fs::write(dir.join("没被碰的文件.txt"), "keep").unwrap();
        for spec in build_steps() {
            for file in spec.outputs {
                std::fs::write(staging.join(file), "new").unwrap();
            }
        }

        let done = deploy_artifacts(&staging, &dir, "t").expect("替换应成功");
        assert_eq!(done.deployed, 9);
        assert_eq!(done.backed_up, 3);
        assert_eq!(std::fs::read_to_string(dir.join("items.index.json")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(dir.join("gm-quests.json")).unwrap(), "new");
        assert_eq!(
            std::fs::read_to_string(done.backup.join("items.index.json")).unwrap(),
            "old"
        );
        // 不在清单里的文件不该被动。
        assert_eq!(std::fs::read_to_string(dir.join("没被碰的文件.txt")).unwrap(), "keep");
        assert!(done.backup.is_dir());
    }

    /// 替换中途失败要回滚：已经换掉的恢复成旧的，新增的删掉，备份目录不留。
    #[test]
    fn deploy_rolls_back_when_a_copy_fails() {
        let tmp = Tmp::new("rollback");
        let dir = tmp.0.join("configs");
        let staging = tmp.0.join("staging");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(dir.join("items.index.json"), "old-items").unwrap();
        std::fs::write(dir.join("gm-growth.json"), "old-growth").unwrap();
        for spec in build_steps() {
            for file in spec.outputs {
                std::fs::write(staging.join(file), "new").unwrap();
            }
        }
        // 拿一个目录顶住最后一件的落点：复制到它上面必然失败。
        std::fs::create_dir_all(dir.join("gm-quests.json")).unwrap();

        let err = deploy_artifacts(&staging, &dir, "rb").expect_err("应当失败");
        assert!(err.contains("回滚"), "{err}");
        assert_eq!(
            std::fs::read_to_string(dir.join("items.index.json")).unwrap(),
            "old-items"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("gm-growth.json")).unwrap(),
            "old-growth"
        );
        // 原来没有的件（名字表）回滚时不该留下。
        assert!(!dir.join("names.client.json").exists());
        assert!(!dir.join(".backup-rb").exists());
    }

    /// 拿真产物目录跑一遍（大文件只读头/尾那几条路径全靠它盯）：
    /// `DFO_GM_CONFIGS=<configs> DFO_GM_PVF=<Script.pvf> cargo test --lib --manifest-path src-tauri/Cargo.toml live_audit -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_audit_over_real_configs() {
        let Ok(dir) = std::env::var("DFO_GM_CONFIGS") else {
            eprintln!("跳过：未设 DFO_GM_CONFIGS");
            return;
        };
        let pvf = std::env::var("DFO_GM_PVF").ok().map(PathBuf::from);
        let view = audit_in(Path::new(&dir), pvf.as_deref());
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&view).expect("序列化校验结果")
        );
        for a in &view.artifacts {
            eprintln!(
                "{} present={} checksum={} size={} built_at={}",
                a.file,
                a.present,
                short(&a.checksum),
                a.size,
                a.built_at
            );
        }
    }
}
