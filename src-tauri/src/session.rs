//! 会话状态机：一次点击 → 一条 `dfolauncher.exe launch --root <root>`。
//!
//! 真正的网关、网络隔离与客户端拉起都在那个编排 CLI 里；启动器负责的是它周围那一圈：
//! 闸门、环境变量注入、逐行日志转述、就绪判定、失败时把证据带回来、以及关游戏后的自动停服。
//! 这些一件都不能少 —— 少了就绪判定就会「报成功但游戏没起来」，
//! 少了自动停服就会留一屋子残留进程。

use std::collections::VecDeque;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::config::{self, Settings, StoredRoute};
use crate::logbus::Bus;
use crate::paths::{has_client_files, Root, CLIENT_IMAGE_NAME, GATEWAY_PORT, SERVER_IMAGE_NAMES};
use crate::portguard::{self, Report};
use crate::ports;
use crate::procutil;
use crate::report;

const SESSION_LAUNCH_TIMEOUT: Duration = Duration::from_secs(180);
const POLL_STEP: Duration = Duration::from_millis(500);
const STOP_SCRIPT_TIMEOUT: Duration = Duration::from_secs(90);
const STOP_PORT_BUDGET: Duration = Duration::from_secs(10);
const STOP_PORT_STEP: Duration = Duration::from_millis(250);
const ORCHESTRATOR_TAIL_LINES: usize = 20;
const VERIFY_BUDGET: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Idle,
    Starting,
    Running,
    Stopping,
    Error,
}

impl Phase {
    fn busy(self) -> bool {
        matches!(self, Phase::Starting | Phase::Stopping)
    }
}

