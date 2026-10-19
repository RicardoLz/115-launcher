// 后端命令与事件的唯一入口：字符串只在这里出现一次，界面组件不手写 invoke 名。
import { invoke } from "@tauri-apps/api/core";
import { type UnlistenFn, listen } from "@tauri-apps/api/event";

export type Phase = "idle" | "starting" | "running" | "stopping" | "error";

export interface Status {
  phase: Phase;
  detail: string;
}

export interface StartOutcome {
  started: boolean;
  phase: Phase;
  detail: string;
}

export interface LogLine {
  seq: number;
  time: string;
  level: string;
  text: string;
}

export interface Readiness {
  can_start: boolean;
  detail: string;
  root: string;
  root_source: string;
  client_ready: boolean;
  client_dir: string;
  server_ready: boolean;
  server_detail: string;
  storage_ready: boolean;
  storage_detail: string;
  gateway_listening: boolean;
}

export type Level = "ok" | "warn" | "error";

export interface HealthItem {
  id: string;
  name: string;
  level: Level;
  value: string;
  hint: string;
}

export interface HealthReport {
  items: HealthItem[];
  ok: number;
  warn: number;
  error: number;
  elapsed_ms: number;
}

export interface PortOwner {
  port: number;
  port_label: string;
  listening: boolean;
  pid: number | null;
  image: string | null;
  kind: string;
  kind_label: string;
}

export interface Leftover {
  pid: number;
  image: string;
}

export interface PortReport {
  gateway: PortOwner;
  leftovers: Leftover[];
  blocked: PortOwner[];
  killable: number[];
  protected_ones: PortOwner[];
  killed: number[];
  failed: string[];
  rechecked: boolean;
  summary: string;
}

export interface AppInfo {
  name: string;
  version: string;
  root: string;
  root_source: string;
  app_dir: string;
  log_dir: string;
}

export interface ModeOption {
  value: string;
  label: string;
}

export interface ConfigView {
  account: string;
  accounts: string[];
  game_mode: string;
  game_modes: ModeOption[];
  auto_stop_on_client_exit: boolean;
  auto_pickup: boolean;
  auto_report: boolean;
  auto_report_prompted: boolean;
  pending_reports: number;
  client_dir: string;
  client_ready: boolean;
  storage_detail: string;
}

export interface ActionRecord {
  message: string;
}

/** 站上一条「问题」的公开摘要。created / updated 是 unix 秒，格式化口径与网页一致。 */
export interface Issue {
  id: string;
  title: string;
  category: string;
  status: string;
  public_note: string;
  fix_version: string;
  created: number;
  updated: number;
  report_count: number;
  duplicate_count: number;
  updates: IssueUpdate[];
}

export interface IssueUpdate {
  created: number;
  status: string;
  message: string;
  fix_version: string;
}

export interface Facet {
  key: string;
  count: number;
}

export interface Board {
  issues: Issue[];
  count: number;
  page: number;
  report_count: number;
  status_counts: Facet[];
  category_counts: Facet[];
}

/** 「我的反馈」的一条：本机存过编号与凭证，站上换回来的最新状态。 */
export interface Mine {
  id: string;
  title: string;
  created: number;
  status: string;
  issue: Issue | null;
}

/** 选项列表与两处预填值都由后端给，前端不另抄一份枚举。 */
export interface FeedbackMeta {
  categories: string[];
  statuses: string[];
  severities: string[];
  frequencies: string[];
  client_version: string;
  environment: string;
  launcher_version: string;
}

export interface Submission {
  title: string;
  character: string;
  steps: string;
  expected: string;
  actual: string;
  /** `2026-10-14T13:45`，留空按当下算。 */
  occurred: string;
  category: string;
  severity: string;
  frequency: string;
  location: string;
  contact: string;
  related_issue_id: string;
}

export interface SubmitOutcome {
  id: string;
  duplicate: boolean;
  issue: Issue | null;
}

export interface BoardQuery {
  page?: number;
  category?: string;
  status?: string;
  q?: string;
  sort?: string;
}

export interface PortBlocked {
  where: string;
  report: PortReport;
  hint: string;
}

/** MOD 页四摞事实：服务端 mod、规则脚本、客户端插件、modkit 安装层。 */
export interface ModsState {
  server: ServerMods;
  scripts: ScriptMods;
  client: ClientMods;
  modkit: ModkitMods;
}

export interface ServerMods {
  mods_dir: string;
  enabled_present: boolean;
  gateway_note: string;
  broken_note: string;
  ignored_note: string;
  evidence_run: string;
  mods: ServerMod[];
}

