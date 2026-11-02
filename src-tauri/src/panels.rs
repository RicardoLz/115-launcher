//! 面板独立窗口：从主窗拖出去就是一个能自由拉伸的普通窗口，拖回主窗（或点「归位」）再停靠回去。
//!
//! 这两扇窗在 `tauri.conf.json` 里跟主窗一起建好，平时藏着，浮出只是 show、归位只是 hide。
//! 运行时建窗在 Windows 上会把事件循环钉死：wry 建 WebView2 控制器时嵌套泵消息，
//! 而那条消息正排在同一个循环里 —— 窗口出来了，webview 永远不导航（实测白屏且整机僵住）。
//! 屏幕坐标与逻辑坐标的换算全压在这一层，界面只认「相对主窗左上角的 CSS 像素」。

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, LogicalPosition, Manager, PhysicalPosition};

pub const PREFIX: &str = "panel-";
/// 归位：带落点回来，界面按落点挑吸附槽。
pub const RETURNED: &str = "panel-returned";
/// 玩家直接关掉了浮动窗口：界面把它当作「回到停靠区」。
pub const CLOSED: &str = "panel-closed";
/// 拖回来的路上：每挪一下就把落点报给主窗，界面照它画窗口内的吸附提示。
pub const HOVER: &str = "panel-hover";

/// 拖动停这么久才判「拖回」：早了会在系统拖拽模态循环里把窗口拆掉。
const SETTLE: Duration = Duration::from_millis(260);
/// 浮动窗口的左上角进入主窗这么多物理像素内，才算真的拖了回来。
const INSET: i32 = 10;

#[derive(Serialize, Clone)]
struct Returned {
    panel: String,
    x: f64,
    y: f64,
}

#[derive(Serialize, Clone)]
struct Hover {
    panel: String,
    x: f64,
    y: f64,
    inside: bool,
}

#[derive(Serialize, Clone)]
struct PanelName {
    panel: String,
}

fn err<T: std::fmt::Display>(e: T) -> String {
    e.to_string()
}

/// 每个窗口一个递增号：拖动线程只在自己那号还是最新时才动手。
#[derive(Default, Clone)]
struct Track {
    ticket: u64,
    /// 上一次停稳时，窗口左上角是否落在主窗范围内。
    /// 浮出时指针还在主窗里，靠它把「刚放下来」和「拖回来」区分开。
    inside: bool,
    /// 主窗此刻有没有画着这扇窗的落点提示，只用来决定要不要补那一条「收掉」。
    hover: bool,
}

fn tracks() -> &'static Mutex<HashMap<String, Track>> {
    static MAP: OnceLock<Mutex<HashMap<String, Track>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock() -> MutexGuard<'static, HashMap<String, Track>> {
    tracks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn panel_of(label: &str) -> String {
    label.trim_start_matches(PREFIX).to_string()
}

/// 浮出：搬到指针那儿再掀开。窗口本来就挂着，所以这一步没有建窗开销。
#[tauri::command]
pub fn panel_open(app: AppHandle, panel: String) -> Result<(), String> {
    let label = format!("{PREFIX}{panel}");
    let win = app
        .get_webview_window(&label)
        .ok_or_else(|| format!("没有「{panel}」这块面板"))?;
    let main = app.get_webview_window("main").ok_or("主窗口还没起来")?;
    let scale = main.scale_factor().map_err(err)?;

    // 指针就当作落在新窗口标题栏上：拖出去时视觉上像同一块面板被拎了起来。
    let (x, y) = match main.cursor_position() {
        Ok(cursor) => (cursor.x / scale - 34.0, cursor.y / scale - 12.0),
        Err(_) => {
            let pos = main.outer_position().map_err(err)?;
            (pos.x as f64 / scale + 80.0, pos.y as f64 / scale + 120.0)
        }
    };
    win.set_position(LogicalPosition::new(x, y)).map_err(err)?;
    win.unminimize().map_err(err)?;
    win.show().map_err(err)?;
    let _ = win.set_focus();
    // 出生在哪就记哪：按「浮出」按钮时指针还在主窗里，硬记成 inside=true 才不会一放下来就自我归位；
    // 而拖出主窗那一下松手在主窗外面，就得记 false，否则下一次拖回来不算「回来」。
    let inside = win
        .outer_position()
        .map(|p| overlaps_main(&app, p))
        .unwrap_or(true);
    lock().entry(label).or_default().inside = inside;
    Ok(())
}

/// 「归位」按钮，也是浮动窗口点关闭按钮时走的那条路。
#[tauri::command]
pub fn panel_dock(app: AppHandle, panel: String) {
    dock(&app, &format!("{PREFIX}{panel}"));
}

fn dock(app: &AppHandle, label: &str) {
    let Some(win) = app.get_webview_window(label) else {
        return;
    };
    let (x, y) = return_point(app, &win).unwrap_or((0.0, 0.0));
    hide(app, label);
    let _ = app.emit_to(
        "main",
        RETURNED,
        Returned {
            panel: panel_of(label),
            x,
            y,
        },
    );
}

