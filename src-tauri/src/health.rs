//! 精简体检：只查「开始游戏」这条路上真会绊人的几项。
//!
//! 错误级的项目就是启动会失败的地方，界面据此把「开始游戏」按钮挡在前面说清楚原因；
//! 警告级不挡路（比如网络隔离探针 —— 原生启动根本不碰它，但调试模式要用）。

use serde::Serialize;

use crate::config;
use crate::paths::{has_client_files, sk_dat_size, Root, GATEWAY_PORT, SK_DAT_REAL_MIN_BYTES};
use crate::portguard;
use crate::procutil;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Warn,
    Error,
}

#[derive(Clone, Debug, Serialize)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub level: Level,
    pub value: String,
    pub hint: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub items: Vec<Item>,
    pub ok: usize,
    pub warn: usize,
    pub error: usize,
    pub elapsed_ms: u64,
}

pub fn check(root: &Root) -> Report {
    let started = std::time::Instant::now();
    let mut items: Vec<Item> = Vec::new();

    items.push(item(
        "app.root",
        "整合包目录",
        if root.path.is_dir() {
            Level::Ok
        } else {
            Level::Error
        },
        root.path.display().to_string(),
        format!("识别方式：{}", root.source),
    ));

    let writable = crate::paths::check_writable(&root.path);
    items.push(item(
        "app.writable",
        "目录可写",
        match &writable {
            Ok(()) => Level::Ok,
            Err(_) => Level::Error,
        },
        root.path.display().to_string(),
        writable.err().unwrap_or_default(),
    ));

    let storage = config::storage_route_state(root);
    let (storage_level, storage_value, storage_hint) = match &storage {
        config::StorageRoute::Ready { path } => {
            let parent_ok = path.parent().map(|p| p.is_dir()).unwrap_or(false);
            if parent_ok {
                (
                    Level::Ok,
                    path.display().to_string(),
                    if path.is_file() {
                        "存档库已存在".to_string()
                    } else {
                        "存档库将在服务端首次连接时创建".to_string()
                    },
                )
            } else {
                (
                    Level::Error,
                    path.display().to_string(),
                    "存档路径所在目录不存在：整合包可能是从别的机器整盘搬来的，需要修正 sqlite_path".to_string(),
                )
            }
        }
        config::StorageRoute::NeedsInit => (
            Level::Warn,
            "尚未初始化".to_string(),
            "点「开始游戏」时会自动落一份配置（不会动库文件）".to_string(),
        ),
        config::StorageRoute::OtherDriver(driver) => (
            Level::Error,
            format!("driver={driver}"),
            "存档配置写的是别的驱动，启动器不会替你改；请确认这份整合包用哪一档".to_string(),
        ),
    };
    items.push(item(
        "app.storage",
        "存档配置",
        storage_level,
        storage_value,
        storage_hint,
    ));

    let client_dir = config::client_dir(root);
    let client_ok = has_client_files(&client_dir);
    let sk_dat = sk_dat_size(&client_dir);
    items.push(item(
        "client.dir",
        "游戏客户端",
        if client_dir.is_dir() {
            Level::Ok
        } else {
            Level::Error
        },
        client_dir.display().to_string(),
        if client_dir.is_dir() {
            String::new()
        } else {
            "目录不存在，请在首页「选择游戏目录」里指一次 DFO.exe 所在目录".to_string()
        },
    ));
    items.push(item(
        "client.files",
        "客户端必备件",
        if client_ok { Level::Ok } else { Level::Error },
        if client_ok {
            // sk.dat 只报状态、不参与判定：已拆壳的整合包本就不带它，或者带的是一份
            // 装不下任何段密钥的占位件（新发布快照就是 128 B 那种）。
            match sk_dat {
                None => "DFO.exe + Script.pvf 齐；无 sk.dat（客户端已拆壳）".to_string(),
                Some(size) if size < SK_DAT_REAL_MIN_BYTES => format!(
                    "DFO.exe + Script.pvf 齐；sk.dat 只有 {size} B，装不下段密钥，\
                     是占位件（客户端已拆壳）"
                ),
                Some(size) => format!("DFO.exe + Script.pvf 齐；sk.dat 在场（{size} B）"),
            }
        } else {
            "缺文件".to_string()
        },
        if client_ok {
            String::new()
        } else {
            format!(
                "{} 里要同时有 DFO.exe、Script.pvf（sk.dat 只在客户端仍带外层保护时必需）",
                client_dir.display()
            )
        },
    ));

    let orchestrator = root.orchestrator_exe();
    items.push(item(
        "server.orchestrator",
        "会话编排程序",
        if orchestrator.is_file() {
            Level::Ok
        } else {
            Level::Error
        },
        orchestrator.display().to_string(),
        if orchestrator.is_file() {
            String::new()
        } else {
            "「开始游戏」整条链路都靠它；缺它请更新整合包内的服务端程序".to_string()
        },
    ));

    let gateway = config::server_binary(root);
    items.push(item(
        "server.gateway",
        "网关程序",
        if gateway.is_file() {
            Level::Ok
        } else {
            Level::Error
        },
        gateway.display().to_string(),
        String::new(),
    ));

    let profile = root.work_dir().join("configs").join("pvf-default.json");
    items.push(item(
        "server.profile",
        "服务端默认配置",
        if profile.is_file() {
            Level::Ok
        } else {
            Level::Error
        },
        profile.display().to_string(),
        String::new(),
    ));

    let probe = root.probe_exe();
    items.push(item(
        "server.probe",
        "网络隔离探针",
        if probe.is_file() {
            Level::Ok
        } else {
            Level::Warn
        },
        probe.display().to_string(),
        "原生启动用不到它；调试模式与部分排障功能需要".to_string(),
    ));

    let report = portguard::check(root);
    items.push(item(
        "port.gateway",
        format!("网关端口 {GATEWAY_PORT}"),
        if report.gateway.listening {
            Level::Error
        } else {
            Level::Ok
        },
        report.summary.clone(),
        if report.gateway.listening {
            portguard::cleanup_hint(root, &report)
        } else if report.leftovers.is_empty() {
            String::new()
        } else {
            "上一轮没退干净：点「停止游戏」或直接开始，启动器会先自己清一遍".to_string()
        },
    ));

    let game_running = !procutil::pids_by_name(crate::paths::CLIENT_IMAGE_NAME).is_empty();
    items.push(item(
        "client.running",
        "游戏进程",
        if game_running { Level::Warn } else { Level::Ok },
        if game_running {
            "已在运行".to_string()
        } else {
            "未运行".to_string()
        },
        String::new(),
    ));

    let error = items.iter().filter(|i| i.level == Level::Error).count();
    let warn = items.iter().filter(|i| i.level == Level::Warn).count();
    let ok = items.len() - error - warn;
    Report {
        items,
        ok,
        warn,
        error,
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}

fn item(id: &str, name: impl Into<String>, level: Level, value: String, hint: String) -> Item {
    Item {
        id: id.to_string(),
        name: name.into(),
        level,
        value,
        hint,
    }
}
