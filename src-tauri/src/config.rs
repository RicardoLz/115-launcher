//! 三份盘上配置的读写：`server/launcher.local.json`、`launcher.settings.json`、
//! `runtime/storage/local.json`。
//!
//! 规矩：读的时候整份留着（`serde_json::Value`），只改自己那一个键，绝不把别人写的键丢掉；
//! 存档路由只在缺 `sqlite_path` 时落一份，绝不覆盖玩家手上的配置，也不预建空的 .sqlite3。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
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

// ------------------------------------------------------------ 服务端内存三键

/// Go 侧把 MiB 左移 20 位换成字节，超过这个值就溢不出来，`memoryBytesFromMiB` 会直接拒绝。
pub const MAX_MEMORY_MIB: u64 = u64::MAX >> 20;
/// 编排器对 `import_max_procs` 的硬门：超过它（`1<<16`）就拒绝启动网关，所以界面得先挡。
pub const MAX_IMPORT_PROCS: u64 = 65536;

/// 三项都是「不写 = 不限」，所以一律用 `Option`；读到的 0 也归一成 `None`，
/// 界面上两者都显示「不限」，写回时干脆不写这个键。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct MemoryTuning {
    pub server_memory_limit_mib: Option<u64>,
    pub go_mem_target_mib: Option<u64>,
    pub import_max_procs: Option<u64>,
}

fn positive_u64(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64).filter(|n| *n > 0)
}

pub fn memory_tuning(root: &Root) -> MemoryTuning {
    let value = read_json(&root.launcher_local_json());
    MemoryTuning {
        server_memory_limit_mib: positive_u64(&value, "server_memory_limit_mib"),
        go_mem_target_mib: positive_u64(&value, "go_mem_target_mib"),
        import_max_procs: positive_u64(&value, "import_max_procs"),
    }
}

/// 只动那三个键，其余键（`client_dir`、`server_binary`、将来别人加的）整份留着。
/// 与 `set_client_dir` 不同，这里**拒绝**在文件读不通时凭空写一份：那份文件同时归 Go 编排器读，
/// 把它重写成只剩三个内存键等于把玩家的客户端目录和服务端二进制一起抹掉。
pub fn set_memory_tuning(root: &Root, tuning: &MemoryTuning) -> Result<MemoryTuning, String> {
    let clean = MemoryTuning {
        server_memory_limit_mib: check_mib(tuning.server_memory_limit_mib, "server_memory_limit_mib")?,
        go_mem_target_mib: check_mib(tuning.go_mem_target_mib, "go_mem_target_mib")?,
        import_max_procs: match tuning.import_max_procs.filter(|n| *n > 0) {
            Some(n) if n > MAX_IMPORT_PROCS => {
                return Err(format!(
                    "导入并发最大 {MAX_IMPORT_PROCS}（编排器超过它就拒绝启动），现在是 {n}"
                ))
            }
            other => other,
        },
    };

    let path = root.launcher_local_json();
    let mut value = read_launcher_object(&path)?;
    let object = value.as_object_mut().expect("read_launcher_object 已保证是对象");
    for (key, wanted) in [
        ("server_memory_limit_mib", clean.server_memory_limit_mib),
        ("go_mem_target_mib", clean.go_mem_target_mib),
        ("import_max_procs", clean.import_max_procs),
    ] {
        match wanted {
            Some(n) => {
                object.insert(key.to_string(), Value::from(n));
            }
            None => {
                object.remove(key);
            }
        }
    }
    write_json(&path, &value, 2)?;
    Ok(clean)
}

fn check_mib(value: Option<u64>, key: &str) -> Result<Option<u64>, String> {
    match value.filter(|n| *n > 0) {
        Some(n) if n > MAX_MEMORY_MIB => Err(format!(
            "{key} 超出可换算范围（最大 {MAX_MEMORY_MIB} MiB），编排器会拒绝启动"
        )),
        other => Ok(other),
    }
}

