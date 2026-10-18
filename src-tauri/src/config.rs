//! 三份盘上配置的读写：`server/launcher.local.json`、`launcher.settings.json`、
//! `runtime/storage/local.json`。
//!
//! 规矩：读的时候整份留着（`serde_json::Value`），只改自己那一个键，绝不把别人写的键丢掉；
//! 存档路由只在缺 `sqlite_path` 时落一份，绝不覆盖玩家手上的配置，也不预建空的 .sqlite3。

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::paths::{self, has_client_files, Root};

pub const DEFAULT_SERVER_BINARY: &str = "work/dfo-lan/bin/wireprobe-handoff-source.exe";
const DEFAULT_CLIENT_REL: &str = "../client";

pub(crate) fn read_json(path: &Path) -> Value {
    let Ok(raw) = std::fs::read(path) else {
        return Value::Object(Map::new());
    };
    let trimmed = raw.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&raw);
    serde_json::from_slice(trimmed).unwrap_or(Value::Object(Map::new()))
}

/// 原子写：先落 `.partial` 再改名，避免半份配置被下一个进程读到。
fn write_json(path: &Path, value: &Value, indent: usize) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("无法创建 {}：{}", parent.display(), err))?;
    }
    let mut rendered =
        serde_json::to_string_pretty(value).map_err(|err| err.to_string())?;
    if indent == 4 {
        rendered = widen_indent(&rendered);
    }
    rendered.push('\n');

    let tmp = path.with_extension(format!("json.partial-{}", std::process::id()));
    std::fs::write(&tmp, rendered).map_err(|err| format!("写入 {} 失败：{}", tmp.display(), err))?;
    std::fs::rename(&tmp, path).map_err(|err| format!("替换 {} 失败：{}", path.display(), err))
}

/// serde_json 的 pretty 只会两个空格；存档路由那份历史上是四个空格，得原样保住，
/// 否则整份文件每行都变，下次 diff 看不出是谁改的。
fn widen_indent(text: &str) -> String {
    text.lines()
        .map(|line| {
            let spaces = line.len() - line.trim_start_matches(' ').len();
            if spaces == 0 {
                line.to_string()
            } else {
                format!("{}{}", " ".repeat(spaces * 2), line.trim_start_matches(' '))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn section_mut<'a>(root: &'a mut Value, key: &str) -> &'a mut Map<String, Value> {
    if !root.is_object() {
        *root = Value::Object(Map::new());
    }
    let object = root.as_object_mut().expect("刚才已保证是对象");
    let entry = object
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    entry.as_object_mut().expect("刚才已保证是对象")
}

// ---------------------------------------------------------------- 客户端目录

/// 配置里写死的那份客户端目录。认不认由 `has_client_files` 判：写着空白、指向已经不在的
/// 目录、或指到一个不像客户端的地方，都算没配 —— 那种值继续生效只会让页面一片空白。
pub fn stored_client_dir(root: &Root) -> Option<PathBuf> {
    let value = read_json(&root.launcher_local_json());
    let configured = value
        .get("client_dir")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())?;
    let path = PathBuf::from(configured);
    let resolved = if path.is_absolute() {
        paths::squash(&path)
    } else {
        paths::squash(&root.path.join("server").join(configured))
    };
    has_client_files(&resolved).then_some(resolved)
}

/// 解析客户端目录（相对基准是 `<root>/server`）。
///
/// 配置不认时**必须**退回 `discover_client` 的候选，不能直接吐历史默认值：`DEFAULT_CLIENT_REL`
/// 是 `../client`，在拆栏布局（root=`服务端`、客户端在同级 `客户端\DFO`）下根本不存在，
/// 那样 MOD 页与健康检查会整块报「不在」，而游戏侧照样全加载。探不到才落回默认值，
/// 让页面显示「不在」的是真不在。兜底全程只读，不写配置。
pub fn client_dir(root: &Root) -> PathBuf {
    stored_client_dir(root).unwrap_or_else(|| {
        discover_client(root)
            .unwrap_or_else(|| paths::squash(&root.path.join("server").join(DEFAULT_CLIENT_REL)))
    })
}

pub fn set_client_dir(root: &Root, dir: &Path) -> Result<PathBuf, String> {
    let absolute = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        root.path.join(dir)
    };
    let stored = paths::relativise_client_dir(&root.path, &absolute);
    let mut value = read_json(&root.launcher_local_json());
    if !value.is_object() {
        value = Value::Object(Map::new());
    }
    value["client_dir"] = Value::String(stored);
    write_json(&root.launcher_local_json(), &value, 2)?;
    Ok(absolute)
}

