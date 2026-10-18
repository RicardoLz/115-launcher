/// 沙盒里的静态数据。字段一律照 src/bridge.ts 的接口写，缺一个字段界面就会在
/// 控制台里报 undefined —— 那也是有用的信号，但会盖掉真正要看的东西。
import type {
  AutofireView,
  Board,
  FeedbackMeta,
  HealthReport,
  ModsState,
  PortReport,
  SavesState,
} from "../src/bridge";

export const health: HealthReport = {
  items: [
    { id: "root", name: "整合包目录", level: "ok", value: "D:\\115us\\服务端", hint: "" },
    {
      id: "client",
      name: "游戏客户端",
      level: "ok",
      value: "D:\\115us\\客户端\\DFO.exe",
      hint: "已识别",
    },
    {
      id: "pvf",
      name: "内层 PVF",
      level: "warn",
      value: "Script.inner.pvf 缺失",
      hint: "启动时会尝试本地生成",
    },
    { id: "vc", name: "运行库", level: "ok", value: "VC++ 2015-2022 x86/x64", hint: "" },
    { id: "storage", name: "存储", level: "ok", value: "sqlite：runtime/storage/local.sqlite3", hint: "" },
  ],
  ok: 3,
  warn: 1,
  error: 0,
  elapsed_ms: 812,
};

const gatewayOwner = {
  port: 7001,
  port_label: "7001 网关",
  listening: false,
  pid: null,
  image: null,
  kind: "gateway",
  kind_label: "服务端网关",
};

export const ports: PortReport = {
  gateway: gatewayOwner,
  leftovers: [],
  blocked: [],
  killable: [],
  protected_ones: [],
  killed: [],
  failed: [],
  rechecked: false,
  summary: "端口干净，没有残留",
};

export function portsBusy(): PortReport {
  return {
    ...ports,
    gateway: {
      ...gatewayOwner,
      listening: true,
      pid: 4242,
      image: "dfolauncher.exe",
      kind_label: "服务端网关（别人的实例）",
    },
    blocked: [
      {
        ...gatewayOwner,
        listening: true,
        pid: 4242,
        image: "dfolauncher.exe",
        kind: "foreign",
        kind_label: "非本启动器进程",
      },
    ],
    protected_ones: [
      {
        port: 8080,
        port_label: "8080 系统服务",
        listening: true,
        pid: 900,
        image: "svchost.exe",
        kind: "protected",
        kind_label: "受保护：绝不结束",
      },
    ],
    leftovers: [{ pid: 5150, image: "DFO.exe" }],
    killable: [4242],
    summary: "网关被别的实例占着，另有 1 个客户端残留",
  };
}

export const mods: ModsState = {
  server: {
    mods_dir: "D:\\115us\\服务端\\mods",
    enabled_present: true,
    gateway_note: "网关读取 enabled.txt 于启动时",
    broken_note: "",
    ignored_note: "2 个目录不在清单里，已忽略",
    evidence_run: "roles_persist_20261017_2045",
    mods: [
      { id: "max-item-period", purpose: "拉长道具期限显示", enabled: true, seen: true },
      { id: "skin-cargo", purpose: "皮肤仓库与伤害字体", enabled: true, seen: true },
      { id: "legacy-loot", purpose: "旧版拾取规则", enabled: false, seen: false },
    ],
  },
  scripts: {
    dir: "D:\\115us\\服务端\\mods\\scripts",
    exists: true,
    scripts: [
      { name: "autofish.nut", size: 4096 },
      { name: "very_long_rule_script_name_for_overflow_checking.nut", size: 128 },
    ],
    provided: ["autofish.nut"],
  },
  client: {
    dir: "D:\\115us\\客户端\\plugins",
    present: true,
    manifest: "present",
    auto_discover: true,
    switches: "present",
    switch_dir: "D:\\115us\\客户端\\mods",
    switchable: true,
    bypass_note: "",
    loader: { seen: true, ready: true, reject_reason: "", declared: 3, started: 3, failed: 0 },
    plugins: [
      {
        id: "dfo-autopickup",
        dll: "dfo_autopickup.dll",
        in_manifest: true,
        enabled: true,
        deps: ["dfo-hook-host"],
        description: "自动拾取",
        loaded: true,
        load_note: "",
        start_return: 0,
        ready: true,
        reject_reason: "",
      },
      {
        id: "dfo-hook-host",
        dll: "dfo_hook_host.dll",
        in_manifest: true,
        enabled: true,
        deps: [],
        description: "统一入口宿主",
        loaded: true,
        load_note: "",
        start_return: 0,
        ready: true,
        reject_reason: "",
      },
    ],
    host_log: "D:\\115us\\客户端\\logs\\host.log",
    host_tail: ["[host] ModStart dfo-autopickup => 0", "[host] ready"],
  },
  modkit: {
    present: true,
    registry: "D:\\115us\\modkit\\registry.json",
    total: 12,
    entries: 9,
    risky: [{ id: "pvf-repack", target: "Script.pvf", kind: "整包替换" }],
  },
};

