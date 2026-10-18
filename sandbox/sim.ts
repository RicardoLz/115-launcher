/// 启动器的沙盒后端。
///
/// 它替代的是 Rust 那一侧的**返回值与事件**，不是 Rust 的实现：命令名、字段形状照
/// src/bridge.ts，状态机（idle→starting→running→stopping）与更新进度（按落盘字节、
/// 只在整数百分比变化时发）照 src-tauri/src/commands.rs 的语义手工搬过来。
/// 所以在这里跑通的东西只证明「界面在各种后端状态下表现正确」，
/// 不证明真服务端会给出这些数据。真机验证仍然只能由玩家点「开始游戏」。
import type {
  ActionRecord,
  AppInfo,
  AutofireConfig,
  AutofireLive,
  AutofireView,
  BackgroundState,
  Board,
  ConfigView,
  FeedbackMeta,
  HealthReport,
  Issue,
  LogLine,
  Mine,
  ModsState,
  Phase,
  PortReport,
  Readiness,
  SavesState,
  SelfUpdateCheck,
  ServerUpdateCheck,
  Status,
  Submission,
  SubmitOutcome,
  UpdateProgress,
} from "../src/bridge";
import * as data from "./data";

export interface SimState {
  phase: Phase;
  detail: string;
  can_start: boolean;
  readiness_detail: string;
  gateway_listening: boolean;
  client_ready: boolean;
  client_dir: string;
  root: string;
  account: string;
  accounts: string[];
  game_mode: string;
  auto_stop: boolean;
  auto_pickup: boolean;
  auto_report: boolean;
  auto_report_prompted: boolean;
  pending_reports: number;
  storage_detail: string;
  background: BackgroundState;
  self_latest: string | null;
  server_latest: string | null;
  server_tag: string | null;
  asset_mb: number;
  start_delay_ms: number;
  stop_delay_ms: number;
  update_duration_ms: number;
  update_mode: UpdateMode;
  update_fail_text: string;
  update_fail_at: number;
  update_stall_at: number;
  command_latency_ms: number;
  fail_all: boolean;
  log: string[];
}

type UpdateMode = "stream" | "manual" | "instant" | "fail-mid" | "stall" | "port-busy" | "network-error";

export function initialState(): SimState {
  return {
    phase: "idle",
    detail: "未运行",
    can_start: true,
    readiness_detail: "可以开始",
    gateway_listening: false,
    client_ready: true,
    client_dir: "D:\\115us\\客户端",
    root: "D:\\115us\\服务端",
    account: "probe",
    accounts: ["probe", "kink"],
    game_mode: "native",
    auto_stop: true,
    auto_pickup: false,
    auto_report: false,
    auto_report_prompted: true,
    pending_reports: 0,
    storage_detail: "sqlite：runtime/storage/local.sqlite3",
    background: { path: "", kind: "none", exists: false, note: "没设置背景素材", accent: "" },
    self_latest: null,
    server_latest: null,
    server_tag: "v20261014-1",
    asset_mb: 2.7,
    start_delay_ms: 6000,
    stop_delay_ms: 1500,
    update_duration_ms: 6000,
    update_mode: "stream",
    update_fail_text: "下载中断：连接被对方关闭",
    update_fail_at: 45,
    update_stall_at: 62,
    command_latency_ms: 0,   // 本地 mock 不再默认加 40 ms：后台标签页里这条 sleep 会被掐成 1 秒，脚本的节奏就全乱了
    fail_all: false,
    log: [],
  };
}

export const st: SimState = initialState();

export function reset(): void {
  Object.assign(st, initialState());
}

const sleep = (ms: number) => new Promise<void>((resolve) => window.setTimeout(resolve, ms));

/// 拒绝类分支不走时钟：后台标签页会把 setTimeout 掐到一秒甚至一分钟，
/// 那堵住的不是被测逻辑，是脚本自己。让出两次宏任务，异步形状一样，只是不读表。
const yieldTwice = (): Promise<void> =>
  new Promise<void>((resolve) => {
    const ch = new MessageChannel();
    let left = 2;
    ch.port1.onmessage = () => {
      if (--left === 0) {
        ch.port1.close();
        resolve();
      } else ch.port2.postMessage(0);
    };
    ch.port2.postMessage(0);
  });

