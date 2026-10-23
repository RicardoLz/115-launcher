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

/** GM 物品库（方案 C）：读 gmbuild 离线产物，运行期不碰 PVF。 */
export interface GmTableInfo {
  present: boolean;
  entries: number;
  source: string;
  built_at: string;
}

export interface GmCatalogStatus {
  dir: string;
  items_source: string;
  count: number;
  equipment: number;
  stackable: number;
  slots: GmTableInfo;
  jobs: GmTableInfo;
  rarity: GmTableInfo;
  names: GmTableInfo;
}

export interface GmStatusView {
  stage: string;
  error: string;
  dir: string;
  catalog: GmCatalogStatus | null;
}

/** 一件 GM 产物：代际指纹来自产物自己记的 `source`。 */
export interface GmArtifact {
  file: string;
  role: string;
  present: boolean;
  checksum: string;
  /** 产物里记的来源名（`Script.pvf` / `Script.inner.pvf`）。 */
  path: string;
  built_at: string;
  /** 产物记录的那份来源文件多大；0 = 没记。 */
  size: number;
}

/** 同源校验的一条结论：`err` 会让写链路整条拒，`warn` 只是代际漂移。 */
export interface GmAuditIssue {
  tone: "ok" | "warn" | "err";
  text: string;
}

/** 现场那份 Script.pvf：只比大小与时间，不做哈希。 */
export interface GmPvfNow {
  path: string;
  size: number;
  modified: string;
  size_matches: boolean;
  newer_than_build: boolean;
}

export interface GmAudit {
  dir: string;
  pvf_path: string;
  pvf: GmPvfNow | null;
  artifacts: GmArtifact[];
  issues: GmAuditIssue[];
  /** 三件 gmbuild 产物是否同一次构建。 */
  consistent: boolean;
}

export interface GmRebuildStep {
  step: string;
  ok: boolean;
  seconds: number;
  detail: string;
}

/** 一键重建的进度：只走 `gm-rebuild-progress` 事件，不进运行日志。 */
export interface GmRebuildProgress {
  step: string;
  /** 1 起算的那一步。 */
  step_index: number;
  step_total: number;
  /** 整轮的总进度，不是单步的。 */
  percent: number;
  text: string;
}

export interface GmRebuildReport {
  dir: string;
  pvf: string;
  /** 新产物共用的归档校验（GM 三件 + 装备目录 + 背包规则戳）。 */
  checksum: string;
  steps: GmRebuildStep[];
  /** 被替换的旧产物备份目录；空串 = 原来没有可备份的。 */
  backup: string;
  elapsed: number;
  audit: GmAudit;
}

export interface GmItemRow {
  id: number;
  name: string;
  name_en: string;
  kind: string;
  grade: number;
  rarity: number;
  rarity_label: string;
  level: number;
  slot: string;
  group: string;
  type_key: string;
  type_label: string;
  type_text: string | null;
  stack_limit: number;
  durability: number;
  jobs: string[] | null;
  grantable: boolean;
  /** 时装（服务端 `special_equipment` 容器 1 那一族）。判据住在 Rust 的 `GmItem::avatar()`，前端不复制规则。 */
  avatar: boolean;
}

export interface GmFacet {
  key: string;
  label: string;
  count: number;
}

export interface GmFacetsView {
  types: GmFacet[];
  slots: GmFacet[];
  rarities: GmFacet[];
  level_bands: GmFacet[];
}

export interface GmSearchResult {
  total: number;
  items: GmItemRow[];
}

export interface GmQuery {
  q: string;
  type_?: string;
  slot?: string;
  level_min?: number;
  level_max?: number;
  rarity?: number | null;
  limit?: number;
  offset?: number;
}

/** 发放前必读：存档是否可写（服务端租约 + 本机管理锁）。 */
export interface GmInventoryStatus {
  editable: boolean;
  server_running: boolean;
  management_busy: boolean;
  message: string;
  db_path: string;
}

export interface GmCharacter {
  id: number;
  account_id: number;
  username: string;
  name: string;
  profession: number;
}

export interface GmGrantItem {
  template: number;
  amount: number;
}

/** 角色详情头部：职业名由客户端职业名表推导，不直接读 profession。 */
export interface GmCharacterHead {
  id: number;
  account_id: number;
  wire_id: number;
  name: string;
  profession: number;
  class_name: string;
  advancement: number;
  awakening: number;
  level: number;
  experience: number;
  gold: number;
  created_at: string;
}

