//! 服务端实时内存：只读解析编排器写在 `gateway.err` 里的 `MEMORY_GUARD` 行。
//!
//! 行按前缀 + `key=value` 认，绝不按字段顺序硬解 —— 编排器将来往行里加字段，
//! 面板不该跟着错位。四种行：
//! - `MEMORY_GUARD active pid=… hard_commit_mib=… policy=…`：本场首条，硬上限与 pid 由它给；
//! - `MEMORY_GUARD sample seconds=… private_mib=… working_mib=… system_available_mib=…`：每 2 秒一条；
//! - `MEMORY_GUARD ended … peak_private_mib=…`：收尾汇总；
//! - `MEMORY_GUARD stopped … reason=…`：测量自己停了（读进程失败 / 系统压力）。
//!
//! 采样与配置里的硬上限无关：不设限也照常打点，所以「不限」时这里照样有数。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;

use crate::paths::Root;

/// 编排器 `memory_windows.go` 当前那一版的策略标记。见到别的（尤其 `-r1`）就是旧编排器在跑。
const POLICY_CURRENT: &str = "local-stability-20261008-r2";
const PREFIX: &str = "MEMORY_GUARD ";
/// `active` 是网关进程起来后写的第一批行，64 KiB 的 stderr 兜得住。
const HEAD_BYTES: u64 = 64 * 1024;
/// 采样每 2 秒一条，128 KiB 够画小曲线的窗口（约 1200 条）。
const TAIL_BYTES: u64 = 128 * 1024;
/// 小曲线最多画这么多个点（2 秒一条 ≈ 3 分钟）。
const SERIES_MAX: usize = 90;
/// 超过这个秒数没有新行 = 这一场不再打点了（已停止，或网关被别的途径杀掉）。
const STALE_AFTER_SECS: u64 = 8;

/// 面板要显示的一页实况。字段一律 snake_case，与其余后端结构同一口径。
#[derive(Serialize)]
pub struct Live {
    /// 会话 tag，就是 `runtime/` 下那个目录名。
    pub session: String,
    pub path: String,
    pub pid: u64,
    pub policy: String,
    /// `None` = 本场没设硬上限（`hard_commit_mib=unlimited`）。
    pub hard_limit_mib: Option<u64>,
    pub low_memory_mib: u64,
    /// 最后一条采样距网关启动的秒数。
    pub seconds: f64,
    /// 进程私有提交内存：判断离硬上限还有多远就看它。
    pub private_mib: u64,
    pub working_mib: u64,
    pub system_available_mib: u64,
    /// 本场峰值：只有收尾行里才有，没结束就是 `None`。
    pub peak_private_mib: Option<u64>,
    pub ended: bool,
    pub stale: bool,
    /// 尾部窗口里读到的采样条数，用来分辨「刚起来只有一条」和「读坏了」。
    pub samples: usize,
    pub series: Vec<u64>,
    /// 空串 = 一切正常；否则是要给玩家看的提醒（旧编排器 / 采样已停）。
    pub note: String,
}

fn kind(line: &str) -> Option<&str> {
    let rest = line.strip_prefix(PREFIX)?;
    rest.split_whitespace().next()
}

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let head = format!("{key}=");
    line.split_whitespace()
        .find_map(|token| token.strip_prefix(head.as_str()))
}

fn number(line: &str, key: &str) -> Option<u64> {
    field(line, key)?.parse().ok()
}

/// 这一场到底设了多硬的墙：`unlimited` 与缺字段都算「无上限」，不猜。
fn hard_limit(line: &str) -> Option<u64> {
    match field(line, "hard_commit_mib") {
        Some("unlimited") | None => None,
        Some(text) => text.parse().ok(),
    }
}

fn elapsed_since(time: SystemTime) -> Option<u64> {
    time.elapsed().ok().map(|d| d.as_secs())
}

/// 最新一场：按 `gateway.err` 自己的 mtime 排，而不是按目录 mtime ——
/// 目录 mtime 只记「建出来那一刻」，一场跑了 7 分钟的会话，它的 err 文件才是活的那一个。
fn newest_guard_log(root: &Root) -> Option<(PathBuf, PathBuf)> {
    let entries = std::fs::read_dir(root.runtime_dir()).ok()?;
    let mut best: Option<(SystemTime, PathBuf, PathBuf)> = None;
    for entry in entries.flatten() {
        let dir = entry.path();
        let is_session = entry
            .file_name()
            .to_string_lossy()
            .starts_with("roles_");
        if !is_session || !dir.is_dir() {
            continue;
        }
        let file = dir.join("gateway.err");
        let Ok(meta) = std::fs::metadata(&file) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if best.as_ref().map(|(t, _, _)| mtime > *t).unwrap_or(true) {
            best = Some((mtime, dir, file));
        }
    }
    best.map(|(_, dir, file)| (dir, file))
}

