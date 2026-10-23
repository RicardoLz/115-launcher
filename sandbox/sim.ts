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
  GmCharacter,
  GmAudit,
  GmFacetsView,
  GmGrantOutcome,
  GmGrantRecord,
  GmInventoryStatus,
  GmItemRow,
  GmQuestPlan,
  GmQuestView,
  GmRebuildProgress,
  GmRebuildReport,
  GmRebuildStep,
  GmSearchResult,
  GmStatusView,
  HealthReport,
  Issue,
  LogLine,
  Mine,
  MemoryConfig,
  MemoryLive,
  MemoryTuning,
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
  mem_tuning: MemoryTuning;
  mem_live: MemoryLive | null;
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
  /// GM 写存档的占用态：none 可写 / server 服务端在线 / busy 管理操作中 / missing 还没有存档。
  gm_lock: "none" | "server" | "busy" | "missing";
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
    mem_tuning: { server_memory_limit_mib: null, go_mem_target_mib: null, import_max_procs: null },
    mem_live: null,
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
    gm_lock: "none",
    log: [],
  };
}

export const st: SimState = initialState();

export function reset(): void {
  Object.assign(st, initialState());
  issuedGrants.clear();
  mailSeq = 42;
}

/// 实机那一场的 MEMORY_GUARD 采样替身：私有提交一路涨到 1.4 GiB 上下，
/// 数字照 runtime/roles_persist_20261019-040032_115us_r2_next01 那一场抄。
export function memLive(patch: Partial<MemoryLive> = {}): MemoryLive {
  const series = Array.from({ length: 40 }, (_, i) => 520 + Math.round(i * 23) + (i % 5) * 11);
  return {
    session: "roles_persist_20261019-040032_115us_r2_next01",
    path: "X:\\DFO-115US\\server\\work\\dfo-lan\\runtime\\roles_persist_20261019-040032_115us_r2_next01\\gateway.err",
    pid: 24064,
    policy: "local-stability-20261008-r2",
    hard_limit_mib: 4096,
    low_memory_mib: 1024,
    seconds: 422.3,
    private_mib: 1431,
    working_mib: 1180,
    system_available_mib: 12984,
    peak_private_mib: null,
    ended: false,
    stale: false,
    samples: 7822,
    series,
    note: "",
    ...patch,
  };
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
  memory_config(): MemoryConfig {
    return {
      tuning: { ...st.mem_tuning },
      path: `${st.root}\\launcher.local.json`,
      max_memory_mib: 17592186044415,
      max_import_procs: 65536,
      cpu_count: 16,
    };
  },
  memory_save(args): ActionRecord {
    const tuning = (args.tuning ?? {}) as MemoryTuning;
    // 与 Rust 侧同一道门：编排器超过 65536 就直接拒绝启动，界面不能先把脏值写进文件。
    if ((tuning.import_max_procs ?? 0) > 65536) {
      throw `导入并发最大 65536（编排器超过它就拒绝启动），现在是 ${tuning.import_max_procs}`;
    }
    st.mem_tuning = { ...tuning };
    const mib = (value: number | null) => (value === null ? "不限" : `${value} MiB`);
    const message =
      `服务端内存设置已写入（${st.root}\\launcher.local.json）：` +
      `提交内存硬上限 ${mib(tuning.server_memory_limit_mib)}，` +
      `Go 内存目标 ${mib(tuning.go_mem_target_mib)}，` +
      `导入并发 ${tuning.import_max_procs ?? "默认"}；下次「开始游戏」才生效`;
    note(message);
    return { message };
  },
  memory_live(): MemoryLive | null {
    return st.mem_live;
  },
  gm_status(): GmStatusView {
    return data.gmStatus;
  },
  gm_reload(): void {
    note("[GM] 沙盒里的产物恒为就绪，重新加载只是把状态原样报回去");
  },
  gm_audit(): GmAudit {
    return data.gmAudit;
  },
  /// 沙盒里不真跑 gmbuild：按启动器那七步各报「开始 / 结束」两格进度，再回一份同形状回执。
  /// 权重照 src-tauri/src/gmrebuild.rs 的实测秒数，进度条的走法因此和真机一致。
  async gm_rebuild(args): Promise<GmRebuildReport> {
    const pvf = String(args.pvf ?? "") || data.gmAudit.pvf_path;
    note(`[GM] 沙盒不真跑 gmbuild；假装已用 ${pvf} 走完七步`);
    const plan: [string, string[], number][] = [
      ["build-names", ["names.client.json"], 14],
      ["build-items", ["items.index.json"], 23],
      ["build-slots", ["equipment.slots.json", "equipment.jobs.json", "item.rarity.json"], 28],
      ["build-growth", ["gm-growth.json"], 13],
      ["build-quests", ["gm-quests.json"], 13],
      ["build-equipment", ["equipment.current37.json"], 20],
      ["refresh-bag", ["inventory.current37.json"], 8],
    ];
    const total = plan.reduce((sum, [, , weight]) => sum + weight, 0);
    const steps: GmRebuildStep[] = [];
    let spent = 0;
    for (let i = 0; i < plan.length; i += 1) {
      const [cmd, outputs, weight] = plan[i];
      const from = Math.floor((spent * 100) / total);
      spent += weight;
      const to = Math.floor((spent * 100) / total);
      const at = (percent: number, text: string) =>
        emit("gm-rebuild-progress", {
          step: cmd,
          step_index: i + 1,
          step_total: plan.length,
          percent,
          text,
        } satisfies GmRebuildProgress);
      at(from, `第 ${i + 1}/${plan.length} 步：${cmd}`);
      // eslint-disable-next-line no-await-in-loop
      await sleep(40);
      at(to, `第 ${i + 1}/${plan.length} 步完成：${cmd}`);
      steps.push({ step: cmd, ok: true, seconds: weight / 10, detail: `${outputs.join("、")} 写出` });
    }
    // 真机上第七步完成后还有「五件校验 → 备份 → 替换 → 重载物品库」几秒钟，100% 那一格是看得见的；
    // 这里若立刻返回，React 会把最后一格进度与解除忙碌合进同一帧，走查脚本就只采到 93%。
    // eslint-disable-next-line no-await-in-loop
    await sleep(150);
    return {
      dir: data.gmAudit.dir,
      pvf,
      checksum: data.gmAudit.artifacts[0].checksum,
      steps,
      backup: `${data.gmAudit.dir}\\.backup-20261021-233000`,
      elapsed: steps.reduce((sum, s) => sum + s.seconds, 0),
      audit: data.gmAudit,
    };
  },
  gm_pick_pvf(): string | null {
    note("[GM] 沙盒没有文件对话框，选择 PVF 直接当取消");
    return null;
  },
  gm_search(args): GmSearchResult {
    const q = String(args.query ?? "").trim().toLowerCase();
    const type = String(args.type_ ?? "");
    const slot = String(args.slot ?? "");
    const rarity = Number(args.rarity ?? 0);
    const limit = Number(args.limit ?? 60) || 60;
    const offset = Number(args.offset ?? 0);
    const hit = data.gmItems.filter((row) => {
      if (type && row.type_key !== type && row.kind !== type) return false;
      if (slot && row.slot !== slot) return false;
      if (rarity > 0 && row.rarity !== rarity) return false;
      if (!q) return true;
      return (
        row.name.toLowerCase().includes(q) ||
        row.name_en.toLowerCase().includes(q) ||
        String(row.id).startsWith(q.replace(/^\*/, ""))
      );
    });
    return { total: hit.length, items: hit.slice(offset, offset + limit) };
  },
  gm_item(args): GmItemRow {
    const row = data.gmItems.find((item) => item.id === Number(args.id));
    if (!row) throw `物品库里没有 ${String(args.id)}`;
    return row;
  },
  gm_facets: (): GmFacetsView => data.gmFacets,
  gm_inventory_status(): GmInventoryStatus {
    const db = "D:\\115us\\服务端\\server\\work\\dfo-lan\\runtime\\storage\\dfolan.sqlite3";
    if (st.gm_lock === "missing") {
      return {
        editable: false,
        server_running: false,
        management_busy: false,
        message: "存档还没有 SQLite 档：先启动一次游戏，服务端会自己建库",
        db_path: "",
      };
    }
    if (st.gm_lock === "busy") {
      return {
        editable: false,
        server_running: false,
        management_busy: true,
        message: "另一项管理操作正在保存，请稍后重试",
        db_path: db,
      };
    }
    if (st.gm_lock === "server") {
      return {
        editable: false,
        server_running: true,
        management_busy: false,
        message: "检测到游戏服务端正在运行，请停止服务端后再修改背包；SQLite 存档文件无需停止",
        db_path: db,
      };
    }
    return { editable: true, server_running: false, management_busy: false, message: "", db_path: db };
  },
  gm_characters: (): GmCharacter[] => data.gmCharacters,
  gm_grant(args): GmGrantOutcome {
    // 校验照 src-tauri/src/gmgrant.rs 的 grant() 逐条抄，界面才验得出真机那句话。
    const id = String(args.id ?? "");
    const account = Number(args.account ?? 0);
    const character = Number(args.character ?? 0);
    const gold = Number(args.gold ?? 0);
    const cera = Number(args.cera ?? 0);
    const items = (args.items ?? []) as { template: number; amount: number }[];
    if (st.gm_lock === "missing") throw "存档还没有 SQLite 档：先启动一次游戏";
    if (st.gm_lock === "busy") throw "另一项管理操作正在保存，请稍后重试";
    if (st.gm_lock === "server") {
      throw "检测到游戏服务端正在运行，请停止服务端后再修改背包；SQLite 存档文件无需停止";
    }
    if (id.length < 16 || !id.startsWith("launcher-")) throw "发放缺少有效的操作流水号";
    if (account <= 0) throw "缺少账号";
    if (items.length > 100 || cera < 0 || (gold === 0 && cera === 0 && items.length === 0)) {
      throw "请选择有效的发放内容，单次最多 100 种物品";
    }
    if (items.some((it) => it.template === 0 || it.amount === 0)) throw "物品及数量必须大于零";
    if ((gold !== 0 || items.length !== 0) && character <= 0) throw "金币与物品发放必须选定角色";
    const seen = issuedGrants.get(id);
    if (seen) {
      return { ...seen, applied: false, message: "该笔发放已存在，未重复发放。" };
    }
    const warehouse = items.filter((it) => it.template === 3033 || it.template === 10415190);
    const bag = items.filter((it) => it.template !== 3033 && it.template !== 10415190);
    const outcome: GmGrantOutcome = {
      applied: true,
      grant_id: id,
      message: warehouse.length
        ? "发放成功，晶体与灵魂材料已存入账号晶体仓库。"
        : "发放成功。",
      note: "游戏内的角色如果正在线上，需要重新选择角色（或小退再进）才能看到新的金币/物品。",
      receipt: {
        grant: id,
        cera_balance: cera,
        ...(gold ? { gold_added: gold, gold_after: 128_650 + gold } : {}),
        ...(bag.length
          ? { items: bag.map((it) => ({ Template: it.template, Amount: it.amount, Slots: [70] })) }
          : {}),
        ...(warehouse.length
          ? {
              materials: warehouse.map((it) => ({
                slot: it.template === 3033 ? 363 : 0,
                template: it.template,
                amount: it.amount,
              })),
            }
          : {}),
      },
    };
    issuedGrants.set(id, outcome);
    return outcome;
  },
  gm_quests: (): GmQuestView => data.gmQuests,
  // 只读流水：照 Rust 那样整份给出去，界面自己认「全存档最近 100 笔」。
  gm_grant_log: (): GmGrantRecord[] => data.gmGrantLog,
  gm_quest_preview_mainline(args): GmQuestPlan {
    // 等级门照 src-tauri/src/gmquest.rs 的 mainline_plan：0 或高于角色当前等级都拒。
    const level = Number(args.level ?? 0);
    if (level === 0 || level > data.gmQuests.level) throw "主线目标等级必须在 1 至角色当前等级之间";
    return { ...data.gmQuestPlan, level };
  },
  gm_quest_preview_prerequisites(args): GmQuestPlan {
    const target = Number(args.target ?? 0);
    const row = data.gmQuests.quests.find((r) => r.id === target);
    if (!row || row.status === "completed") throw "请选择当前角色进行中的任务";
    return {
      quests: [3001],
      blocked: [],
      before: [],
      level: 0,
      signature: data.gmQuestPlan.signature,
    };
  },
  gm_manage(args): GmGrantOutcome {
    const input = (args.input ?? {}) as {
      action?: string;
      id?: string;
      quests?: number[];
      plan?: string;
      level?: number;
      revision?: string;
      character?: number;
      gold?: number;
      template?: number;
      amount?: number;
      body?: string;
    };
    if (st.gm_lock === "missing") throw "存档还没有 SQLite 档：先启动一次游戏";
    if (st.gm_lock === "busy") throw "另一项管理操作正在保存，请稍后重试";
    if (st.gm_lock === "server") {
      throw "检测到游戏服务端正在运行，请停止服务端后再修改背包；SQLite 存档文件无需停止";
    }
    const action = String(input.action ?? "");
    if (action === "mail.send") return mailSend(input);
    if (!action.startsWith("quests.")) throw `沙盒没实现管理动作 ${action}`;
    const quests = (input.quests ?? []).map(Number);
    if (action !== "quests.mainline" && (quests.length === 0 || quests.length > 100)) {
      throw "单次请选择 1 至 100 个任务";
    }
    if (action === "quests.mainline" || action === "quests.prerequisites") {
      if (!input.plan) throw "角色或任务已变化，请重新预览任务范围";
      const message =
        action === "quests.mainline"
          ? "主线任务已按预览完成，未发放奖励；重新选择角色后生效"
          : "前置任务已按预览完成，未发放奖励，目标任务保留；重新选择角色后生效";
      return {
        applied: true,
        grant_id: String(input.id ?? ""),
        message,
        note: "",
        receipt: { action, plan: data.gmQuestPlan, rewards: false },
      };
    }
    return {
      applied: true,
      grant_id: String(input.id ?? ""),
      message: "修改已保存并记录审计",
      note: "",
      receipt: { action, before: [], quests, rewards: false },
    };
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

/// 沙盒也按流水号幂等：同一个 id 第二次只回原回执，不再入账——和真链路那条 claim 语义一致。
const issuedGrants = new Map<string, GmGrantOutcome>();

/// 邮件附件的到期值：写 0 客户端显示「剩余期限已过」，与 Go 的 MaxItemPeriod 一致。
const GRANT_EXPIRE_TIME = 2_147_483_647;
/// 兜底堆叠上限：与真产物 `inventory.current37.json` 的 `missing_stack_limit` 同值。
const MISSING_STACK_LIMIT = 1000;
/// 沙盒不真写库，号段从发放记录夹具里那封（41）之后接着走，保持「附件先取号、消息号最后」。
let mailSeq = 42;

type MailInput = {
  id?: string;
  character?: number;
  gold?: number;
  template?: number;
  amount?: number;
  body?: string;
};

/** 邮件发放的替身：拒绝话术与 `src-tauri/src/gmmail.rs` 逐条对齐，回执形状也照它。 */
function mailSend(input: MailInput): GmGrantOutcome {
  const id = String(input.id ?? "");
  const seen = issuedGrants.get(id);
  if (seen) return { ...seen, applied: false, message: "该笔操作已存在，未重复执行。" };
  const gold = Number(input.gold ?? 0);
  const template = Number(input.template ?? 0);
  const amount = Number(input.amount ?? 0);
  const body = String(input.body ?? "");
  if (new TextEncoder().encode(body).length > 512) throw "邮件正文过长";
  if (gold === 0 && template === 0) throw "邮件至少要带金币或物品";

  const assets: { id: number; gold?: number; item?: Record<string, unknown> }[] = [];
  if (gold > 0) assets.push({ id: 0, gold });
  if (template !== 0) {
    if (template < 2 || amount === 0) throw "邮件附件缺少真实物品或数量";
    const row = data.gmItems.find((it) => it.id === template);
    if (!row) throw `物品 ${template} 不在物品库里`;
    if (row.kind === "stackable") {
      const limit = row.stack_limit > 0 ? row.stack_limit : MISSING_STACK_LIMIT;
      if (amount > limit) throw `邮件数量超过堆叠上限${limit}`;
      assets.push({
        id: 0,
        item: { stack: { slot: 0, Template: template, Amount: amount, expire_time: GRANT_EXPIRE_TIME } },
      });
    } else if (row.avatar) {
      if (amount > 9) throw "装备每批最多9件";
      for (let i = 0; i < amount; i += 1) {
        assets.push({
          id: 0,
          item: {
            equipment: { slot: 0, template, durability: 0, period: GRANT_EXPIRE_TIME },
            space: 1,
          },
        });
      }
    } else {
      throw `装备实例发放未支持（见 #27）：物品 ${template} 不是可堆叠材料或时装`;
    }
  }
  const first = mailSeq;
  mailSeq += assets.length + 1;
  assets.forEach((asset, i) => {
    asset.id = first + i;
  });
  const outcome: GmGrantOutcome = {
    applied: true,
    grant_id: id,
    message: "邮件已写入邮箱；停服发放，玩家重新进入游戏后可见",
    note: "",
    receipt: {
      action: "mail.send",
      mail_id: first + assets.length,
      recipient: Number(input.character ?? 0),
      sender: "GM",
      body,
      gold,
      template,
      amount,
      assets,
      capacity_before: { messages: 12, unclaimed_assets: 3 },
    },
  };
  issuedGrants.set(id, outcome);
  return outcome;
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