pub fn discover_client(root: &Root) -> Option<PathBuf> {
    // 旧包是 `<root>/client`；新发布快照把客户端和服务端拆成同级两目录，
    // root=`D:\115us\服务端` 时客户端在 `D:\115us\客户端\DFO`。
    let parent = root.path.parent();
    let mut candidates: Vec<PathBuf> = Vec::new();
    candidates.push(root.path.join("client"));
    if let Some(parent) = parent {
        candidates.push(parent.join("client"));
        candidates.push(parent.join("客户端").join("DFO"));
    }
    candidates.push(root.path.join("game").join("client"));
    candidates.push(root.path.join("客户端").join("DFO"));
    candidates.into_iter().find(|dir| has_client_files(dir))
}

// ---------------------------------------------------------------- 服务端二进制

pub fn server_binary(root: &Root) -> PathBuf {
    let value = read_json(&root.launcher_local_json());
    let configured = value
        .get("server_binary")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(DEFAULT_SERVER_BINARY);
    let path = PathBuf::from(configured);
    if path.is_absolute() {
        path
    } else {
        root.path.join("server").join(path)
    }
}

/// dfolauncher.exe 启动时硬性要求 `launcher.local.json` 里有 `server_binary`，
/// 否则直接退出并报 "launcher.local.json must set server_binary"。
/// 首次部署后这个文件可能根本不存在或缺字段，启动时幂等补一份。
pub fn ensure_launcher_local_json(root: &Root) -> Result<(), String> {
    let mut value = read_json(&root.launcher_local_json());
    if !value.is_object() {
        value = Value::Object(Map::new());
    }
    let need_write = {
        let entry = value.get("server_binary").and_then(Value::as_str);
        match entry {
            Some(s) if !s.trim().is_empty() => false,
            _ => true,
        }
    };
    if need_write {
        value["server_binary"] = Value::String(DEFAULT_SERVER_BINARY.to_string());
        write_json(&root.launcher_local_json(), &value, 2)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- 存档路由

pub fn storage_route_state(root: &Root) -> StorageRoute {
    let value = read_json(&root.storage_local_json());
    let configured = value
        .get("sqlite_path")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from);
    if let Some(path) = configured {
        return StorageRoute::Ready { path };
    }
    // 没有 sqlite_path 却写了别的驱动：那是历史遗留的 PostgreSQL 档，不替玩家改。
    let driver = value
        .get("driver")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if driver.is_empty() || driver == "sqlite" {
        StorageRoute::NeedsInit
    } else {
        StorageRoute::OtherDriver(driver)
    }
}

pub enum StorageRoute {
    /// `sqlite_path` 已写实，直接可以开会话。
    Ready { path: PathBuf },
    /// 还没写：点「开始游戏」时自动落一份（只写配置，不碰库文件）。
    NeedsInit,
    /// 写的是别的驱动（历史遗留的 PostgreSQL 档）—— 不替玩家改，报出来让人决定。
    OtherDriver(String),
}

/// 只在缺 `sqlite_path` 时落一份，格式（四个空格、结尾换行）与既有的保持一致。
pub fn ensure_storage_route(root: &Root) -> Result<StoredRoute, String> {
    match storage_route_state(root) {
        StorageRoute::Ready { path } => {
            let describe = describe_route(&path);
            Ok(StoredRoute {
                created: false,
                describe,
            })
        }
        StorageRoute::OtherDriver(driver) => Err(format!(
            "存档配置写的是 driver={driver}，启动器不会替你改成 SQLite；请手动确认 {}",
            root.storage_local_json().display()
        )),
        StorageRoute::NeedsInit => {
            let sqlite_path = root.default_sqlite_path();
            let mut ordered = Map::new();
            ordered.insert("driver".into(), Value::String("sqlite".into()));
            ordered.insert(
                "sqlite_path".into(),
                Value::String(sqlite_path.to_string_lossy().into_owned()),
            );
            write_json(&root.storage_local_json(), &Value::Object(ordered), 4)?;
            Ok(StoredRoute {
                created: true,
                describe: describe_route(&sqlite_path),
            })
        }
    }
}

pub struct StoredRoute {
    pub created: bool,
    pub describe: String,
}

fn describe_route(sqlite_path: &Path) -> String {
    format!("SQLite（{}）", sqlite_path.display())
}

// ---------------------------------------------------------------- 玩家设置

#[derive(Clone, Debug, Default)]
pub struct Settings {
    pub account: String,
    pub account_used: Vec<String>,
    pub game_mode: String,
    pub auto_stop_on_client_exit: bool,
    /// 自动拾取：服务端只在环回连接且本值为 `"1"` 时打开，默认关。
    pub auto_pickup: bool,
    /// 自动上报要玩家主动勾选，默认关。
    pub auto_report: bool,
    /// 上传范围说明只弹一次。
    pub auto_report_prompted: bool,
}

pub fn settings(root: &Root) -> Settings {
    let value = read_json(&root.settings_json());
    let game = value.get("game");
    let account_used = game
        .and_then(|g| g.get("account_used"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Settings {
        account: game
            .and_then(|g| g.get("account"))
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("probe")
            .to_string(),
        account_used,
        game_mode: game
            .and_then(|g| g.get("mode"))
            .and_then(Value::as_str)
            .unwrap_or("auto")
            .to_string(),
        // 默认开：关游戏即停服，这是整合包玩家的常态预期。
        auto_stop_on_client_exit: game
            .and_then(|g| g.get("autoStopOnClientExit"))
            .and_then(Value::as_bool)
            .unwrap_or(true),
        auto_pickup: game
            .and_then(|g| g.get("autoPickup"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        auto_report: value
            .get("report")
            .and_then(|r| r.get("autoReport"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        auto_report_prompted: value
            .get("report")
            .and_then(|r| r.get("prompted"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

pub fn set_setting<F>(root: &Root, f: F) -> Result<(), String>
where
    F: FnOnce(&mut Map<String, Value>),
{
    set_section(root, "game", f)
}

/// 只改指定小节下的键，其余内容原样保留。
pub fn set_section<F>(root: &Root, section: &str, f: F) -> Result<(), String>
where
    F: FnOnce(&mut Map<String, Value>),
{
    let mut value = read_json(&root.settings_json());
    let target = section_mut(&mut value, section);
    f(target);
    write_json(&root.settings_json(), &value, 2)
}

/// 游戏模式对应的环境变量（`DFO_ODYSSEY_MODE`）；`auto` 不注入，交给服务端自己判。
pub fn game_mode_env(mode: &str) -> Option<(&'static str, &'static str)> {
    match mode {
        "odyssey" => Some(("DFO_ODYSSEY_MODE", "1")),
        "scenario" => Some(("DFO_ODYSSEY_MODE", "0")),
        _ => None,
    }
}

pub fn game_mode_label(mode: &str) -> &'static str {
    match mode {
        "odyssey" => "奥德赛（Odyssey）",
        "scenario" => "剧情（Scenario）",
        _ => "自动（由服务端判定）",
    }
}

/// 账号会进客户端的连接载荷（`?` 分隔），带 `?` 或过长的名字会破坏载荷。
pub fn valid_account(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=32).contains(&bytes.len())
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 拆栏布局（root=`服务端`、客户端在同级 `客户端\DFO`）下，历史默认值 `../client`
    /// 指向一个不存在的路径。三种「配置不认」的写法都必须兜底探到真客户端 —— 否则 MOD 页
    /// 与健康检查整块报「不在」，而游戏侧照样全加载。全程临时目录，不碰真包。
    #[test]
    fn an_unusable_client_dir_falls_back_to_discovery() {
        let base = std::env::temp_dir().join(format!("dfo-config-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let client = base.join("客户端").join("DFO");
        std::fs::create_dir_all(base.join("服务端").join("server")).expect("造 server");
        std::fs::create_dir_all(&client).expect("造客户端目录");
        for name in paths::CLIENT_REQUIRED_FILES {
            std::fs::write(client.join(name), b"placeholder").expect("造必备件");
        }
        let root = Root {
            path: base.join("服务端"),
            source: "test",
        };
        let local = root.launcher_local_json();
        let want = paths::squash(&client);

        for (label, text) in [
            ("配置缺席", None),
            ("值是空白", Some(r#"{"client_dir": ""}"#)),
            ("指向已经不存在的目录", Some(r#"{"client_dir": "../../没了的客户端/DFO"}"#)),
        ] {
            let _ = std::fs::remove_file(&local);
            if let Some(text) = text {
                std::fs::write(&local, text).expect("写 launcher.local.json");
            }
            assert_eq!(stored_client_dir(&root), None, "{label} 不该算已配置");
            assert_eq!(client_dir(&root), want, "{label} 该兜底探到客户端");
        }

        // 相对与绝对两种写法都要认账，认账时不走兜底。
        let absolute = client.display().to_string().replace('\\', "/");
        for (label, text) in [
            ("相对路径", r#"{"client_dir": "../../客户端/DFO"}"#.to_string()),
            ("绝对路径", format!(r#"{{"client_dir": "{absolute}"}}"#)),
        ] {
            std::fs::write(&local, text).expect("写 launcher.local.json");
            assert_eq!(
                stored_client_dir(&root),
                Some(want.clone()),
                "{label} 该被认成已配置"
            );
            assert_eq!(client_dir(&root), want);
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}