fn read_head(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let take = HEAD_BYTES.min(file.metadata().ok()?.len());
    let mut buf = vec![0u8; take as usize];
    file.read_exact(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// 只读尾部，且丢掉第一条可能被截断的行（截断行前缀匹配不上，但没必要留着）。
fn read_tail(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let offset = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = Vec::with_capacity((len - offset) as usize);
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    Some(if offset == 0 {
        text
    } else {
        match text.find('\n') {
            Some(cut) => text[cut + 1..].to_string(),
            None => String::new(),
        }
    })
}

/// 读不到就返回 `None`：面板宁可不显示，也不摆一个看着像真数的假占用。
pub fn live(root: &Root) -> Option<Live> {
    let (dir, file) = newest_guard_log(root)?;
    let active = read_head(&file)?
        .lines()
        .find(|line| kind(line) == Some("active"))
        .map(str::to_string)?;
    let tail = read_tail(&file)?;

    let mut last_sample: Option<String> = None;
    let mut samples = 0usize;
    let mut series: Vec<u64> = Vec::new();
    let mut ended: Option<String> = None;
    let mut stopped: Option<String> = None;
    for line in tail.lines() {
        match kind(line) {
            Some("sample") => {
                samples += 1;
                if let Some(private) = number(line, "private_mib") {
                    series.push(private);
                }
                last_sample = Some(line.to_string());
            }
            Some("ended") => ended = Some(line.to_string()),
            Some("stopped") => stopped = Some(line.to_string()),
            _ => {}
        }
    }
    if series.len() > SERIES_MAX {
        series.drain(..series.len() - SERIES_MAX);
    }
    let sample = last_sample?;
    let stale = elapsed_since(file.metadata().ok()?.modified().ok()?)
        .map(|secs| secs > STALE_AFTER_SECS)
        .unwrap_or(false);

    let policy = field(&active, "policy").unwrap_or("").to_string();
    let mut notes: Vec<String> = Vec::new();
    if policy != POLICY_CURRENT {
        notes.push(format!(
            "编排器记录的策略是 {policy}（当前应为 {POLICY_CURRENT}）：跑的是旧编排器，本场上限口径按旧版算"
        ));
    }
    if let Some(line) = &stopped {
        let reason = field(line, "reason").unwrap_or("未知原因");
        notes.push(format!("采样已停止（{reason}），下面最后是停止前那一条"));
    } else if stale && ended.is_none() {
        notes.push("这一场已经没有新采样：服务端多半停了".to_string());
    }

    Some(Live {
        session: dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: file.display().to_string(),
        pid: number(&active, "pid").unwrap_or_default(),
        policy,
        hard_limit_mib: hard_limit(&active),
        low_memory_mib: number(&active, "low_memory_mib").unwrap_or_default(),
        seconds: sample
            .split_whitespace()
            .find_map(|token| token.strip_prefix("seconds="))
            .and_then(|text| text.parse().ok())
            .unwrap_or_default(),
        private_mib: number(&sample, "private_mib").unwrap_or_default(),
        working_mib: number(&sample, "working_mib").unwrap_or_default(),
        system_available_mib: number(&sample, "system_available_mib").unwrap_or_default(),
        peak_private_mib: ended
            .as_ref()
            .and_then(|line| number(line, "peak_private_mib")),
        ended: ended.is_some(),
        stale,
        samples,
        series,
        note: notes.join("；"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACTIVE_R2: &str = "MEMORY_GUARD active pid=29728 scope=gateway hard_commit_mib=unlimited low_memory_mib=2048 system_pressure_action=warn_only policy=local-stability-20261008-r2\n";
    const SAMPLE_1: &str = "MEMORY_GUARD sample scope=gateway seconds=0.3 private_mib=51 working_mib=12 system_available_mib=14696\n";
    const SAMPLE_2: &str = "MEMORY_GUARD sample scope=gateway seconds=418.0 private_mib=1430 working_mib=1398 system_available_mib=8454\n";
    const SAMPLE_3: &str = "MEMORY_GUARD sample scope=gateway seconds=422.3 private_mib=1431 working_mib=1398 system_available_mib=8671\n";
    const ENDED: &str = "MEMORY_GUARD ended scope=gateway peak_private_mib=2099\n";

    /// 这些行是从部署包 2026-10-19 那几场 `gateway.err` 里原样抄的，不是按格式编的。
    fn session_dir(base: &Path, tag: &str, body: &str) -> PathBuf {
        let dir = base.join("server").join("work").join("dfo-lan").join("runtime").join(tag);
        std::fs::create_dir_all(&dir).expect("造会话目录");
        std::fs::write(dir.join("gateway.err"), body).expect("写 gateway.err");
        dir
    }

    fn root_at(base: &Path) -> Root {
        Root { path: base.to_path_buf(), source: "test" }
    }

    #[test]
    fn reads_the_real_guard_lines() {
        let base = std::env::temp_dir().join(format!("dfo-mem-rt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        session_dir(&base, "roles_persist_x_20261019_010406_next37", &format!("{ACTIVE_R2}{SAMPLE_1}{SAMPLE_2}{SAMPLE_3}{ENDED}"));
        let got = live(&root_at(&base)).expect("该读出实况");
        assert_eq!(got.pid, 29728);
        assert_eq!(got.session, "roles_persist_x_20261019_010406_next37");
        assert_eq!(got.hard_limit_mib, None, "unlimited 要读成无上限");
        assert_eq!(got.low_memory_mib, 2048);
        assert_eq!(got.private_mib, 1431, "取最后一条采样");
        assert_eq!(got.working_mib, 1398);
        assert_eq!(got.system_available_mib, 8671);
        assert_eq!(got.seconds, 422.3);
        assert_eq!(got.series, vec![51, 1430, 1431]);
        assert_eq!(got.peak_private_mib, Some(2099));
        assert!(got.ended);
        assert!(!got.stale, "刚写的文件不算过期");
        assert_eq!(got.note, "", "r2 + 正常收尾不该报警：{}", got.note);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 旧编排器（`-r1`，写死 4 GiB 那一版）必须说出来，否则玩家以为面板在骗他。
    #[test]
    fn flags_the_old_orchestrator_policy() {
        let base = std::env::temp_dir().join(format!("dfo-mem-r1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        session_dir(&base, "roles_persist_old", "MEMORY_GUARD active pid=5528 scope=gateway hard_commit_mib=4096 low_memory_mib=2048 system_pressure_action=warn_only policy=local-stability-20261008-r1\nMEMORY_GUARD sample scope=gateway seconds=2.0 private_mib=88 working_mib=64 system_available_mib=17277\n");
        let got = live(&root_at(&base)).expect("该读出实况");
        assert_eq!(got.hard_limit_mib, Some(4096));
        assert!(got.note.contains("旧编排器"), "{}", got.note);
        assert_eq!(got.peak_private_mib, None, "没收尾就没有峰值");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 读不到 active 行就当没有这回事：宁可不显示，也不摆假数。
    #[test]
    fn no_guard_lines_means_no_readout() {
        let base = std::env::temp_dir().join(format!("dfo-mem-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        assert!(live(&root_at(&base)).is_none(), "runtime 目录都没有");
        session_dir(&base, "roles_persist_plain", "gateway starting\nlisten 7001\n");
        assert!(live(&root_at(&base)).is_none(), "没有 MEMORY_GUARD active 行");
        // 只有 active、一条采样都还没有：也读不出占用，但这一场是活的。
        session_dir(&base, "roles_persist_fresh", ACTIVE_R2);
        assert!(live(&root_at(&base)).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 目录 mtime 只记建目录那一刻，跑着的那场 err 才是活的：必须按 err 的 mtime 挑。
    #[test]
    fn picks_the_session_still_writing() {
        let base = std::env::temp_dir().join(format!("dfo-mem-newest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let older = session_dir(&base, "roles_persist_older", &format!("{ACTIVE_R2}{SAMPLE_1}"));
        std::thread::sleep(std::time::Duration::from_millis(60));
        session_dir(&base, "roles_persist_newer", &format!("{ACTIVE_R2}{SAMPLE_1}"));
        // 往「旧」那场再写一条：它的 err 现在最新，目录却仍是新那场更晚。
        std::thread::sleep(std::time::Duration::from_millis(60));
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(older.join("gateway.err"))
            .expect("追加");
        use std::io::Write;
        file.write_all(SAMPLE_2.as_bytes()).expect("追加采样");
        drop(file);
        let got = live(&root_at(&base)).expect("该读出实况");
        assert_eq!(got.session, "roles_persist_older");
        assert_eq!(got.private_mib, 1430);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 小曲线只留最近一段，条数仍然报全部（用来分辨「刚起来」和「读坏了」）。
    #[test]
    fn caps_the_series_but_counts_every_sample() {
        let base = std::env::temp_dir().join(format!("dfo-mem-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let mut body = String::from(ACTIVE_R2);
        for i in 0..200 {
            body.push_str(&format!(
                "MEMORY_GUARD sample scope=gateway seconds={i}.0 private_mib={} working_mib=1 system_available_mib=1\n",
                100 + i
            ));
        }
        session_dir(&base, "roles_persist_long", &body);
        let got = live(&root_at(&base)).expect("该读出实况");
        assert_eq!(got.samples, 200);
        assert_eq!(got.series.len(), SERIES_MAX);
        assert_eq!(got.series.last().copied(), Some(299));
        assert_eq!(got.series.first().copied(), Some(299 - (SERIES_MAX as u64 - 1)));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 字段顺序变了也不该错位：行里插一个新字段。
    #[test]
    fn parses_by_key_not_by_position() {
        let base = std::env::temp_dir().join(format!("dfo-mem-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        session_dir(&base, "roles_persist_future", "MEMORY_GUARD active scope=gateway pid=4242 policy=local-stability-20261008-r2 hard_commit_mib=6144 low_memory_mib=2048 brand_new_field=hello\nMEMORY_GUARD sample working_mib=77 scope=gateway private_mib=88 seconds=4.0 system_available_mib=999 brand_new_field=hello\n");
        let got = live(&root_at(&base)).expect("该读出实况");
        assert_eq!(got.pid, 4242);
        assert_eq!(got.hard_limit_mib, Some(6144));
        assert_eq!(got.private_mib, 88);
        assert_eq!(got.working_mib, 77);
        assert_eq!(got.note, "");
        let _ = std::fs::remove_dir_all(&base);
    }
}
