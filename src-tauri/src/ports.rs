//! 端口探测与占用归属。
//!
//! 只有一个真正有意义的端口：网关 7001。它固定、不轮换，所以「能不能开始游戏」=
//! 「这一秒能不能真的绑上 7001」。占用者用一次 `netstat -ano` 快照解析 PID，
//! 镜像名再按前缀分类，决定它算不算「我们自己的残留」。

use std::collections::HashMap;
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use serde::Serialize;

use crate::paths::GATEWAY_PORT;
use crate::procutil;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockKind {
    Free,
    OwnServer,
    OwnOrchestrator,
    OwnClient,
    Unknown,
}

impl BlockKind {
    pub fn label(self) -> &'static str {
        match self {
            BlockKind::Free => "空闲",
            BlockKind::OwnServer => "本项目网关残留",
            BlockKind::OwnOrchestrator => "本项目编排残留",
            BlockKind::OwnClient => "游戏客户端",
            BlockKind::Unknown => "非本项目进程",
        }
    }

    /// 只有本项目的服务端/编排残留才允许自动清理；玩家自己开着的客户端和外人一律不动手。
    pub fn auto_cleanable(self) -> bool {
        matches!(self, BlockKind::OwnServer | BlockKind::OwnOrchestrator)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Owner {
    pub port: u16,
    pub port_label: String,
    pub listening: bool,
    pub pid: Option<u32>,
    pub image: Option<String>,
    pub kind: BlockKind,
    /// 界面直接显示的中文归属，免得两边各写一份映射表。
    pub kind_label: String,
}

pub fn listening(port: u16) -> bool {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok()
}

/// 一次 `netstat -ano` 拿到 LISTENING 的 端口→PID。
pub fn listening_pid_map() -> HashMap<u16, u32> {
    let mut map = HashMap::new();
    let Ok(output) = procutil::command_str("netstat")
        .args(["-ano"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .and_then(|mut child| {
            let mut buffer = Vec::new();
            if let Some(mut stdout) = child.stdout.take() {
                let _ = stdout.read_to_end(&mut buffer);
            }
            child.wait().map(|status| (status, buffer))
        })
    else {
        return map;
    };
    let text = String::from_utf8_lossy(&output.1);
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5 {
            continue;
        }
        if !fields[0].eq_ignore_ascii_case("TCP") && !fields[0].eq_ignore_ascii_case("TCPv6") {
            continue;
        }
        if !fields[3].eq_ignore_ascii_case("LISTENING") {
            continue;
        }
        let Some(port) = fields[1]
            .rsplit(':')
            .next()
            .and_then(|s| s.parse::<u16>().ok())
        else {
            continue;
        };
        let Ok(pid) = fields[4].parse::<u32>() else {
            continue;
        };
        map.entry(port).or_insert(pid);
    }
    map
}

fn classify(image: Option<&str>) -> BlockKind {
    let Some(name) = image else {
        return BlockKind::Unknown;
    };
    let lower = name.to_lowercase();
    if lower.starts_with("dfolauncher") {
        BlockKind::OwnOrchestrator
    } else if lower.starts_with("wireprobe") || lower.starts_with("probe") {
        BlockKind::OwnServer
    } else if lower.starts_with("dfo") {
        BlockKind::OwnClient
    } else {
        BlockKind::Unknown
    }
}

pub fn owner_of(port: u16, snapshot: &HashMap<u16, u32>) -> Owner {
    let is_listening = listening(port);
    // 非管理员时 netstat 会把别人进程的 PID 报成 0，那就等同于「认不出是谁」。
    let pid = if is_listening {
        snapshot.get(&port).copied().filter(|pid| *pid != 0)
    } else {
        None
    };
    let image = pid.and_then(procutil::name_of);
    let kind = if is_listening {
        classify(image.as_deref())
    } else {
        BlockKind::Free
    };
    Owner {
        port,
        port_label: format!("127.0.0.1:{port}"),
        listening: is_listening,
        pid: if is_listening { pid } else { None },
        image: if is_listening { image } else { None },
        kind_label: kind.label().to_string(),
        kind,
    }
}

/// 网关端口的归属快照（含 netstat + 进程名，两次系统调用级别，够轻）。
pub fn gateway_owner() -> Owner {
    owner_of(GATEWAY_PORT, &listening_pid_map())
}