function note(line: string): void {
  st.log.unshift(line);
  if (st.log.length > 200) st.log.length = 200;
}

function readiness(): Readiness {
  return {
    can_start: st.can_start,
    detail: st.readiness_detail,
    root: st.root,
    root_source: "同级目录推断",
    client_ready: st.client_ready,
    client_dir: st.client_dir,
    server_ready: true,
    server_detail: "编排程序就位",
    storage_ready: true,
    storage_detail: st.storage_detail,
    gateway_listening: st.gateway_listening,
  };
}

function configView(): ConfigView {
  return {
    account: st.account,
    accounts: [...st.accounts],
    game_mode: st.game_mode,
    game_modes: [
      { value: "native", label: "原生启动" },
      { value: "compat", label: "兼容模式" },
      { value: "server-only", label: "仅服务端" },
    ],
    auto_stop_on_client_exit: st.auto_stop,
    auto_pickup: st.auto_pickup,
    auto_report: st.auto_report,
    auto_report_prompted: st.auto_report_prompted,
    pending_reports: st.pending_reports,
    client_dir: st.client_dir,
    client_ready: st.client_ready,
    storage_detail: st.storage_detail,
  };
}

function status(): Status {
  return { phase: st.phase, detail: st.detail };
}

export function pushStatus(): void {
  emit("game-phase", status());
}

export function pushReadiness(): void {
  emit("readiness-changed", readiness());
}

function selfCheck(): SelfUpdateCheck | null {
  if (st.self_latest === null) return null;
  return {
    current_version: "0.1.0",
    latest_version: st.self_latest,
    has_update: true,
    asset_name: "DFO115USLauncher-setup.exe",
    asset_size: Math.round(st.asset_mb * 1048576),
    download_url: "https://example.invalid/launcher.exe",
  };
}

function serverCheck(): ServerUpdateCheck | null {
  if (st.server_latest === null) return null;
  return {
    current_tag: st.server_tag,
    latest_tag: st.server_latest,
    has_update: true,
    asset_name: "115us-server.zip",
    asset_size: Math.round(st.asset_mb * 1048576),
    download_url: "https://example.invalid/server.zip",
  };
}

// ------------------------------------------------------------------ 事件出口

import { emitTo } from "./tauri/event";

export function emit<T>(name: string, payload: T): void {
  emitTo(name, payload);
}

// ------------------------------------------------------------------ 更新引擎

let updateToken = 0;
let startToken = 0;
const manualWaits = new Set<(err?: unknown) => void>();

/// manual 模式：下载不自己走，命令挂在这条 promise 上，由脚本用 pump()/pumpDeploy()/pumpFail() 推。
/// 后台标签页把 setTimeout 掐到 1 秒，自动流在这里只能拿出被掐过的节奏；真机节奏要脚本定步调才测得准。
function releaseManual(err?: unknown): void {
  const waiters = Array.from(manualWaits);
  note(`[更新] manual 收口 ${waiters.length} 条挂账：${err === undefined ? "下载完成" : String(err)}`);
  manualWaits.clear();
  for (const waiter of waiters) waiter(err);
}

