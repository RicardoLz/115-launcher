//! 自动报错：把启动器与游戏侧的真实故障送到社区反馈站，网络不通时先在本地排队。
//!
//! 三条不可让步的规矩：
//! - 玩家没勾「自动上报」之前，一个字节都不发出去（接口侧同样强制 `consent=true`）。
//! - 查询凭证 `receipt` 只写私有文件，绝不进运行日志、绝不进 URL 或界面公开区。
//! - 出网前先脱敏：用户目录、登录账号、看着像令牌/密码的字样一律抹掉。
//!
//! 传输走系统自带的 `curl.exe`：反馈站的证书是 Let's Encrypt 签给 IP 的，七天一轮换，
//! 所以只能按系统根证书正常校验（实测通过），不能固定公钥。

use std::collections::HashMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config;
use crate::logbus::Bus;
use crate::paths::Root;
use crate::procutil;
use crate::site::SITE;

const ENDPOINT: &str = "/api/launcher/reports";

/// 队列上限：磁盘上最多攒这么多条，超了丢最老的。
const MAX_QUEUE: usize = 40;
/// 私有回执留档条数。
const MAX_SENT: usize = 200;
/// 一条报告最多重试几次就放弃（约 30 秒 → 6 小时，够跨过临时故障与限流窗口）。
const MAX_ATTEMPTS: u32 = 8;
/// 同一个异常在这个窗口内只报一次。
const DEDUPE: Duration = Duration::from_secs(600);
const FLUSH_TICK: Duration = Duration::from_secs(30);
const HTTP_TIMEOUT: Duration = Duration::from_secs(90);

/// 一次故障事件。字段对齐接口协议的 `LauncherSubmission`。
pub struct Event {
    pub stage: &'static str,
    pub title: String,
    pub error_code: Option<String>,
    pub actual: String,
    pub steps: String,
    pub expected: String,
    pub severity: &'static str,
    pub frequency: &'static str,
    /// 需要随报告带上去的本地日志（文件名, 路径）。
    pub logs: Vec<(String, PathBuf)>,
}