export interface ServerMod {
  id: string;
  purpose: string;
  enabled: boolean;
  seen: boolean;
}

export interface ScriptMods {
  dir: string;
  exists: boolean;
  scripts: { name: string; size: number }[];
  provided: string[];
}

export interface ClientMods {
  dir: string;
  present: boolean;
  /** plugins.json 自己在不在、认不认；宿主把认不出的清单当成没有清单。 */
  manifest: "present" | "missing" | "corrupt";
  auto_discover: boolean;
  /** mods/mods.json —— 迁移之后启动器读写的就是这一份。 */
  switches: "present" | "missing" | "corrupt";
  switch_dir: string;
  /** 勾是不是改了算数：只有统一入口这一层在跑才算。 */
  switchable: boolean;
  /** 开关会被绕过、或根本没人读时的报警；空串表示正常。 */
  bypass_note: string;
  loader: LoaderStatus;
  plugins: ClientPlugin[];
  host_log: string;
  host_tail: string[];
}

/** mod-loader 落在 mods/mod-loader.status.json 里的一页账。 */
export interface LoaderStatus {
  seen: boolean;
  ready: boolean | null;
  reject_reason: string;
  declared: number;
  started: number;
  failed: number;
}

export interface ClientPlugin {
  id: string;
  dll: string;
  in_manifest: boolean;
  enabled: boolean;
  deps: string[];
  /** 内置中文功能提示，表里没有就是空串。 */
  description: string;
  /** 统一入口报的实装结果；null = 这一层还没跑过。 */
  loaded: boolean | null;
  load_note: string;
  /** 统一入口调 `ModStart` 的返回值：0 = 入口认账，非 0 = 装进进程了但它拒绝启动。 */
  start_return: number;
  ready: boolean | null;
  reject_reason: string;
}

export interface ModkitMods {
  present: boolean;
  registry: string;
  total: number;
  entries: number;
  risky: { id: string; target: string; kind: string }[];
}

/// 浮出/归位面板窗口：x、y 是相对主窗左上角的 CSS 像素。
export interface PanelMoved {
  panel: string;
  x: number;
  y: number;
}

/** 独立窗口拖回主窗的路上：落点用主窗的逻辑坐标，inside=false 表示已经拖出去了。 */
export interface PanelHover {
  panel: string;
  x: number;
  y: number;
  inside: boolean;
}

/**
 * 连发配置。字段是 camelCase —— 这份 JSON 要和旧启动器共用同一个 `autofire` 小节，
 * 磁盘上的键名不归我们改；下面的实况结构是内存里的，沿用后端惯用的 snake_case。
 */
export interface AutofireStep {
  key: string;
  holdMs: number;
  gapMs: number;
}

export interface AutofireEntry {
  type: "rapid" | "combo";
  key: string;
  intervalMs: number;
  holdMs: number;
  trigger: string;
  steps: AutofireStep[];
  enabled: boolean;
}

export interface AutofireConfig {
  enabled: boolean;
  mute: boolean;
  hotkey: string;
  targets: string[];
  entries: AutofireEntry[];
}

export interface KeyOption {
  label: string;
  value: string;
}

export interface AutofireEntryLive {
  label: number;
  kind: string;
  held: boolean;
  busy: boolean;
  taps: number;
}

export interface AutofireLive {
  running: boolean;
  master: boolean;
  focused: boolean;
  hotkey: string;
  hotkey_error: string;
  targets: string[];
  pids: number[];
  entries: AutofireEntryLive[];
  hook_events: number;
  sends: number;
  send_fails: number;
  last_fail: string;
  skip_focus: number;
  skip_master: number;
}

export interface AutofireView {
  config: AutofireConfig;
  keys: KeyOption[];
  live: AutofireLive;
}

export interface SaveInfo {
  db_path: string;
  storage_dir: string;
  backup_dir: string;
  exists: boolean;
  size_text: string;
  modified: string;
  accounts: number;
  characters: number;
  detail: string;
  sidecars: string[];
}

export interface BackupItem {
  name: string;
  path: string;
  size_text: string;
  modified: string;
  usable: boolean;
  note: string;
  accounts: number;
  characters: number;
  is_current: boolean;
}

export interface SavesState {
  save: SaveInfo;
  backups: BackupItem[];
}