/// 与 ProgressWatch 同一条口径：percent 只在整数变化时发，字节下满之后转 deploy。
async function streamUpdate(kind: "self" | "server"): Promise<ActionRecord> {
  const token = ++updateToken;
  const totalBytes = Math.round(st.asset_mb * 1048576);
  const stepMs = Math.max(6, Math.round(st.update_duration_ms / 100));
  note(`[更新] ${kind} 开始下载 ${st.asset_mb} MB，模式 ${st.update_mode}`);

  if (st.update_mode === "port-busy") {
    await yieldTwice();
    throw "服务端网关端口 7001 正在监听，请先停止服务端再更新";
  }
  if (st.update_mode === "network-error") {
    await yieldTwice();
    throw st.update_fail_text;
  }

  if (st.update_mode === "manual") {
    note(`[更新] ${kind} 走 manual：进度由脚本 pump() 决定，界面不会自己动`);
    await new Promise<void>((resolve, reject) => {
      const settle = (err?: unknown): void => {
        manualWaits.delete(settle);
        if (err === undefined) resolve();
        else reject(err);
      };
      manualWaits.add(settle);
    });
  } else if (st.update_mode !== "instant") {
    for (let percent = 1; percent <= 100; percent++) {
      await sleep(stepMs);
      if (token !== updateToken) throw "沙盒：这次下载被中止了";
      if (st.update_mode === "fail-mid" && percent === st.update_fail_at) {
        throw st.update_fail_text;
      }
      if (st.update_mode === "stall" && percent === st.update_stall_at) {
        note(`[更新] 卡在 ${percent}%，不再产生新字节`);
        // 卡住 = 承诺永远不落地：界面必须一直锁在「更新中」，直到人手动 abort。
        // eslint-disable-next-line no-await-in-loop
        while (token === updateToken) await sleep(120);
        throw "沙盒：这次下载被中止了";
      }
      const got = Math.floor((percent / 100) * totalBytes);
      const payload: UpdateProgress = {
        kind,
        phase: "download",
        percent,
        text: `${Math.floor(got / 1048576)} MB / ${st.asset_mb} MB`,
      };
      emit("update-progress", payload);
    }
  }

  const deployText = kind === "self" ? "正在替换并重启" : "解压部署中";
  emit("update-progress", {
    kind,
    phase: "deploy",
    percent: 100,
    text: deployText,
  } satisfies UpdateProgress);
  if (st.update_mode !== "manual") await sleep(Math.max(200, Math.round(st.update_duration_ms / 6)));
  if (token !== updateToken) throw "沙盒：这次下载被中止了";

  if (kind === "self") {
    const target = st.self_latest ?? "";
    st.self_latest = null;
    note(`[更新] 启动器已换成 ${target}，界面随即退出`);
    return { message: `启动器已更新到 ${target}，正在重启` };
  }
  st.server_tag = st.server_latest;
  st.server_latest = null;
  note(`[更新] 服务端已部署到 ${st.server_tag}`);
  return { message: `服务端已更新到 ${st.server_tag}` };
}

// ------------------------------------------------------------------ 命令表

type Handler = (args: Record<string, unknown>) => unknown;

