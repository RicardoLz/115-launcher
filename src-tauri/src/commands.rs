//! 界面能调用的全部命令。语义与旧版 webview 桥一致，
//! 但去掉了那层 `rpc.result` 往返 —— Tauri 的 command 本身就是异步的。

use std::path::PathBuf;

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, FilePath};

use crate::autofire::{self, Engine as AutofireEngine};
use crate::config::{self, valid_account};
use crate::health;
use crate::logbus::Line;
use crate::mods;
use crate::paths::{self, Root, CLIENT_IMAGE_NAME};
use crate::portguard::{self, Report};
use crate::procutil;
use crate::report;
use crate::saves;
use crate::session::{self, Mode, Phase, Readiness, Session, StartOutcome, Status};
use crate::self_updater;
use crate::site;
use crate::updater;

pub const APP_NAME: &str = "DFO-115US 启动器";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
pub struct AppState {
    pub session: Session,
    pub reporter: std::sync::Arc<report::Reporter>,
    pub autofire: std::sync::Arc<AutofireEngine>,
    pub app_dir: PathBuf,
    pub root: Root,
}

#[derive(Serialize)]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub root: String,
    pub root_source: String,
    pub app_dir: String,
    pub log_dir: String,
}

#[derive(Serialize)]
pub struct ConfigView {
    pub account: String,
    pub accounts: Vec<String>,
    pub game_mode: String,
    pub game_modes: Vec<ModeOption>,
    pub auto_stop_on_client_exit: bool,
    pub auto_pickup: bool,
    pub auto_report: bool,
    pub auto_report_prompted: bool,
    pub pending_reports: usize,
    pub client_dir: String,
    pub client_ready: bool,
    pub storage_detail: String,
}

#[derive(Serialize)]
pub struct ModeOption {
    pub value: String,
    pub label: String,
}

#[derive(Serialize)]
pub struct ActionRecord {
    pub message: String,
}

/// 只要结果的长任务：丢到 blocking 线程池，主线程一卡窗口就「未响应」。
async fn block<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|err| format!("后台任务异常终止：{err}"))
}

/// 本身就会失败的长任务：两层结果压成一层。
async fn join<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    match tauri::async_runtime::spawn_blocking(f).await {
        Ok(inner) => inner,
        Err(err) => Err(format!("后台任务异常终止：{err}")),
    }
}

fn describe_storage(state: &config::StorageRoute) -> String {
    match state {
        config::StorageRoute::Ready { path } => format!("SQLite（{}）", path.display()),
        config::StorageRoute::NeedsInit => "尚未初始化".to_string(),
        config::StorageRoute::OtherDriver(driver) => format!("driver={driver}"),
    }
}

/// 与其余多数命令一样走 `async`：页脚的「整合包：…」一直停在 `??` 兜底上没被替换过，
/// 而这条当时是全文件少数几条同步命令之一（另两条见 `log_snapshot`、`reports_retry`）。
#[tauri::command]
pub async fn app_info(state: tauri::State<'_, AppState>) -> Result<AppInfo, String> {
    Ok(AppInfo {
        name: APP_NAME,
        version: APP_VERSION,
        root: state.root.path.display().to_string(),
        root_source: state.root.source.to_string(),
        app_dir: state.app_dir.display().to_string(),
        log_dir: state.session.bus().log_dir.display().to_string(),
    })
}

#[tauri::command]
pub fn log_snapshot(state: tauri::State<'_, AppState>, limit: Option<usize>) -> Vec<Line> {
    state.session.bus().snapshot(limit.unwrap_or(600))
}

#[tauri::command]
pub async fn readiness_state(state: tauri::State<'_, AppState>) -> Result<Readiness, String> {
    let session = state.session.clone();
    block(move || session.readiness()).await
}

#[tauri::command]
pub async fn env_check(state: tauri::State<'_, AppState>) -> Result<health::Report, String> {
    let session = state.session.clone();
    block(move || health::check(&session.root())).await
}

#[tauri::command]
pub async fn game_status(state: tauri::State<'_, AppState>) -> Result<Status, String> {
    let session = state.session.clone();
    block(move || session.status()).await
}

#[tauri::command]
pub async fn game_start(
    state: tauri::State<'_, AppState>,
    mode: Option<String>,
) -> Result<StartOutcome, String> {
    let session = state.session.clone();
    let chosen = Mode::from_param(mode.as_deref());
    join(move || session.start(chosen)).await
}