impl Event {
    pub fn new(stage: &'static str, title: &str, actual: &str) -> Self {
        Event {
            stage,
            title: title.to_string(),
            error_code: None,
            actual: actual.to_string(),
            steps: String::new(),
            expected: String::new(),
            severity: "影响正常游玩",
            frequency: "只遇到一次",
            logs: Vec::new(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Queued {
    payload: Value,
    attempts: u32,
    next_at: u64,
    created: String,
}

/// 一条已受理的报告。`receipt` 是私有凭证，只存在 `sent.json` 里。
#[derive(Clone, Serialize, Deserialize)]
pub struct Sent {
    pub stage: String,
    pub title: String,
    pub id: String,
    pub receipt: String,
    pub issue_id: String,
    pub duplicate: bool,
    pub status: String,
    pub category: String,
    pub fix_version: String,
    pub public_note: String,
    pub report_count: i64,
    pub occurred: String,
    pub sent_at: String,
}

enum PostError {
    /// 网络/限流/服务不可用：留在队列里退避重试。
    Retry(String),
    /// 服务端明确拒绝：重试只会再拒绝一次。
    Fatal(String),
}

pub struct Reporter {
    dir: PathBuf,
    bus: Arc<Bus>,
    root: Root,
    recent: Mutex<HashMap<String, SystemTime>>,
    busy: Mutex<()>,
    wake: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn stamp() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

impl Reporter {
    pub fn new(app_dir: &Path, root: Root, bus: Arc<Bus>) -> Arc<Self> {
        let dir = app_dir.join("reports");
        let _ = fs::create_dir_all(dir.join("queue"));
        let _ = fs::create_dir_all(dir.join("dropped"));
        Arc::new(Reporter {
            dir,
            bus,
            root,
            recent: Mutex::new(HashMap::new()),
            busy: Mutex::new(()),
            wake: Mutex::new(None),
        })
    }

    /// 后台常驻：被唤醒或有定时钟就把到期的报告送一遍。
    pub fn start(self: &Arc<Self>) {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        *locked(&self.wake) = Some(tx);
        let me = self.clone();
        std::thread::spawn(move || {
            // 上次退出前攒下的队列，开机就先补一轮，不等满一个时钟周期。
            me.flush();
            loop {
                let _ = rx.recv_timeout(FLUSH_TICK);
                me.flush();
            }
        });
    }

    /// 玩家插上网线后手动催一次：把所有条目的到期时间挪到当下。
    pub fn flush_now(&self) {
        for path in queue_files(&self.dir) {
            if let Some(mut item) = read_queued(&path) {
                item.next_at = 0;
                let _ = write_json(&path, &item);
            }
        }
        self.nudge();
    }

    fn nudge(&self) {
        if let Some(tx) = locked(&self.wake).as_ref() {
            let _ = tx.send(());
        }
    }

    pub fn enabled(&self) -> bool {
        config::settings(&self.root).auto_report
    }

    pub fn pending(&self) -> usize {
        queue_files(&self.dir).len()
    }

    /// 攒下的（编号, 查询凭证）对，最多 50 条：「我的反馈」拿它去站上换最新状态。
    pub fn receipt_pairs(&self) -> Vec<(String, String)> {
        read_sent(&self.dir)
            .into_iter()
            .filter(|item| !item.id.is_empty() && !item.receipt.is_empty())
            .rev()
            .take(50)
            .map(|item| (item.id, item.receipt))
            .collect()
    }

    /// 玩家在启动器的反馈页里自己提交成功的那一条：凭证同样只落私有文件，日志里只出现编号。
    pub fn save_site_receipt(&self, id: &str, receipt: &str, issue_id: &str) -> Result<(), String> {
        let record = Sent {
            stage: "manual_submit".to_string(),
            title: "在启动器里提交".to_string(),
            id: id.to_string(),
            receipt: receipt.to_string(),
            issue_id: issue_id.to_string(),
            duplicate: false,
            status: String::new(),
            category: String::new(),
            fix_version: String::new(),
            public_note: String::new(),
            report_count: 0,
            occurred: stamp(),
            sent_at: stamp(),
        };
        push_sent(&self.dir, &record)?;
        self.bus.ok(format!(
            "已保存这条反馈的查询凭证（{id}），换设备时能用它找回"
        ));
        Ok(())
    }

    /// 记一条故障。没开自动上报也照样入队，只是绝不发出去（flush 自己会把关）：
    /// 玩家后来改主意时，之前那几次现场还在。
    pub fn report(&self, ev: Event) {
        let sending = self.enabled();
        let fingerprint = format!(
            "{}|{:?}|{}",
            ev.stage,
            ev.error_code,
            chars(&ev.actual, 160)
        );
        if self.seen_recently(&fingerprint) {
            self.bus.info("同类故障刚记过一次，这条按频率限制并掉了");
            return;
        }
        let payload = match self.build(&ev) {
            Ok(payload) => payload,
            Err(err) => {
                self.bus.warn(format!("报错载荷组装失败：{err}"));
                return;
            }
        };
        let key = payload["request_key"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let item = Queued {
            payload,
            attempts: 0,
            next_at: now_secs(),
            created: stamp(),
        };
        let path = self.dir.join("queue").join(format!("{key}.json"));
        if let Err(err) = write_json(&path, &item) {
            self.bus.warn(format!("故障报告没能入队：{err}"));
            return;
        }
        trim_dir(&self.dir.join("queue"), MAX_QUEUE);
        self.bus.info(format!(
            "故障已记入{}（{key}，{}）",
            if sending {
                "上报队列"
            } else {
                "本机待报队列"
            },
            ev.stage
        ));
        if sending {
            self.nudge();
        }
    }

    fn seen_recently(&self, fingerprint: &str) -> bool {
        let mut map = locked(&self.recent);
        let now = SystemTime::now();
        map.retain(|_, seen| {
            now.duration_since(*seen)
                .map(|d| d < DEDUPE)
                .unwrap_or(false)
        });
        if map.contains_key(fingerprint) {
            return true;
        }
        map.insert(fingerprint.to_string(), now);
        false
    }

    fn build(&self, ev: &Event) -> Result<Value, String> {
        let version = client_version(&self.root);
        let mut actual = redact(&self.root, &chars(&strip_ansi(&ev.actual), 4000));
        if actual.trim().is_empty() {
            actual = format!("{}（未取到具体错误文本）", ev.title);
        }
        let steps = if ev.steps.trim().is_empty() {
            default_steps(ev.stage)
        } else {
            redact(&self.root, &chars(&ev.steps, 4000))
        };
        let expected = if ev.expected.trim().is_empty() {
            "游戏正常进入并可玩到结束".to_string()
        } else {
            redact(&self.root, &chars(&ev.expected, 2000))
        };
        let title = redact(&self.root, &chars(&ev.title, 100));
        let title = if title.chars().count() < 4 {
            format!("{}（启动器自动上报）", ev.stage)
        } else {
            title
        };

        let mut payload = json!({
            "request_key": request_key(ev.stage),
            "stage": ev.stage,
            "version": version,
            "launcher_version": crate::commands::APP_VERSION,
            "consent": true,
            "title": title,
            "severity": ev.severity,
            "frequency": ev.frequency,
            "occurred": stamp(),
            "environment": environment(),
            "steps": chars(&steps, 4000),
            "expected": chars(&expected, 2000),
            "actual": actual,
            "error_message": actual,
            "files": [],
        });
        if let Some(code) = &ev.error_code {
            payload["error_code"] = json!(chars(code, 80));
        }
        // 角色名不传：启动器拿不到真实角色名（存档在服务端 sqlite 里），
        // 网站对缺省的字段自己填「启动前 / 未获取角色」，比编一个值诚实。
        let files = attachments(&self.root, ev);
        if !files.is_empty() {
            payload["files"] = json!(files);
        }
        Ok(payload)
    }

    /// 把到期条目逐条送出去。同一时刻只允许一个发送者，靠队列目录本身当锁。
    fn flush(&self) {
        if !self.enabled() {
            return;
        }
        // 定时线程和手动催发会撞车：同一时刻只允许一个发送者。
        let _sending = match self.busy.try_lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let due = now_secs();
        for path in queue_files(&self.dir) {
            let Some(mut item) = read_queued(&path) else {
                let _ = fs::remove_file(&path);
                continue;
            };
            if item.next_at > due {
                continue;
            }
            match self.post(&item.payload) {
                Ok(body) => {
                    self.archive(&item, &body);
                    let _ = fs::remove_file(&path);
                }
                Err(PostError::Fatal(err)) => {
                    self.bus
                        .warn(format!("反馈站拒绝了这条报告，不再重试：{err}"));
                    self.stash(&path);
                }
                Err(PostError::Retry(err)) => {
                    item.attempts += 1;
                    if item.attempts >= MAX_ATTEMPTS {
                        self.bus.warn(format!(
                            "这条报告重试 {MAX_ATTEMPTS} 次仍未成功，先搁置在本机"
                        ));
                        self.stash(&path);
                        continue;
                    }
                    item.next_at = now_secs() + backoff(item.attempts);
                    let _ = write_json(&path, &item);
                    self.bus
                        .info(format!("上报暂时没成功（{err}），已留在本地队列"));
                }
            }
        }
    }

    /// 不再重试的条目挪到 dropped/，留在本机供人工排查，不再占队列。
    fn stash(&self, path: &Path) {
        let name = match path.file_name() {
            Some(name) => name,
            None => {
                let _ = fs::remove_file(path);
                return;
            }
        };
        if fs::rename(path, self.dir.join("dropped").join(name)).is_err() {
            let _ = fs::remove_file(path);
        }
    }

    fn post(&self, payload: &Value) -> Result<Value, PostError> {
        let tmp = self.dir.join(format!("out-{}.json", std::process::id()));
        let resp = self.dir.join(format!("resp-{}.json", std::process::id()));
        let _ = fs::remove_file(&resp);
        fs::write(&tmp, payload.to_string().as_bytes())
            .map_err(|err| PostError::Retry(format!("写请求体失败：{err}")))?;
        let mut cmd = procutil::command(&curl_exe());
        cmd.args([
            "-s",
            "--max-time",
            &HTTP_TIMEOUT.as_secs().to_string(),
            "-o",
            &resp.to_string_lossy(),
            "-w",
            "STATUS=%{http_code}",
            "-H",
            "Content-Type: application/json; charset=utf-8",
            "--data-binary",
            &format!("@{}", tmp.display()),
            &format!("{SITE}{ENDPOINT}"),
        ])
        .stdin(Stdio::null());
        let out = procutil::run_capture_with(cmd, HTTP_TIMEOUT + Duration::from_secs(5))
            .map_err(PostError::Retry)?;
        let _ = fs::remove_file(&tmp);
        let code = out
            .rsplit("STATUS=")
            .next()
            .and_then(|tail| tail.trim().parse::<u16>().ok())
            .unwrap_or(0);
        let body = fs::read_to_string(&resp).unwrap_or_default();
        let _ = fs::remove_file(&resp);
        let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let server_error = parsed
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        match code {
            200 if parsed.get("id").and_then(Value::as_str).is_some() => Ok(parsed),
            200 => Err(PostError::Fatal(if_empty(
                server_error,
                "返回里没有反馈编号",
            ))),
            400 | 403 | 404 | 405 | 413 | 415 => Err(PostError::Fatal(if_empty(
                server_error,
                &format!("HTTP {code}"),
            ))),
            429 => Err(PostError::Retry(if_empty(server_error, "HTTP 429 限流"))),
            0 => Err(PostError::Retry(if_empty(server_error, "连不上反馈站"))),
            other if other >= 500 => Err(PostError::Retry(format!("HTTP {other}"))),
            other => Err(PostError::Fatal(format!("HTTP {other}"))),
        }
    }

    /// 先私下存回执，再让队列项消失 —— 顺序反了就会丢凭证。
    fn archive(&self, item: &Queued, body: &Value) {
        let stage = item.payload["stage"].as_str().unwrap_or("").to_string();
        let title = item.payload["title"].as_str().unwrap_or("").to_string();
        let issue = body.get("issue").cloned().unwrap_or(Value::Null);
        let record = Sent {
            stage,
            title,
            id: text(&body, "id"),
            receipt: text(&body, "receipt"),
            issue_id: text(&body, "issue_id"),
            duplicate: body["duplicate"].as_bool().unwrap_or(false),
            status: text(&issue, "status"),
            category: text(&issue, "category"),
            fix_version: text(&issue, "fix_version"),
            public_note: chars(&text(&issue, "public_note"), 400),
            report_count: issue["report_count"].as_i64().unwrap_or(0),
            occurred: item.payload["occurred"].as_str().unwrap_or("").to_string(),
            sent_at: stamp(),
        };
        if let Err(err) = push_sent(&self.dir, &record) {
            // 存不下就别删队列项，下次重试还会命中同一个 request_key。
            self.bus
                .warn(format!("回执保存失败，这条报告留在队列：{err}"));
            return;
        }
        let merged = if record.duplicate {
            format!("已并入已有问题 {}", record.issue_id)
        } else {
            format!("已新建问题 {}", record.issue_id)
        };
        self.bus
            .ok(format!("故障已上报：{}（{merged}）", record.id));
    }
}

// ------------------------------------------------------------------ 载荷取材

/// 客户端版本号：优先取 DFO.exe 的文件版本，取不到就用它的构建日期，两者都是真实值。
pub fn client_version(root: &Root) -> String {
    let exe = config::client_dir(root).join(crate::paths::CLIENT_IMAGE_NAME);
    if let Some(found) = file_version(&exe) {
        return chars(&found, 80);
    }
    let mtime = fs::metadata(&exe)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| {
            DateTime::<Local>::from(t)
                .format("%Y%m%d")
                .to_string()
                .into()
        });
    mtime.unwrap_or_else(|| "未知版本".to_string())
}

#[cfg(windows)]
fn file_version(path: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };

    if !path.is_file() {
        return None;
    }
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let name = PCWSTR::from_raw(wide.as_ptr());
    unsafe {
        let size = GetFileVersionInfoSizeW(name, None);
        if size == 0 {
            return None;
        }
        let mut block = vec![0u8; size as usize];
        GetFileVersionInfoW(name, 0, size, block.as_mut_ptr() as *mut _).ok()?;
        let mut lang: *const u8 = std::ptr::null();
        let mut lang_len = 0u32;
        let key: Vec<u16> = "\\VarFileInfo\\Translation"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        VerQueryValueW(
            block.as_ptr() as *const _,
            PCWSTR::from_raw(key.as_ptr()),
            &mut lang as *mut *const u8 as *mut *mut _,
            &mut lang_len,
        )
        .ok()
        .ok()?;
        if lang.is_null() || lang_len < 4 {
            return None;
        }
        let pair = std::slice::from_raw_parts(lang, 4);
        let sub: Vec<u16> = format!(
            "\\StringFileInfo\\{:04x}{:04x}\\FileVersion",
            u16::from_le_bytes([pair[0], pair[1]]),
            u16::from_le_bytes([pair[2], pair[3]])
        )
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
        let mut value: *const u16 = std::ptr::null();
        let mut value_len = 0u32;
        VerQueryValueW(
            block.as_ptr() as *const _,
            PCWSTR::from_raw(sub.as_ptr()),
            &mut value as *mut *const u16 as *mut *mut _,
            &mut value_len,
        )
        .ok()
        .ok()?;
        if value.is_null() || value_len == 0 {
            return None;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(
            value,
            (value_len as usize).saturating_sub(1),
        ));
        Some(text.trim().to_string())
    }
}

#[cfg(not(windows))]
fn file_version(_path: &Path) -> Option<String> {
    None
}

/// 一句话把系统与架构说清楚，够排查用，也不带任何个人标识。
pub fn environment() -> String {
    format!("{} · {}", windows_name(), std::env::consts::ARCH)
}

/// 系统名读注册表的 CurrentVersion（一次 `reg query` 拿全三个值，结果常驻）。
fn windows_name() -> String {
    static CACHED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CACHED
        .get_or_init(|| {
            let mut found = Vec::new();
            if let Ok(out) = procutil::run_capture(
                &Path::new(&std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()))
                    .join("System32")
                    .join("reg.exe"),
                &[
                    "query",
                    r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion",
                ],
                Duration::from_secs(5),
            ) {
                for name in ["ProductName", "DisplayVersion", "CurrentBuildNumber"] {
                    if let Some(value) = reg_value(&out, name) {
                        found.push(value);
                    }
                }
            }
            if found.is_empty() {
                return "Windows".to_string();
            }
            chars(&found.join(" "), 480)
        })
        .clone()
}

/// `reg query` 的一行长这样：`    ProductName    REG_SZ    Windows 10 Pro`。
fn reg_value(out: &str, name: &str) -> Option<String> {
    let line = out
        .lines()
        .find(|line| line.trim_start().starts_with(name))?;
    let after = line.rsplit_once("REG_")?.1;
    // 类型名（SZ / DWORD 之类）后面到值之间全是空白，字母一停就说明类型读完了。
    let value = after
        .trim_start_matches(|c: char| c.is_ascii_alphabetic())
        .trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn default_steps(stage: &str) -> String {
    match stage {
        "game_start_failed" => "在启动器首页点「开始游戏」，环境编排走到该阶段时失败。".to_string(),
        "game_crash" => "在启动器首页点「开始游戏」，进入游戏后客户端进程异常结束。".to_string(),
        "launcher_exception" => "正常使用启动器过程中出现，未做其它操作。".to_string(),
        other => format!("启动器在 {other} 阶段失败。"),
    }
}

pub(crate) fn curl_exe() -> PathBuf {
    std::env::var("SystemRoot")
        .map(|root| Path::new(&root).join("System32").join("curl.exe"))
        .unwrap_or_else(|_| PathBuf::from("curl.exe"))
}

pub fn request_key(stage: &str) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mut x = now.as_nanos() as u64
        ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ ((std::process::id() as u64) << 17);
    // splitmix64：没有随机数依赖时也要保证跨机器不撞号。
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    let slug = stage.replace('_', "");
    format!("lr-{}-{:013}-{:08x}", chars(&slug, 18), now.as_millis(), z)
}

fn backoff(attempts: u32) -> u64 {
    match attempts {
        0..=1 => 30,
        2 => 120,
        3 => 600,
        4 => 1800,
        _ => 3600,
    }
}

// ------------------------------------------------------------------ 脱敏与附件

/// 上传前把个人标识抹掉：整条用户目录、登录账号、以及看着像凭据的赋值。
fn redact(root: &Root, text: &str) -> String {
    let mut out = text.to_string();
    for needle in private_needles(root) {
        if needle.is_empty() {
            continue;
        }
        out = out.replace(&needle, "<本机路径>");
    }
    let account = config::settings(root).account;
    if !account.trim().is_empty() {
        out = out.replace(&account, "<账号>");
    }
    // 没有正则依赖：按行找关键字，把等号/冒号后面整段换掉。
    let flagged: Vec<String> = out
        .lines()
        .map(|line| {
            let lower = line.to_lowercase();
            if ["token", "password", "passwd", "secret", "cookie", "验证码"]
                .iter()
                .any(|k| lower.contains(k))
            {
                if let Some(cut) = line.find(|c| c == '=' || c == ':') {
                    return format!("{}：<已隐去>", &line[..cut]);
                }
            }
            line.to_string()
        })
        .collect();
    flagged.join("\n")
}

fn private_needles(root: &Root) -> Vec<String> {
    vec![
        std::env::var("USERPROFILE").unwrap_or_default(),
        std::env::var("LOCALAPPDATA").unwrap_or_default(),
        std::env::var("APPDATA").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default(),
        root.path.to_string_lossy().to_string(),
    ]
}

/// 只带尾部若干行，且总长压到 200 KB 以内。
fn attachments(root: &Root, ev: &Event) -> Vec<Value> {
    let mut out = Vec::new();
    for (name, path) in ev.logs.iter().take(6) {
        let Some(bytes) = tail_bytes(path, 200_000) else {
            continue;
        };
        if bytes.contains(&0) {
            // 接口不收含 NUL 的二进制；UTF-16 日志也不在这几条里，直接放弃附件。
            continue;
        }
        let text = String::from_utf8_lossy(&bytes).to_string();
        let clean = redact(root, &strip_ansi(&text));
        out.push(json!({ "name": chars(name, 80), "data": base64(clean.as_bytes()) }));
    }
    out
}

fn tail_bytes(path: &Path, limit: usize) -> Option<Vec<u8>> {
    let raw = fs::read(path).ok()?;
    if raw.is_empty() {
        return None;
    }
    let start = raw.len().saturating_sub(limit);
    Some(raw[start..].to_vec())
}

// ------------------------------------------------------------------ 盘上小工具

fn queue_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir.join("queue")) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    out.sort();
    out
}

fn read_queued(path: &Path) -> Option<Queued> {
    let raw = fs::read(path).ok()?;
    serde_json::from_slice(&raw).ok()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("partial");
    let mut file = fs::File::create(&tmp).map_err(|err| format!("{err}"))?;
    file.write_all(serde_json::to_string(value).unwrap_or_default().as_bytes())
        .map_err(|err| format!("{err}"))?;
    drop(file);
    fs::rename(&tmp, path).map_err(|err| format!("{err}"))
}

fn trim_dir(dir: &Path, max: usize) {
    let mut files = queue_files_in(dir);
    if files.len() <= max {
        return;
    }
    files.sort_by_key(|path| {
        fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(UNIX_EPOCH)
    });
    let drop_count = files.len() - max;
    for path in files.into_iter().take(drop_count) {
        let _ = fs::remove_file(path);
    }
}

fn queue_files_in(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect()
        })
        .unwrap_or_default()
}

fn sent_path(dir: &Path) -> PathBuf {
    dir.join("sent.json")
}

fn read_sent(dir: &Path) -> Vec<Sent> {
    fs::read(sent_path(dir))
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

fn push_sent(dir: &Path, record: &Sent) -> Result<(), String> {
    let mut items = read_sent(dir);
    items.retain(|item| item.id != record.id);
    items.push(record.clone());
    if items.len() > MAX_SENT {
        let cut = items.len() - MAX_SENT;
        items.drain(..cut);
    }
    write_json(&sent_path(dir), &items)
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn if_empty(text: String, fallback: &str) -> String {
    if text.trim().is_empty() {
        fallback.to_string()
    } else {
        text
    }
}

fn chars(text: &str, max: usize) -> String {
    let taken: String = text.chars().take(max).collect();
    taken
}

fn strip_ansi(text: &str) -> String {
    if !text.contains('\x1b') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            out.push(ch);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() || next == '~' {
                    break;
                }
            }
        }
    }
    out
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(TABLE[(n >> 18 & 63) as usize] as char);
        out.push(TABLE[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