impl Default for Phase {
    fn default() -> Phase {
        Phase::Idle
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 原生启动：网关 + 客户端。
    Compat,
    /// 只起服务端，不起客户端（调试用）。
    ServerOnly,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Mode::Compat => "原生启动（网关 + 客户端）",
            Mode::ServerOnly => "仅服务端",
        }
    }

    pub(crate) fn from_param(value: Option<&str>) -> Mode {
        match value {
            Some("server-only") => Mode::ServerOnly,
            _ => Mode::Compat,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub phase: Phase,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct StartOutcome {
    pub started: bool,
    pub phase: Phase,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Readiness {
    pub can_start: bool,
    pub detail: String,
    pub root: String,
    pub root_source: String,
    pub client_ready: bool,
    pub client_dir: String,
    pub server_ready: bool,
    pub server_detail: String,
    pub storage_ready: bool,
    pub storage_detail: String,
    pub gateway_listening: bool,
}

#[derive(Default)]
struct State {
    phase: Phase,
    detail: String,
    run_dir: Option<PathBuf>,
    client_pid: Option<u32>,
    /// 客户端由编排程序拉起，启动器不持有它的出生句柄，停止时只能按镜像名收。
    client_from_orchestrator: bool,
}

#[derive(Clone)]
pub struct Session {
    inner: Arc<Inner>,
}

struct Inner {
    root: Root,
    bus: Arc<Bus>,
    state: Mutex<State>,
    /// 每次「开始游戏 / 停止游戏」自增：正在跑的启动流程下一次轮询发现对不上就退出。
    epoch: AtomicU64,
    watcher_cancel: Arc<AtomicBool>,
    app: Mutex<Option<AppHandle>>,
    reporter: Mutex<Option<Arc<report::Reporter>>>,
    /// 编排程序的 `--help` 探测结果，按「大小 ^ 修改时间」缓存，换一份 CLI 就重测。
    orchestrator_probe: Mutex<Option<(u64, bool)>>,
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Inner {
    fn lock_state(&self) -> MutexGuard<'_, State> {
        locked(&self.state)
    }
}

impl Session {
    pub fn new(root: Root, bus: Arc<Bus>) -> Session {
        Session {
            inner: Arc::new(Inner {
                root,
                bus,
                state: Mutex::new(State {
                    phase: Phase::Idle,
                    detail: "未运行".to_string(),
                    ..Default::default()
                }),
                epoch: AtomicU64::new(0),
                watcher_cancel: Arc::new(AtomicBool::new(false)),
                app: Mutex::new(None),
                reporter: Mutex::new(None),
                orchestrator_probe: Mutex::new(None),
            }),
        }
    }

    pub fn attach(&self, app: AppHandle) {
        *locked(&self.inner.app) = Some(app);
    }

    pub fn attach_reporter(&self, reporter: Arc<report::Reporter>) {
        *locked(&self.inner.reporter) = Some(reporter);
    }

    /// 把故障交给上报器；没接上（比如 setup 之前就炸了）就当没这回事。
    fn report_event(&self, event: report::Event) {
        if let Some(reporter) = locked(&self.inner.reporter).clone() {
            reporter.report(event);
        }
    }

    /// 这一场的错误日志，作为报告的随附附件（上报器会再做一次大小与编码过滤）。
    fn run_logs(&self) -> Vec<(String, PathBuf)> {
        let Some(run_dir) = self.inner.root.newest_run_dir() else {
            return Vec::new();
        };
        ["gateway.err", "helper.err"]
            .into_iter()
            .map(|name| (name.to_string(), run_dir.join(name)))
            .filter(|(_, path)| path.is_file())
            .collect()
    }

    pub fn root(&self) -> Root {
        self.inner.root.clone()
    }

    pub fn bus(&self) -> Arc<Bus> {
        self.inner.bus.clone()
    }

    fn emit<T: Serialize>(&self, event: &str, payload: &T) {
        if let Some(app) = locked(&self.inner.app).as_ref() {
            let _ = app.emit(event, payload);
        }
    }

    fn set(&self, phase: Phase, detail: impl Into<String>) {
        let detail = detail.into();
        {
            let mut state = locked(&self.inner.state);
            state.phase = phase;
            state.detail = detail.clone();
        }
        self.emit(
            "game-phase",
            &Status {
                phase,
                detail: detail.clone(),
            },
        );
        self.emit("readiness-changed", &self.readiness());
    }

    pub fn status(&self) -> Status {
        let mut state = locked(&self.inner.state);
        // 以盘上事实为准：网关在监听但状态机还停在 idle，说明这一场会话不是本启动器起的。
        if state.phase == Phase::Idle && ports::listening(GATEWAY_PORT) {
            state.phase = Phase::Running;
            state.detail = "运行中（网关已在监听，非本启动器发起）".to_string();
        }
        Status {
            phase: state.phase,
            detail: state.detail.clone(),
        }
    }

    fn alert(&self, text: impl Into<String>) {
        let text = text.into();
        self.inner.bus.warn(text.clone());
        self.emit("game-alert", &AlertPayload { text });
    }

    // ------------------------------------------------------------------ 启动

    pub fn start(&self, mode: Mode) -> Result<StartOutcome, String> {
        let current = self.status();
        if current.phase.busy() {
            return Err(format!("已有任务进行中（{}）", current.detail));
        }
        // running 也算「不能再点一次」：否则会在同一场会话上再叠一场。
        if current.phase == Phase::Running {
            return Err("已经有会话在运行，请先点「停止游戏」".to_string());
        }
        let epoch = self.inner.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        self.set(Phase::Starting, "准备启动");

        let settings = config::settings(&self.inner.root);
        self.inner.watcher_cancel.store(false, Ordering::SeqCst);
        {
            let mut state = locked(&self.inner.state);
            state.client_from_orchestrator = mode == Mode::Compat;
        }

        // 监听线程要在拉起会话之前就开始：客户端秒退的那一种也要能触发自动停服。
        if mode == Mode::Compat {
            let cancel = self.inner.watcher_cancel.clone();
            let session = self.clone();
            std::thread::spawn(move || session.watch_client_exit(epoch, settings.auto_stop_on_client_exit, cancel));
        }

        match self.start_flow(mode, epoch, &settings) {
            Ok(detail) => {
                self.set(Phase::Running, detail.clone());
                // 游戏跑起来了，更新包 zip 缓存可以删了。
                crate::updater::cleanup_download_cache();
                crate::self_updater::cleanup_self_update_cache();
                Ok(StartOutcome {
                    started: true,
                    phase: Phase::Running,
                    detail,
                })
            }
            Err(err) => {
                self.inner.watcher_cancel.store(true, Ordering::SeqCst);
                self.set(Phase::Error, err.clone());
                let mut event = report::Event::new("game_start_failed", "游戏启动失败", &err);
                event.severity = "无法继续游戏";
                event.logs = self.run_logs();
                self.report_event(event);
                Err(err)
            }
        }
    }

    fn start_flow(&self, mode: Mode, epoch: u64, settings: &Settings) -> Result<String, String> {
        let root = self.inner.root.clone();
        let bus = self.inner.bus.clone();

        // 1. 存档路由：只在缺 sqlite_path 时落一份，绝不覆盖玩家手上的配置。
        let StoredRoute { describe, created, .. } = config::ensure_storage_route(&root)?;
        if created {
            bus.ok(format!("存档：已落盘 SQLite 档 —— {describe}"));
        } else {
            bus.info(format!("存档：{describe}"));
        }

        // 2. 客户端必备件（sk.dat 是可选件，不在这里判：需要它时编排程序会报精确错误）。
        let client_dir = config::client_dir(&root);
        if !has_client_files(&client_dir) {
            return Err(format!(
                "还没有配置好游戏客户端：请先点「选择 DFO.exe…」选中游戏目录里的 DFO.exe\
                 （它旁边应有 Script.pvf）。当前指向：{}",
                client_dir.display()
            ));
        }

        // 3. 编排 CLI 在不在、支不支持这条子命令。
        self.require_orchestrator("launch")?;

        // 4. 端口闸门。
        self.run_start_gate(&root)?;

        // 5. 环境变量。
        let env = self.launch_env(mode, settings);
        bus.info(format!("本次以「{}」模式启动", config::game_mode_label(&settings.game_mode)));
        if settings.auto_stop_on_client_exit && mode == Mode::Compat {
            bus.info("已开启「关闭游戏后自动停止服务端」：退出游戏客户端后会自动停掉服务端环境");
        }

        // 6. 闸门之后复核一次：闸门到现在的几百毫秒里可能刚好有人绑上端口。
        if ports::listening(GATEWAY_PORT) {
            return Err(format!(
                "游戏网关端口 {GATEWAY_PORT} 已在监听：可能已有环境在运行，请先停止并检查残留进程"
            ));
        }

        self.set(Phase::Starting, "启动游戏网关与客户端");
        let trace_before = client_trace_mtime();
        let detail = self.run_orchestrator(mode, epoch, env, &root, &bus)?;

        let session = self.clone();
        std::thread::spawn(move || session.verify_client(mode, trace_before));
        Ok(detail)
    }

    fn launch_env(&self, mode: Mode, settings: &Settings) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = Vec::new();
        if let Some((key, value)) = config::game_mode_env(&settings.game_mode) {
            env.push((key.to_string(), value.to_string()));
        }
        // "probe" 本来就是客户端载荷里的默认账号，特意写回去反而让载荷与历史会话不一致。
        if settings.account != "probe" && config::valid_account(&settings.account) {
            env.push(("DFO_ACCOUNT".to_string(), settings.account.clone()));
            self.inner
                .bus
                .info(format!("本次以账号「{}」登录…", settings.account));
        } else {
            self.inner.bus.info("本次以默认账号「probe」登录");
        }
        env.push((
            "DFO_REWARD_SCRIPTS_DIR".to_string(),
            self.inner.root.mods_dir().to_string_lossy().into_owned(),
        ));
        // 服务端读的是 `DFO_AUTO_PICKUP == "1"`，且只对环回连接生效；其余值一律算关。
        let auto_pickup = if settings.auto_pickup { "1" } else { "0" };
        env.push(("DFO_AUTO_PICKUP".to_string(), auto_pickup.to_string()));
        self.inner.bus.info(if settings.auto_pickup {
            "本次开启自动拾取（仅本机连接生效）"
        } else {
            "本次关闭自动拾取"
        });
        if mode == Mode::ServerOnly {
            self.inner.bus.info("本次只启动服务端，不拉起游戏客户端");
        }
        env
    }

    fn require_orchestrator(&self, subcommand: &str) -> Result<(), String> {
        let exe = self.inner.root.orchestrator_exe();
        if !exe.is_file() {
            return Err(orchestrator_missing_error(&exe, subcommand));
        }
        let fingerprint = std::fs::metadata(&exe)
            .map(|meta| meta.len() ^ modified_secs(&meta))
            .unwrap_or_default();
        let cached_ok = {
            let cached = locked(&self.inner.orchestrator_probe);
            cached.as_ref().is_some_and(|(seen, ok)| *seen == fingerprint && *ok)
        };
        if cached_ok {
            return Ok(());
        }
        let help = procutil::run_capture(&exe, &["--help"], Duration::from_secs(10))
            .map_err(|err| format!("编排程序无响应：{err}"))?;
        let supported = help
            .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-')
            .any(|token| token == subcommand);
        *locked(&self.inner.orchestrator_probe) = Some((fingerprint, supported));
        if !supported {
            return Err(orchestrator_missing_error(&exe, subcommand));
        }
        Ok(())
    }

    fn run_start_gate(&self, root: &Root) -> Result<(), String> {
        let bus = self.inner.bus.clone();
        let mut report = portguard::check(root);
        if report.blocked_now() {
            let (cleaned, acted) = portguard::auto_clean_own(root);
            if acted {
                if !cleaned.killed.is_empty() {
                    bus.ok(format!("启动前清理本项目残留进程 PID {:?}", cleaned.killed));
                }
                if !cleaned.failed.is_empty() {
                    bus.warn(format!(
                        "启动前清理：有 {} 个残留进程没能结束：{}",
                        cleaned.failed.len(),
                        cleaned.failed.join("、")
                    ));
                }
                report = cleaned;
            }
        }
        if report.blocked_now() {
            let hint = portguard::cleanup_hint(root, &report);
            bus.error(format!("启动前校验未通过：{}", report.summary));
            bus.error(hint.clone());
            self.emit_port_blocked("start", &report, &hint);
            return Err(format!("启动前校验未通过：{}", report.summary));
        }
        if !report.leftovers.is_empty() {
            // 没占端口但有自己人的残留：照样报出来，它可能马上就绑上端口。
            bus.warn(format!("启动前校验：{}", report.summary));
        }
        Ok(())
    }

    fn emit_port_blocked(&self, where_: &str, report: &Report, hint: &str) {
        self.emit(
            "game-port-blocked",
            &PortBlockedPayload {
                r#where: where_,
                report,
                hint,
            },
        );
    }

    /// 唯一真正开会话的地方：起编排 CLI，逐行转述它的输出，然后盯就绪信号。
    fn run_orchestrator(
        &self,
        mode: Mode,
        epoch: u64,
        env: Vec<(String, String)>,
        root: &Root,
        bus: &Arc<Bus>,
    ) -> Result<String, String> {
        let exe = root.orchestrator_exe();
        let mut args: Vec<String> =
            vec!["launch".into(), "--root".into(), root.path.display().to_string()];
        if mode == Mode::ServerOnly {
            args.push("--server-only".into());
        }
        bus.info(format!("[启动] 会话编排：{}", mode.label()));

        let mut cmd = procutil::command(&exe);
        cmd.args(&args)
            .current_dir(&root.path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in &env {
            cmd.env(key, value);
        }

        let started_at = Instant::now();
        let wall_launched_at = SystemTime::now();
        let mut child = cmd
            .spawn()
            .map_err(|err| format!("无法启动会话编排 CLI（{}）：{err}", exe.display()))?;

        let stdout = child.stdout.take().expect("刚才已 piped");
        let stderr = child.stderr.take().expect("刚才已 piped");
        let relay = LogRelay {
            bus: bus.clone(),
            tail: Arc::new(Mutex::new(VecDeque::new())),
            entered: Arc::new(AtomicBool::new(false)),
            session: self.clone(),
        };
        relay.clone().spawn(stdout);
        relay.clone().spawn(stderr);

        let mut gateway_seen = false;
        loop {
            if self.inner.epoch.load(Ordering::SeqCst) != epoch {
                let _ = child.kill();
                return Err("启动已被「停止游戏」取消".to_string());
            }
            if !gateway_seen && ports::listening(GATEWAY_PORT) {
                gateway_seen = true;
                self.set(Phase::Starting, "网关已就绪，正在拉起客户端");
            }
            let client_pids = procutil::pids_by_name(CLIENT_IMAGE_NAME);
            if gateway_seen && (mode == Mode::ServerOnly || !client_pids.is_empty()) {
                // 交接：会话宿主继续在后台跑，客户端关闭后由它自己拆隔离、停网关。
                self.inner.lock_state().client_pid = client_pids.first().copied();
                bus.ok(format!(
                    "会话编排：完整链路已在后台运行（网关 {GATEWAY_PORT} 已监听，{}已就绪）",
                    if mode == Mode::ServerOnly {
                        ""
                    } else {
                        "游戏客户端"
                    }
                ));
                bus.info("客户端关闭后由编排程序自己拆除网络隔离并停止网关（也可点「停止游戏」立即收尾）");
                return Ok(match mode {
                    Mode::Compat => "运行中".to_string(),
                    Mode::ServerOnly => "服务端运行中".to_string(),
                });
            }
            if let Ok(Some(exit)) = child.try_wait() {
                let lines = relay.tail_lines();
                return Err(self.session_exited_error(
                    exit.code(),
                    relay.entered(),
                    &lines,
                    root,
                    wall_launched_at,
                ));
            }
            if started_at.elapsed() > SESSION_LAUNCH_TIMEOUT {
                let lines = relay.tail_lines();
                let evidence = evidence_tail(root, wall_launched_at);
                return Err(format!(
                    "启动超时（{} 秒未完成），请检查运行日志{evidence}{}",
                    SESSION_LAUNCH_TIMEOUT.as_secs(),
                    tail_block(&lines)
                ));
            }
            std::thread::sleep(POLL_STEP);
        }
    }

    fn session_exited_error(
        &self,
        code: Option<i32>,
        entered_session: bool,
        lines: &[String],
        root: &Root,
        wall_launched_at: SystemTime,
    ) -> String {
        let evidence = evidence_tail(root, wall_launched_at);
        let mut message = if entered_session {
            format!("会话编排异常退出（退出码 {}）{evidence}{}", code_text(code), tail_block(lines))
        } else {
            format!(
                "会话编排异常退出（尚未进入会话，CLI：{}，退出码 {}）{evidence}{}{}",
                root.orchestrator_exe().display(),
                code_text(code),
                tail_block(lines),
                missing_files_hint(root)
            )
        };
        trim_to(&mut message, 800);
        message
    }

    // ------------------------------------------------------------------ 就绪复核

    /// 「started=true」不等于画面里有人：再用 30 秒确认客户端真的起来了。
    fn verify_client(&self, mode: Mode, trace_before: Option<SystemTime>) {
        let root = self.inner.root.clone();
        let deadline = Instant::now() + VERIFY_BUDGET;
        while Instant::now() < deadline {
            let pids = procutil::pids_by_name(CLIENT_IMAGE_NAME);
            if !pids.is_empty() {
                self.inner.bus.ok(format!(
                    "已确认游戏客户端正在运行（{CLIENT_IMAGE_NAME} · PID {}）",
                    pids[0]
                ));
                self.inner.lock_state().client_pid = Some(pids[0]);
                return;
            }
            if let Some(run_dir) = self.run_dir(&root) {
                let log = run_dir.join("client.log");
                if log.is_file() {
                    self.inner
                        .bus
                        .ok(format!("已确认客户端启动记录：{}", log.display()));
                    return;
                }
            }
            if let (Some(before), Some(now)) = (trace_before, client_trace_mtime()) {
                if now > before {
                    self.inner
                        .bus
                        .ok("已确认客户端写入了运行痕迹（DFO.trc 已更新）");
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(1000));
        }
        if mode == Mode::ServerOnly || self.status().phase != Phase::Running {
            return;
        }
        let trace = client_trace_path().map(|p| p.display().to_string())
            .unwrap_or_else(|| "%USERPROFILE%\\AppData\\LocalLow\\DNF\\DFO.trc".to_string());
        self.alert(format!(
            "未能确认游戏客户端已启动：没有 {CLIENT_IMAGE_NAME} 进程，也没有新的客户端日志（{trace} 未更新）"
        ));
        self.set(Phase::Running, "未见客户端，详见运行日志");
    }

    fn run_dir(&self, root: &Root) -> Option<PathBuf> {
        self.inner
            .lock_state()
            .run_dir
            .clone()
            .or_else(|| root.newest_run_dir())
    }

    // ------------------------------------------------------------------ 自动停服监听

    /// 先锁 PID（进程句柄判活，150ms 一轮），锁不到就退回按名字轮询（先见后判）。
    fn watch_client_exit(&self, epoch: u64, auto_stop: bool, cancel: Arc<AtomicBool>) {
        let stale_or_cancelled = || {
            cancel.load(Ordering::SeqCst) || self.inner.epoch.load(Ordering::SeqCst) != epoch
        };

        // 锁 PID 的窗口要跟就绪探测同一口径：内层 PVF 预备 + 网关绑 7001 实测要 70 s
        // （23:31:24 点下 → 23:32:34 才起客户端），60 s 会在 DFO.exe 出现之前就关掉，
        // 于是永远锁不到 PID、只能走下面那条按名字轮询的兜底路。
        let probe_deadline = Instant::now() + Duration::from_secs(180);
        let mut pid: Option<u32> = None;
        while Instant::now() < probe_deadline {
            if stale_or_cancelled() {
                return;
            }
            if let Some(found) = procutil::pids_by_name(CLIENT_IMAGE_NAME).first().copied() {
                pid = Some(found);
                break;
            }
            std::thread::sleep(Duration::from_millis(120));
        }

        if let Some(pid) = pid {
            if auto_stop {
                self.inner
                    .bus
                    .info(format!("已锁定客户端 PID {pid}，退出后将自动停止服务端"));
            }
            // 句柄只能现在就开：进程一没，按 PID 再也开不出来，退出码跟着一起没了。
            let handle = procutil::ProcHandle::open(pid);
            let mut code: Option<u32> = None;
            while !stale_or_cancelled() {
                match &handle {
                    Some(h) => {
                        if h.wait_exit(Duration::from_millis(200)) {
                            code = h.exit_code();
                            break;
                        }
                    }
                    // 开不出来就退回原来的轮询：拿不到退出码，后面就不作崩溃认定。
                    None => {
                        if !procutil::is_running(pid) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(150));
                    }
                }
            }
            // 是「停止游戏」按下去的那一种就不算异常：那是玩家自己让结束的。
            if !stale_or_cancelled() {
                self.on_client_closed(code);
                if auto_stop {
                    self.on_client_gone(epoch);
                }
            }
            return;
        }

        // 按名字兜底：必须先看见它活过，再连续 1.2 秒看不见才算退出。
        // 这一路只有「不在了」这一个事实，拿不到退出码，所以不参与崩溃判定。
        // 看见之后就不再设总时限 —— 什么时候关游戏是玩家决定的，固定 180 s 到点就散会
        // 把「玩得比较久」这一整类变成静默不自动停服；只有停止/新会话（epoch 变化）收手。
        let sight_deadline = Instant::now() + Duration::from_secs(180);
        let mut seen_alive = false;
        let mut gone_since: Option<Instant> = None;
        while !stale_or_cancelled() {
            if !procutil::pids_by_name(CLIENT_IMAGE_NAME).is_empty() {
                seen_alive = true;
                gone_since = None;
            } else if seen_alive {
                let since = gone_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_millis(1200) {
                    if auto_stop {
                        self.on_client_gone(epoch);
                    }
                    return;
                }
            } else if Instant::now() >= sight_deadline {
                self.inner
                    .bus
                    .warn("未观察到游戏客户端进程，本次不会自动停止服务端");
                return;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }

    /// 客户端结束了：把退出码和编排程序自己写的那行 SUMMARY 放在一起判，
    /// 只有判成崩溃才上报 —— 正常退出报上去是噪音，也会污染别人的问题计数。
    fn on_client_closed(&self, code: Option<u32>) {
        let run_dir = self.run_dir(&self.inner.root);
        let summary = client_log_summary(run_dir.as_deref());
        if !is_crash(code, summary) {
            return;
        }
        let code = code.unwrap_or_default();
        let named = exit_code_name(code)
            .map(|n| format!("{n}，"))
            .unwrap_or_default();
        let summary_text = match summary {
            Some((exit, normal)) => format!(
                "客户端日志 SUMMARY 记录 exit=0x{exit:x} normal_run={}",
                normal as u8
            ),
            None => "客户端日志没有留下 SUMMARY 行".to_string(),
        };
        let actual = format!(
            "游戏客户端（{CLIENT_IMAGE_NAME}）退出码 0x{code:08X}（{named}{summary_text}）"
        );
        let mut event = report::Event::new("game_crash", "游戏客户端异常退出", &actual);
        event.error_code = Some(format!("0x{code:08X}"));
        event.severity = "无法继续游戏";
        if let Some(dir) = run_dir {
            event.logs = ["client.log", "gateway.err", "helper.err"]
                .into_iter()
                .map(|name| (name.to_string(), dir.join(name)))
                .filter(|(_, path)| path.is_file())
                .collect();
        }
        self.report_event(event);
    }

    fn on_client_gone(&self, epoch: u64) {
        std::thread::sleep(Duration::from_millis(150));
        if self.status().phase != Phase::Running || self.inner.epoch.load(Ordering::SeqCst) != epoch
        {
            return;
        }
        self.alert("已检测到游戏关闭，正在自动停止服务端环境…");
        self.set(Phase::Stopping, "客户端已退出，正在自动停止");
        self.stop_environment();
        self.set(Phase::Idle, "未运行（游戏已关闭）");
    }

    // ------------------------------------------------------------------ 停止

    /// 界面点「停止游戏」立刻返回，拆环境在后台线程走完整条链。
    pub fn stop(&self) -> Result<bool, String> {
        let current = self.status();
        if current.phase == Phase::Stopping {
            return Ok(true);
        }
        if current.phase == Phase::Idle && !ports::listening(GATEWAY_PORT) {
            if procutil::running_named(&SERVER_IMAGE_NAMES).is_empty() {
                return Ok(false);
            }
        }
        self.inner.epoch.fetch_add(1, Ordering::SeqCst);
        self.set(Phase::Stopping, "正在停止环境");
        let session = self.clone();
        std::thread::spawn(move || {
            session.stop_environment();
        });
        Ok(true)
    }

    /// 停止链：任何一步失败都继续往下走 —— 「停不下来」比「停得不干净」糟糕得多。
    pub fn stop_environment(&self) {
        let root = self.inner.root.clone();
        let bus = self.inner.bus.clone();
        self.inner.watcher_cancel.store(true, Ordering::SeqCst);

        let client_pid = self.inner.lock_state().client_pid;
        if let Some(pid) = client_pid {
            if procutil::kill_tree(pid) {
                bus.info(format!("已结束客户端进程树 PID {pid}"));
            }
        }
        if self.inner.lock_state().client_from_orchestrator {
            procutil::kill_by_name(CLIENT_IMAGE_NAME);
        }
        for name in SERVER_IMAGE_NAMES {
            procutil::kill_by_name(name);
        }
        {
            let mut state = self.inner.lock_state();
            state.client_pid = None;
        }

        // 编排程序的 stop：由它自己拆网络隔离、收网关。缺 CLI 也只警告，不让整条链失败。
        let exe = root.orchestrator_exe();
        if exe.is_file() {
            let mut cmd = procutil::command(&exe);
            cmd.args(["stop", "--root", &root.path.display().to_string()])
                .current_dir(&root.path);
            match procutil::run_capture_with(cmd, STOP_SCRIPT_TIMEOUT) {
                Ok(text) => {
                    for line in text.lines().filter(|line| !line.trim().is_empty()) {
                        bus.info(format!("[停止] {}", line.trim_end()));
                    }
                }
                Err(err) => bus.warn(format!("[停止] {err}")),
            }
        } else {
            bus.warn(format!("[停止] {}", orchestrator_missing_error(&exe, "stop")));
        }

        let deadline = Instant::now() + STOP_PORT_BUDGET;
        loop {
            if !ports::listening(GATEWAY_PORT) || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(STOP_PORT_STEP);
        }

        if ports::listening(GATEWAY_PORT) {
            let report = portguard::check(&root);
            let hint = portguard::cleanup_hint(&root, &report);
            bus.error(format!("端口 {GATEWAY_PORT} 仍未释放：{}", report.summary));
            bus.error(hint.clone());
            self.emit_port_blocked("stop", &report, &hint);
            self.set(Phase::Error, format!("端口未释放：{}", report.summary));
            return;
        }
        bus.ok("环境已完全停止");
        {
            let mut state = self.inner.lock_state();
            state.run_dir = None;
        }
        self.set(Phase::Idle, "未运行");
    }

    // ------------------------------------------------------------------ 只读快照

    pub fn readiness(&self) -> Readiness {
        let root = self.inner.root.clone();
        let client_dir = config::client_dir(&root);
        let client_ready = has_client_files(&client_dir);
        let orchestrator = root.orchestrator_exe();
        let server_ready = orchestrator.is_file();
        let storage = config::storage_route_state(&root);
        let storage_ready = matches!(storage, config::StorageRoute::Ready { .. });
        // NeedsInit 不算拦路：真开始游戏时会自动补那一份配置。
        let storage_blocked = matches!(storage, config::StorageRoute::OtherDriver(_));
        let gateway_listening = ports::listening(GATEWAY_PORT);
        let current = self.status();

        let mut blockers: Vec<String> = Vec::new();
        if !server_ready {
            blockers.push(format!("缺少服务端编排程序（{}）", orchestrator.display()));
        }
        if !client_ready {
            blockers.push("未找到游戏客户端（DFO.exe 旁边要有 Script.pvf）".to_string());
        }
        if storage_blocked {
            blockers.push("存档配置写的是别的驱动，需要人工确认".to_string());
        }
        if current.phase == Phase::Idle && gateway_listening {
            blockers.push(format!("网关端口 {GATEWAY_PORT} 已被占用"));
        }
        if current.phase == Phase::Running {
            blockers.push("已经有会话在运行".to_string());
        }

        Readiness {
            can_start: !current.phase.busy() && blockers.is_empty(),
            detail: if blockers.is_empty() {
                "环境就绪".to_string()
            } else {
                blockers.join("；")
            },
            root: root.path.display().to_string(),
            root_source: root.source.to_string(),
            client_ready,
            client_dir: client_dir.display().to_string(),
            server_ready,
            server_detail: if server_ready {
                "编排程序就绪".to_string()
            } else {
                format!("未见 {}", orchestrator.display())
            },
            storage_ready,
            storage_detail: describe_storage(&storage),
            gateway_listening,
        }
    }

    /// 不改状态，只把当前状态再广播一次：后台准备跑完后让首屏立刻是准的。
    pub fn touch_readiness(&self) {
        let current = self.status();
        self.emit("game-phase", &current);
        self.emit("readiness-changed", &self.readiness());
    }
}

#[derive(Serialize)]
struct AlertPayload {
    text: String,
}

#[derive(Serialize)]
struct PortBlockedPayload<'a> {
    r#where: &'a str,
    report: &'a Report,
    hint: &'a str,
}

/// 编排程序输出的逐行转述：进日志总线推给界面，同时留 20 行原文当失败证据。
#[derive(Clone)]
struct LogRelay {
    bus: Arc<Bus>,
    tail: Arc<Mutex<VecDeque<String>>>,
    entered: Arc<AtomicBool>,
    session: Session,
}

impl LogRelay {
    fn spawn(self, stream: impl Read + Send + 'static) {
        std::thread::spawn(move || {
            let mut buffered = std::io::BufReader::new(stream);
            loop {
                let mut line = String::new();
                match buffered.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => self.observe(line),
                }
            }
        });
    }

    fn observe(&self, raw: String) {
        let line = raw.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() {
            return;
        }
        {
            let mut ring = locked(&self.tail);
            if ring.len() >= ORCHESTRATOR_TAIL_LINES {
                ring.pop_front();
            }
            ring.push_back(line.to_string());
        }
        // `Logs:` 这一行是启动器唯一得知本场运行目录的途径；少了它，后面的失败证据就无从可寻。
        if let Some((_, dir)) = line.split_once("Logs:") {
            self.entered.store(true, Ordering::SeqCst);
            let dir = dir.trim();
            if !dir.is_empty() {
                self.set_run_dir(PathBuf::from(dir));
            }
            return;
        }
        if line.contains("Client launch requested") {
            self.entered.store(true, Ordering::SeqCst);
            self.bus.warn("[启动] 已请求启动游戏客户端（不校验画面是否出现，稍后自动核实）");
            return;
        }
        self.bus.info(format!("[启动] {line}"));
    }

    fn set_run_dir(&self, path: PathBuf) {
        let mut state = self.session.inner.lock_state();
        let same = state.run_dir.as_ref().is_some_and(|seen| seen == &path);
        if same {
            return;
        }
        state.run_dir = Some(path.clone());
        drop(state);
        self.bus
            .info(format!("[启动] 本次运行日志目录：{}", path.display()));
    }

    fn tail_lines(&self) -> Vec<String> {
        locked(&self.tail).iter().cloned().collect()
    }

    fn entered(&self) -> bool {
        self.entered.load(Ordering::SeqCst)
    }
}

fn orchestrator_missing_error(exe: &Path, subcommand: &str) -> String {
    format!(
        "缺少服务端 Go 编排 CLI：{} 不存在、或该版本不支持 {subcommand} 子命令；\
         请更新服务端程序（整合包内 server\\work\\dfo-lan\\bin 的那一份）",
        exe.display()
    )
}

fn missing_files_hint(root: &Root) -> String {
    let mut missing: Vec<String> = Vec::new();
    if !root.orchestrator_exe().is_file() {
        missing.push(format!("编排程序 {}", root.orchestrator_exe().display()));
    }
    let gateway = config::server_binary(root);
    if !gateway.is_file() {
        missing.push(format!("网关程序 {}", gateway.display()));
    }
    let profile = root.work_dir().join("configs").join("pvf-default.json");
    if !profile.is_file() {
        missing.push(format!("服务端配置 {}", profile.display()));
    }
    if missing.is_empty() {
        return String::new();
    }
    format!(
        " —— 这些文件不在位：{}。它们由整合包提供，启动器不会替你下载或编译。",
        missing.join("、")
    )
}

/// 失败时把编排程序自己的错误文件尾巴带回来，但只认本场时间戳之后的，
/// 免得把上一轮的旧故障说成是这一次的。
/// 编排程序收尾时往 `client.log` 写的那一行：`… SUMMARY exit=0x0 normal_run=1`。
/// 它是「这一场到底算不算正常结束」的现场记录，比启动器在外面猜可靠。
fn client_log_summary(run_dir: Option<&Path>) -> Option<(u32, bool)> {
    let raw = std::fs::read(run_dir?.join("client.log")).ok()?;
    let text = String::from_utf8_lossy(&raw);
    for line in text.lines().rev() {
        let Some(rest) = line.split("SUMMARY").nth(1) else {
            continue;
        };
        let exit = rest
            .split_whitespace()
            .find(|field| field.starts_with("exit=0x"))
            .and_then(|field| u32::from_str_radix(&field[7..], 16).ok())?;
        return Some((exit, rest.contains("normal_run=1")));
    }
    None
}

/// 退出了不等于崩了：要退出码与客户端日志两头都对得上才算。
fn is_crash(code: Option<u32>, summary: Option<(u32, bool)>) -> bool {
    match (code, summary) {
        // 没有退出码就只剩「不在了」这一个事实，不足以认定崩溃。
        (None, _) => false,
        // 现场日志自己说这一场是正常收场。
        (_, Some((_, true))) => false,
        (Some(0), _) => false,
        // 日志说不正常，码也非零。
        (Some(_), Some(_)) => true,
        // 只剩退出码时，只认 NTSTATUS 失败位那一头（0xC0000000 以上）。
        (Some(code), None) => code >= 0xC000_0000,
    }
}

/// 只收实战里真会撞上的几种，其余原样报十六进制。
fn exit_code_name(code: u32) -> Option<&'static str> {
    match code {
        0xC000_0005 => Some("内存访问违例"),
        0xC000_00FD => Some("栈溢出"),
        0xC000_0409 => Some("安全检查失败（fastfail）"),
        0xC000_041D => Some("未处理的回调异常"),
        0xE06D_7363 => Some("未捕获的 C++ 异常"),
        _ => None,
    }
}

fn evidence_tail(root: &Root, launched_at: SystemTime) -> String {
    let Some(run_dir) = root.newest_run_dir() else {
        return String::new();
    };
    let fresh = run_dir
        .metadata()
        .and_then(|meta| meta.modified())
        .map(|mtime| mtime >= launched_at)
        .unwrap_or(false);
    if !fresh {
        return String::new();
    }
    let mut out = String::new();
    for name in ["gateway.err", "helper.err"] {
        let path = run_dir.join(name);
        let Ok(raw) = std::fs::read(&path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&raw);
        let all: Vec<&str> = text.lines().collect();
        let clipped = &all[all.len().saturating_sub(8)..];
        if clipped.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "\n（{name} 末尾 {} 行）\n{}",
            clipped.len(),
            clipped.join("\n")
        ));
    }
    out
}

fn tail_block(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    format!("（编排输出原文）\n{}", lines.join("\n"))
}

fn code_text(code: Option<i32>) -> String {
    code.map(|c| c.to_string())
        .unwrap_or_else(|| "未知".to_string())
}

fn client_trace_path() -> Option<PathBuf> {
    let profile = std::env::var("USERPROFILE").ok()?;
    Some(
        Path::new(&profile)
            .join("AppData")
            .join("LocalLow")
            .join("DNF")
            .join("DFO.trc"),
    )
}

fn client_trace_mtime() -> Option<SystemTime> {
    client_trace_path()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok())
}

fn trim_to(text: &mut String, max_chars: usize) {
    let count = text.chars().count();
    if count <= max_chars {
        return;
    }
    let keep: String = text.chars().take(max_chars).collect();
    *text = format!("{keep}…");
}

fn modified_secs(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn describe_storage(state: &config::StorageRoute) -> String {
    match state {
        config::StorageRoute::Ready { path } => format!("SQLite（{}）", path.display()),
        config::StorageRoute::NeedsInit => "尚未初始化（点开始游戏会自动落配置）".to_string(),
        config::StorageRoute::OtherDriver(driver) => format!("非 SQLite 档（driver={driver}）"),
    }
}