/** 存档里那份概览：缺键按零值，属性/技能原样透传。 */
export interface GmCharacterSummary {
  level: number;
  experience: number;
  skill_points: [number, number];
  technique_points: [number, number];
  currency_slot2: number;
  advancement: number;
  awakening: number;
  /** 原生装备栏标志：bit0 辅助装备、bit1 魔法石、bit4 耳环。 */
  equipment_slot_flags: number;
  attributes: unknown;
  learned_skills: unknown;
  skill_slots: unknown;
}

/** 一行的 revision 是编辑用的乐观锁令牌（只读浏览时只展示）。 */
export interface GmBagRow {
  collection: string;
  space: number;
  slot: number;
  group: number;
  template: number;
  name: string;
  count: number;
  worn: boolean;
  revision: string;
  can_edit_count: boolean;
  stack_limit?: number;
  /** 装备两张表（`equipment` / `worn`）那一行的当前耐久；没有这一键就不是装备行。 */
  durability?: number;
  kind?: string;
  type_label?: string;
  rarity: number;
  rarity_label?: string;
  level: number;
}

export interface GmCharacterDetail {
  character: GmCharacterHead;
  cera: number;
  summary: GmCharacterSummary;
  bag: GmBagRow[];
  bag_error: string;
}

export interface GmGrantOutcome {
  applied: boolean;
  grant_id: string;
  message: string;
  note: string;
  receipt: unknown;
}

/** 发放流水一行：只读服务端那张 `admin_grants` 审计表，最近 100 笔，新的在前。 */
export interface GmGrantRecord {
  grant_id: string;
  account_id: number;
  /** 0 = 这一笔是账号级的（只发点券可以不选角色）。 */
  character_id: number;
  /** 角色改名/删号或这一笔没角色时是空串，界面退回显示角色号。 */
  character_name: string;
  /** 管理动作的名字；发放笔写 "grant"。 */
  action: string;
  operator: string;
  /** 存档里的微秒时戳，格式化归界面。 */
  created_at: number;
  request: string;
  receipt: string;
}

/** 账号晶体仓库的一格：槽位 + 模板 + 名称（查不到时是「材料 N」）+ 当前数量。 */
export interface GmMaterialRow {
  slot: number;
  template: number;
  name: string;
  amount: number;
}

/** 一次管理操作。revision / expected_amount / template 是背包编辑的乐观锁三件套。 */
export interface GmManageInput {
  action:
    | "inventory.amount"
    | "inventory.delete"
    | "equipment.durability"
    | "materials.set"
    | "mail.send"
    | "sp"
    | "tp"
    | "sp.recalculate"
    | "equipment.unlock"
    | "level"
    | "advancement"
    | "awakening"
    | "profession"
    | "quests.ready"
    | "quests.complete"
    | "quests.abandon"
    | "quests.mainline"
    | "quests.prerequisites";
  id: string;
  account: number;
  character: number;
  template?: number;
  amount?: number;
  expected_amount?: number | null;
  /** `equipment.durability` 的第三把钥匙：界面读到的当前耐久。装备行的数量不是用户看的字段，所以不跟 `expected_amount` 混用。 */
  expected_durability?: number | null;
  reason?: string;
  revision?: string;
  collection?: string;
  space?: number;
  slot?: number;
  group?: number;
  /** 只有 `profession`（转职分支一步到位）带它，其余动作不写这一键。 */
  stage?: number | null;
  /** `quests.*` 点名的任务 id（1 至 100 个）。 */
  quests?: number[];
  /** `quests.mainline` 的目标等级：1 至角色当前等级。 */
  level?: number;
  /** 计划类动作（`quests.mainline` / `quests.prerequisites`）带来预览算出的签名。 */
  plan?: string;
  /** `mail.send` 的金币附件（0 = 不带金币）。与发放里「直接进背包」的那笔金币是两回事。 */
  gold?: number;
  /** `mail.send` 的正文。上限 512 字节，就是 `character_mail.body` 的那条 CHECK。 */
  body?: string;
}

/** 换分支可选：分支号、由客户端职业名表推导的显示名、该分支能走的觉醒阶段（0 恒在首位）。 */
export interface GmBranchOption {
  branch: number;
  name: string;
  stages: number[];
}

/** 「角色数值改写」卡片打开时读一次：当前等级/经验/转职/觉醒 + 可选分支。 */
export interface GmGrowOptions {
  level: number;
  experience: number;
  level_cap: number;
  advancement: number;
  awakening: number;
  /** 升到下一级还要多少经验；已在满级时为 0。 */
  next_level_experience: number;
  branches: GmBranchOption[];
  /** 这次写的是哪一份离线构建（gm-growth.json 的归档校验）。 */
  source_checksum: string;
  profession: number;
  class_name: string;
}

