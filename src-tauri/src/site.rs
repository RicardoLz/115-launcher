//! 反馈站的公开接口：看板、单条问题、我的反馈、提交问题，全从这里走。
//!
//! 启动器不再开第二扇窗去嵌网页 —— 站的 CSP 写着 `frame-ancestors 'none'`，本来就嵌不进来；
//! 原生界面只读这几个接口，反而比宿主桥少一整层能出问题的东西。
//!
//! 传输一律走系统 curl：证书是 Let's Encrypt 签的，系统根证书能验，所以不加 `-k`，
//! 也不为此引一个新 TLS 依赖。玩家自己写的正文只进临时文件、由 curl 自己读
//! （`--data-binary @文件`），绝不拼进命令行或 URL 查询串。

use std::collections::HashMap;
use std::fs;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::paths::Root;
use crate::procutil;
use crate::report::{self, curl_exe};

pub const SITE: &str = "https://134.175.57.52";

/// 看板是人在盯着等的，不该套用后台队列那 90 秒。
const TIMEOUT: Duration = Duration::from_secs(45);

/// 以下四组与站上的 board.CATEGORIES / STATUSES / SEVERITIES / FREQUENCIES 一一对应，
/// 界面直接用这里的列表画选项，免得两边各写一份再对不上。
pub const CATEGORIES: [&str; 8] = [
    "启动器与更新",
    "登录与连接",
    "副本与任务",
    "装备与道具",
    "技能与战斗",
    "时装与显示",
    "组队与社交",
    "其他问题",
];
pub const STATUSES: [&str; 5] = ["待处理", "排查中", "待补充", "已修复", "暂不处理"];
pub const SEVERITIES: [&str; 3] = ["一般问题", "影响正常游玩", "无法继续游戏"];
pub const FREQUENCIES: [&str; 3] = ["每次出现", "偶尔出现", "只遇到一次"];

/// 角色名是必填项，但人在登录之前就崩了，所以留一个和站上一致的说法。
const NO_CHARACTER: &str = "启动前 / 未获取角色";

#[derive(Serialize, Deserialize)]
pub struct Issue {
    pub id: String,
    pub title: String,
    pub category: String,
    pub status: String,
    #[serde(default)]
    pub public_note: String,
    #[serde(default)]
    pub fix_version: String,
    /// unix 秒。站上是 REAL，界面按东八区格式化，和网页看到的一模一样。
    #[serde(default)]
    pub created: f64,
    #[serde(default)]
    pub updated: f64,
    #[serde(default)]
    pub report_count: i64,
    #[serde(default)]
    pub duplicate_count: i64,
    #[serde(default)]
    pub updates: Vec<Update>,
}

#[derive(Serialize, Deserialize)]
pub struct Update {
    #[serde(default)]
    pub created: f64,
    pub status: String,
    pub message: String,
    #[serde(default)]
    pub fix_version: String,
}

/// 一个键值计数对，按站上的固定顺序排好，界面直接画成筛选条。
#[derive(Serialize)]
pub struct Facet {
    pub key: String,
    pub count: i64,
}

#[derive(Serialize)]
pub struct Board {
    pub issues: Vec<Issue>,
    pub count: i64,
    pub page: i64,
    pub report_count: i64,
    pub status_counts: Vec<Facet>,
    pub category_counts: Vec<Facet>,
}

#[derive(Serialize, Deserialize)]
pub struct Mine {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub created: f64,
    pub status: String,
    #[serde(default)]
    pub issue: Option<Issue>,
}

/// 提交表单原样搬过来的东西。凭证、版本、环境这些不由玩家填，服务端自己补。
#[derive(Deserialize)]
pub struct Submission {
    pub title: String,
    pub character: String,
    pub steps: String,
    pub expected: String,
    pub actual: String,
    /// `2026-10-14T13:45` 这种本地时间，留空就按当下算。
    pub occurred: String,
    pub category: String,
    pub severity: String,
    pub frequency: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub contact: String,
    #[serde(default)]
    pub related_issue_id: String,
}

/// 站上给回来的全部东西，包括查询凭证。凭证只准在 Rust 侧活着。
#[derive(Deserialize)]
pub struct Submitted {
    pub id: String,
    pub receipt: String,
    #[serde(default)]
    pub duplicate: bool,
    #[serde(default)]
    pub issue: Option<Issue>,
}

/// 交给界面的那一份：没有 receipt。
#[derive(Serialize)]
pub struct SubmitOutcome {
    pub id: String,
    pub duplicate: bool,
    pub issue: Option<Issue>,
}