#[tauri::command]
pub async fn game_stop(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let session = state.session.clone();
    join(move || session.stop()).await
}

#[tauri::command]
pub async fn ports_report(state: tauri::State<'_, AppState>) -> Result<Report, String> {
    let session = state.session.clone();
    block(move || portguard::check(&session.root())).await
}

/// 「强制清理并继续」：清完顺手把结果写进日志，方便对着时间线看是谁动的。
#[tauri::command]
pub async fn force_clean(state: tauri::State<'_, AppState>) -> Result<Report, String> {
    let session = state.session.clone();
    block(move || {
        let report = portguard::force_clean(&session.root());
        if report.killed.is_empty() {
            session.bus().warn("强制清理：没有可结束的残留进程");
        } else {
            session
                .bus()
                .ok(format!("强制清理：已结束 PID {:?}", report.killed));
        }
        report
    })
    .await
}

#[tauri::command]
pub async fn config_get(state: tauri::State<'_, AppState>) -> Result<ConfigView, String> {
    let session = state.session.clone();
    let reporter = state.reporter.clone();
    block(move || {
        let root = session.root();
        let settings = config::settings(&root);
        let client_dir = config::client_dir(&root);
        ConfigView {
            account: settings.account,
            accounts: settings.account_used,
            game_mode: settings.game_mode.clone(),
            game_modes: ["auto", "odyssey", "scenario"]
                .iter()
                .map(|value| ModeOption {
                    value: (*value).to_string(),
                    label: config::game_mode_label(value).to_string(),
                })
                .collect(),
            auto_stop_on_client_exit: settings.auto_stop_on_client_exit,
            auto_pickup: settings.auto_pickup,
            auto_report: settings.auto_report,
            auto_report_prompted: settings.auto_report_prompted,
            pending_reports: reporter.pending(),
            storage_detail: describe_storage(&config::storage_route_state(&root)),
            client_ready: paths::has_client_files(&client_dir),
            client_dir: client_dir.display().to_string(),
        }
    })
    .await
}

#[tauri::command]
pub async fn set_account(
    state: tauri::State<'_, AppState>,
    account: String,
) -> Result<ActionRecord, String> {
    let session = state.session.clone();
    let account = account.trim().to_string();
    if !valid_account(&account) {
        return Err(
            "账号名只能用字母、数字、下划线与短横线，长度 1~32（它要进客户端的连接载荷）".to_string(),
        );
    }
    join(move || {
        let root = session.root();
        let mut used = config::settings(&root).account_used;
        if !used.iter().any(|seen| seen == &account) {
            used.insert(0, account.clone());
        }
        used.truncate(20);
        let history = used.clone();
        let name = account.clone();
        config::set_setting(&root, |game| {
            game.insert("account".into(), serde_json::Value::String(name));
            game.insert(
                "account_used".into(),
                serde_json::Value::Array(
                    history
                        .iter()
                        .map(|n| serde_json::Value::String(n.clone()))
                        .collect(),
                ),
            );
        })?;
        session
            .bus()
            .info(format!("登录账号已设为「{account}」，下次「开始游戏」生效"));
        Ok(ActionRecord {
            message: account,
        })
    })
    .await
}

#[tauri::command]
pub async fn set_game_mode(
    state: tauri::State<'_, AppState>,
    mode: String,
) -> Result<ActionRecord, String> {
    let session = state.session.clone();
    if !matches!(mode.as_str(), "auto" | "odyssey" | "scenario") {
        return Err("游戏模式只能是 auto / odyssey / scenario".to_string());
    }
    join(move || {
        let root = session.root();
        config::set_setting(&root, |game| {
            game.insert("mode".into(), serde_json::Value::String(mode.clone()));
        })?;
        let message = config::game_mode_label(&mode).to_string();
        session.bus().info(format!("游戏模式已设为「{message}」"));
        Ok(ActionRecord { message })
    })
    .await
}