/** 主页背景素材：kind 决定画 <img> 还是 <video>，exists 说素材文件还在不在。 */
export interface BackgroundState {
  path: string;
  kind: "none" | "image" | "video";
  exists: boolean;
  note: string;
  /** 壁纸主题色 `#rrggbb`；空串 = 取不到（没背景、视频、坏图、整张灰阶），样式退回各自兜底色。 */
  accent: string;
}

/** 服务端更新检查结果。 */
export interface ServerUpdateCheck {
  current_tag: string | null;
  latest_tag: string;
  has_update: boolean;
  asset_name: string;
  asset_size: number;
  download_url: string;
}

export interface SelfUpdateCheck {
  current_version: string;
  latest_version: string;
  has_update: boolean;
  asset_name: string;
  asset_size: number;
  download_url: string;
}

/** 一条下载的真实进度。phase=deploy 表示字节已经下完，正在解压部署 / 替换重启。 */
export interface UpdateProgress {
  kind: "self" | "server";
  phase: "download" | "deploy";
  percent: number;
  text: string;
}

/**
 * 服务端内存三键。`null` = 这个键不写 = 不限，与 Go 侧 `*uint64` 的口径一一对应；
 * 键名沿用磁盘上的写法（snake_case），因为这份文件是编排器在读。
 */
export interface MemoryTuning {
  server_memory_limit_mib: number | null;
  go_mem_target_mib: number | null;
  import_max_procs: number | null;
}

export interface MemoryConfig {
  tuning: MemoryTuning;
  path: string;
  /** 边界由后端给：Go 改了这里，界面不用跟着改代码。 */
  max_memory_mib: number;
  max_import_procs: number;
  /** 不写 import_max_procs 时 Go 自己用的默认值（CPU 逻辑核数）。 */
  cpu_count: number;
}

/** 从服务端 `gateway.err` 的 MEMORY_GUARD 采样行读来的实况；`null` = 读不到，界面就藏起这块。 */
export interface MemoryLive {
  session: string;
  path: string;
  pid: number;
  policy: string;
  /** `null` = 本场没设硬上限。 */
  hard_limit_mib: number | null;
  low_memory_mib: number;
  seconds: number;
  /** 进程私有提交：离硬上限多远看这个数。 */
  private_mib: number;
  working_mib: number;
  system_available_mib: number;
  /** 只有本场收尾后才有。 */
  peak_private_mib: number | null;
  ended: boolean;
  stale: boolean;
  samples: number;
  series: number[];
  /** 空串 = 正常；否则是要给玩家看的提醒。 */
  note: string;
}