impl From<Submitted> for SubmitOutcome {
    fn from(value: Submitted) -> Self {
        SubmitOutcome {
            id: value.id,
            duplicate: value.duplicate,
            issue: value.issue,
        }
    }
}

pub fn board(
    page: u32,
    category: &str,
    status: &str,
    q: &str,
    sort: &str,
) -> Result<Board, String> {
    let page = page.to_string();
    let raw = get(
        "/api/issues",
        &[
            ("page", &page),
            ("category", category),
            ("status", status),
            ("q", q),
            ("sort", sort),
        ],
    )?;
    let issues = take_list(&raw, "issues");
    Ok(Board {
        count: int_of(&raw, "count"),
        page: int_of(&raw, "page"),
        report_count: int_of(&raw, "report_count"),
        status_counts: facets(&map_of(&raw, "counts"), &STATUSES),
        category_counts: facets(&map_of(&raw, "categories"), &CATEGORIES),
        issues: issues
            .into_iter()
            .filter_map(|item| serde_json::from_value(item).ok())
            .collect(),
    })
}

pub fn issue(id: &str) -> Result<Issue, String> {
    let parsed = get("/api/issue", &[("id", id)])?;
    serde_json::from_value(parsed).map_err(|_| "这条问题的返回看不明白".to_string())
}

/// 拿本机存下的（编号, 凭证）去换状态。空列表就不开口，省一次无谓的请求。
pub fn my_reports(items: &[(String, String)]) -> Result<Vec<Mine>, String> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let payload = json!({
        "reports": items.iter().map(|(id, receipt)| json!({"id": id, "receipt": receipt})).collect::<Vec<_>>()
    });
    let parsed = post("/api/my-reports", &payload)?;
    Ok(take_list(&parsed, "reports")
        .into_iter()
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect())
}

pub fn submit(input: Submission, root: &Root) -> Result<Submitted, String> {
    let title = check(&input.title, "问题标题", 4, 100)?;
    let character = if input.character.trim().is_empty() {
        NO_CHARACTER.to_string()
    } else {
        check(&input.character, "角色名", 1, 40)?
    };
    let steps = check(&input.steps, "复现步骤", 8, 4000)?;
    let expected = check(&input.expected, "期望结果", 2, 2000)?;
    let actual = check(&input.actual, "实际结果", 2, 4000)?;
    let occurred = occurred_at(&input.occurred)?;
    let payload = json!({
        "request_key": report::request_key("manual"),
        "consent": true,
        "source": "launcher",
        "title": title,
        "character": character,
        "steps": steps,
        "expected": expected,
        "actual": actual,
        "occurred": occurred,
        "category": one_of(&input.category, &CATEGORIES, "问题类型")?,
        "severity": one_of(&input.severity, &SEVERITIES, "影响程度")?,
        "frequency": one_of(&input.frequency, &FREQUENCIES, "出现频率")?,
        "location": clipped(&input.location, 100),
        "contact": clipped(&input.contact, 100),
        "related_issue_id": clipped(&input.related_issue_id, 40),
        "version": report::client_version(root),
        "environment": report::environment(),
        "launcher_version": env!("CARGO_PKG_VERSION"),
    });
    let parsed = post("/api/report", &payload)?;
    serde_json::from_value(parsed).map_err(|_| "提交成功了，但没读到反馈编号".to_string())
}

fn get(path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    let mut url = format!("{SITE}{path}");
    let joined: Vec<String> = query
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| format!("{key}={}", percent_encode(value)))
        .collect();
    if !joined.is_empty() {
        url.push('?');
        url.push_str(&joined.join("&"));
    }
    call(&url, None)
}

fn post(path: &str, payload: &Value) -> Result<Value, String> {
    call(&format!("{SITE}{path}"), Some(payload))
}

