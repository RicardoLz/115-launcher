//! 运行日志：内存环形 + 落盘 + 推给界面，三件事同一个入口。
//!
//! 界面看到的每一行（含编排程序 stdout/stderr 转述进来的那些）都从这里过一遍。
//! 每行带一个自增 `seq`：界面先挂监听、再取环形快照，靠它把两头重叠的部分去重，
//! 这样「快照发出之后、监听挂上之前」那几行既不会丢也不会重复。

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::Local;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

const RING_CAPACITY: usize = 2000;

#[derive(Clone, Debug, Serialize)]
pub struct Line {
    pub seq: u64,
    pub time: String,
    pub level: &'static str,
    pub text: String,
}

pub struct Bus {
    seq: AtomicU64,
    ring: Mutex<VecDeque<Line>>,
    file: Mutex<Option<std::fs::File>>,
    app: Mutex<Option<AppHandle>>,
    pub log_dir: PathBuf,
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Bus {
    pub fn new(log_dir: &Path) -> Arc<Self> {
        let _ = std::fs::create_dir_all(log_dir);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join(format!("launcher-{}.log", Local::now().format("%Y%m%d"))))
            .ok();
        Arc::new(Bus {
            seq: AtomicU64::new(1),
            ring: Mutex::new(VecDeque::new()),
            file: Mutex::new(file),
            app: Mutex::new(None),
            log_dir: log_dir.to_path_buf(),
        })
    }

    /// 窗口建好后接上 AppHandle：接上之后 push 出来的行才会同时发给界面。
    pub fn attach(&self, app: AppHandle) {
        *locked(&self.app) = Some(app);
    }

    pub fn push(&self, level: &'static str, text: impl Into<String>) {
        let text = text.into();
        if text.trim().is_empty() {
            return;
        }
        let line = Line {
            seq: self.seq.fetch_add(1, Ordering::Relaxed),
            time: Local::now().format("%H:%M:%S").to_string(),
            level,
            text,
        };
        if let Some(file) = locked(&self.file).as_mut() {
            let _ = writeln!(
                file,
                "[{}] {:5} {}",
                line.time,
                level.to_uppercase(),
                strip_ansi(&line.text)
            );
        }
        {
            let mut ring = locked(&self.ring);
            if ring.len() >= RING_CAPACITY {
                ring.pop_front();
            }
            ring.push_back(line.clone());
        }
        emit(&locked(&self.app), &line);
    }

    pub fn info(&self, text: impl Into<String>) {
        self.push("info", text);
    }
    pub fn ok(&self, text: impl Into<String>) {
        self.push("ok", text);
    }
    pub fn warn(&self, text: impl Into<String>) {
        self.push("warn", text);
    }
    pub fn error(&self, text: impl Into<String>) {
        self.push("error", text);
    }

    pub fn snapshot(&self, limit: usize) -> Vec<Line> {
        let ring = locked(&self.ring);
        let start = ring.len().saturating_sub(limit);
        ring.iter().skip(start).cloned().collect()
    }

    /// 只发给界面的进度事件：不进环形、不落盘。
    ///
    /// 重建一次能打出几百行 `进度 n/m`，走 `push` 的话日志被刷满、真正的告警行会被挤出
    /// 那 2000 格；而进度条是瞬时状态，重启后没有任何回看价值，所以给它单独一条 emit。
    pub fn progress(&self, payload: &impl Serialize) {
        emit_value(&locked(&self.app), "gm-rebuild-progress", payload);
    }
}

fn emit(slot: &MutexGuard<Option<AppHandle>>, line: &Line) {
    if let Some(app) = slot.as_ref() {
        let _ = app.emit("log", line);
    }
}

fn emit_value(slot: &MutexGuard<Option<AppHandle>>, event: &str, payload: &impl Serialize) {
    if let Some(app) = slot.as_ref() {
        let _ = app.emit(event, payload);
    }
}

/// 编排程序偶尔带 ANSI 颜色码，落到文件和界面上都是乱码。
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
        // 跳过 ESC ... 命令字符（m / K / J 等）
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