const handlers: Record<string, Handler> = {
  app_info(): AppInfo {
    return {
      name: "DFO-115US 启动器",
      version: "0.1.0",
      root: st.root,
      root_source: "同级目录推断",
      app_dir: "D:\\115us\\服务端",
      log_dir: "C:\\Users\\Player\\AppData\\Local\\DFO115US-Tauri\\logs",
    };
  },
  log_snapshot(): LogLine[] {
    return st.log.slice(0, 20).map((text, index) => ({
      seq: index,
      time: "21:26:30",
      level: "INFO",
      text,
    }));
  },
  readiness_state: readiness,
  env_check: (): HealthReport => data.health,
  game_status: status,
  game_start(args): StartOutcomeShape {
    const mode = String(args.mode ?? "native");
    startToken++;
    st.phase = "starting";
    st.detail = mode === "server-only" ? "正在拉起服务端" : "正在拉起服务端与客户端";
    pushStatus();
    const token = startToken;
    void sleep(st.start_delay_ms).then(() => {
      if (token !== startToken) return;
      st.gateway_listening = true;
      st.phase = "running";
      st.detail = "网关已监听 7001，客户端就位";
      pushStatus();
      pushReadiness();
      note("[启动] 环境已就绪");
    });
    return { started: true, phase: st.phase, detail: st.detail };
  },
  game_stop(): boolean {
    startToken++;
    st.phase = "stopping";
    st.detail = "正在收尾";
    pushStatus();
    const token = startToken;
    void sleep(st.stop_delay_ms).then(() => {
      if (token !== startToken) return;
      st.gateway_listening = false;
      st.phase = "idle";
      st.detail = "未运行";
      pushStatus();
      pushReadiness();
    });
    return true;
  },
  ports_report(): PortReport {
    return st.gateway_listening ? data.portsBusy() : data.ports;
  },
  force_clean(): PortReport {
    st.gateway_listening = false;
    return data.ports;
  },
  config_get: configView,
  set_account(args): ActionRecord {
    st.account = String(args.account ?? "");
    if (!st.accounts.includes(st.account)) st.accounts = [st.account, ...st.accounts];
    return { message: `下次开始游戏用 ${st.account}` };
  },
  set_game_mode(args): ActionRecord {
    st.game_mode = String(args.mode ?? "");
    return { message: "启动方式已改" };
  },
  set_auto_stop(args): ActionRecord {
    st.auto_stop = Boolean(args.enabled);
    return { message: st.auto_stop ? "关闭游戏后会自动停服" : "不再自动停服" };
  },
  set_auto_pickup(args): ActionRecord {
    st.auto_pickup = Boolean(args.enabled);
    return { message: st.auto_pickup ? "自动拾取已开" : "自动拾取已关" };
  },
  set_auto_report(args): ActionRecord {
    st.auto_report = Boolean(args.enabled);
    st.auto_report_prompted = true;
    return { message: st.auto_report ? "故障会自动上报" : "不会自动上报" };
  },
  mods_state: (): ModsState => data.mods,
  set_server_mod(args): ActionRecord {
    return { message: `服务端 mod ${args.id} 已${args.enabled ? "启用" : "停用"}` };
  },
  set_client_mod(args): ActionRecord {
    return { message: `客户端 mod ${args.id} 已${args.enabled ? "启用" : "停用"}` };
  },
  import_client_mod(): ActionRecord {
    return { message: "已导入 1 个客户端插件" };
  },
  import_server_mod(): ActionRecord {
    return { message: "已导入 2 个服务端 mod，1 个规则脚本" };
  },
  open_modkit_dir(): ActionRecord {
    return { message: "已打开 modkit 目录" };
  },
  autofire_get: (): AutofireView => data.autofire,
  autofire_live(): AutofireLive {
    const live = { ...data.autofire.live };
    live.master = !live.master;
    return live;
  },
  autofire_set_master(args): AutofireLive {
    const live = { ...data.autofire.live };
    live.master = Boolean(args.on);
    return live;
  },
  autofire_save(args): ActionRecord {
    const config = args.config as AutofireConfig | undefined;
    return { message: `连发配置已保存（${config?.entries.length ?? 0} 条）` };
  },
  autofire_stop(): ActionRecord {
    return { message: "连发已停" };
  },
  saves_state: (): SavesState => data.saves,
  saves_backup(): ActionRecord {
    return { message: "已备份当前存档" };
  },
  saves_restore(args): ActionRecord {
    return { message: `已替换为 ${String(args.path).split("\\").pop()}` };
  },
  saves_delete(args): ActionRecord {
    return { message: `已删除备份 ${String(args.path).split("\\").pop()}` };
  },
  background_state: (): BackgroundState => ({ ...st.background }),
  background_pick(): BackgroundState {
    st.background = {
      path: "C:\\sandbox\\wallpaper.png",
      kind: "image",
      exists: true,
      note: "",
      // 选完壁纸后端就该顺手给出主题色：沙盒里固定一枚青色，验的是接线不是取色算法。
      accent: "#12a4b9",
    };
    return { ...st.background };
  },
  background_clear(): BackgroundState {
    st.background = { path: "", kind: "none", exists: false, note: "没设置背景素材", accent: "" };
    return { ...st.background };
  },
  reports_retry(): void {
    st.pending_reports = 0;
  },
  feedback_meta: (): FeedbackMeta => data.feedbackMeta,
  feedback_board: (): Board => data.board,
  feedback_issue(args): Issue {
    return data.board.issues.find((item) => item.id === args.id) ?? data.board.issues[0];
  },
  feedback_mine: (): Mine[] => [
    {
      id: "DFO-142",
      title: "关闭游戏后服务端没跟着停",
      created: 1760000000,
      status: "已修复",
      issue: data.board.issues[0],
    },
  ],
  feedback_submit(args): SubmitOutcome {
    const input = args.input as Submission | undefined;
    return {
      id: "DFO-199",
      duplicate: false,
      issue: { ...data.board.issues[1], title: input?.title ?? "新反馈" },
    };
  },
  pick_client_dir(): ActionRecord {
    return { message: "已选择客户端目录" };
  },
  discover_client(): ActionRecord {
    return { message: "识别到 DFO.exe" };
  },
  server_update_check(): ServerUpdateCheck {
    const info = serverCheck();
    if (info) return info;
    return {
      current_tag: st.server_tag,
      latest_tag: st.server_tag ?? "",
      has_update: false,
      asset_name: "",
      asset_size: 0,
      download_url: "",
    };
  },
  server_update_apply(): Promise<ActionRecord> {
    const info = serverCheck();
    if (!info) return Promise.resolve({ message: `已是最新（${st.server_tag}）` });
    return streamUpdate("server");
  },
  self_update_check(): SelfUpdateCheck {
    const info = selfCheck();
    if (info) return info;
    return {
      current_version: "0.1.0",
      latest_version: "0.1.0",
      has_update: false,
      asset_name: "",
      asset_size: 0,
      download_url: "",
    };
  },
  self_update_apply(): Promise<ActionRecord> {
    return streamUpdate("self");
  },
  self_update_cleanup(): void {
    note("[更新] 清理了缓存");
  },
  quit_with_choice(args): ActionRecord {
    st.log.unshift(`[退出] choice=${String(args.choice)}`);
    return { message: `已按 ${String(args.choice)} 退出` };
  },
  panel_open(args): ActionRecord {
    return { message: `面板 ${String(args.panel)} 已浮出` };
  },
  panel_dock(): void {},
  panel_closed(): void {},
};

