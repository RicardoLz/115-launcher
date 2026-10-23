/// 沙盒里的静态数据。字段一律照 src/bridge.ts 的接口写，缺一个字段界面就会在
/// 控制台里报 undefined —— 那也是有用的信号，但会盖掉真正要看的东西。
import type {
  AutofireView,
  Board,
  FeedbackMeta,
  GmCharacter,
  GmAudit,
  GmFacetsView,
  GmGrantRecord,
  GmItemRow,
  GmQuestPlan,
  GmQuestView,
  GmStatusView,
  GmTableInfo,
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

// ------------------------------------------------------------------ GM 物品库

const gmTable = (entries: number): GmTableInfo => ({
  present: true,
  entries,
  source: "sandbox-sandbox-sandbox",
  built_at: "2026-10-20T09:00:00Z",
});

export const gmStatus: GmStatusView = {
  stage: "就绪",
  error: "",
  dir: "D:\\115us\\源码\\115US当前源码\\gm-tool\\configs",
  catalog: {
    dir: "D:\\115us\\源码\\115US当前源码\\gm-tool\\configs",
    items_source: "32dd76978ff079529fcb0356a33ef986f9aed66ec855a908160e3c048e0dae61",
    count: 671234,
    equipment: 512345,
    stackable: 158889,
    slots: gmTable(40),
    jobs: gmTable(18),
    rarity: gmTable(9),
    names: gmTable(671234),
  },
};

/// 同源校验的沙盒夹具：三件 gmbuild 产物同一次构建、PVF 也对得上，界面该显示「一致」。
const GM_CHECKSUM = "32dd76978ff079529fcb0356a33ef986f9aed66ec855a908160e3c048e0dae61";
const GM_ARTIFACT = (file: string, role: string, checksum = GM_CHECKSUM) => ({
  file,
  role,
  present: true,
  checksum,
  path: "Script.pvf",
  built_at: "2026-10-21T05:17:55+08:00",
  size: 865854418,
});

export const gmAudit: GmAudit = {
  dir: "D:\\115us\\源码\\115US当前源码\\gm-tool\\configs",
  pvf_path: "D:\\115us\\客户端\\DFO\\Script.pvf",
  pvf: {
    path: "D:\\115us\\客户端\\DFO\\Script.pvf",
    size: 865854418,
    modified: "2026-10-21 04:58:12",
    size_matches: true,
    newer_than_build: false,
  },
  artifacts: [
    GM_ARTIFACT("items.index.json", "物品库 · gmbuild"),
    GM_ARTIFACT("gm-growth.json", "成长表 · gmbuild"),
    GM_ARTIFACT("gm-quests.json", "任务目录 · gmbuild"),
    GM_ARTIFACT("inventory.current37.json", "背包规则 · 服务端导出"),
    GM_ARTIFACT("equipment.current37.json", "装备目录 · 服务端导出"),
    {
      file: "experience.compat90.json",
      role: "经验规则 · 服务端导出",
      present: true,
      checksum: "",
      path: "",
      built_at: "",
      size: 0,
    },
    {
      file: "names.client.json",
      role: "名字表 · gmbuild",
      present: true,
      checksum: "",
      path: "",
      built_at: "",
      size: 0,
    },
    GM_ARTIFACT("equipment.slots.json", "部位表 · gmbuild", ""),
    GM_ARTIFACT("equipment.jobs.json", "职业限制表 · gmbuild", ""),
    GM_ARTIFACT("item.rarity.json", "品级表 · gmbuild", ""),
  ],
  issues: [{ tone: "ok", text: `三件 GM 产物同源（${GM_CHECKSUM.slice(0, 12)}…），装备目录与背包规则也同源。` }],
  consistent: true,
};

/// 行只留几件够点出来的：一件堆叠、一件账号仓库材料、一件时装、一件装备、一件发不了的。
export const gmItems: GmItemRow[] = [
  {
    id: 2002,
    name: "古老的魔法石",
    name_en: "Ancient Magicstone",
    kind: "stackable",
    grade: 0,
    rarity: 3,
    rarity_label: "稀有",
    level: 0,
    slot: "",
    group: "",
    type_key: "stackable",
    type_label: "材料",
    type_text: "[material]",
    stack_limit: 1000,
    durability: 0,
    jobs: null,
    grantable: true,
    avatar: false,
  },
  {
    id: 3033,
    name: "矛盾的碎片",
    name_en: "Fragment of Conflict",
    kind: "stackable",
    grade: 0,
    rarity: 5,
    rarity_label: "史诗",
    level: 0,
    slot: "",
    group: "",
    type_key: "stackable",
    type_label: "晶体材料",
    type_text: "[material]",
    stack_limit: 0,
    durability: 0,
    jobs: null,
    grantable: true,
    avatar: false,
  },
  {
    id: 5001,
    name: "上级符咒",
    name_en: "Superior Enchantment",
    kind: "stackable",
    grade: 0,
    rarity: 1,
    rarity_label: "普通",
    level: 0,
    slot: "",
    group: "",
    type_key: "stackable",
    type_label: "消耗品",
    type_text: "[throw]",
    stack_limit: 100,
    durability: 0,
    jobs: null,
    grantable: true,
    avatar: false,
  },
  {
    // 现网形态的时装：`kind` 仍是 equipment（真产物 676,442 行只有 stackable/equipment 两种值），
    // 是部位旁表把 slot 覆盖成「时装」的，所以界面与邮件都读 `avatar` 那一格。
    id: 100200301,
    name: "冰雪之心的回忆",
    name_en: "Glacier Heart Memory",
    kind: "equipment",
    grade: 0,
    rarity: 1,
    rarity_label: "普通",
    level: 0,
    slot: "时装",
    group: "时装",
    type_key: "时装",
    type_label: "时装",
    type_text: "[hat avatar]",
    stack_limit: 0,
    durability: 0,
    jobs: null,
    grantable: true,
    avatar: true,
  },
  {
    id: 100050791,
    name: "荒古遗尘太刀",
    name_en: "Diluted Bone Katana",
    kind: "equipment",
    grade: 0,
    rarity: 7,
    rarity_label: "传说",
    level: 90,
    slot: "[weapon]",
    group: "sword",
    type_key: "weapon",
    type_label: "武器",
    type_text: "[weapon]",
    stack_limit: 0,
    durability: 35,
    jobs: ["鬼剑士"],
    grantable: true,
    avatar: false,
  },
  {
    id: 10415190,
    name: "光辉的灵魂",
    name_en: "Radiant Soul",
    kind: "stackable",
    grade: 0,
    rarity: 5,
    rarity_label: "史诗",
    level: 0,
    slot: "",
    group: "",
    type_key: "stackable",
    type_label: "灵魂材料",
    type_text: "[material]",
    stack_limit: 0,
    durability: 0,
    jobs: null,
    grantable: true,
    avatar: false,
  },
  {
    id: 700001,
    name: "不认识的盒子",
    name_en: "Unknown Box",
    kind: "other",
    grade: 0,
    rarity: 0,
    rarity_label: "普通",
    level: 0,
    slot: "",
    group: "",
    type_key: "other",
    type_label: "其它",
    type_text: null,
    stack_limit: 0,
    durability: 0,
    jobs: null,
    grantable: false,
    avatar: false,
  },
];

export const gmFacets: GmFacetsView = {
  types: [
    { key: "equipment", label: "装备", count: 512345 },
    { key: "stackable", label: "消耗品/材料", count: 158889 },
    { key: "avatar", label: "时装", count: 0 },
    { key: "other", label: "其它", count: 0 },
  ],
  slots: [
    { key: "[weapon]", label: "武器", count: 120000 },
    { key: "[jacket]", label: "上衣", count: 90000 },
    { key: "[avatar]", label: "时装", count: 0 },
  ],
  rarities: [
    { key: "1", label: "普通", count: 200000 },
    { key: "3", label: "稀有", count: 150000 },
    { key: "5", label: "史诗", count: 90000 },
    { key: "7", label: "传说", count: 40000 },
  ],
  level_bands: [
    { key: "~20", label: "1–20", count: 80000 },
    { key: "81-100", label: "81–100", count: 120000 },
    { key: "116+", label: "116+", count: 4000 },
  ],
};

export const gmCharacters: GmCharacter[] = [
  { id: 10, account_id: 1, username: "probe", name: "剑魂", profession: 0 },
  { id: 11, account_id: 1, username: "probe", name: "鬼泣", profession: 1 },
  { id: 12, account_id: 2, username: "kink", name: "气功师", profession: 2 },
];

/** 存档那列 `created_at` 是微秒时戳，夹具按界面可读的时刻算，别手抄十六位数。 */
const micros = (text: string) => Date.parse(text) * 1000;

/**
 * 发放流水夹具：把这张表在界面上会走到的几种形态各摆一笔 ——
 * 发放本身（`grant`）、带中文对照的管理动作、账号级（没有角色）、服务端自己记的那类
 * （`operator` 不是 launcher），以及角色查不到名字的那一笔（退回显示角色号）。
 */
export const gmGrantLog: GmGrantRecord[] = [
  {
    grant_id: "launcher-mvi2xixv-2kindx",
    account_id: 1,
    character_id: 10,
    character_name: "剑魂",
    action: "grant",
    operator: "launcher",
    created_at: micros("2026-10-22T17:40:12"),
    request:
      '{"id":"launcher-mvi2xixv-2kindx","account_id":1,"character_id":10,"items":[{"template":15,"amount":1}],"reason":"启动器发放","operator":"launcher"}',
    receipt:
      '{"grant":"launcher-mvi2xixv-2kindx","cera_balance":295722,"items":[{"Template":15,"Amount":1,"Slots":[71]}]}',
  },
  {
    grant_id: "launcher-mvgzyaz1-xj56nf",
    account_id: 1,
    character_id: 11,
    character_name: "鬼泣",
    action: "inventory.amount",
    operator: "launcher",
    created_at: micros("2026-10-22T15:02:44"),
    request:
      '{"id":"launcher-mvgzyaz1-xj56nf","account_id":1,"character_id":11,"reason":"{\\"action\\":\\"inventory.amount\\",\\"template\\":10418035,\\"amount\\":3}","operator":"launcher"}',
    receipt: '{"applied":true,"before":{"amount":1},"after":{"amount":3},"revision":"r-8841"}',
  },
  {
    // 只发点券的那一笔可以不带角色：character_id 是 0，界面要写「账号级」而不是「角色 0」。
    grant_id: "launcher-rehearsal-20261021-a",
    account_id: 2,
    character_id: 0,
    character_name: "",
    action: "grant",
    operator: "launcher",
    created_at: micros("2026-10-21T22:14:09"),
    request: '{"id":"launcher-rehearsal-20261021-a","account_id":2,"cera":7,"reason":"实机彩排"}',
    receipt: '{"grant":"launcher-rehearsal-20261021-a","cera_balance":100007}',
  },
  {
    // 服务端建角奖励走的是同一张表：流水号不是 launcher- 前缀，操作者也不是启动器。
    grant_id: "reward:character_create:newchar.lua:3:cera",
    account_id: 1,
    character_id: 0,
    character_name: "",
    action: "grant",
    operator: "reward",
    created_at: micros("2026-10-19T09:31:57"),
    request: '{"id":"reward:character_create:newchar.lua:3:cera","account_id":1,"cera":100000}',
    receipt: '{"cera":295715}',
  },
  {
    grant_id: "launcher-mvf0aaaa-1zqqqq",
    account_id: 1,
    character_id: 12,
    character_name: "",
    // 对照表里没有的动作键：原样显示，不猜一个中文名。
    action: "grant_item",
    operator: "launcher",
    created_at: micros("2026-10-18T20:05:33"),
    request: '{"id":"launcher-mvf0aaaa-1zqqqq","account_id":1,"character_id":12,"gold":100000}',
    receipt: '{"grant":"launcher-mvf0aaaa-1zqqqq","gold_added":100000,"gold_after":11223456}',
  },
  {
    // 邮件发放那一笔：附件先取号、消息号在最后，所以 mail_id 比每个 asset id 都大。
    grant_id: "launcher-mvf0bbbb-2zqqqq",
    account_id: 1,
    character_id: 12,
    character_name: "剑帝",
    action: "mail.send",
    operator: "launcher",
    created_at: micros("2026-10-20T11:42:08"),
    request:
      '{"id":"launcher-mvf0bbbb-2zqqqq","account":1,"character":12,"action":"mail.send","gold":50000,"template":10327301,"amount":20,"body":"补给"}',
    receipt:
      '{"action":"mail.send","mail_id":41,"recipient":12,"sender":"GM","body":"补给","gold":50000,"template":10327301,"amount":20,' +
      '"assets":[{"id":39,"gold":50000},{"id":40,"item":{"stack":{"slot":0,"Template":10327301,"Amount":20,"expire_time":2147483647}}}],' +
      '"capacity_before":{"messages":12,"unclaimed_assets":3}}',
  },
];

/**
 * 任务列表夹具：把界面上要看的四种形态各摆一条 —— 已完成、可交付（进度已归零）、
 * 进行中（带「或」分支前置）、被挡住（进度模型不认）。前置那一段是 chip 换行，
 * 说明那两段是长文本，这两处才是这张卡最容易破的地方。
 */
export const gmQuests: GmQuestView = {
  quests: [
    {
      id: 3001,
      name: "初入班图城",
      grade: "[epic]",
      level: 1,
      status: "completed",
      progress: 0,
      description: "前往班图城，向赛莉亚报到。她就在城南的公告板旁边。",
      condition: "与赛莉亚对话",
      solve: "赛莉亚会告诉你祭坛的事。",
      prerequisites: [],
      can_abandon: false,
      can_modify: false,
      can_trace: false,
      blocked: "",
    },
    {
      id: 3002,
      name: "沉默的祭坛",
      grade: "[epic]",
      level: 12,
      status: "ready",
      progress: 0,
      description: "祭坛已经沉默了很久，需要先确认封印的裂缝在哪儿。",
      condition: "通关「禁忌的祭坛」地下城 1 次",
      solve: "回到赛莉亚那里复命。",
      prerequisites: [[{ id: 3001, name: "初入班图城", completed: true }]],
      can_abandon: true,
      can_modify: true,
      can_trace: true,
      blocked: "",
    },
    {
      id: 3003,
      name: "双子巨龙的踪迹",
      grade: "[epic]",
      level: 40,
      status: "accepted",
      progress: 2,
      description:
        "两条巨龙各走各的方向，先追哪一条由玩家在游戏里定；这一段文字故意写得长一些，" +
        "用来看列表里的截断与 title 全文是不是各说各话。",
      condition: "通关「双子巨龙巢穴」2 次（还剩 2 次）",
      solve: "把龙鳞交给班图城的猎人。",
      prerequisites: [
        [{ id: 3002, name: "沉默的祭坛", completed: false }],
        [{ id: 2900, name: "未解析名称 #2900", completed: false }],
      ],
      can_abandon: true,
      can_modify: true,
      can_trace: true,
      blocked: "",
    },
    {
      id: 3004,
      name: "使徒的残响（活动）",
      grade: "[legend]",
      level: 60,
      status: "accepted",
      progress: 5,
      description: "活动任务的进度模型还没进产物目录，所以这一行只能看不能改。",
      condition: "收集使徒残响 20 个",
      solve: "交给活动管理员。",
      prerequisites: [],
      can_abandon: true,
      can_modify: false,
      can_trace: false,
      blocked: "此任务进度暂不支持强制修改",
    },
  ],
  revision: "0f1e2d3c4b5a697889a0b1c2d3e4f5061728394a5b6c7d8e9f0a1b2c3d4e5f60",
  level: 85,
  usable: true,
  unusable_reason: "",
  source_checksum: "c638346fa77a523a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f30",
};

/** 一键完成主线那份预览：3 条待补、1 条被互斥挡住、问答取第 1 项。 */
export const gmQuestPlan: GmQuestPlan = {
  quests: [3002, 3003, 3100],
  blocked: [3004],
  before: [
    {
      id: 3002,
      status: "accepted",
      progress: 0,
      config_version: "c638346fa77a523a",
      progress_model: "single-meet-npc-remaining-v1",
    },
  ],
  level: 85,
  signature: "9a8b7c6d5e4f30129a8b7c6d5e4f30129a8b7c6d5e4f30129a8b7c6d5e4f3012",
  answer_picks: { "2001": 0 },
};