export const commands = {
  appInfo: () => invoke<AppInfo>("app_info"),
  logSnapshot: (limit = 600) => invoke<LogLine[]>("log_snapshot", { limit }),
  readiness: () => invoke<Readiness>("readiness_state"),
  envCheck: () => invoke<HealthReport>("env_check"),
  status: () => invoke<Status>("game_status"),
  start: (mode?: string) => invoke<StartOutcome>("game_start", { mode }),
  stop: () => invoke<boolean>("game_stop"),
  ports: () => invoke<PortReport>("ports_report"),
  forceClean: () => invoke<PortReport>("force_clean"),
  config: () => invoke<ConfigView>("config_get"),
  setAccount: (account: string) => invoke<ActionRecord>("set_account", { account }),
  setGameMode: (mode: string) => invoke<ActionRecord>("set_game_mode", { mode }),
  setAutoStop: (enabled: boolean) => invoke<ActionRecord>("set_auto_stop", { enabled }),
  setAutoPickup: (enabled: boolean) => invoke<ActionRecord>("set_auto_pickup", { enabled }),
  setAutoReport: (enabled: boolean) => invoke<ActionRecord>("set_auto_report", { enabled }),
  modsState: () => invoke<ModsState>("mods_state"),
  setServerMod: (id: string, enabled: boolean) =>
    invoke<ActionRecord>("set_server_mod", { id, enabled }),
  setClientMod: (id: string, enabled: boolean) =>
    invoke<ActionRecord>("set_client_mod", { id, enabled }),
  importClientMod: () => invoke<ActionRecord>("import_client_mod"),
  importServerMod: () => invoke<ActionRecord>("import_server_mod"),
  openModkitDir: () => invoke<ActionRecord>("open_modkit_dir"),
  autofireGet: () => invoke<AutofireView>("autofire_get"),
  autofireLive: () => invoke<AutofireLive>("autofire_live"),
  autofireSetMaster: (on: boolean) => invoke<AutofireLive>("autofire_set_master", { on }),
  autofireSave: (config: AutofireConfig) =>
    invoke<ActionRecord>("autofire_save", { config }),
  autofireStop: () => invoke<ActionRecord>("autofire_stop"),
  savesState: () => invoke<SavesState>("saves_state"),
  savesBackup: () => invoke<ActionRecord>("saves_backup"),
  savesRestore: (path: string) => invoke<ActionRecord>("saves_restore", { path }),
  savesDelete: (path: string) => invoke<ActionRecord>("saves_delete", { path }),
  backgroundState: () => invoke<BackgroundState>("background_state"),
  backgroundPick: () => invoke<BackgroundState>("background_pick"),
  backgroundClear: () => invoke<BackgroundState>("background_clear"),
  reportsRetry: () => invoke<void>("reports_retry"),
  feedbackMeta: () => invoke<FeedbackMeta>("feedback_meta"),
  feedbackBoard: (query: BoardQuery) =>
    invoke<Board>("feedback_board", {
      page: query.page ?? 1,
      category: query.category ?? "",
      status: query.status ?? "",
      q: query.q ?? "",
      sort: query.sort ?? "",
    }),
  feedbackIssue: (id: string) => invoke<Issue>("feedback_issue", { id }),
  feedbackMine: () => invoke<Mine[]>("feedback_mine"),
  feedbackSubmit: (input: Submission) => invoke<SubmitOutcome>("feedback_submit", { input }),
  pickClientDir: () => invoke<ActionRecord>("pick_client_dir"),
  discoverClient: () => invoke<ActionRecord>("discover_client"),
  memoryConfig: () => invoke<MemoryConfig>("memory_config"),
  memorySave: (tuning: MemoryTuning) => invoke<ActionRecord>("memory_save", { tuning }),
  memoryLive: () => invoke<MemoryLive | null>("memory_live"),
  serverUpdateCheck: () => invoke<ServerUpdateCheck>("server_update_check"),
  serverUpdateApply: () => invoke<ActionRecord>("server_update_apply"),
  selfUpdateCheck: () => invoke<SelfUpdateCheck>("self_update_check"),
  selfUpdateApply: (info: SelfUpdateCheck) =>
    invoke<ActionRecord>("self_update_apply", { info }),
  selfUpdateCleanup: () => invoke<void>("self_update_cleanup"),
  quit: (choice: "stop" | "keep" | "cancel") =>
    invoke<ActionRecord>("quit_with_choice", { choice }),
  panelOpen: (panel: string) => invoke<void>("panel_open", { panel }),
  panelDock: (panel: string) => invoke<void>("panel_dock", { panel }),
  panelClosed: (panel: string) => invoke<void>("panel_closed", { panel }),
};

export const events = {
  onLog: (handler: (line: LogLine) => void) => listen<LogLine>("log", (e) => handler(e.payload)),
  onPhase: (handler: (status: Status) => void) =>
    listen<Status>("game-phase", (e) => handler(e.payload)),
  onReadiness: (handler: (ready: Readiness) => void) =>
    listen<Readiness>("readiness-changed", (e) => handler(e.payload)),
  onAlert: (handler: (alert: { text: string }) => void) =>
    listen<{ text: string }>("game-alert", (e) => handler(e.payload)),
  onPortBlocked: (handler: (blocked: PortBlocked) => void) =>
    listen<PortBlocked>("game-port-blocked", (e) => handler(e.payload)),
  onPanelReturned: (handler: (moved: PanelMoved) => void) =>
    listen<PanelMoved>("panel-returned", (e) => handler(e.payload)),
  onPanelClosed: (handler: (panel: string) => void) =>
    listen<{ panel: string }>("panel-closed", (e) => handler(e.payload.panel)),
  onPanelHover: (handler: (hover: PanelHover) => void) =>
    listen<PanelHover>("panel-hover", (e) => handler(e.payload)),
  onFirstRunNoServer: (handler: () => void) =>
    listen("first-run-no-server", () => handler()),
  onServerUpdateAvailable: (handler: (info: ServerUpdateCheck) => void) =>
    listen<ServerUpdateCheck>("server-update-available", (e) => handler(e.payload)),
  onUpdateProgress: (handler: (progress: UpdateProgress) => void) =>
    listen<UpdateProgress>("update-progress", (e) => handler(e.payload)),
};

export const PHASE_LABEL: Record<Phase, string> = {
  idle: "未运行",
  starting: "启动中",
  running: "运行中",
  stopping: "停止中",
  error: "异常",
};

/** 后端返回的 Err 字符串就是给人看的话，界面不再另外编一套文案。 */
export function errText(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  return String(err);
}

export type { UnlistenFn };