/** 库里一行任务的原样：`before` 用它把改前的状态整份记进审计。 */
export interface GmQuestRowState {
  id: number;
  status: string;
  progress: number;
  config_version: string;
  progress_model: string;
}

/** 一条前置：组内是「必须全部完成」，组与组之间是可选分支。 */
export interface GmQuestPre {
  id: number;
  name: string;
  completed: boolean;
}

/** 列表里的一行：`status` 可能是 `ready`（进度已够、只差提交），`blocked` 非空即不可改。 */
export interface GmQuestRow {
  id: number;
  name: string;
  /** `[grade]` 段原文，`[epic]` 就是主线。 */
  grade: string;
  level: number;
  status: string;
  progress: number;
  description: string;
  condition: string;
  solve: string;
  prerequisites: GmQuestPre[][];
  can_abandon: boolean;
  can_modify: boolean;
  can_trace: boolean;
  blocked: string;
}

export interface GmQuestView {
  quests: GmQuestRow[];
  /** 逐任务改写的乐观锁：界面带着它提交，库里变了就整笔拒。 */
  revision: string;
  /** 角色当前等级：一键完成主线的目标等级上限。 */
  level: number;
  usable: boolean;
  unusable_reason: string;
  source_checksum: string;
}

/** 一键完成的范围：`signature` 必须原样带回执行，变了就得重新预览。 */
export interface GmQuestPlan {
  quests: number[];
  blocked: number[];
  before: GmQuestRowState[];
  level: number;
  signature: string;
  /** 「哪个问答任务选了第几个答案」：被答案门挡掉的分支要在界面上说清楚。 */
  answer_picks?: Record<string, number>;
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
  gmStatus: () => invoke<GmStatusView>("gm_status"),
  gmReload: () => invoke<void>("gm_reload"),
  gmAudit: () => invoke<GmAudit>("gm_audit"),
  gmRebuild: (pvf?: string) => invoke<GmRebuildReport>("gm_rebuild", { pvf: pvf ?? null }),
  gmPickPvf: () => invoke<string | null>("gm_pick_pvf"),
  gmSearch: (query: GmQuery) =>
    invoke<GmSearchResult>("gm_search", {
      query: query.q,
      type_: query.type_ ?? "",
      slot: query.slot ?? "",
      level_min: query.level_min ?? 0,
      level_max: query.level_max ?? 0,
      rarity: query.rarity ?? null,
      limit: query.limit ?? 0,
      offset: query.offset ?? 0,
    }),
  gmItem: (id: number) => invoke<GmItemRow>("gm_item", { id }),
  gmFacets: () => invoke<GmFacetsView>("gm_facets"),
  gmInventoryStatus: () => invoke<GmInventoryStatus>("gm_inventory_status"),
  gmCharacters: () => invoke<GmCharacter[]>("gm_characters"),
  gmCharacterDetail: (account: number, character: number) =>
    invoke<GmCharacterDetail>("gm_character_detail", { account, character }),
  gmGrant: (grant: {
    id: string;
    account: number;
    character?: number;
    gold?: number;
    cera?: number;
    items?: GmGrantItem[];
    reason?: string;
  }) =>
    invoke<GmGrantOutcome>("gm_grant", {
      id: grant.id,
      account: grant.account,
      character: grant.character ?? 0,
      gold: grant.gold ?? 0,
      cera: grant.cera ?? 0,
      items: grant.items ?? [],
      reason: grant.reason ?? "",
    }),
  gmMaterials: (account: number) => invoke<GmMaterialRow[]>("gm_materials", { account }),
  gmGrantLog: () => invoke<GmGrantRecord[]>("gm_grant_log"),
  gmGrowthOptions: (account: number, character: number) =>
    invoke<GmGrowOptions>("gm_growth_options", { account, character }),
  gmManage: (input: GmManageInput) => invoke<GmGrantOutcome>("gm_manage", { input }),
  gmQuests: (account: number, character: number) =>
    invoke<GmQuestView>("gm_quests", { account, character }),
  gmQuestPreviewMainline: (account: number, character: number, level: number) =>
    invoke<GmQuestPlan>("gm_quest_preview_mainline", { account, character, level }),
  gmQuestPreviewPrerequisites: (account: number, character: number, target: number) =>
    invoke<GmQuestPlan>("gm_quest_preview_prerequisites", { account, character, target }),
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
  onGmRebuildProgress: (handler: (progress: GmRebuildProgress) => void) =>
    listen<GmRebuildProgress>("gm-rebuild-progress", (e) => handler(e.payload)),
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