/// 收回去：窗口不销毁，下次浮出还是原来那块内容、原来那个尺寸。
fn hide(app: &AppHandle, label: &str) {
    if let Some(win) = app.get_webview_window(label) {
        let _ = win.hide();
    }
    {
        let mut map = lock();
        let track = map.entry(label.to_string()).or_default();
        track.inside = true;
        track.hover = false;
    }
    // 拖回来那一下，系统多半还卡在标题栏的拖拽模态循环里：那次的 hide 会在循环收尾时被盖掉。
    // 补几次，直到窗口真的不见。
    let handle = app.clone();
    let label = label.to_string();
    std::thread::spawn(move || {
        for _ in 0..6 {
            std::thread::sleep(Duration::from_millis(160));
            let Some(win) = handle.get_webview_window(&label) else {
                return;
            };
            if !win.is_visible().unwrap_or(false) {
                return;
            }
            let _ = win.hide();
        }
    });
}

/// 玩家点了浮动窗口的关闭按钮：界面当作「回到停靠区」，窗口本身只是藏起来。
#[tauri::command]
pub fn panel_closed(app: AppHandle, panel: String) {
    let label = format!("{PREFIX}{panel}");
    hide(&app, &label);
    let _ = app.emit_to("main", CLOSED, PanelName { panel });
}

/// 浮动窗口左上角换算成「相对主窗的逻辑坐标」。
fn return_point(app: &AppHandle, win: &tauri::WebviewWindow) -> Option<(f64, f64)> {
    point_in_main(app, win.outer_position().ok()?)
}

/// 同一个坐标口径：主窗的界面只认这套逻辑坐标。
fn point_in_main(app: &AppHandle, pos: PhysicalPosition<i32>) -> Option<(f64, f64)> {
    let main = app.get_webview_window("main")?;
    let scale = main.scale_factor().ok()?;
    let m = main.outer_position().ok()?;
    Some(((pos.x - m.x) as f64 / scale, (pos.y - m.y) as f64 / scale))
}

pub fn note_moved(app: &AppHandle, label: &str, pos: PhysicalPosition<i32>) {
    let inside = overlaps_main(app, pos);
    let point = point_in_main(app, pos);
    let (ticket, over, tell) = {
        let mut map = lock();
        match map.get_mut(label) {
            Some(track) => {
                track.ticket += 1;
                // 只提示「本来飘在外面、又拖回主窗上空」的移动：刚浮出就落在主窗里的那种挪动不会归位，
                // 给它画提示等于许愿。刚离开时补一条 false，让界面把提示收掉。
                let over = inside && !track.inside;
                let was = std::mem::replace(&mut track.hover, over);
                (track.ticket, over, over || was)
            }
            None => return,
        }
    };
    if let (true, Some((x, y))) = (tell, point) {
        let _ = app.emit_to(
            "main",
            HOVER,
            Hover {
                panel: panel_of(label),
                x,
                y,
                inside: over,
            },
        );
    }
    let app = app.clone();
    let label = label.to_string();
    std::thread::spawn(move || {
        std::thread::sleep(SETTLE);
        // 停下来了但左键还按着：那是「正拖着路过主窗」，不是「放下来」。等真松手才归位，
        // 中途窗口又动了就交棒给下一次移动（票据已经不是自己那号了）。
        while held() && latest(&label, ticket) {
            std::thread::sleep(Duration::from_millis(40));
        }
        let Some(win) = app.get_webview_window(&label) else {
            return;
        };
        let Ok(now) = win.outer_position() else {
            return;
        };
        let inside = overlaps_main(&app, now);
        // 只有最后一个移动事件说了算：中间态一律不判。
        let dock_now = {
            let mut map = lock();
            match map.get_mut(&label) {
                Some(track) if track.ticket == ticket => {
                    let was = track.inside;
                    track.inside = inside;
                    inside && !was
                }
                _ => false,
            }
        };
        if dock_now {
            dock(&app, &label);
        }
    });
}

/// 这扇窗最新的一次移动是不是自己这一次。
fn latest(label: &str, ticket: u64) -> bool {
    lock()
        .get(label)
        .map(|track| track.ticket == ticket)
        .unwrap_or(false)
}

/// 左键此刻按着没有：系统拿着鼠标时界面收不到任何事件，只能问系统。
#[cfg(windows)]
fn held() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    // 返回值最高位为真表示当下按着，也就是有符号的负数。
    unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 }
}

/// 别处没有这套系统级标题栏拖拽，等松手这一步直接跳过。
#[cfg(not(windows))]
fn held() -> bool {
    false
}

fn overlaps_main(app: &AppHandle, pos: PhysicalPosition<i32>) -> bool {
    let Some(main) = app.get_webview_window("main") else {
        return false;
    };
    let (Ok(origin), Ok(size)) = (main.outer_position(), main.outer_size()) else {
        return false;
    };
    let (w, h) = (size.width as i32, size.height as i32);
    pos.x >= origin.x + INSET
        && pos.y >= origin.y + INSET
        && pos.x <= origin.x + w - INSET
        && pos.y <= origin.y + h - INSET
}