fn call(url: &str, payload: Option<&Value>) -> Result<Value, String> {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let tag = (std::process::id() as u64) << 20 | SEQ.fetch_add(1, Ordering::Relaxed);
    let body_file = std::env::temp_dir().join(format!("dfo115us-site-{tag}.json"));
    let out_file = std::env::temp_dir().join(format!("dfo115us-site-out-{tag}.json"));
    let _ = fs::remove_file(&out_file);

    let mut cmd = procutil::command(&curl_exe());
    cmd.args([
        "-s",
        "--max-time",
        &TIMEOUT.as_secs().to_string(),
        "-o",
        &out_file.to_string_lossy(),
        "-w",
        "STATUS=%{http_code}",
        "-H",
        "Content-Type: application/json; charset=utf-8",
    ]);
    if let Some(body) = payload {
        fs::write(&body_file, body.to_string().as_bytes())
            .map_err(|err| format!("写请求体失败：{err}"))?;
        cmd.args(["--data-binary", &format!("@{}", body_file.display())]);
    }
    cmd.arg(url).stdin(Stdio::null());

    let captured = procutil::run_capture_with(cmd, TIMEOUT + Duration::from_secs(5))
        .map_err(|err| format!("联系反馈站没成功：{err}"));
    let _ = fs::remove_file(&body_file);
    let captured = captured?;
    let status = captured
        .rsplit("STATUS=")
        .next()
        .and_then(|tail| tail.trim().parse::<u16>().ok())
        .unwrap_or(0);
    let text = fs::read_to_string(&out_file).unwrap_or_default();
    let _ = fs::remove_file(&out_file);
    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let note = parsed
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match status {
        200 if parsed.is_object() => Ok(parsed),
        200 => Err("反馈站的返回不是预期的 JSON".to_string()),
        // 限流和服务不可用是「等一会儿再来」，不是玩家写错了。
        429 => Err(or_note(&note, "反馈站正在限流，过几分钟再试")),
        0 => Err("连不上反馈站，检查一下网络再试".to_string()),
        other if other >= 500 => Err(or_note(&note, &format!("反馈站暂时不可用（HTTP {other}）"))),
        other => Err(or_note(
            &note,
            &format!("反馈站拒绝了这次请求（HTTP {other}）"),
        )),
    }
}

/// 只保留 URL 里的非保留字符，其余一律百分号编码（UTF-8 逐字节）。
fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn or_note(note: &str, fallback: &str) -> String {
    if note.trim().is_empty() {
        fallback.to_string()
    } else {
        note.to_string()
    }
}

fn take_list(parsed: &Value, key: &str) -> Vec<Value> {
    parsed
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn map_of(parsed: &Value, key: &str) -> HashMap<String, i64> {
    parsed
        .get(key)
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(name, value)| value.as_i64().map(|n| (name.clone(), n)))
                .collect()
        })
        .unwrap_or_default()
}

fn int_of(parsed: &Value, key: &str) -> i64 {
    parsed.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// 先按站上的固定顺序排，认不出来的键再按条数往后挂，免得界面漏掉一类。
fn facets(counts: &HashMap<String, i64>, order: &[&str]) -> Vec<Facet> {
    let mut out: Vec<Facet> = order
        .iter()
        .filter_map(|key| {
            counts.get(*key).map(|n| Facet {
                key: (*key).to_string(),
                count: *n,
            })
        })
        .collect();
    let mut rest: Vec<Facet> = counts
        .iter()
        .filter(|(key, _)| !order.contains(&key.as_str()))
        .map(|(key, n)| Facet {
            key: key.clone(),
            count: *n,
        })
        .collect();
    rest.sort_by(|a, b| b.count.cmp(&a.count));
    out.extend(rest);
    out
}

fn check(text: &str, name: &str, min: usize, max: usize) -> Result<String, String> {
    let trimmed = text.trim();
    let len = trimmed.chars().count();
    if len < min {
        return Err(format!("{name}至少要 {min} 个字"));
    }
    if len > max {
        return Err(format!("{name}最多 {max} 个字，现在写了 {len} 个"));
    }
    Ok(trimmed.to_string())
}

fn one_of<'a>(value: &'a str, choices: &[&'a str], name: &str) -> Result<&'a str, String> {
    let trimmed = value.trim();
    if choices.contains(&trimmed) {
        Ok(trimmed)
    } else {
        Err(format!("请从列表里选一个{name}"))
    }
}

fn clipped(text: &str, max: usize) -> String {
    text.trim().chars().take(max).collect()
}

/// 站上用 `datetime.fromisoformat` 校验，且把不带时区的值当东八区时间，
/// 所以这里只放行 `YYYY-MM-DDTHH:MM[:SS]`，留空就取当下。
fn occurred_at(text: &str) -> Result<String, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Local::now().format("%Y-%m-%dT%H:%M").to_string());
    }
    let ok = chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S"))
        .is_ok();
    if !ok {
        return Err("发生时间看起来不对，重新选一次".to_string());
    }
    Ok(clipped(trimmed, 40))
}
