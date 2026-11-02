//! 整合包根目录与全部固定路径。
//!
//! 根目录优先级：`--root <路径>` > 环境变量 `DFO_ROOT` > 启动器所在目录 > 当前工作目录。
//! 刻意不做「向上查找」：找错根目录会让下面每一条相对路径都静默指错地方。

use std::path::{Component, Path, PathBuf};

pub const GATEWAY_PORT: u16 = 7001;

/// 编排 CLI 与服务端进程镜像名（停止时按这些名字清理残留）。
pub const SERVER_IMAGE_NAMES: [&str; 8] = [
    "wireprobe-pvf.exe",
    "wireprobe-dungeon39.exe",
    "wireprobe-handoff-source.exe",
    "wireprobe.exe",
    "wireprobe-character.exe",
    "probe.exe",
    "probe-rebuilt.exe",
    "dfolauncher.exe",
];

pub const CLIENT_IMAGE_NAME: &str = "DFO.exe";

/// 判定“这是一个游戏客户端目录”所需的最小文件集合。`sk.dat` 刻意不在此列：
/// 它是外层 PVF 保护的段密钥密文，已拆壳的整合包不带它，需要时由编排程序报精确错误。
pub const CLIENT_REQUIRED_FILES: [&str; 2] = ["DFO.exe", "Script.pvf"];

#[derive(Clone, Debug)]
pub struct Root {
    pub path: PathBuf,
    pub source: &'static str,
}

impl Root {
    pub fn log_line(&self) -> String {
        format!("使用的整合包：{}（{}）", self.path.display(), self.source)
    }

    pub fn launcher_local_json(&self) -> PathBuf {
        self.path.join("server").join("launcher.local.json")
    }

    pub fn settings_json(&self) -> PathBuf {
        self.path.join("launcher.settings.json")
    }

    pub fn work_dir(&self) -> PathBuf {
        self.path.join("server").join("work").join("dfo-lan")
    }

    pub fn orchestrator_exe(&self) -> PathBuf {
        self.work_dir().join("bin").join("dfolauncher.exe")
    }

    pub fn runtime_dir(&self) -> PathBuf {
        self.work_dir().join("runtime")
    }

    pub fn storage_local_json(&self) -> PathBuf {
        self.runtime_dir().join("storage").join("local.json")
    }

    pub fn default_sqlite_path(&self) -> PathBuf {
        self.runtime_dir().join("storage").join("dfolan.sqlite3")
    }

    pub fn mods_dir(&self) -> PathBuf {
        self.path.join("mods")
    }

    /// 调试用的网络隔离探针：原生启动不碰它，只有排障功能会用到。
    pub fn probe_exe(&self) -> PathBuf {
        self.path
            .join("server")
            .join("work")
            .join("dfo_probe_tools")
            .join("probe.exe")
    }

    /// 本次会话的运行目录（编排程序自己创建，启动器只读）。
    pub fn newest_run_dir(&self) -> Option<PathBuf> {
        let entries = std::fs::read_dir(self.runtime_dir()).ok()?;
        let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.starts_with("roles_") || !path.is_dir() {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                if best.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
                    best = Some((mtime, path));
                }
            }
        }
        best.map(|(_, p)| p)
    }
}

pub fn resolve_root(exe_path: Option<&Path>) -> Root {
    let args: Vec<String> = std::env::args().collect();
    for (i, arg) in args.iter().enumerate() {
        if arg == "--root" {
            if let Some(value) = args.get(i + 1) {
                return finish(PathBuf::from(value), "--root 指定");
            }
        } else if let Some(value) = arg.strip_prefix("--root=") {
            return finish(PathBuf::from(value), "--root 指定");
        }
    }
    if let Ok(value) = std::env::var("DFO_ROOT") {
        if !value.trim().is_empty() {
            return finish(PathBuf::from(value), "环境变量 DFO_ROOT 指定");
        }
    }
    if let Some(exe) = exe_path {
        if let Some(dir) = exe.parent() {
            return finish(dir.to_path_buf(), "启动器所在目录");
        }
    }
    finish(
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        "当前工作目录（未能确定启动器自身位置）",
    )
}

fn finish(path: PathBuf, source: &'static str) -> Root {
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    Root {
        path: absolute,
        source,
    }
}

/// 可写性探针：放到只读目录（Program Files、光盘）时立刻失败，而不是等到写存档配置。
pub fn check_writable(root: &Path) -> Result<(), String> {
    if !root.is_dir() {
        return Err(format!("整合包目录不存在：{}", root.display()));
    }
    let probe = format!(".dfolauncher-write-probe-{}", std::process::id());
    let file = root.join(probe);
    match std::fs::write(&file, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&file);
            Ok(())
        }
        Err(err) => Err(format!(
            "整合包目录不可写：{}（{}）—— 请把启动器放到可写目录后再运行",
            root.display(),
            err
        )),
    }
}

pub fn app_dir() -> PathBuf {
    if let Ok(value) = std::env::var("DFO_LAUNCHER_HOME") {
        if !value.trim().is_empty() {
            return PathBuf::from(value);
        }
    }
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    base.join("DFO115US-Tauri")
}

/// 折叠 `..` 与 `.`，只为显示：`<root>/server/../client` 这种拼法给玩家看会像写错了路径。
/// 不做存在性判断，也不碰前缀（`\\?\`、UNC 一律原样保留）。
pub fn squash(path: &Path) -> PathBuf {
    let mut kept: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(kept.last(), Some(Component::Normal(_))) {
                    kept.pop();
                    continue;
                }
                kept.push(component);
            }
            other => kept.push(other),
        }
    }
    let mut out = PathBuf::new();
    for component in kept {
        out.push(component.as_os_str());
    }
    out
}

/// 客户端必备件：DFO.exe + Script.pvf。sk.dat 不在这里 —— 它是外层保护的段密钥密文，
/// 已经拆壳的整合包里没有任何东西可由它解开，编排程序只在归档仍带保护时才要求它。
/// 目录判定只认这两件，否则合法的裸包会被启动器一路当成“不是客户端”。
pub fn has_client_files(dir: &Path) -> bool {
    CLIENT_REQUIRED_FILES
        .iter()
        .all(|name| dir.join(name).is_file())
}

/// `sk.dat` 在场与否只影响健康检查的措辞，永远不构成阻塞：需要它的客户端会由编排程序
/// 报出「读取 sk.dat 失败」，那才是精确的诊断。不在场给 `None`。
pub fn sk_dat_size(dir: &Path) -> Option<u64> {
    let meta = std::fs::metadata(dir.join("sk.dat")).ok()?;
    meta.is_file().then_some(meta.len())
}

/// 能装东西的 `sk.dat` 最小字节数：80 条 32 字节段密钥的 RSA 密文实测 2,560 B。
/// 比这小的只可能是占位件，解不出任何外层保护，所以不能按「客户端仍加密」来报。
pub const SK_DAT_REAL_MIN_BYTES: u64 = 2_560;

/// `client_dir` 存盘时优先写相对 `<root>/server` 的形式（默认值就是 `../client`），
/// 这样换盘符、换机器后同一份整合包依然有效；盘外目录才写绝对路径。
pub fn relativise_client_dir(root: &Path, client_dir: &Path) -> String {
    let base = root.join("server");
    match client_dir.strip_prefix(&base) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => client_dir.to_string_lossy().replace('\\', "/"),
    }
}
