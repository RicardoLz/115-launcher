//! Win32 进程侧的薄封装：隐藏控制台、按名字找 PID、判活、结束进程树。
//!
//! 一律走 Toolhelp32 快照而不是 `tasklist`：就绪轮询与自动停服监听要每 100~500ms 问一次
//! 「DFO.exe 还在不在」，每次都 fork 一个 tasklist 会把机器点着。

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x08000000;

/// 每个子进程都要挂上这个 flag，否则黑框一闪而过（玩家会以为中了毒）。
fn apply_hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

pub fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    apply_hidden(&mut cmd);
    cmd
}

pub fn command_str(program: &str) -> Command {
    let mut cmd = Command::new(program);
    apply_hidden(&mut cmd);
    cmd
}

#[derive(Clone, Debug)]
pub struct ProcEntry {
    pub pid: u32,
    /// 小写的可执行文件名（含扩展名）。
    pub name: String,
}

#[cfg(windows)]
pub fn snapshot() -> Vec<ProcEntry> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut out = Vec::new();
    unsafe {
        let Ok(handle) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(handle, &mut entry).is_ok() {
            loop {
                let name = wide_to_string(&entry.szExeFile).to_lowercase();
                if !name.is_empty() {
                    out.push(ProcEntry {
                        pid: entry.th32ProcessID,
                        name,
                    });
                }
                if Process32NextW(handle, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(handle);
    }
    out
}

#[cfg(not(windows))]
pub fn snapshot() -> Vec<ProcEntry> {
    Vec::new()
}

#[cfg(windows)]
fn wide_to_string(raw: &[u16]) -> String {
    let end = raw.iter().position(|c| *c == 0).unwrap_or(raw.len());
    String::from_utf16_lossy(&raw[..end])
}

pub fn pids_by_name(name: &str) -> Vec<u32> {
    let needle = name.to_lowercase();
    snapshot()
        .into_iter()
        .filter(|entry| entry.name == needle)
        .map(|entry| entry.pid)
        .collect()
}

pub fn running_named(names: &[&str]) -> Vec<ProcEntry> {
    let lowered: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    snapshot()
        .into_iter()
        .filter(|entry| lowered.iter().any(|n| n == &entry.name))
        .collect()
}

pub fn name_of(pid: u32) -> Option<String> {
    snapshot()
        .into_iter()
        .find(|entry| entry.pid == pid)
        .map(|entry| entry.name)
}

/// 进程还活着吗。`GetExitCodeProcess` 返回 `STILL_ACTIVE(259)` 才算活着；
/// 打不开句柄（进程已没了 / 权限不够）时保守当作已退出，交由上层复核。
pub fn is_running(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return false;
            };
            let mut code = 0u32;
            let alive =
                GetExitCodeProcess(handle, &mut code).is_ok() && code == STILL_ACTIVE.0 as u32;
            let _ = CloseHandle(handle);
            alive
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        false
    }
}

/// 结束进程树。先 `taskkill /F /T`，没成功再自己 `TerminateProcess` 兜一层。
pub fn kill_tree(pid: u32) -> bool {
    let status = command_str("taskkill")
        .args(["/F", "/T", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if matches!(status, Ok(code) if code.success()) {
        return true;
    }
    force_terminate(pid)
}

pub fn kill_by_name(name: &str) -> bool {
    let status = command_str("taskkill")
        .args(["/F", "/IM", name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if status.is_err() {
        return pids_by_name(name)
            .into_iter()
            .map(force_terminate)
            .all(|ok| ok);
    }
    pids_by_name(name).is_empty()
}

#[cfg(windows)]
fn force_terminate(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    };
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, pid) else {
            let can_query = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
            if let Ok(existing) = can_query {
                let _ = CloseHandle(existing);
                return false;
            }
            return true; // 句柄都开不出来：进程已经不在了
        };
        let done = TerminateProcess(handle, 1).is_ok();
        let _ = CloseHandle(handle);
        done
    }
}

#[cfg(not(windows))]
fn force_terminate(_pid: u32) -> bool {
    false
}

/// 自己握着、自己关的进程句柄。
///
/// 要拿到「进程自己报告的退出码」就必须在它还活着的时候把句柄开下来一直攥着：
/// 进程一没，按 PID 再 `OpenProcess` 就开不出句柄了，那时只剩「不在了」这一个事实。
/// 客户端由编排程序拉起，启动器没有出生句柄，只能在锁到 PID 的当下补开一个。
#[cfg(windows)]
pub struct ProcHandle(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
unsafe impl Send for ProcHandle {}

#[cfg(windows)]
impl Drop for ProcHandle {
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
impl ProcHandle {
    pub fn open(pid: u32) -> Option<ProcHandle> {
        use windows::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_ACCESS_RIGHTS,
        };
        // SYNCHRONIZE(0x100000)：不加它，WaitForSingleObject 直接 WAIT_FAILED。
        let rights = PROCESS_ACCESS_RIGHTS(PROCESS_QUERY_LIMITED_INFORMATION.0 | 0x0010_0000);
        unsafe { OpenProcess(rights, false, pid).ok().map(ProcHandle) }
    }

    /// 最多等 `wait` 这么久；返回进程是否已经退出。
    pub fn wait_exit(&self, wait: Duration) -> bool {
        use windows::Win32::Foundation::WAIT_OBJECT_0;
        use windows::Win32::System::Threading::WaitForSingleObject;
        let ms = wait.as_millis().min(u32::MAX as u128) as u32;
        unsafe { WaitForSingleObject(self.0, ms) == WAIT_OBJECT_0 }
    }

    pub fn exit_code(&self) -> Option<u32> {
        use windows::Win32::System::Threading::GetExitCodeProcess;
        let mut code = 0u32;
        unsafe { GetExitCodeProcess(self.0, &mut code).ok().map(|_| code) }
    }
}

#[cfg(not(windows))]
pub struct ProcHandle;

#[cfg(not(windows))]
impl ProcHandle {
    pub fn open(_pid: u32) -> Option<ProcHandle> {
        None
    }
    pub fn wait_exit(&self, _wait: Duration) -> bool {
        false
    }
    pub fn exit_code(&self) -> Option<u32> {
        None
    }
}

/// 跑一条命令，stdout+stderr 一起收回来；超时就把它整个树干掉。
pub fn run_capture(program: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut cmd = command(program);
    cmd.args(args);
    run_capture_with(cmd, timeout)
}

/// 已经配好参数/工作目录/环境变量的命令，收输出并带超时。
pub fn run_capture_with(cmd: Command, timeout: Duration) -> Result<String, String> {
    run_with_status(cmd, timeout).map(|(_, text)| text)
}

/// 同 `run_capture_with`，但当「跑完了」还不够、还要知道它成没成时用这个：
/// 返回 `(退出码是否为零, 输出)`。超时与被信号打断仍然算 `Err`。
pub fn run_with_status(mut cmd: Command, timeout: Duration) -> Result<(bool, String), String> {
    let label = cmd.get_program().to_string_lossy().into_owned();
    cmd.stdin(Stdio::null());
    let child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("无法启动 {label}：{err}"))?;
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => {
            let mut text = String::from_utf8_lossy(&output.stdout).to_string();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            Ok((output.status.success(), text))
        }
        Ok(Err(err)) => Err(format!("{label} 执行失败：{err}")),
        Err(_) => {
            kill_tree(pid);
            Err(format!("{label} 超时（{} 秒）", timeout.as_secs()))
        }
    }
}