/// 严格版读取：文件在就必须解析得开。缺席才算空对象。
fn read_launcher_object(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Ok(Value::Object(Map::new()));
    }
    let raw = std::fs::read(path).map_err(|err| format!("读取 {} 失败：{err}", path.display()))?;
    let trimmed = raw.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&raw);
    let value: Value = serde_json::from_slice(trimmed).map_err(|err| {
        format!(
            "{} 读不通（{err}）—— 启动器不会覆盖它，请先手工修好；Go 编排器同样读不了这份",
            path.display()
        )
    })?;
    if !value.is_object() {
        return Err(format!(
            "{} 顶层不是 JSON 对象，启动器不会覆盖它",
            path.display()
        ));
    }
    Ok(value)
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
    /// 主页背景素材（壁纸图片或动态壁纸视频）的绝对路径；空串等于不用背景。
    pub background: String,
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
        background: value
            .get("ui")
            .and_then(|u| u.get("background"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
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

    fn temp_root(label: &str) -> (Root, PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "dfo-memory-test-{}-{}",
            std::process::id(),
            label
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("server")).expect("造 server");
        (
            Root {
                path: base.clone(),
                source: "test",
            },
            base,
        )
    }

    /// 任务书红线：写回必须 JSON 往返保留未知键，且必须无 BOM（Go 的 `json.Unmarshal` 拒 BOM）。
    /// 顺手验「不限」= 键干脆不写，而不是写 0。
    #[test]
    fn memory_tuning_round_trips_and_writes_no_bom() {
        let (root, base) = temp_root("roundtrip");
        let local = root.launcher_local_json();
        std::fs::write(
            &local,
            "\u{feff}{\"client_dir\":\"../client\",\"server_binary\":\"work/dfo-lan/bin/x.exe\",\"channel_identity\":false,\"future_key\":{\"a\":1}}",
        )
        .expect("写 launcher.local.json");

        set_memory_tuning(
            &root,
            &MemoryTuning {
                server_memory_limit_mib: Some(4096),
                go_mem_target_mib: Some(1536),
                import_max_procs: Some(4),
            },
        )
        .expect("写入该成功");

        let raw = std::fs::read(&local).expect("读回");
        assert!(!raw.starts_with(&[0xEF, 0xBB, 0xBF]), "必须无 BOM");
        assert!(raw.starts_with(b"{"), "首个字节就该是 JSON");
        let text = String::from_utf8(raw).expect("UTF-8");
        for keep in ["client_dir", "server_binary", "channel_identity", "future_key"] {
            assert!(text.contains(keep), "{keep} 不该被丢掉");
        }
        assert_eq!(
            memory_tuning(&root),
            MemoryTuning {
                server_memory_limit_mib: Some(4096),
                go_mem_target_mib: Some(1536),
                import_max_procs: Some(4),
            }
        );

        // 全填 0 = 全部不限：三个键从文件里消失，别人的键一个不少。
        set_memory_tuning(&root, &MemoryTuning::default()).expect("清空该成功");
        let text = std::fs::read_to_string(&local).expect("读回");
        for key in ["server_memory_limit_mib", "go_mem_target_mib", "import_max_procs"] {
            assert!(!text.contains(key), "{key} 该被删掉而不是留个 0");
        }
        for keep in ["client_dir", "server_binary", "channel_identity", "future_key"] {
            assert!(text.contains(keep), "{keep} 不该被丢掉");
        }
        assert_eq!(memory_tuning(&root), MemoryTuning::default());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 界面前置校验要报的是编排器自己那句话的口径，别让玩家写出一份启动不来的配置。
    #[test]
    fn memory_tuning_refuses_what_the_orchestrator_would_refuse() {
        let (root, base) = temp_root("reject");
        let local = root.launcher_local_json();
        std::fs::write(&local, r#"{"client_dir":"../client"}"#).expect("写 launcher.local.json");
        let before = std::fs::read(&local).expect("读回");

        let too_many = MemoryTuning {
            server_memory_limit_mib: None,
            go_mem_target_mib: None,
            import_max_procs: Some(MAX_IMPORT_PROCS + 1),
        };
        let err = set_memory_tuning(&root, &too_many).expect_err("65536 以上该拒");
        assert!(err.contains("65536"), "{err}");

        let too_big = MemoryTuning {
            server_memory_limit_mib: Some(MAX_MEMORY_MIB + 1),
            go_mem_target_mib: None,
            import_max_procs: None,
        };
        let err = set_memory_tuning(&root, &too_big).expect_err("换算溢出该拒");
        assert!(err.contains("server_memory_limit_mib"), "{err}");

        // 边界本身要放行：正好 65536 是 Go 允许的上限。
        set_memory_tuning(
            &root,
            &MemoryTuning {
                server_memory_limit_mib: None,
                go_mem_target_mib: None,
                import_max_procs: Some(MAX_IMPORT_PROCS),
            },
        )
        .expect("正好 65536 该放行");
        assert_eq!(memory_tuning(&root).import_max_procs, Some(MAX_IMPORT_PROCS));

        // 被拒的那两次没动过文件。
        std::fs::write(&local, &before).expect("复原以比对");
        let err = set_memory_tuning(&root, &too_many).expect_err("仍然该拒");
        assert!(err.contains("65536"), "{err}");
        assert_eq!(std::fs::read(&local).expect("读回"), before, "拒绝时不许落盘");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 这份文件同时归 Go 读。它读不通时只能报出来，绝不能顺手写一份「干净」的覆盖玩家配置。
    #[test]
    fn memory_tuning_never_clobbers_an_unreadable_launcher_local_json() {
        let (root, base) = temp_root("clobber");
        let local = root.launcher_local_json();
        std::fs::write(&local, b"{\n  \"client_dir\": ").expect("写半份 JSON");
        let err = set_memory_tuning(
            &root,
            &MemoryTuning {
                server_memory_limit_mib: Some(1024),
                go_mem_target_mib: None,
                import_max_procs: None,
            },
        )
        .expect_err("坏文件该报错");
        assert!(err.contains("不会覆盖"), "{err}");
        assert_eq!(std::fs::read(&local).expect("读回"), b"{\n  \"client_dir\": ");
        // 文件缺席时另当别论：那是首次部署，写一份是安全的。
        std::fs::remove_file(&local).expect("删掉");
        assert_eq!(memory_tuning(&root), MemoryTuning::default());
        let _ = std::fs::remove_dir_all(&base);
    }
}