export const saves: SavesState = {
  save: {
    db_path: "D:\\115us\\服务端\\runtime\\storage\\local.sqlite3",
    storage_dir: "D:\\115us\\服务端\\runtime\\storage",
    backup_dir: "D:\\115us\\服务端\\runtime\\storage\\backups",
    exists: true,
    size_text: "18.4 MB",
    modified: "2026-10-17 20:58",
    accounts: 2,
    characters: 7,
    detail: "sqlite 单文件 + WAL",
    sidecars: ["local.sqlite3-wal", "local.sqlite3-shm"],
  },
  backups: [
    {
      name: "2026-10-17-204531 手动",
      path: "D:\\115us\\服务端\\runtime\\storage\\backups\\2026-10-17-204531",
      size_text: "18.1 MB",
      modified: "2026-10-17 20:45",
      usable: true,
      note: "",
      accounts: 2,
      characters: 7,
      is_current: true,
    },
    {
      name: "2026-10-16-231204 启动前自动",
      path: "D:\\115us\\服务端\\runtime\\storage\\backups\\2026-10-16-231204",
      size_text: "17.9 MB",
      modified: "2026-10-16 23:12",
      usable: true,
      note: "缺 shm",
      accounts: 1,
      characters: 5,
      is_current: false,
    },
  ],
};

export const autofire: AutofireView = {
  config: {
    enabled: true,
    mute: false,
    hotkey: "F9",
    targets: ["DFO.exe"],
    entries: [
      { type: "rapid", key: "X", intervalMs: 40, holdMs: 20, trigger: "X", steps: [], enabled: true },
      {
        type: "combo",
        key: "C",
        intervalMs: 0,
        holdMs: 0,
        trigger: "C",
        steps: [
          { key: "A", holdMs: 30, gapMs: 20 },
          { key: "S", holdMs: 30, gapMs: 20 },
        ],
        enabled: false,
      },
    ],
  },
  keys: [
    { label: "X 键", value: "X" },
    { label: "C 键", value: "C" },
    { label: "空格", value: "SPACE" },
  ],
  live: {
    running: true,
    master: false,
    focused: false,
    hotkey: "F9",
    hotkey_error: "",
    targets: ["DFO.exe"],
    pids: [7777],
    entries: [
      { label: 0, kind: "rapid", held: false, busy: false, taps: 128 },
      { label: 1, kind: "combo", held: false, busy: false, taps: 0 },
    ],
    hook_events: 4096,
    sends: 128,
    send_fails: 0,
    last_fail: "",
    skip_focus: 12,
    skip_master: 3,
  },
};

export const feedbackMeta: FeedbackMeta = {
  categories: ["启动", "战斗", "拾取", "皮肤", "其它"],
  statuses: ["待处理", "已确认", "修复中", "已修复", "不修复"],
  severities: ["轻微", "一般", "严重", "阻塞"],
  frequencies: ["偶发", "必现"],
  client_version: "inner-20261014",
  environment: "Windows 11 · 原生启动",
  launcher_version: "0.1.0",
};

export const board: Board = {
  issues: [
    {
      id: "DFO-142",
      title: "关闭游戏后服务端没跟着停",
      category: "启动",
      status: "已修复",
      public_note: "已由启动器侧监听客户端退出解决",
      fix_version: "0.1.0",
      created: 1760000000,
      updated: 1760100000,
      report_count: 9,
      duplicate_count: 3,
      updates: [
        { created: 1760090000, status: "修复中", message: "定位到退出回调", fix_version: "" },
        { created: 1760100000, status: "已修复", message: "已发版", fix_version: "0.1.0" },
      ],
    },
    {
      id: "DFO-155",
      title: "副本内切换伤害字体后外观回退",
      category: "皮肤",
      status: "待处理",
      public_note: "",
      fix_version: "",
      created: 1760200000,
      updated: 1760200000,
      report_count: 2,
      duplicate_count: 0,
      updates: [],
    },
  ],
  count: 2,
  page: 1,
  report_count: 41,
  status_counts: [
    { key: "待处理", count: 1 },
    { key: "已修复", count: 1 },
  ],
  category_counts: [
    { key: "启动", count: 1 },
    { key: "皮肤", count: 1 },
  ],
};