#[tauri::command]
pub async fn set_auto_stop(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<ActionRecord, String> {
    let session = state.session.clone();
    join(move || {
        let root = session.root();
        config::set_setting(&root, |game| {
            game.insert("autoStopOnClientExit".into(), serde_json::Value::Bool(enabled));
        })?;
        let message = if enabled {
            "关闭游戏后自动停止服务端（下次「开始游戏」生效）"
        } else {
            "关闭游戏后保持服务端运行（下次「开始游戏」生效）"
        };
        session.bus().info(format!("已选择：{message}"));
        Ok(ActionRecord {
            message: message.to_string(),
        })
    })
    .await
}

/// 自动拾取开关。它是网关进程的环境变量，只能在下次启动时生效。
#[tauri::command]
pub async fn set_auto_pickup(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<ActionRecord, String> {
    let session = state.session.clone();
    join(move || {
        let root = session.root();
        config::set_setting(&root, |game| {
            game.insert("autoPickup".into(), serde_json::Value::Bool(enabled));
        })?;
        let message = if enabled {
            "开启自动拾取（下次「开始游戏」生效）"
        } else {
            "关闭自动拾取（下次「开始游戏」生效）"
        };
        session.bus().info(format!("已选择：{message}"));
        Ok(ActionRecord {
            message: message.to_string(),
        })
    })
    .await
}

/// MOD 页的数据：一次只读扫描（服务端 mod、Lua 脚本、客户端插件、modkit 安装层）。
#[tauri::command]
pub async fn mods_state(state: tauri::State<'_, AppState>) -> Result<mods::ModsState, String> {
    let root = state.root.clone();
    block(move || mods::state(&root)).await
}

/// 勾/撤一个服务端 mod —— 写 `mods/enabled.json` 的禁用名单。
///
/// 服务端只在 boot 时读一次这份名单，所以运行中一律拒绝改：改了不生效，
/// 但界面勾会变，玩家会以为已经生效。
#[tauri::command]
pub async fn set_server_mod(
    state: tauri::State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<ActionRecord, String> {
    let session = state.session.clone();
    let root = state.root.clone();
    let running = matches!(
        session.status().phase,
        Phase::Starting | Phase::Running | Phase::Stopping
    );
    if running {
        return Err(
            "服务端正在运行，mod 名单只在启动时读一次；请先停止服务端再改".to_string(),
        );
    }
    let message = join(move || mods::set_mod_enabled(&root, &id, enabled)).await?;
    session.bus().info(message.clone());
    Ok(ActionRecord { message })
}

/// 客户端 mod 在跑的时候一律拒绝改清单：宿主与统一入口都只在启动时读一次，
/// 而且装进来的模块被 PIN 到进程结束 —— 改了不生效，但界面勾会变，玩家会以为已经生效。
fn client_running_note() -> Option<String> {
    if procutil::running_named(&[CLIENT_IMAGE_NAME]).is_empty() {
        return None;
    }
    Some("游戏客户端正在运行，客户端 mod 清单只在启动时读一次；请先退出游戏再改".to_string())
}

/// 勾/撤一个客户端插件 —— 写 `<客户端>/mods/mods.json` 里那一条的 `enabled`。
#[tauri::command]
pub async fn set_client_mod(
    state: tauri::State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<ActionRecord, String> {
    let session = state.session.clone();
    if let Some(note) = client_running_note() {
        return Err(note);
    }
    let root = state.root.clone();
    let message = join(move || mods::set_client_mod_enabled(&root, &id, enabled)).await?;
    session.bus().info(message.clone());
    Ok(ActionRecord { message })
}

/// 导入一个 mod 目录：选目录 → 整份拷进 `<客户端>/mods/<mod-id>/` → 登记进 mods.json。
#[tauri::command]
pub async fn import_client_mod(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<ActionRecord, String> {
    let picked = block(move || app.dialog().file().blocking_pick_folder()).await?;
    let session = state.session.clone();
    let Some(raw) = picked else {
        return Err("已取消选择".to_string());
    };
    let path = match raw {
        FilePath::Path(path) => path,
        FilePath::Url(url) => return Err(format!("只能选本地目录，收到的是：{url}")),
    };
    if let Some(note) = client_running_note() {
        return Err(note);
    }
    let root = session.root();
    let message = join(move || mods::import_client_mod(&root, &path)).await?;
    session.bus().info(message.clone());
    Ok(ActionRecord { message })
}

/// 自动上报开关。开启前界面必须先展示一次上传范围说明（`prompted` 一并落盘）。
#[tauri::command]
pub async fn set_auto_report(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<ActionRecord, String> {
    let session = state.session.clone();
    join(move || {
        let root = session.root();
        config::set_section(&root, "report", |report| {
            report.insert("autoReport".into(), serde_json::Value::Bool(enabled));
            report.insert("prompted".into(), serde_json::Value::Bool(true));
        })?;
        let message = if enabled {
            "已开启自动上报故障摘要（只送诊断文本，不碰客户端与服务端文件）"
        } else {
            "已关闭自动上报；本机已排队的报告原样保留，不会偷偷发出去"
        };
        session.bus().info(format!("已选择：{message}"));
        Ok(ActionRecord {
            message: message.to_string(),
        })
    })
    .await
}

/// 玩家插上网线后手动催一次队列发送。
#[tauri::command]
pub fn reports_retry(state: tauri::State<'_, AppState>) {
    state.reporter.flush_now();
}

/// 反馈页要画的选项列表和两处预填值，一次取全，免得前端自己抄一份枚举。
#[derive(Serialize)]
pub struct FeedbackMeta {
    pub categories: Vec<String>,
    pub statuses: Vec<String>,
    pub severities: Vec<String>,
    pub frequencies: Vec<String>,
    pub client_version: String,
    pub environment: String,
    pub launcher_version: String,
}

fn strings(list: &[&'static str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

#[tauri::command]
pub async fn feedback_meta(state: tauri::State<'_, AppState>) -> Result<FeedbackMeta, String> {
    let session = state.session.clone();
    block(move || {
        let root = session.root();
        FeedbackMeta {
            categories: strings(&site::CATEGORIES),
            statuses: strings(&site::STATUSES),
            severities: strings(&site::SEVERITIES),
            frequencies: strings(&site::FREQUENCIES),
            client_version: report::client_version(&root),
            environment: report::environment(),
            launcher_version: APP_VERSION.to_string(),
        }
    })
    .await
}

/// 问题看板。`category` / `status` / `q` 空着就是不过滤，`sort` 只认 `popular`。
#[tauri::command]
pub async fn feedback_board(
    page: u32,
    category: String,
    status: String,
    q: String,
    sort: String,
) -> Result<site::Board, String> {
    join(move || site::board(page.max(1), &category, &status, &q, &sort)).await
}

/// 单条问题连同它的处理时间线。
#[tauri::command]
pub async fn feedback_issue(id: String) -> Result<site::Issue, String> {
    join(move || site::issue(&id)).await
}

/// 我的反馈：拿本机留档的（编号, 凭证）去换最新状态。凭证只在 Rust 侧用，不出去。
#[tauri::command]
pub async fn feedback_mine(state: tauri::State<'_, AppState>) -> Result<Vec<site::Mine>, String> {
    let reporter = state.reporter.clone();
    let session = state.session.clone();
    join(move || {
        let pairs = reporter.receipt_pairs();
        let list = site::my_reports(&pairs)?;
        session
            .bus()
            .info(format!("我的反馈：本机存了 {} 个编号，站上认 {} 个", pairs.len(), list.len()));
        Ok(list)
    })
    .await
}

/// 玩家自己在启动器里提一条。成功后凭证落私有留档，日志里只出现编号。
#[tauri::command]
pub async fn feedback_submit(
    state: tauri::State<'_, AppState>,
    input: site::Submission,
) -> Result<site::SubmitOutcome, String> {
    let reporter = state.reporter.clone();
    let session = state.session.clone();
    join(move || {
        let root: Root = session.root();
        let submitted = site::submit(input, &root)?;
        let receipt = submitted.receipt.clone();
        let id = submitted.id.clone();
        let issue_id = submitted
            .issue
            .as_ref()
            .map(|issue| issue.id.clone())
            .unwrap_or_default();
        let outcome = site::SubmitOutcome::from(submitted);
        reporter.save_site_receipt(&id, &receipt, &issue_id)?;
        session.bus().info(format!("反馈已提交，编号 {id}"));
        Ok(outcome)
    })
    .await
}

/// 手动指一次客户端目录（整合包结构被动过时才需要）。
#[tauri::command]
pub async fn pick_client_dir(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<ActionRecord, String> {
    let picked = block(move || app.dialog().file().blocking_pick_folder()).await?;
    let session = state.session.clone();
    let Some(raw) = picked else {
        return Err("已取消选择".to_string());
    };
    let path = match raw {
        FilePath::Path(path) => path,
        FilePath::Url(url) => return Err(format!("只能选本地目录，收到的是：{url}")),
    };
    if !paths::has_client_files(&path) {
        return Err(format!(
            "{} 里没有 DFO.exe + Script.pvf，不像游戏客户端目录",
            path.display()
        ));
    }
    join(move || {
        let root = session.root();
        let saved = config::set_client_dir(&root, &path)?;
        session
            .bus()
            .ok(format!("游戏客户端目录已设为：{}", saved.display()));
        Ok(ActionRecord {
            message: saved.display().to_string(),
        })
    })
    .await
}

/// 按常见布局自动找一遍客户端目录，找到就写进配置。
#[tauri::command]
pub async fn discover_client(state: tauri::State<'_, AppState>) -> Result<ActionRecord, String> {
    let session = state.session.clone();
    join(move || {
        let root = session.root();
        let found = config::discover_client(&root)
            .ok_or_else(|| "没能自动找到客户端目录（要含 DFO.exe + Script.pvf）；请手动选一次".to_string())?;
        let saved = config::set_client_dir(&root, &found)?;
        session
            .bus()
            .ok(format!("已识别游戏客户端：{}", saved.display()));
        Ok(ActionRecord {
            message: saved.display().to_string(),
        })
    })
    .await
}

/// 退出前的二选一：停止环境再退 / 保持环境运行直接退。「返回」由界面自己关掉弹层，不走这条命令。
#[tauri::command]
pub async fn quit_with_choice(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    choice: String,
) -> Result<ActionRecord, String> {
    match choice.as_str() {
        "stop" => {
            let session = state.session.clone();
            join(move || session.stop()).await?;
            // 拆除是后台走的，等到状态机回到 idle 再退，别把残留留在那儿。
            let waiter = state.session.clone();
            block(move || {
                for _ in 0..80 {
                    if waiter.status().phase == session::Phase::Idle {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            })
            .await?;
        }
        "keep" => {}
        other => return Err(format!("未知的选择：{other}")),
    }
    state.session.bus().info("启动器退出");
    app.exit(0);
    Ok(ActionRecord {
        message: "已退出".to_string(),
    })
}

// ---------------------------------------------------------------- 工具：连发

#[derive(Serialize)]
pub struct AutofireView {
    pub config: autofire::Config,
    /// 键下拉的全部候选，由后端给（含旧版那套名字，玩家手填的十六进制也认）。
    pub keys: Vec<autofire::KeyOption>,
    pub live: autofire::State,
}

#[tauri::command]
pub async fn autofire_get(state: tauri::State<'_, AppState>) -> Result<AutofireView, String> {
    let root = state.root.clone();
    let engine = state.autofire.clone();
    block(move || AutofireView {
        config: autofire::load(&root),
        keys: autofire::key_options(),
        live: engine.state(),
    })
    .await
}

/// 本场实况：界面 1s 轮一次，够玩家看清「热键开着没 / 焦点在游戏里没 / 打了多少下」。
/// 诊断话的落日志归引擎自己的线程管（`autofire::flusher_loop`），这里只抄状态——
/// 以前靠这个轮询顺手抽走，页面没开着就等于那一整场什么都没记下来。
#[tauri::command]
pub async fn autofire_live(state: tauri::State<'_, AppState>) -> Result<autofire::State, String> {
    let engine = state.autofire.clone();
    block(move || engine.state()).await
}

/// 界面那个开关点的就是引擎里的总开关，和按一下热键落到同一个状态。
#[tauri::command]
pub async fn autofire_set_master(
    state: tauri::State<'_, AppState>,
    on: bool,
) -> Result<autofire::State, String> {
    let session = state.session.clone();
    let engine = state.autofire.clone();
    let view = block(move || engine.set_master(on)).await??;
    session
        .bus()
        .info(format!("连发总开关已{}（界面里点的）", if on { "开" } else { "关" }));
    Ok(view)
}

#[tauri::command]
pub async fn autofire_save(
    state: tauri::State<'_, AppState>,
    config: autofire::Config,
) -> Result<ActionRecord, String> {
    let session = state.session.clone();
    let root = state.root.clone();
    let engine = state.autofire.clone();
    let message = join(move || engine.apply(&root, config)).await?;
    session.bus().info(message.clone());
    Ok(ActionRecord { message })
}

#[tauri::command]
pub async fn autofire_stop(state: tauri::State<'_, AppState>) -> Result<ActionRecord, String> {
    let session = state.session.clone();
    let engine = state.autofire.clone();
    join(move || {
        engine.stop();
        let message = "连发已停用，键盘监听已摘掉".to_string();
        session.bus().info(message.clone());
        Ok(message)
    })
    .await
    .map(|message| ActionRecord { message })
}

// ---------------------------------------------------------------- 存档页

#[tauri::command]
pub async fn saves_state(state: tauri::State<'_, AppState>) -> Result<saves::SavesState, String> {
    let root = state.root.clone();
    block(move || saves::state(&root)).await
}

/// 备份一份：只读打开 + `VACUUM INTO`，服务端在跑也不会碰原库。
#[tauri::command]
pub async fn saves_backup(state: tauri::State<'_, AppState>) -> Result<ActionRecord, String> {
    let session = state.session.clone();
    let root = state.root.clone();
    join(move || {
        let name = saves::backup(&root)?;
        session.bus().ok(format!("存档已备份：{name}"));
        Ok(ActionRecord { message: name })
    })
    .await
}

/// 替换存档：整库换一个文件，服务端正开着连接就等于对着活体改底层文件，一律拒绝。
/// 换之前会把当前库先备份进 `backup/`，失败也能原样退回去。
#[tauri::command]
pub async fn saves_restore(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<ActionRecord, String> {
    let session = state.session.clone();
    let root = state.root.clone();
    let running = matches!(
        session.status().phase,
        Phase::Starting | Phase::Running | Phase::Stopping
    );
    if running {
        return Err(
            "服务端正在使用这份存档，请先停止服务端再替换（替换会整库覆盖，账号与角色都在里面）".to_string(),
        );
    }
    join(move || {
        let message = saves::restore(&root, &path)?;
        session.bus().warn(message.clone());
        Ok(ActionRecord { message })
    })
    .await
}

// ---------------------------------------------------------------- 服务端更新

/// 查询 GitHub 最新 Release，对比本地已部署版本。
#[tauri::command]
pub async fn server_update_check(
    state: tauri::State<'_, AppState>,
) -> Result<updater::UpdateCheck, String> {
    let root = state.root.clone();
    join(move || updater::check(&root)).await
}

/// 检查并下载安装最新服务端。已是最新则不下载。
#[tauri::command]
pub async fn server_update_apply(
    state: tauri::State<'_, AppState>,
) -> Result<ActionRecord, String> {
    let session = state.session.clone();
    let root = state.root.clone();
    let bus = session.bus().clone();
    join(move || {
        let info = updater::check(&root)?;
        if !info.has_update {
            return Ok(ActionRecord {
                message: format!("已是最新（{}）", info.latest_tag),
            });
        }
        let message = updater::apply(&root, &info, &bus)?;
        Ok(ActionRecord { message })
    })
    .await
}

#[tauri::command]
pub async fn self_update_check(state: tauri::State<'_, AppState>) -> Result<self_updater::SelfUpdateCheck, String> {
    let bus = state.session.bus().clone();
    join(move || {
        bus.info("[自更新] 正在检查启动器更新…");
        let result = self_updater::check();
        match &result {
            Ok(info) => {
                if info.has_update {
                    bus.info(format!("[自更新] 发现新版本 v{}（当前 v{}）", info.latest_version, info.current_version));
                } else {
                    bus.info(format!("[自更新] 已是最新（v{}）", info.current_version));
                }
            }
            Err(e) => bus.warn(format!("[自更新] 检查失败：{e}")),
        }
        result
    }).await
}

#[tauri::command]
pub async fn self_update_apply(state: tauri::State<'_, AppState>, info: self_updater::SelfUpdateCheck) -> Result<ActionRecord, String> {
    let bus = state.session.bus().clone();
    join(move || {
        bus.info(format!("[自更新] 开始下载 v{}…", info.latest_version));
        let bus2 = bus.clone();
        match self_updater::apply_self_update(&info, move |msg| bus2.info(msg)) {
            Ok(msg) => {
                bus.info(format!("[自更新] {msg}"));
                Ok(ActionRecord { message: msg })
            }
            Err(e) => {
                bus.error(format!("[自更新] {e}"));
                Err(e)
            }
        }
    }).await
}

#[tauri::command]
pub fn self_update_cleanup() {
    self_updater::cleanup_self_update_cache();
}