interface StartOutcomeShape {
  started: boolean;
  phase: Phase;
  detail: string;
}

/// 4 秒一轮的轮询命令不参与「整条挂掉」模拟，否则错误提示会自己刷屏。
const POLLED = new Set(["game_status", "readiness_state"]);

export async function dispatch(cmd: string, args?: Record<string, unknown>): Promise<unknown> {
  const handler = handlers[cmd];
  note(`→ ${cmd}${args ? ` ${JSON.stringify(args).slice(0, 90)}` : ""}`);
  if (!handler) throw `沙盒没实现命令 ${cmd}`;
  if (st.fail_all && !POLLED.has(cmd)) throw "后端没响应（沙盒：整条命令链已断开）";
  if (st.command_latency_ms > 0 && !POLLED.has(cmd)) {
    // eslint-disable-next-line no-await-in-loop
    await sleep(st.command_latency_ms);
  }
  const result = await handler(args ?? {});
  return result === undefined ? undefined : structuredClone(result);
}

/// 手动发一条进度：自动化脚本要能自己定步调。后台标签页里 setTimeout 会被掐到 1 秒，
/// 靠自动流看不出真机节奏，所以自动流留着给人点，逐条 pump 给脚本用。
export function pump(kind: "self" | "server", percent: number): void {
  const totalBytes = Math.round(st.asset_mb * 1048576);
  const got = Math.floor((Math.max(0, Math.min(100, percent)) / 100) * totalBytes);
  emit("update-progress", {
    kind,
    phase: "download",
    percent: Math.max(0, Math.min(100, percent)),
    text: `${Math.floor(got / 1048576)} MB / ${st.asset_mb} MB`,
  } satisfies UpdateProgress);
}

export function pumpDeploy(kind: "self" | "server"): void {
  emit("update-progress", {
    kind,
    phase: "deploy",
    percent: 100,
    text: kind === "self" ? "正在替换并重启" : "解压部署中",
  } satisfies UpdateProgress);
  releaseManual();
}

/// 手动让下载在中途失败：真机上就是 zip 下到一半断流，界面必须解锁并退回「更新版本」。
export function pumpFail(text: string): void {
  note(`[更新] 手动判失败：${text}`);
  releaseManual(text);
}

/// 卡住的下载要能人为收口：让那条永远不落地的承诺抛出去，界面才会解锁。
export function cancelUpdate(): void {
  updateToken++;
  releaseManual("沙盒：这次下载被中止了");
}