/// 流式版 `run_with_status`：子进程每吐一行就回调一次，跑完仍返回 `(是否成功, stdout+stderr)`。
///
/// 为什么要另开一条路：重建要在界面上走出进度，而 `wait_with_output` 是等子进程整个跑完才
/// 一次性吐输出，进度条就会「90 秒一动不动，然后一下全绿」。
/// 两路输出各收各的、最后照 stdout、stderr 的顺序拼，所以 `detail` 那份尾巴与上面那条一致。
pub fn run_with_lines(
    mut cmd: Command,
    timeout: Duration,
    mut on_line: impl FnMut(&str),
) -> Result<(bool, String), String> {
    enum Msg {
        Line { stream: u8, text: String },
        Done(Result<bool, std::io::Error>),
    }
    use std::io::{BufRead, Read};

    let label = cmd.get_program().to_string_lossy().into_owned();
    cmd.stdin(Stdio::null());
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("无法启动 {label}：{err}"))?;
    let pid = child.id();
    let stdout = child.stdout.take().expect("stdout 已 piped");
    let stderr = child.stderr.take().expect("stderr 已 piped");

    let (tx, rx) = std::sync::mpsc::channel::<Msg>();
    // 三条线程共用一条通道，主线程只认「三个发送端都掉了」：那一刻拿到的才是完整输出。
    // 自己这一份必须先丢掉，否则永远断不了。
    let readers: Vec<(u8, Box<dyn Read + Send>)> =
        vec![(0u8, Box::new(stdout)), (1u8, Box::new(stderr))];
    for (stream, reader) in readers {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = std::io::BufReader::new(reader);
            let mut raw: Vec<u8> = Vec::new();
            // 按字节读行再 lossy：编排程序偶尔吐出非 UTF-8 的 PVF 片段，
            // read_line 会当场报 InvalidData，整路输出就断在这里了。
            while let Ok(n) = buf.read_until(b'\n', &mut raw) {
                if n == 0 {
                    break;
                }
                let text = String::from_utf8_lossy(&raw).trim_end_matches(['\r', '\n']).to_string();
                raw.clear();
                if tx.send(Msg::Line { stream, text }).is_err() {
                    break;
                }
            }
        });
    }
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Done(child.wait().map(|status| status.success())));
        });
    }
    drop(tx);

    let deadline = Instant::now() + timeout;
    let mut parts: [String; 2] = [String::new(), String::new()];
    let mut status: Option<Result<bool, std::io::Error>> = None;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left) {
            Ok(Msg::Line { stream, text }) => {
                parts[stream as usize].push_str(&text);
                parts[stream as usize].push('\n');
                on_line(&text);
            }
            Ok(Msg::Done(result)) => status = Some(result),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
        }
    }
    let Some(status) = status else {
        kill_tree(pid);
        return Err(format!("{label} 超时（{} 秒）", timeout.as_secs()));
    };
    let mut text = parts[0].clone();
    text.push_str(&parts[1]);
    status.map(|ok| (ok, text)).map_err(|err| format!("{label} 执行失败：{err}"))
}
