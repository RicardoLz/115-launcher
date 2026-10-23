import {
  Fragment,
  useCallback,
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type ReactNode,
  type SetStateAction,
} from "react";
import { GM_KEYS, type DockNode, type GmKey } from "../dock";
import { attributeRows } from "../attrLabel";
import DockLayer from "./DockLayer";
import { PanelFrame } from "./PanelFrame";
import {
  commands,
  errText,
  events,
  type GmAudit,
  type GmBagRow,
  type GmCharacter,
  type GmCharacterDetail,
  type GmFacetsView,
  type GmGrantOutcome,
  type GmGrantRecord,
  type GmGrowOptions,
  type GmInventoryStatus,
  type GmItemRow,
  type GmManageInput,
  type GmMaterialRow,
  type GmQuestPlan,
  type GmQuestView,
  type GmRebuildProgress,
  type GmRebuildStep,
  type GmStatusView,
  type GmTableInfo,
} from "../bridge";

const PAGE = 60;

/** 九张卡在停靠层里的名字：摆放栏的胶囊与幽灵块都读这一份。 */
const GM_TITLES: Record<GmKey, string> = {
  search: "物品库查询",
  results: "查询结果",
  target: "发放目标",
  detail: "物品详情",
  bag: "角色详情与背包",
  stats: "角色数值改写",
  quests: "任务与一键完成",
  materials: "账号晶体仓库",
  records: "发放记录",
};

/**
 * 流水里那一行的动作名：`grant` 是发放本身（请求体没有 `action` 键时 Rust 给的就是它），
 * 其余照 `GmManageInput.action`。对不上的键原样显示——多出来的动作说明得加，不能在这里猜个中文名。
 */
const ACTION_LABELS: Record<string, string> = {
  grant: "发放",
  "inventory.amount": "改背包数量",
  "inventory.delete": "删背包一行",
  "equipment.durability": "改装备耐久",
  "materials.set": "改晶体仓库",
  "mail.send": "发邮件",
  sp: "写 SP",
  tp: "写 TP",
  "sp.recalculate": "SP 按等级算满",
  "equipment.unlock": "开装备栏位",
  level: "改等级",
  advancement: "转职",
  awakening: "觉醒",
  profession: "改职业",
  "quests.ready": "任务置为可完成",
  "quests.complete": "完成任务",
  "quests.abandon": "放弃任务",
  "quests.mainline": "一键主线",
  "quests.prerequisites": "追溯前置",
};

/**
 * `admin_grants.created_at` 是微秒时戳（建表默认值 `(julianday('now')-2440587.5)*86400000000`，
 * 本机真档 7 笔实测全是 integer）。换算成毫秒交给 Date，异常值说「时间未知」而不是硬凑一个日期。
 */
function grantTime(us: number): string {
  if (!Number.isFinite(us) || us <= 0) return "时间未知";
  const at = new Date(Math.round(us / 1000));
  if (Number.isNaN(at.getTime())) return "时间未知";
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())} ${pad(at.getHours())}:${pad(at.getMinutes())}`;
}

/** 流水号：前缀 + 毫秒时戳 + 随机尾，服务端按它做幂等，重放不会二次入账。 */
const newGrantId = () =>
  `launcher-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;

function receiptText(out: GmGrantOutcome): string {
  const r = (out.receipt ?? {}) as {
    cera_balance?: number;
    gold_added?: number;
    gold_after?: number;
    items?: { Amount?: number }[];
    materials?: { amount: number }[];
  };
  const parts: string[] = [];
  if (r.gold_added) {
    parts.push(`金币 +${r.gold_added.toLocaleString()}（现存 ${r.gold_after?.toLocaleString() ?? 0}）`);
  }
  if (r.cera_balance) parts.push(`点券余额 ${r.cera_balance.toLocaleString()}`);
  if (r.items?.length) {
    const n = r.items.reduce((sum, it) => sum + (it.Amount ?? 0), 0);
    parts.push(`物品 ${n.toLocaleString()} 件，分 ${r.items.length} 行`);
  }
  if (r.materials?.length) {
    parts.push(`晶体/灵魂材料 ${r.materials.length} 项已进账号仓库`);
  }
  // Rust 那句回执自带句号，接明细时要先摘掉，否则出现「发放成功。：金币…」。
  const head = parts.length ? out.message.replace(/。$/, "") : out.message;
  return `${head}${parts.length ? `：${parts.join("、")}。` : ""}${out.note}`;
}

/** 等级档的界内区间：与产物 LEVEL_BANDS 同表，`max` 0 = 无上限。 */
const BAND_RANGE: Record<string, [number, number]> = {
  "~20": [1, 20],
  "21-50": [21, 50],
  "51-80": [51, 80],
  "81-100": [81, 100],
  "101-110": [101, 110],
  "111-115": [111, 115],
  "116+": [116, 0],
};

const KIND_LABEL: Record<string, string> = {
  equipment: "装备",
  stackable: "消耗品/材料",
  avatar: "时装",
  other: "其它",
};

/** Go readGMInventory 展开的容器；特殊装备表按空间再分时装与宠物。 */
const BAG_LABEL: Record<string, string> = {
  items: "背包",
  equipment: "装备库",
  worn: "已穿戴",
};

function bagLabel(row: GmBagRow): string {
  if (row.collection !== "special_equipment") return BAG_LABEL[row.collection] ?? row.collection;
  return row.space === 1 ? "时装" : row.space === 7 ? "宠物" : `特殊空间 ${row.space}`;
}

/** 原生装备栏的三个位：服务端入城时查的就是这三档，写侧掩码 0x13。 */
const SLOT_BITS: { value: number; label: string }[] = [
  { value: 0x1, label: "辅助装备" },
  { value: 0x2, label: "魔法石" },
  { value: 0x10, label: "耳环" },
];

const clampPoint = (raw: string) => Math.min(65535, Math.max(0, Number(raw) || 0));

/** 觉醒阶段的显示名：0 是「未觉醒」那一档，Go 的表里也是这一格没有等级门。 */
const AWAKENING_LABEL = ["未觉醒", "一觉", "二觉", "三觉"];

function tableLine(label: string, t: GmTableInfo) {
  return t.present ? `${label} 表 ${t.entries.toLocaleString()} 条` : `${label} 表缺失`;
}

/** 字节数给玩家看：1.2 GB 比 1288490188 有用。 */
function bytes(n: number) {
  if (n <= 0) return "0 B";
  if (n >= 1024 ** 3) return `${(n / 1024 ** 3).toFixed(2)} GB`;
  if (n >= 1024 ** 2) return `${(n / 1024 ** 2).toFixed(1)} MB`;
  return `${(n / 1024).toFixed(0)} KB`;
}

/** `ready` 是读侧按进度推出来的「只差提交」，库里那一行仍是 accepted。 */
const QUEST_STATUS: Record<string, string> = {
  accepted: "进行中",
  ready: "可交付",
  completed: "已完成",
};

const QUEST_FILTERS = ["全部", "进行中", "主线"] as const;

/** 任务说明能到几百字，列表里截断，全文挂 title。 */
const clip = (text: string, n = 48) => (text.length > n ? `${text.slice(0, n)}…` : text);

export default function GmPage({
  onNotice,
  dock: gmDock,
  setDock: setGmDock,
}: {
  onNotice: (text: string, tone?: "ok" | "err") => void;
  /// 九张卡的摆法存在 App 上：换页回来还是玩家摆好的样子，和开发者页同一个口径。
  dock: DockNode<GmKey> | null;
  setDock: Dispatch<SetStateAction<DockNode<GmKey> | null>>;
}) {
  const [status, setStatus] = useState<GmStatusView | null>(null);
  const [facets, setFacets] = useState<GmFacetsView | null>(null);
  const [q, setQ] = useState("");
  const [type, setType] = useState("");
  const [slot, setSlot] = useState("");
  const [band, setBand] = useState("");
  const [rarity, setRarity] = useState("");
  const [rows, setRows] = useState<GmItemRow[]>([]);
  const [total, setTotal] = useState(0);
  const [offset, setOffset] = useState(0);
  const [note, setNote] = useState("");
  const [audit, setAudit] = useState<GmAudit | null>(null);
  const [auditBusy, setAuditBusy] = useState(false);
  const [rebuildBusy, setRebuildBusy] = useState(false);
  const [pvf, setPvf] = useState("");
  const [rebuildNote, setRebuildNote] = useState<{ text: string; ok: boolean } | null>(null);
  // 重建进度是事件推过来的（不进运行日志），逐步明细只在一次跑完以后留在这儿。
  const [progress, setProgress] = useState<GmRebuildProgress | null>(null);
  const [steps, setSteps] = useState<GmRebuildStep[] | null>(null);
  const [selected, setSelected] = useState<GmItemRow | null>(null);
  const [inv, setInv] = useState<GmInventoryStatus | null>(null);
  const [chars, setChars] = useState<GmCharacter[]>([]);
  const [target, setTarget] = useState(0);
  const [amount, setAmount] = useState(1);
  const [gold, setGold] = useState(0);
  const [cera, setCera] = useState(0);
  const [reason, setReason] = useState("");
  const [mailBody, setMailBody] = useState("");
  const [granting, setGranting] = useState(false);
  const [outcome, setOutcome] = useState<{ text: string; ok: boolean } | null>(null);
  const [detail, setDetail] = useState<GmCharacterDetail | null>(null);
  const [detailBusy, setDetailBusy] = useState(false);
  const [detailError, setDetailError] = useState("");
  const [edit, setEdit] = useState<{ row: GmBagRow; value: number; mode: "count" | "durability" } | null>(
    null,
  );
  const [doomed, setDoomed] = useState<GmBagRow | null>(null);
  const [attrsOpen, setAttrsOpen] = useState(false);
  const [manageBusy, setManageBusy] = useState(false);
  const [manageOutcome, setManageOutcome] = useState<{ text: string; ok: boolean } | null>(null);
  const [materials, setMaterials] = useState<GmMaterialRow[] | null>(null);
  const [materialDraft, setMaterialDraft] = useState<Record<number, number>>({});
  const [materialBusy, setMaterialBusy] = useState(false);
  const [records, setRecords] = useState<GmGrantRecord[] | null>(null);
  const [recordsBusy, setRecordsBusy] = useState(false);
  const [recordsError, setRecordsError] = useState("");
  /** 展开看请求与回执的那一笔（按流水号认，列表本身不重排）。 */
  const [recordOpen, setRecordOpen] = useState<string | null>(null);
  const recordSeq = useRef(0);
  const loadRecords = useCallback(async () => {
    const my = ++recordSeq.current;
    setRecordsBusy(true);
    try {
      const rows = await commands.gmGrantLog();
      if (my !== recordSeq.current) return;
      setRecords(rows);
      setRecordsError("");
    } catch (err) {
      if (my !== recordSeq.current) return;
      setRecords(null);
      setRecordsError(errText(err));
    } finally {
      if (my === recordSeq.current) setRecordsBusy(false);
    }
  }, []);
  const [points, setPoints] = useState({ sp: 100, tp: 100 });
  const [unlock, setUnlock] = useState(0);
  const [grow, setGrow] = useState<GmGrowOptions | null>(null);
  const [growBusy, setGrowBusy] = useState(false);
  const [growError, setGrowError] = useState("");
  const [levelTarget, setLevelTarget] = useState(2);
  const [branch, setBranch] = useState(0);
  const [stage, setStage] = useState(0);
  const growSeq = useRef(0);
  const [quests, setQuests] = useState<GmQuestView | null>(null);
  const [questBusy, setQuestBusy] = useState(false);
  const [questError, setQuestError] = useState("");
  const [questFilter, setQuestFilter] = useState<(typeof QUEST_FILTERS)[number]>("全部");
  const [questPicked, setQuestPicked] = useState<number[]>([]);
  const [questTrace, setQuestTrace] = useState<number | null>(null);
  /** 展开看说明的那一行（条件 / 完成两段折叠自产物）。 */
  const [questOpen, setQuestOpen] = useState<number | null>(null);
  const [questPlan, setQuestPlan] = useState<{
    kind: "mainline" | "prerequisites";
    /** 追溯前置的那条目标任务；主线不用（0）。 */
    target: number;
    plan: GmQuestPlan;
  } | null>(null);
  const [questDoomed, setQuestDoomed] = useState<number[] | null>(null);
  const [questPlanBusy, setQuestPlanBusy] = useState(false);
  const [mainlineLevel, setMainlineLevel] = useState(0);
  const questSeq = useRef(0);
  const questPlanSeq = useRef(0);
  const seq = useRef(0);

  const loadTarget = useCallback(async () => {
    const [state, list] = await Promise.allSettled([
      commands.gmInventoryStatus(),
      commands.gmCharacters(),
    ]);
    if (state.status === "fulfilled") setInv(state.value);
    if (list.status === "fulfilled") setChars(list.value);
  }, []);

  useEffect(() => {
    void loadTarget();
  }, [loadTarget]);

  // 后台加载是线程干的：没就绪前 2s 一跳，就绪即停（和连发实况同一路数）。
  // 重建 / 重新加载之后还会再起一次，所以抽成可复用的「盯到就绪为止」。
  const watchStatus = useCallback(async (ticks = 90) => {
    for (let i = 0; i < ticks; i += 1) {
      try {
        const view = await commands.gmStatus();
        setStatus(view);
        if (view.stage === "就绪") {
          commands.gmFacets().then(setFacets, () => undefined);
          return view;
        }
      } catch {
        /* 状态命令失败就不打扰，下一跳再说 */
      }
      await new Promise((resolve) => window.setTimeout(resolve, 2000));
    }
    return null;
  }, []);

  useEffect(() => {
    void watchStatus();
  }, [watchStatus]);

  // 重建进度走单独一条事件（它不进运行日志，所以只能在这儿接）。
  // percent 是绝对值而不是增量，晚订阅一格也只是少画一格，不会画错。
  useEffect(() => {
    let alive = true;
    const bind = async () => {
      const stop = await events.onGmRebuildProgress((next) => {
        if (alive) setProgress(next);
      });
      if (alive) return;
      stop();
    };
    void bind();
    return () => {
      alive = false;
    };
  }, []);

  const runSearch = useCallback(
    async (at: number) => {
      const my = ++seq.current;
      setNote("");
      const [lo, hi] = band ? BAND_RANGE[band] ?? [0, 0] : [0, 0];
      try {
        const res = await commands.gmSearch({
          q,
          type_: type,
          slot,
          level_min: lo,
          level_max: hi,
          rarity: rarity === "" ? null : Number(rarity),
          limit: PAGE,
          offset: at,
        });
        if (my !== seq.current) return;
        setRows(res.items);
        setTotal(res.total);
        setOffset(at);
      } catch (err) {
        if (my !== seq.current) return;
        setRows([]);
        setTotal(0);
        const text = errText(err);
        setNote(
          text === "加载中"
            ? "物品库还在后台加载（约 67 万条），稍等再查。"
            : text,
        );
      }
    },
    [q, type, slot, band, rarity],
  );

  const ready = status?.stage === "就绪";

  const reload = async () => {
    try {
      await commands.gmReload();
      onNotice("已重新提交加载，产物读完后自动恢复查询");
      await watchStatus();
    } catch (err) {
      onNotice(errText(err), "err");
    }
  };

  // 同源校验：哪几件产物是同一次构建、现场那份 PVF 像不像换过。只读，随时能点。
  const loadAudit = useCallback(async () => {
    setAuditBusy(true);
    try {
      setAudit(await commands.gmAudit());
    } catch (err) {
      setAudit(null);
      setRebuildNote({ text: `校验失败：${errText(err)}`, ok: false });
    } finally {
      setAuditBusy(false);
    }
  }, []);

  useEffect(() => {
    void loadAudit();
  }, [loadAudit]);

  const pickPvf = async () => {
    try {
      const picked = await commands.gmPickPvf();
      if (picked) {
        setPvf(picked);
        setRebuildNote({ text: `重建将用这份 PVF：${picked}`, ok: true });
      }
    } catch (err) {
      setRebuildNote({ text: errText(err), ok: false });
    }
  };

  // 重建走外部 gmbuild：七步、整轮两分钟上下，进度由 gm-rebuild-progress 事件一格一格推过来。
  const rebuild = async () => {
    setRebuildBusy(true);
    setProgress(null);
    setSteps(null);
    setRebuildNote({ text: "重建中：七步（名字表 → 物品库 → 旁表 → 养成 → 任务 → 装备目录 → 背包换戳）…", ok: true });
    try {
      const report = await commands.gmRebuild(pvf || undefined);
      setAudit(report.audit);
      setSteps(report.steps);
      setRebuildNote({
        text: `已重建：归档校验 ${report.checksum.slice(0, 12)}…，用时 ${report.elapsed.toFixed(1)}s${
          report.backup ? `，旧产物备份在 ${report.backup}` : ""
        }`,
        ok: true,
      });
      onNotice("GM 产物已换新，物品库正在重载", "ok");
      await watchStatus();
    } catch (err) {
      setRebuildNote({ text: errText(err), ok: false });
      onNotice(errText(err), "err");
    } finally {
      setRebuildBusy(false);
    }
  };

  const select = (row: GmItemRow) => {
    setSelected(row);
    setOutcome(null);
    commands.gmItem(row.id).then(setSelected, () => undefined);
  };

  const character = chars.find((c) => c.id === target) ?? null;

  // 只读浏览跟着「发放目标」的角色走；发放后刷新角色列表也会顺带把背包重读一遍。
  const detailSeq = useRef(0);
  const loadDetail = useCallback(async (account: number, id: number) => {
    const my = ++detailSeq.current;
    setDetailBusy(true);
    try {
      const view = await commands.gmCharacterDetail(account, id);
      if (my !== detailSeq.current) return;
      setDetail(view);
      setDetailError("");
      // 重读一遍就是换了一批修订号，手上那个未保存的编辑器要作废。
      setEdit(null);
      setDoomed(null);
    } catch (err) {
      if (my !== detailSeq.current) return;
      setDetail(null);
      setDetailError(errText(err));
    } finally {
      if (my === detailSeq.current) setDetailBusy(false);
    }
  }, []);

  // 养成四档的可选清单：读一次就知道当前等级/经验/分支/觉醒，以及每个分支有几段觉醒授予表。
  // 读失败（产物没部署、存档身份不符）时这一档整块只给原因，不给按钮。
  const loadGrow = useCallback(async (account: number, id: number) => {
    const my = ++growSeq.current;
    setGrowBusy(true);
    try {
      const view = await commands.gmGrowthOptions(account, id);
      if (my !== growSeq.current) return;
      setGrow(view);
      setGrowError("");
      setLevelTarget(Math.min(view.level_cap, view.level + 1));
      // 未转职时把分支下拉停在产物里那条真实可用的第一支，避免下拉与状态各说一套。
      setBranch(view.advancement || view.branches[0]?.branch || 0);
      setStage(view.awakening);
    } catch (err) {
      if (my !== growSeq.current) return;
      setGrow(null);
      setGrowError(errText(err));
    } finally {
      if (my === growSeq.current) setGrowBusy(false);
    }
  }, []);

  // 任务列表：判据全在离线产物 gm-quests.json + 库里那几行，读一次拿到 revision 与等级上限。
  const loadQuests = useCallback(async (account: number, id: number) => {
    const my = ++questSeq.current;
    setQuestBusy(true);
    try {
      const view = await commands.gmQuests(account, id);
      if (my !== questSeq.current) return;
      setQuests(view);
      setQuestError("");
      setMainlineLevel(view.level);
      setQuestPicked([]);
      setQuestTrace(null);
      setQuestOpen(null);
      setQuestPlan(null);
    } catch (err) {
      if (my !== questSeq.current) return;
      setQuests(null);
      setQuestError(errText(err));
    } finally {
      if (my === questSeq.current) setQuestBusy(false);
    }
  }, []);

  // 仓库跟账号，不跟角色：换角色不用重读，换账号才读。
  const materialSeq = useRef(0);
  const loadMaterials = useCallback(async (account: number) => {
    const my = ++materialSeq.current;
    setMaterialBusy(true);
    try {
      const list = await commands.gmMaterials(account);
      if (my !== materialSeq.current) return;
      setMaterials(list);
      setMaterialDraft({});
    } catch {
      if (my !== materialSeq.current) return;
      setMaterials(null);
    } finally {
      if (my === materialSeq.current) setMaterialBusy(false);
    }
  }, []);

  /** 一次管理操作：写完之后重读这一眼存档，界面上不留旧修订号。 */
  const manage = async (
    input: Omit<GmManageInput, "id" | "account" | "character" | "reason">,
    account: number,
    characterId: number,
    after?: () => void,
  ) => {
    if (!inv?.editable) return;
    setManageBusy(true);
    setManageOutcome(null);
    try {
      const out = await commands.gmManage({
        ...input,
        id: newGrantId(),
        account,
        character: characterId,
        reason,
      });
      setManageOutcome({ text: `${out.message}${out.note}`, ok: true });
      onNotice(out.message, "ok");
      after?.();
      void loadTarget();
      void loadDetail(account, characterId);
      // 记录卡已经翻开过才顺手重读：没点开过就不替用户多读一趟存档。
      if (records) void loadRecords();
    } catch (err) {
      const text = errText(err);
      setManageOutcome({ text, ok: false });
      onNotice(text, "err");
      // 乐观锁被拒说明存档变了，顺手重读，让用户拿新的修订号再来。
      void loadDetail(account, characterId);
    } finally {
      setManageBusy(false);
    }
  };

  const saveBagEdit = () => {
    if (!edit || !character) return;
    const row = edit.row;
    void manage(
      {
        action: edit.mode === "durability" ? "equipment.durability" : "inventory.amount",
        template: row.template,
        amount: edit.value,
        // 装备那一行的数量不是界面看的字段，所以耐久档不吃 expected_amount，只带当前耐久。
        expected_amount: edit.mode === "durability" ? null : row.count,
        expected_durability: edit.mode === "durability" ? row.durability : null,
        revision: row.revision,
        collection: row.collection,
        space: row.space,
        slot: row.slot,
        group: row.group,
      },
      character.account_id,
      character.id,
      () => setEdit(null),
    );
  };

  const deleteBagRow = () => {
    if (!doomed || !character) return;
    const row = doomed;
    void manage(
      {
        action: "inventory.delete",
        template: row.template,
        amount: 0,
        expected_amount: row.count,
        revision: row.revision,
        collection: row.collection,
        space: row.space,
        slot: row.slot,
        group: row.group,
      },
      character.account_id,
      character.id,
      () => setDoomed(null),
    );
  };

  const saveMaterial = (row: GmMaterialRow, value: number) => {
    if (!character) return;
    const account = character.account_id;
    void manage(
      {
        action: "materials.set",
        template: row.template,
        amount: value,
        expected_amount: row.amount,
      },
      account,
      character.id,
      () => void loadMaterials(account),
    );
  };

  const addPoints = (action: "sp" | "tp") => {
    if (!character) return;
    void manage(
      { action, amount: points[action], expected_amount: null },
      character.account_id,
      character.id,
    );
  };

  /** 满值那一档不读输入框：求和在服务端产物里做，界面只负责点名当前等级。 */
  const recalculateSp = () => {
    if (!character) return;
    void manage(
      { action: "sp.recalculate", expected_amount: null },
      character.account_id,
      character.id,
    );
  };

  const unlockEquipment = () => {
    if (!character || unlock === 0) return;
    void manage(
      { action: "equipment.unlock", amount: unlock, expected_amount: null },
      character.account_id,
      character.id,
      () => setUnlock(0),
    );
  };

  /** 换分支时把阶段拉回该分支的合法档位：别的分支不可能已有觉醒进度。 */
  const selectBranch = (next: number, options: GmGrowOptions | null) => {
    setBranch(next);
    if (!options) return;
    setStage(next === options.advancement ? options.awakening : 0);
  };

  // 等级那一档要的目标：与 Go 的门一致（≥2、不超上限、必须高于当前）。
  const levelOk =
    !!grow && levelTarget >= 2 && levelTarget <= grow.level_cap && levelTarget > grow.level;

  // 阶段下拉只给产物里真有觉醒授予表的那几档（0 恒在首位，代表「停在未觉醒」）。
  const stageList = grow?.branches.find((b) => b.branch === branch)?.stages ?? [];

  /** 养成四档写完都要重读一次可选清单：等级、分支、觉醒都变了，下拉不能留着旧值。 */
  const reloadGrow = () => {
    if (character) void loadGrow(character.account_id, character.id);
  };

  const applyLevel = () => {
    if (!character || !levelOk) return;
    void manage(
      { action: "level", amount: levelTarget, expected_amount: null },
      character.account_id,
      character.id,
      reloadGrow,
    );
  };

  const applyGrow = (action: "advancement" | "awakening" | "profession") => {
    if (!character || !grow) return;
    void manage(
      {
        action,
        amount: action === "awakening" ? stage : branch,
        stage: action === "profession" ? stage : null,
        expected_amount: null,
      },
      character.account_id,
      character.id,
      reloadGrow,
    );
  };

  const reloadQuests = () => {
    if (!character) return;
    void loadQuests(character.account_id, character.id);
  };

  /** 逐条改写：带着列表那一眼的 revision 提交，库里变了整笔拒。 */
  const runQuestAction = (action: "quests.ready" | "quests.complete") => {
    if (!character || !quests || questPicked.length === 0) return;
    void manage(
      {
        action,
        quests: [...questPicked].sort((a, b) => a - b),
        revision: quests.revision,
        expected_amount: null,
      },
      character.account_id,
      character.id,
      reloadQuests,
    );
  };

  /** 放弃是删行：弹窗确认之后才发这一笔。 */
  const abandonQuests = () => {
    if (!character || !quests || !questDoomed?.length) return;
    const ids = [...questDoomed].sort((a, b) => a - b);
    setQuestDoomed(null);
    void manage(
      { action: "quests.abandon", quests: ids, revision: quests.revision, expected_amount: null },
      character.account_id,
      character.id,
      reloadQuests,
    );
  };

  const askPlan = async (
    kind: "mainline" | "prerequisites",
    target: number,
    fetch: () => Promise<GmQuestPlan>,
  ) => {
    const my = ++questPlanSeq.current;
    setQuestPlanBusy(true);
    setQuestPlan(null);
    try {
      const plan = await fetch();
      if (my !== questPlanSeq.current) return;
      setQuestPlan({ kind, target, plan });
    } catch (err) {
      if (my !== questPlanSeq.current) return;
      setQuestError(errText(err));
    } finally {
      if (my === questPlanSeq.current) setQuestPlanBusy(false);
    }
  };

  const previewMainline = () => {
    if (!character) return;
    const account = character.account_id;
    const id = character.id;
    void askPlan("mainline", 0, () => commands.gmQuestPreviewMainline(account, id, mainlineLevel));
  };

  const previewTrace = (target: number) => {
    if (!character) return;
    const account = character.account_id;
    const id = character.id;
    void askPlan("prerequisites", target, () => commands.gmQuestPreviewPrerequisites(account, id, target));
  };

  /** 执行要把预览那份签名原样带回：任务在两次点击之间变了就整笔拒。 */
  const runQuestPlan = () => {
    if (!character || !questPlan) return;
    const { kind, target, plan } = questPlan;
    void manage(
      kind === "mainline"
        ? { action: "quests.mainline", level: mainlineLevel, plan: plan.signature, expected_amount: null }
        : {
            action: "quests.prerequisites",
            quests: [target],
            plan: plan.signature,
            expected_amount: null,
          },
      character.account_id,
      character.id,
      () => {
        setQuestPlan(null);
        setQuestTrace(null);
        reloadQuests();
      },
    );
  };

  useEffect(() => {
    if (!character) {
      detailSeq.current += 1;
      growSeq.current += 1;
      questSeq.current += 1;
      questPlanSeq.current += 1;
      setDetail(null);
      setDetailError("");
      setEdit(null);
      setDoomed(null);
      setManageOutcome(null);
      setMaterials(null);
      setMaterialDraft({});
      setUnlock(0);
      setGrow(null);
      setGrowError("");
      setQuests(null);
      setQuestError("");
      setQuestPicked([]);
      setQuestTrace(null);
      setQuestOpen(null);
      setQuestPlan(null);
      setQuestDoomed(null);
      return;
    }
    setEdit(null);
    setDoomed(null);
    setManageOutcome(null);
    setAttrsOpen(false);
    setUnlock(0);
    setGrow(null);
    setGrowError("");
    setQuestPlan(null);
    setQuestDoomed(null);
    void loadDetail(character.account_id, character.id);
    void loadMaterials(character.account_id);
    void loadGrow(character.account_id, character.id);
    void loadQuests(character.account_id, character.id);
  }, [character, loadDetail, loadMaterials, loadGrow, loadQuests]);

  const attrRows = attributeRows(detail?.summary.attributes);
  const coreRows = attrRows.filter((r) => r.core);
  const otherRows = attrRows.filter((r) => !r.core);
  const untranslatedRows = attrRows.filter((r) => !r.translated);
  const visibleAttrRows = attrsOpen ? attrRows : coreRows;

  // 租约在按钮层就拒（与 Go 的 inventoryEditStatus 同口径），不等写到一半再报错。
  const canManage = !!inv?.editable && !!character && !manageBusy;

  // 任务这一侧多一道门：存档身份与目录不匹配时（usable=false）整档不给按钮。
  const canQuest = canManage && (quests?.usable ?? false);

  const questRows = (quests?.quests ?? []).filter((row) => {
    if (questFilter === "进行中") return row.status !== "completed";
    if (questFilter === "主线") return row.grade.includes("[epic]");
    return true;
  });

  // 只发金币/点券时不需要这件物品能进背包；一旦数量 >0，就必须是产物里能分类的物品。
  const wantsItem = amount > 0;
  const canGrant =
    !!inv?.editable &&
    !!character &&
    !granting &&
    (!wantsItem || !!selected?.grantable) &&
    (wantsItem || gold > 0 || cera > 0);

  // 邮件这一支的发件人永远是「GM」，正文留给界面填；能装进附件的只有金币、可堆叠材料与时装。
  // 时装在服务端按件拆成独立附件（一件一个实例），所以每批 9 件是 Go `grantGMItem` 的那道门。
  const mailable = !!selected && (selected.kind === "stackable" || selected.avatar);
  const mailBlocked = !inv?.editable || !character
    ? "先在「发放目标」选角色；服务端占用时整条写侧都不给按。"
    : gold === 0 && !wantsItem
      ? "金币与物品数量至少给一样，空邮件没有可发的东西。"
      : wantsItem && !mailable
        ? "邮件只带金币、可堆叠材料与时装；带打造参数的装备实例还没做。"
        : wantsItem && selected?.avatar && amount > 9
          ? "时装按件拆附件，每批最多 9 件。"
          : wantsItem && selected && selected.kind === "stackable" && selected.stack_limit > 0 && amount > selected.stack_limit
            ? `这件的堆叠上限是 ${selected.stack_limit}，邮件一次带不下一批超上限的数。`
            : "";
  const canMail = mailBlocked === "" && !manageBusy;

  const grant = async () => {
    if (!character || (wantsItem && !selected)) return;
    setGranting(true);
    setOutcome(null);
    try {
      const out = await commands.gmGrant({
        id: newGrantId(),
        account: character.account_id,
        character: character.id,
        gold,
        cera,
        items: wantsItem && selected ? [{ template: selected.id, amount }] : [],
        reason,
      });
      setOutcome({ text: receiptText(out), ok: true });
      onNotice(out.message, "ok");
      void loadTarget();
      if (records) void loadRecords();
    } catch (err) {
      const text = errText(err);
      setOutcome({ text, ok: false });
      onNotice(text, "err");
    } finally {
      setGranting(false);
    }
  };

  /** 邮件发放：还是上面那三样，只是装进一封系统邮件由玩家自己领取，不直接落背包。 */
  const sendMail = () => {
    if (!character || !canMail) return;
    void manage(
      {
        action: "mail.send",
        gold,
        template: wantsItem && selected ? selected.id : 0,
        amount: wantsItem ? amount : 0,
        body: mailBody,
      },
      character.account_id,
      character.id,
    );
  };

  const s = status?.catalog ?? null;

  // 九张卡的正文：标题栏、抓手、收起与分割都由停靠层给，这里只留内容本身。
  // 标题读 GM_TITLES（摆放栏的胶囊与幽灵块用的是同一份），所以正文里不再各写一个 <h2>。
  const CARDS: Record<GmKey, ReactNode> = {
    search: (
      <PanelFrame bodyClass="gm-card-body">
        <div className="row">
          <span className="chip" data-tone={ready ? "ok" : status?.stage === "失败" ? "err" : "busy"}>
            {status?.stage ?? "…"}
          </span>
          {s ? <span className="chip">物品库 {s.count.toLocaleString()} 条</span> : null}
          {s ? <span className="chip">装备 {s.equipment.toLocaleString()} · 堆叠 {s.stackable.toLocaleString()}</span> : null}
          <span className="spacer" />
          <button className="btn" data-tone="ghost" onClick={() => void reload()}>
            重新加载
          </button>
        </div>
        <div className="row">
          <span
            className="chip"
            data-tone={audit ? (audit.consistent ? "ok" : "err") : "busy"}
            title="三件 gmbuild 产物（物品库 / 成长表 / 任务目录）是否同一次构建"
          >
            产物同源 {audit ? (audit.consistent ? "一致" : "不一致") : "…"}
          </span>
          {audit?.pvf ? (
            <span className="chip" data-tone={audit.pvf.size_matches ? "ok" : "err"}>
              PVF 快照 {audit.pvf.size_matches ? "对得上" : "换过"}
            </span>
          ) : null}
          <span className="spacer" />
          <button className="btn" data-tone="ghost" disabled={auditBusy || rebuildBusy} onClick={() => void loadAudit()}>
            {auditBusy ? "校验中…" : "校验"}
          </button>
          <button className="btn" data-tone="ghost" disabled={rebuildBusy} onClick={() => void pickPvf()}>
            选择 PVF…
          </button>
          <button className="btn" disabled={rebuildBusy} onClick={() => void rebuild()}>
            {rebuildBusy ? "重建中…" : "一键重建"}
          </button>
        </div>
        {(audit?.issues ?? []).map((issue, i) => (
          <p key={`${issue.tone}-${i}`} className="hint" data-tone={issue.tone === "ok" ? undefined : issue.tone}>
            {issue.text}
          </p>
        ))}
        {audit?.pvf ? (
          <p className="hint">
            PVF：{audit.pvf.path}（{bytes(audit.pvf.size)}，改于 {audit.pvf.modified}）
          </p>
        ) : null}
        {pvf ? <p className="hint">重建将用：{pvf}</p> : null}
        {rebuildBusy ? (
          <Fragment>
            <div className="mem-track" title={progress?.text ?? "重建刚起步，第一格进度还没到"}>
              <i data-tone="busy" style={{ width: `${progress?.percent ?? 0}%` }} />
            </div>
            <p className="hint">
              {progress
                ? `第 ${progress.step_index}/${progress.step_total} 步 · ${progress.percent}% · ${progress.text}`
                : "重建准备中…"}
            </p>
          </Fragment>
        ) : null}
        {rebuildNote ? (
          <p className="hint" data-tone={rebuildNote.ok ? undefined : "err"}>
            {rebuildNote.text}
          </p>
        ) : null}
        {/* 逐步明细：哪一步跑了多久、失败那步的尾巴，只有跑过一次之后才有得看。 */}
        {steps
          ? steps.map((s) => (
              <p key={s.step} className="hint" data-tone={s.ok ? undefined : "err"}>
                {s.step} · {s.seconds.toFixed(1)}s{s.ok ? "" : ` · ${s.detail}`}
              </p>
            ))
          : null}
        {status && status.stage !== "就绪" && status.error ? (
          <p className="hint" data-tone="err">
            {status.error}
          </p>
        ) : null}
        <p className="hint">
          这里读的是离线构建器产出的五份 JSON（物品库 / 名字表 / 部位表 / 职业表 / 品级表），启动器不碰 PVF。
          换过 PVF 后旧产物不会自己失效：查询照开，发放按旧分类、养成与任务会被拒；
          「一键重建」七步全在这里跑（含装备目录全量枚举、背包规则换代际戳），先写暂存、校验同源再替换，
          旧的备份在产物目录旁边。经验规则那份没有来源戳，是人工口径，重建不动它。
        </p>
        {s ? (
          <p className="hint">
            {s.dir}
            <br />
            物品库指纹 {s.items_source.slice(0, 12)}… · {tableLine("部位", s.slots)} · {tableLine("职业", s.jobs)} ·{" "}
            {tableLine("品级", s.rarity)} · {tableLine("名字", s.names)}
          </p>
        ) : null}

        <div className="field">
          <label>关键字（子串、前后缀通配 `*巨剑*`，或纯数字按 ID 前缀）</label>
          <input
            type="text"
            value={q}
            disabled={!ready}
            placeholder="名字 / 英文名 / 物品 ID"
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void runSearch(0);
            }}
          />
        </div>
        <div className="row">
          <select value={type} disabled={!ready} onChange={(e) => setType(e.target.value)}>
            <option value="">全部分类</option>
            {(facets?.types ?? []).map((f) => (
              <option key={f.key} value={f.key}>
                {f.label}（{f.count.toLocaleString()}）
              </option>
            ))}
          </select>
          <select value={slot} disabled={!ready} onChange={(e) => setSlot(e.target.value)}>
            <option value="">全部部位</option>
            {(facets?.slots ?? [])
              .filter((f) => f.count > 0)
              .map((f) => (
                <option key={f.key} value={f.key}>
                  {f.label}（{f.count.toLocaleString()}）
                </option>
              ))}
          </select>
          <select value={band} disabled={!ready} onChange={(e) => setBand(e.target.value)}>
            <option value="">全部等级</option>
            {(facets?.level_bands ?? []).map((f) => (
              <option key={f.key} value={f.key}>
                {f.label}（{f.count.toLocaleString()}）
              </option>
            ))}
          </select>
          <select value={rarity} disabled={!ready} onChange={(e) => setRarity(e.target.value)}>
            <option value="">全部品级</option>
            {(facets?.rarities ?? [])
              .filter((f) => f.count > 0)
              .map((f) => (
                <option key={f.key} value={f.key}>
                  {f.label}（{f.count.toLocaleString()}）
                </option>
              ))}
          </select>
          <button className="btn" disabled={!ready} onClick={() => void runSearch(0)}>
            查询
          </button>
        </div>
        {note ? (
          <p className="hint" data-tone="err">
            {note}
          </p>
        ) : null}
        <p className="hint">
          什么都不筛时沿用旧工具口径：只列等级 ≤10、强化档 ≤3 的那一批。给等级条件时，未标注等级的物品不参与。
        </p>
      </PanelFrame>
    ),
    results: (
      <PanelFrame bodyClass="gm-card-body">
        {rows.length === 0 && !note ? (
          <p className="hint">{ready ? "点「查询」或按回车。" : "物品库就绪后即可查询。"}</p>
        ) : (
          <table className="af-live">
            <thead>
              <tr>
                <th>ID</th>
                <th>名称</th>
                <th>类别</th>
                <th>品级</th>
                <th>等级</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={row.id} onClick={() => select(row)}>
                  <td>{row.id}</td>
                  <td>
                    {row.name}
                    {selected?.id === row.id ? <strong> ←</strong> : null}
                  </td>
                  <td>{row.kind === "equipment" || row.kind === "avatar" ? row.slot : row.type_label}</td>
                  <td>{row.rarity_label}</td>
                  <td>{row.level > 0 ? row.level : "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        <div className="row">
          <button
            className="btn"
            data-tone="ghost"
            disabled={!ready || offset === 0}
            onClick={() => void runSearch(Math.max(0, offset - PAGE))}
          >
            上一页
          </button>
          <span className="chip">
            {total === 0 ? "—" : `${offset + 1}–${Math.min(offset + PAGE, total).toLocaleString()}`} / {total.toLocaleString()}
          </span>
          <button
            className="btn"
            data-tone="ghost"
            disabled={!ready || offset + PAGE >= total}
            onClick={() => void runSearch(offset + PAGE)}
          >
            下一页
          </button>
        </div>
      </PanelFrame>
    ),
    target: (
      <PanelFrame bodyClass="gm-card-body">
        <div className="row">
          <span
            className="chip"
            data-tone={
              inv
                ? inv.editable
                  ? "ok"
                  : inv.server_running || inv.management_busy
                    ? "busy"
                    : "err"
                : undefined
            }
          >
            {inv
              ? inv.editable
                ? "存档可写"
                : inv.server_running
                  ? "服务端占用"
                  : inv.management_busy
                    ? "管理操作中"
                    : "存档未就绪"
              : "存档状态…"}
          </span>
          <span className="spacer" />
          <button className="btn" data-tone="ghost" onClick={() => void loadTarget()}>
            刷新
          </button>
        </div>
        <div className="field">
          <label>角色（金币与物品写进这个背包，点券记在它所属账号上）</label>
          <select value={target} onChange={(e) => setTarget(Number(e.target.value))}>
            <option value={0}>请选择角色</option>
            {chars.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}（账号 {c.username}）
              </option>
            ))}
          </select>
        </div>
        {chars.length === 0 ? (
          <p className="hint">没读到角色：先启动一次游戏建号，服务端会自动建 SQLite 存档。</p>
        ) : null}
        {inv && !inv.editable && inv.message ? (
          <p className="hint" data-tone="err">
            {inv.message}
          </p>
        ) : null}
        {inv?.db_path ? <p className="hint">{inv.db_path}</p> : null}
        <p className="hint">
          写存档走服务端那套事务：一个流水号只入账一次，重复提交回原回执；角色不在这个账号名下整笔回滚。
          邮件发放用的是同一条管道（在「物品详情」那张卡上按「发放为邮件」）；带附加属性的装备实例发放不在这一批。
        </p>
      </PanelFrame>
    ),
    detail: (
      <PanelFrame bodyClass="gm-card-body">
        {selected ? (
          <>
            <div className="row">
              <span className="chip">{selected.id}</span>
              <span className="chip">{KIND_LABEL[selected.kind] ?? selected.kind}</span>
              <span className="chip">{selected.rarity_label}</span>
            </div>
            <p className="hint">
              <strong>{selected.name}</strong>
              <br />
              {selected.name_en}
            </p>
            <p className="hint">
              {selected.kind === "stackable"
                ? `类型：${selected.type_label}${selected.type_text ? `（${selected.type_text}）` : ""}`
                : `部位：${selected.slot}（${selected.group}）${selected.type_text ? ` · ${selected.type_text}` : ""}`}
              <br />
              {selected.level > 0 ? `最低等级 ${selected.level}` : "等级未标注"}
              {selected.stack_limit > 0 ? ` · 堆叠上限 ${selected.stack_limit}` : ""}
              {selected.durability > 0 ? ` · 耐久 ${selected.durability}` : ""}
              <br />
              {selected.jobs
                ? `职业限制：${selected.jobs.join("、")}`
                : "职业：通用（[all]）"}
            </p>
            <div className="row">
              <label className="num">
                <span>数量</span>
                <input
                  type="number"
                  min={0}
                  max={9999}
                  value={amount}
                  disabled={!selected.grantable}
                  onChange={(e) => setAmount(Math.max(0, Number(e.target.value) || 0))}
                />
                <em>件</em>
              </label>
              <label className="num">
                <span>金币</span>
                <input
                  type="number"
                  min={0}
                  max={800000000}
                  value={gold}
                  onChange={(e) => setGold(Math.max(0, Number(e.target.value) || 0))}
                />
                <em>金</em>
              </label>
              <label className="num">
                <span>点券</span>
                <input
                  type="number"
                  min={0}
                  value={cera}
                  onChange={(e) => setCera(Math.max(0, Number(e.target.value) || 0))}
                />
                <em>券</em>
              </label>
            </div>
            <div className="field">
              <label>原因（写进发放审计，留空记「启动器发放」）</label>
              <input
                type="text"
                value={reason}
                maxLength={60}
                placeholder="启动器发放"
                onChange={(e) => setReason(e.target.value)}
              />
            </div>
            <div className="field">
              <label>邮件正文（只有「发放为邮件」用；上限 512 字节，中文按 3 字节算）</label>
              <input
                type="text"
                value={mailBody}
                placeholder="维护补偿"
                onChange={(e) => setMailBody(e.target.value)}
              />
            </div>
            <div className="row">
              <button className="btn" data-tone="accent" disabled={!canGrant} onClick={() => void grant()}>
                {granting ? "发放中…" : "发放到存档"}
              </button>
              <button
                className="btn"
                data-tone="accent"
                disabled={!canMail}
                onClick={() => void sendMail()}
                title={mailBlocked}
              >
                {manageBusy ? "写入中…" : "发放为邮件"}
              </button>
              <span className="spacer" />
            </div>
            {outcome ? (
              <p className="hint" data-tone={outcome.ok ? undefined : "err"}>
                {outcome.text}
              </p>
            ) : null}
            {manageOutcome ? (
              <p className="hint" data-tone={manageOutcome.ok ? undefined : "err"}>
                {manageOutcome.text}
              </p>
            ) : null}
            {!mailBlocked ? null : <p className="hint">{mailBlocked}</p>}
            <p className="hint">
              「发放为邮件」把上面这两样装进一封署名 GM 的系统邮件，附件由玩家自己领取，15 天有效；
              直接进背包那条不看正文，也不占收件箱。邮件到的是「角色」，同一账号的别的角色看不见这一封。
            </p>
            {!selected.grantable ? (
              <p className="hint">这件物品的类型在产物里落不进任何背包分类，不能发放。</p>
            ) : null}
          </>
        ) : (
          <p className="hint">点结果里的一行看详情。</p>
        )}
      </PanelFrame>
    ),
    bag: (
      <PanelFrame bodyClass="gm-card-body">
        <div className="row">
          <span className="chip" data-tone={detail ? "ok" : detailError ? "err" : "busy"}>
            {detailBusy
              ? "读取中…"
              : detail
                ? `${detail.character.name} Lv.${detail.character.level}`
                : character
                  ? "未读到"
                  : "未选角色"}
          </span>
          <span className="spacer" />
          <button
            className="btn"
            data-tone="ghost"
            disabled={!character || detailBusy}
            onClick={() => void loadDetail(character?.account_id ?? 0, character?.id ?? 0)}
          >
            重读背包
          </button>
        </div>
        {!character ? (
          <p className="hint">在「发放目标」里选角色，这里就只读展示它的数值与背包，不写一个字节。</p>
        ) : null}
        {detailError ? (
          <p className="hint" data-tone="err">
            {detailError}
          </p>
        ) : null}
        {detail ? (
          <>
            <p className="hint">
              <strong>{detail.character.class_name}</strong>
              {detail.character.advancement > 0 ? ` · 转职档 ${detail.character.advancement}` : ""}
              {detail.character.awakening > 0 ? ` · 觉醒档 ${detail.character.awakening}` : ""}
              <br />
              角色 {detail.character.id}（账号 {detail.character.account_id} · 网络 ID {detail.character.wire_id} · 职业码 {detail.character.profession}）· 创建于 {detail.character.created_at}
              <br />
              金币 {detail.character.gold.toLocaleString()} · 点券 {detail.cera.toLocaleString()} · 经验 {detail.character.experience.toLocaleString()}
              <br />
              技能点 {detail.summary.skill_points[0]} / {detail.summary.skill_points[1]} · 技巧点 {detail.summary.technique_points[0]} / {detail.summary.technique_points[1]}
            </p>
            <div className="field">
              <div className="row">
                <span className="chip">属性 {attrRows.length} 项</span>
                {untranslatedRows.length > 0 ? (
                  <span className="chip" data-tone="busy">
                    未汉化 {untranslatedRows.length} 项
                  </span>
                ) : null}
                <span className="spacer" />
                {otherRows.length > 0 ? (
                  <button className="btn" data-tone="ghost" onClick={() => setAttrsOpen((v) => !v)}>
                    {attrsOpen ? "收起" : `展开全部属性（含其余 ${otherRows.length} 项）`}
                  </button>
                ) : null}
              </div>
              {visibleAttrRows.length > 0 ? (
                <table className="af-live">
                  <thead>
                    <tr>
                      <th>属性</th>
                      <th>存档原值</th>
                    </tr>
                  </thead>
                  <tbody>
                    {visibleAttrRows.map((r) => (
                      <tr key={r.key} title={r.key}>
                        <td>
                          {r.label}
                          {!r.translated ? <em>（未汉化）</em> : null}
                        </td>
                        <td>{r.value.toLocaleString()}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              ) : (
                <p className="hint">
                  {attrRows.length === 0
                    ? "这一档存档里没有属性表。"
                    : "这一档的属性键都不在精选组里，点「展开全部属性」看。"}
                </p>
              )}
              <p className="hint">
                中文名逐条对着客户端文本表的用词，只出现在角色存档里真有的键上；对不上的键原样列出并标
                「未汉化」，不猜名字，也不给改写。数值是存档原值，不做换算——速度类存档是千分位
                （850 就是面板上的 85%），负重是克（50100 就是 50.1kg）。鼠标停在行上能看到内部键名。
              </p>
            </div>
            <p className="hint">
              背包 {detail.bag.length} 行
              {detail.bag_error ? `：${detail.bag_error}` : ""}
            </p>
            {detail.bag.length > 0 ? (
              <table className="af-live">
                <thead>
                  <tr>
                    <th>容器</th>
                    <th>位置</th>
                    <th>名称</th>
                    <th>数量</th>
                    <th>修订</th>
                    <th>操作</th>
                  </tr>
                </thead>
                <tbody>
                  {detail.bag.map((row, i) => (
                    <tr key={`${row.collection}-${row.space}-${row.slot}-${row.template}-${i}`}>
                      <td>{bagLabel(row)}</td>
                      <td>
                        {row.slot}
                        {row.group > 0 ? `（组 ${row.group}）` : ""}
                      </td>
                      <td>
                        {row.name}
                        <br />
                        <span className="mono">{row.template}</span>
                        {row.kind ? ` · ${KIND_LABEL[row.kind] ?? row.kind}` : ""}
                        {row.rarity_label ? ` · ${row.rarity_label}` : ""}
                        {row.worn ? " · 已穿戴" : ""}
                        {row.durability !== undefined ? ` · 耐久 ${row.durability}` : ""}
                      </td>
                      <td>
                        {row.count.toLocaleString()}
                        {row.stack_limit ? ` / ${row.stack_limit.toLocaleString()}` : ""}
                      </td>
                      <td title={row.revision}>{row.revision.slice(0, 8)}</td>
                      <td>
                        {edit?.row === row ? (
                          <button className="btn" data-tone="ghost" onClick={() => setEdit(null)}>
                            收起
                          </button>
                        ) : (
                          <>
                            {row.can_edit_count ? (
                              <button
                                className="btn"
                                data-tone="ghost"
                                disabled={!canManage}
                                title={canManage ? "" : inv?.message || "现在不能改"}
                                onClick={() => setEdit({ row, value: row.count, mode: "count" })}
                              >
                                改数量
                              </button>
                            ) : null}{" "}
                            {row.durability !== undefined ? (
                              <button
                                className="btn"
                                data-tone="ghost"
                                disabled={!canManage}
                                title={
                                  canManage
                                    ? "上限取装备目录里这件装备的耐久上限；查不到上限会被拒绝"
                                    : inv?.message || "现在不能改"
                                }
                                onClick={() =>
                                  // 外层三元已保证这一键在；闭包里 TS 不留这个前提，取个 0 兜底，
                                  // 真漏了服务端按「缺少原装备快照」拒，不会写坏。
                                  setEdit({ row, value: row.durability ?? 0, mode: "durability" })
                                }
                              >
                                改耐久
                              </button>
                            ) : null}{" "}
                            <button
                              className="btn"
                              data-tone="ghost"
                              disabled={!canManage}
                              title={canManage ? "" : inv?.message || "现在不能改"}
                              onClick={() => setDoomed(row)}
                            >
                              删除
                            </button>
                          </>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            ) : null}
            {edit ? (
              <div className="field">
                <p className="hint">
                  {edit.mode === "count" ? (
                    <>
                      改「{edit.row.name}」（{bagLabel(edit.row)} 位置 {edit.row.slot}）的数量：
                      现在 {edit.row.count.toLocaleString()}
                      {edit.row.stack_limit ? `，上限 ${edit.row.stack_limit.toLocaleString()}` : ""}。
                      只换数量这一个字段，期限与属性原样留着。
                    </>
                  ) : (
                    <>
                      改「{edit.row.name}」（{bagLabel(edit.row)} 位置 {edit.row.slot}）的耐久：
                      现在 {edit.row.durability}。上限取装备目录里这件装备的耐久上限，
                      目录查不到这一件就整笔拒绝，不给写一个游戏里不存在的耐久。
                      只换耐久这一个字段，精炼与别的属性原样留着。
                    </>
                  )}
                </p>
                <div className="row">
                  <label className="num">
                    <span>{edit.mode === "count" ? "数量" : "耐久"}</span>
                    <input
                      type="number"
                      min={1}
                      max={edit.mode === "count" ? edit.row.stack_limit || 9999 : undefined}
                      value={edit.value}
                      disabled={manageBusy}
                      onChange={(e) =>
                        setEdit({
                          row: edit.row,
                          value: Math.max(0, Number(e.target.value) || 0),
                          mode: edit.mode,
                        })
                      }
                    />
                    <em>{edit.mode === "count" ? "件" : "点"}</em>
                  </label>
                  <span className="spacer" />
                  <button
                    className="btn"
                    data-tone="accent"
                    disabled={
                      !canManage ||
                      edit.value === 0 ||
                      edit.value === (edit.mode === "count" ? edit.row.count : edit.row.durability)
                    }
                    onClick={() => void saveBagEdit()}
                  >
                    {manageBusy ? "保存中…" : edit.mode === "count" ? "保存数量" : "保存耐久"}
                  </button>
                  <button className="btn" data-tone="ghost" disabled={manageBusy} onClick={() => setEdit(null)}>
                    取消
                  </button>
                </div>
              </div>
            ) : null}
            {manageOutcome ? (
              <p className="hint" data-tone={manageOutcome.ok ? undefined : "err"}>
                {manageOutcome.text}
              </p>
            ) : null}
            <p className="hint">
              修订号是这一行字段按键名排序后的指纹（与 Go 同算法），编辑时拿它做乐观锁：
              这一行在存档里被服务端改过，修订号就对不上，修改会被拒绝而不是覆盖，并顺手重读给你新的。
              可堆叠数量只有产物标了上限的那些能改；装备两张表（装备栏与已穿戴）给改耐久，
              上限只认装备目录里那一件，属性不提供改写；时装与宠物只给删除。
              {inv && !inv.editable ? `当前不能改：${inv.message}` : ""}
            </p>
          </>
        ) : null}
      </PanelFrame>
    ),
    stats: (
      <PanelFrame bodyClass="gm-card-body">
        <div className="row">
          <span className="chip" data-tone={canManage ? "ok" : "busy"}>
            {canManage ? "可写入" : "现在不能写"}
          </span>
          {detail ? (
            <span className="chip">
              技能点 {detail.summary.skill_points[0]} / {detail.summary.skill_points[1]} · 技巧点{" "}
              {detail.summary.technique_points[0]} / {detail.summary.technique_points[1]}
            </span>
          ) : null}
          {grow ? (
            <span className="chip" title={`成长表构建指纹 ${grow.source_checksum}`}>
              {grow.class_name} · Lv {grow.level}/{grow.level_cap} · 经验{" "}
              {grow.experience.toLocaleString()}
            </span>
          ) : null}
          <span className="spacer" />
          <button
            className="btn"
            data-tone="ghost"
            disabled={!character || growBusy}
            onClick={reloadGrow}
          >
            重读养成
          </button>
          {manageOutcome ? (
            <span className="chip" data-tone={manageOutcome.ok ? "ok" : "err"} title={manageOutcome.text}>
              {manageOutcome.ok ? "上一次已保存" : "上一次被拒"}
            </span>
          ) : null}
        </div>
        {!detail ? (
          <p className="hint">先在「发放目标」里选角色，这张卡才知道要改谁。</p>
        ) : (
          <>
            <div className="row">
              <label className="num">
                <span>技能点</span>
                <input
                  type="number"
                  min={1}
                  max={65535}
                  value={points.sp}
                  disabled={!canManage}
                  onChange={(e) => setPoints((p) => ({ ...p, sp: clampPoint(e.target.value) }))}
                />
                <em>点</em>
              </label>
              <button
                className="btn"
                data-tone="accent"
                disabled={!canManage || points.sp === 0}
                onClick={() => void addPoints("sp")}
              >
                {manageBusy ? "写入中…" : "加技能点"}
              </button>
              <button
                className="btn"
                data-tone="ghost"
                disabled={!canManage}
                onClick={() => void recalculateSp()}
                title="按这一列里的当前等级，把两格 SP 写成服务端本该给的总量"
              >
                按 Lv.{detail.summary.level} 重算为满值
              </button>
              <label className="num">
                <span>技巧点</span>
                <input
                  type="number"
                  min={1}
                  max={65535}
                  value={points.tp}
                  disabled={!canManage}
                  onChange={(e) => setPoints((p) => ({ ...p, tp: clampPoint(e.target.value) }))}
                />
                <em>点</em>
              </label>
              <button
                className="btn"
                data-tone="accent"
                disabled={!canManage || points.tp === 0}
                onClick={() => void addPoints("tp")}
              >
                加技巧点
              </button>
            </div>
            <p className="hint">
              两格账本各加同一份点数（服务端升级时也是这么记的），任何一格加不下 65535 就整笔拒，不会只加半笔。
              「重算为满值」不看上面那个输入框：它按这一行里的当前等级，把两格各写成服务端升到这一级本该累计的技能点
              ——一级不发点，所以 1 级算出来就是 0；已经花掉的也一并回到满值。等级不在成长表范围内（当前上限{" "}
              {grow?.level_cap ?? "未知"}）整笔拒。
            </p>
            <div className="field">
              <label>装备栏解锁（只加不减，已开的保持开着）</label>
              <div className="segmented">
                {SLOT_BITS.map((bit) => (
                  <button
                    key={bit.value}
                    aria-pressed={(detail.summary.equipment_slot_flags & bit.value) !== 0}
                    disabled={!canManage}
                    onClick={() =>
                      setUnlock((prev) => (prev & bit.value) === 0 ? prev | bit.value : prev & ~bit.value)
                    }
                  >
                    {bit.label}
                    {(detail.summary.equipment_slot_flags & bit.value) !== 0 ? "（已开）" : ""}
                  </button>
                ))}
              </div>
              <div className="row">
                <span className="hint">
                  当前标志 0x{detail.summary.equipment_slot_flags.toString(16).padStart(2, "0")}
                </span>
                <span className="spacer" />
                <button
                  className="btn"
                  data-tone="accent"
                  disabled={!canManage || unlock === 0}
                  onClick={() => void unlockEquipment()}
                >
                  {unlock === 0 ? "先勾选栏位" : "开启所选栏位"}
                </button>
                {unlock > 0 ? (
                  <button className="btn" data-tone="ghost" disabled={!canManage} onClick={() => setUnlock(0)}>
                    清空勾选
                  </button>
                ) : null}
              </div>
            </div>
            {growError ? (
              <p className="hint" data-tone="err">
                等级、转职与觉醒这一档用不了：{growError}
              </p>
            ) : !grow ? (
              <p className="hint">{growBusy ? "养成目录读取中…" : "养成目录还没读到。"}</p>
            ) : (
              <>
                <div className="field">
                  <label>等级（把经验一次补到目标等级的阈值，属性和技能点按跨过的每一级一起结）</label>
                  <div className="row">
                    <label className="num">
                      <span>目标等级</span>
                      <input
                        type="number"
                        min={2}
                        max={grow.level_cap}
                        value={levelTarget}
                        disabled={!canManage}
                        onChange={(e) =>
                          setLevelTarget(Math.min(grow.level_cap, Math.max(0, Number(e.target.value) || 0)))
                        }
                      />
                      <em>级</em>
                    </label>
                    <button
                      className="btn"
                      data-tone="accent"
                      disabled={!canManage || !levelOk}
                      onClick={applyLevel}
                    >
                      {manageBusy ? "写入中…" : "升到目标等级"}
                    </button>
                    {grow.level < grow.level_cap ? (
                      <button
                        className="btn"
                        data-tone="ghost"
                        disabled={!canManage}
                        onClick={() => setLevelTarget(grow.level + 1)}
                      >
                        下一级（还差 {grow.next_level_experience.toLocaleString()} 经验）
                      </button>
                    ) : (
                      <span className="chip">已在服务器等级上限 {grow.level_cap}</span>
                    )}
                    {!levelOk && levelTarget > grow.level ? (
                      <span className="hint">这一档要的经验已经超过目标等级的阈值，请先检查存档。</span>
                    ) : null}
                  </div>
                </div>
                <div className="field">
                  <label>转职分支与觉醒（三个按钮各自只动自己那一段，都不往下退）</label>
                  <div className="row">
                    <label className="num">
                      <span>分支</span>
                      <select
                        value={branch}
                        disabled={!canManage || grow.branches.length === 0}
                        onChange={(e) => selectBranch(Number(e.target.value), grow)}
                      >
                        {grow.branches.map((b) => (
                          <option key={b.branch} value={b.branch}>
                            {b.name || `分支 ${b.branch}`}（{b.branch}）
                          </option>
                        ))}
                      </select>
                    </label>
                    <label className="num">
                      <span>觉醒阶段</span>
                      <select
                        value={stage}
                        disabled={!canManage || stageList.length === 0}
                        onChange={(e) => setStage(Number(e.target.value))}
                      >
                        {stageList.map((s) => (
                          <option key={s} value={s}>
                            {AWAKENING_LABEL[s] ?? `阶段 ${s}`}
                            {s > 0 ? `（需 ${[0, 50, 75, 100][s]} 级）` : ""}
                          </option>
                        ))}
                      </select>
                    </label>
                  </div>
                  <div className="row">
                    <button
                      className="btn"
                      data-tone="accent"
                      disabled={!canManage || branch === 0 || grow.branches.length === 0}
                      onClick={() => void applyGrow("advancement")}
                    >
                      只换分支
                    </button>
                    <button
                      className="btn"
                      data-tone="accent"
                      disabled={!canManage || stage === 0 || grow.advancement === 0 || branch !== grow.advancement}
                      onClick={() => void applyGrow("awakening")}
                    >
                      只推进觉醒
                    </button>
                    <button
                      className="btn"
                      data-tone="accent"
                      disabled={!canManage || branch === 0 || grow.branches.length === 0}
                      onClick={() => void applyGrow("profession")}
                    >
                      分支加觉醒一步到位
                    </button>
                  </div>
                </div>
                <p className="hint">
                  这三条的口径与 Go 一致：换分支会把觉醒记回未觉醒，推进觉醒要一级一级走，
                  一步到位那条先落分支、再把跨过的每一段补齐。等级、经验、属性、技能点、已学技能
                  都是按离线那份 gm-growth.json 算出来的，来源指纹与存档不符时整笔拒在写库之前。
                  第三次觉醒还会把技巧点第一格记 5 并铺满变异槽。
                  {grow.advancement === 0
                    ? "这个角色还没转职，「只推进觉醒」用不了，先用「只换分支」或一步到位。"
                    : ""}
                </p>
              </>
            )}
          </>
        )}
      </PanelFrame>
    ),
    quests: (
      <PanelFrame bodyClass="gm-card-body">
        <div className="row">
          <span className="chip" data-tone={canQuest ? "ok" : "busy"}>
            {canQuest ? "可写入" : "现在不能写"}
          </span>
          {quests ? (
            <span className="chip">
              进行中 {quests.quests.filter((r) => r.status !== "completed").length} / 共{" "}
              {quests.quests.length} 条 · 角色 Lv {quests.level}
            </span>
          ) : null}
          {quests ? (
            <span
              className="chip"
              title={`任务目录构建指纹 ${quests.source_checksum}`}
            >
              目录指纹 {quests.source_checksum.slice(0, 12)}…
            </span>
          ) : null}
          <span className="spacer" />
          <button
            className="btn"
            data-tone="ghost"
            disabled={!character || questBusy}
            onClick={reloadQuests}
          >
            重读任务
          </button>
          {manageOutcome ? (
            <span className="chip" data-tone={manageOutcome.ok ? "ok" : "err"} title={manageOutcome.text}>
              {manageOutcome.ok ? "上一次已保存" : "上一次被拒"}
            </span>
          ) : null}
        </div>
        <p className="hint">
          判据全部来自离线构建的任务目录（前置、互斥、答案分支、进度模型都折叠在产物里），启动器不读 PVF。
          一键完成只补完成记录：不发任务奖励，也不伪造奥德赛通关、毕业或剧情已读标记。
        </p>
        {!character ? (
          <p className="hint">先在「发放目标」里选角色，这张卡才知道要处理谁的任务。</p>
        ) : quests && !quests.usable ? (
          <p className="hint" data-tone="err">
            {quests.unusable_reason}：任务这一侧只认当前存档契约版本，请先确认目录与存档是同一次构建。
          </p>
        ) : questError ? (
          <p className="hint" data-tone="err">{questError}</p>
        ) : quests ? (
          <>
            <div className="field">
              <label>一键完成主线（目标等级：只把等级要求不超过它的任务算进范围）</label>
              <div className="row">
                <label className="num">
                  <span>目标等级</span>
                  <input
                    type="number"
                    min={1}
                    max={Math.max(1, quests.level)}
                    value={mainlineLevel}
                    disabled={!canQuest}
                    onChange={(e) =>
                      setMainlineLevel(Math.min(quests.level, Math.max(0, Number(e.target.value) || 0)))
                    }
                  />
                  <em>级</em>
                </label>
                <button
                  className="btn"
                  data-tone="accent"
                  disabled={!canQuest || mainlineLevel === 0 || questPlanBusy}
                  onClick={previewMainline}
                >
                  {questPlanBusy ? "预览中…" : "预览主线范围"}
                </button>
                <span className="hint">
                  上限取角色当前等级；写低了会漏掉高等级那一段，预览会如实报出被挡住的条数。
                </span>
              </div>
            </div>

            {questPlan ? (
              <div className="field">
                <div className="row">
                  <span className="chip" data-tone="busy">
                    {questPlan.kind === "mainline"
                      ? `主线 ${questPlan.plan.quests.length} 条待补`
                      : `前置 ${questPlan.plan.quests.length} 条待补`}
                  </span>
                  {questPlan.plan.blocked.length ? (
                    <span className="chip">被挡住 {questPlan.plan.blocked.length} 条</span>
                  ) : null}
                  {questPlan.plan.answer_picks ? (
                    <span className="chip">{Object.keys(questPlan.plan.answer_picks).length} 处答案分支</span>
                  ) : null}
                  <span className="spacer" />
                  <button
                    className="btn"
                    data-tone="accent"
                    disabled={!canQuest || questPlan.plan.quests.length === 0 || manageBusy}
                    onClick={runQuestPlan}
                  >
                    {manageBusy ? "写入中…" : "按预览执行"}
                  </button>
                  <button className="btn" data-tone="ghost" disabled={manageBusy} onClick={() => setQuestPlan(null)}>
                    丢掉预览
                  </button>
                </div>
                <p className="hint" title={questPlan.plan.signature}>
                  签名 {questPlan.plan.signature.slice(0, 12)}…：执行时库里必须还是这一批任务，
                  中途变了（自己玩过去了、或换了角色）就整笔拒，回去重新预览。
                </p>
                {questPlan.plan.answer_picks ? (
                  <p className="hint mono-wrap">
                    {Object.entries(questPlan.plan.answer_picks)
                      .map(([question, pick]) => `问答 ${question} 取第 ${Number(pick) + 1} 项`)
                      .join("、")}
                  </p>
                ) : null}
                {questPlan.plan.quests.length === 0 ? (
                  <p className="hint">这一批都已经是完成态，没有要补的。</p>
                ) : (
                  <p className="hint mono-wrap">{questPlan.plan.quests.join("、")}</p>
                )}
                {questPlan.plan.blocked.length ? (
                  <p className="hint">
                    被挡住的（互斥分支已被占用、答案门选了另一支、或目录里的条件待核实）：
                    <span className="mono-wrap">{questPlan.plan.blocked.join("、")}</span>
                  </p>
                ) : null}
              </div>
            ) : null}

            <div className="segmented">
              {QUEST_FILTERS.map((f) => (
                <button key={f} aria-pressed={questFilter === f} onClick={() => setQuestFilter(f)}>
                  {f}
                </button>
              ))}
              <span className="spacer" />
              <span className="hint">勾选要改的行，再用下面三个按钮；一行都不勾时三个都不可点。</span>
            </div>
            <div className="row">
              <button
                className="btn"
                data-tone="accent"
                disabled={!canQuest || questPicked.length === 0}
                onClick={() => runQuestAction("quests.ready")}
              >
                {manageBusy ? "写入中…" : "置为可交付"}
              </button>
              <button
                className="btn"
                data-tone="accent"
                disabled={!canQuest || questPicked.length === 0}
                onClick={() => runQuestAction("quests.complete")}
              >
                标记完成
              </button>
              <button
                className="btn"
                data-tone="danger"
                disabled={!canQuest || questPicked.length === 0}
                onClick={() => setQuestDoomed([...questPicked])}
              >
                放弃所选
              </button>
              <span className="spacer" />
              {questPicked.length ? (
                <button className="btn" data-tone="ghost" onClick={() => setQuestPicked([])}>
                  清空勾选（{questPicked.length}）
                </button>
              ) : null}
            </div>
            {questRows.length === 0 ? (
              <p className="hint">{quests.quests.length === 0 ? "这个角色还没有任务记录。" : "这一档没有任务行。"}</p>
            ) : (
              <table className="af-live af-live--quests">
                <thead>
                  <tr>
                    <th>选</th>
                    <th>任务</th>
                    <th>阶段</th>
                    <th>前置</th>
                    <th>操作</th>
                  </tr>
                </thead>
                <tbody>
                  {questRows.map((row) => (
                    <tr key={row.id}>
                      <td>
                        <input
                          type="checkbox"
                          checked={questPicked.includes(row.id)}
                          disabled={!canQuest || (!row.can_modify && !row.can_abandon)}
                          onChange={() =>
                            setQuestPicked((prev) =>
                              prev.includes(row.id) ? prev.filter((v) => v !== row.id) : [...prev, row.id],
                            )
                          }
                        />
                      </td>
                      <td>
                        {row.name}
                        <br />
                        <span className="mono">{row.id}</span>
                        {row.grade ? ` · ${row.grade}` : ""}
                        {row.level > 1 ? ` · 需 ${row.level} 级` : ""}
                        {row.blocked ? (
                          <>
                            <br />
                            <span className="hint" data-tone="err">{row.blocked}</span>
                          </>
                        ) : null}
                        {row.description ? (
                          <p className="hint" title={`${row.description}\n${row.condition}\n${row.solve}`}>
                            {clip(row.description)}
                          </p>
                        ) : null}
                      </td>
                      <td>
                        {QUEST_STATUS[row.status] ?? row.status}
                        {row.status === "accepted" ? ` · 进度 ${row.progress.toLocaleString()}` : ""}
                      </td>
                      <td>
                        {row.prerequisites.length === 0 ? (
                          "—"
                        ) : (
                          <>
                            {row.prerequisites.map((group, i) => (
                              <span key={i}>
                                {i > 0 ? " 或 " : ""}
                                {group.map((pre) => (
                                  <span key={pre.id} className="chip" title={pre.name}>
                                    {pre.name}
                                    {pre.completed ? "（已完成）" : ""}
                                  </span>
                                ))}
                              </span>
                            ))}
                          </>
                        )}
                      </td>
                      <td>
                        {row.can_trace ? (
                          <button
                            className="btn"
                            data-tone="ghost"
                            disabled={!canQuest || questPlanBusy}
                            title={canQuest ? "" : inv?.message || "现在不能改"}
                            onClick={() => {
                              setQuestTrace(row.id);
                              previewTrace(row.id);
                            }}
                          >
                            {questTrace === row.id ? "追溯中…" : "追溯前置"}
                          </button>
                        ) : null}{" "}
                        {row.condition || row.solve ? (
                          <button
                            className="btn"
                            data-tone="ghost"
                            onClick={() => setQuestOpen((prev) => (prev === row.id ? null : row.id))}
                          >
                            {questOpen === row.id ? "收起说明" : "看说明"}
                          </button>
                        ) : null}
                        {questOpen === row.id ? (
                          <p className="hint">
                            <strong>条件</strong>
                            <br />
                            {row.condition}
                            <br />
                            <strong>完成</strong>
                            <br />
                            {row.solve}
                          </p>
                        ) : null}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </>
        ) : (
          <p className="hint">{questBusy ? "任务目录与存档读取中…" : "任务列表还没读到。"}</p>
        )}
      </PanelFrame>
    ),
    materials: (
      <PanelFrame bodyClass="gm-card-body">
        <div className="row">
          <span className="chip" data-tone={materials ? "ok" : "busy"}>
            {character
              ? `账号 ${character.account_id} · ${materials ? `${materials.length} 格` : "读取中…"}`
              : "未选角色"}
          </span>
          <span className="spacer" />
          <button
            className="btn"
            data-tone="ghost"
            disabled={!character || materialBusy}
            onClick={() => void loadMaterials(character?.account_id ?? 0)}
          >
            重读仓库
          </button>
        </div>
        {!character ? (
          <p className="hint">仓库跟着账号走：在「发放目标」里选这个账号下的任意角色就能看。</p>
        ) : null}
        {character && !materials ? (
          <p className="hint">这份存档还没有仓库记录，或服务端刚换过档；点「重读仓库」。</p>
        ) : null}
        {materials ? (
          <>
            <table className="af-live">
              <thead>
                <tr>
                  <th>槽位</th>
                  <th>材料</th>
                  <th>现有</th>
                  <th>改成</th>
                  <th>保存</th>
                </tr>
              </thead>
              <tbody>
                {materials.map((row) => {
                  const draft = materialDraft[row.slot] ?? row.amount;
                  const dirty = draft !== row.amount;
                  return (
                    <tr key={row.slot}>
                      <td>{row.slot}</td>
                      <td>
                        {row.name}
                        <br />
                        <span className="mono">{row.template}</span>
                      </td>
                      <td>{row.amount.toLocaleString()}</td>
                      <td>
                        <input
                          type="number"
                          min={0}
                          max={999999999}
                          value={draft}
                          disabled={!canManage}
                          onChange={(e) =>
                            setMaterialDraft((prev) => ({
                              ...prev,
                              [row.slot]: Math.max(0, Number(e.target.value) || 0),
                            }))
                          }
                        />
                      </td>
                      <td>
                        <button
                          className="btn"
                          data-tone={dirty ? "accent" : "ghost"}
                          disabled={!canManage || !dirty || manageBusy}
                          title={canManage ? "" : inv?.message || "现在不能改"}
                          onClick={() => void saveMaterial(row, draft)}
                        >
                          {dirty ? (draft === 0 ? "清空" : "写入") : "已同步"}
                        </button>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
            <p className="hint">
              19 格槽位固定，写 0 就是把这一格清掉（与 Go 一致，不是留一个 0 在册）。
              同账号共享，改完要重新进入角色才会同步游戏显示。
            </p>
          </>
        ) : null}
      </PanelFrame>
    ),
    records: (
      <PanelFrame bodyClass="gm-card-body">
        <div className="row">
          <span className="chip" data-tone={recordsError ? "err" : records ? "ok" : "busy"}>
            {recordsBusy
              ? "读取中…"
              : recordsError ||
                (records ? `${records.length} 笔（最近 100 条，新的在前）` : "还没读")}
          </span>
          <span className="spacer" />
          <button
            className="btn"
            data-tone="ghost"
            disabled={recordsBusy}
            onClick={() => void loadRecords()}
          >
            重读流水
          </button>
        </div>
        {recordsError && !records ? (
          <p className="hint" data-tone="err">{recordsError}</p>
        ) : null}
        {records && records.length === 0 ? (
          <p className="hint">
            这张表里一笔流水都没有。发放与管理都会往服务端那张 <span className="mono">admin_grants</span> 记账，
            没记就说明这份存档还没被改过。
          </p>
        ) : null}
        {records && records.length > 0 ? (
          <>
            <table className="af-live">
              <thead>
                <tr>
                  <th>时间</th>
                  <th>动作</th>
                  <th>角色</th>
                  <th>流水号</th>
                  <th>明细</th>
                </tr>
              </thead>
              <tbody>
                {records.map((row) => (
                  <Fragment key={row.grant_id}>
                    <tr>
                      <td>{grantTime(row.created_at)}</td>
                      <td>
                        {ACTION_LABELS[row.action] ?? row.action}
                        {ACTION_LABELS[row.action] ? (
                          <>
                            <br />
                            <span className="mono">{row.action}</span>
                          </>
                        ) : null}
                      </td>
                      <td>
                        {row.character_name ||
                          (row.character_id ? `角色 ${row.character_id}` : "账号级")}
                        <br />
                        <span className="mono">账号 {row.account_id} · {row.operator}</span>
                      </td>
                      <td>
                        <span className="mono">{row.grant_id}</span>
                      </td>
                      <td>
                        <button
                          className="btn"
                          data-tone="ghost"
                          onClick={() => setRecordOpen(recordOpen === row.grant_id ? null : row.grant_id)}
                        >
                          {recordOpen === row.grant_id ? "收起" : "看请求"}
                        </button>
                      </td>
                    </tr>
                    {recordOpen === row.grant_id ? (
                      <tr>
                        <td colSpan={5}>
                          <p className="hint mono-wrap">请求：{row.request}</p>
                          <p className="hint mono-wrap">回执：{row.receipt}</p>
                        </td>
                      </tr>
                    ) : null}
                  </Fragment>
                ))}
              </tbody>
            </table>
            <p className="hint">
              这一张只读，不领管理租约、也不改一个字节，服务端在跑也读得动。
              列表是全存档最近 100 笔（含服务端自己记的建角奖励那类），点开「看请求」才铺请求与回执两份 JSON。
              <br />
              凡是覆盖过角色存档的那一笔，回执里都带一个 <span className="mono-wrap">state_backup</span>：
              那是改前那一刻那一列的原文，落在存档目录下的 <span className="mono-wrap">storage\backup\state\</span>，
              文件名只有账号号与角色号。备份文件没写成功的操作会被整笔拒掉，库里连流水号都不留。
            </p>
          </>
        ) : null}
      </PanelFrame>
    ),
  };

  return (
    <>
      <DockLayer<GmKey>
        keys={GM_KEYS}
        node={gmDock}
        setNode={setGmDock}
        renderBody={(k) => CARDS[k]}
        // 「结果」那张卡的标题带条数，所以这里现算一份，其余照 GM_TITLES。
        titles={{ ...GM_TITLES, results: `结果（${total.toLocaleString()} 条）` }}
        emptyTitle="GM 工具"
        emptyText={
          <>
            物品库查询、结果、发放目标、物品详情、角色背包、数值改写、任务、仓库与发放记录这九张卡都摆在这一页：
            <br />
            按住卡片标题栏在页内拖动可以换位、分割，拖到别的卡片上按提示落位；拖动分割条调大小。
            <br />
            标题栏上那枚胶囊图标把卡片收进右侧摆放栏，点胶囊立刻归位，也可以把它拖到想落的格子上。
          </>
        }
        trayHint="卡片收起来排在这里"
      />

      {questDoomed?.length ? (
        <div className="scrim" onMouseDown={() => setQuestDoomed(null)}>
          <div
            className="sheet sheet--narrow"
            role="dialog"
            aria-modal="true"
            onMouseDown={(event) => event.stopPropagation()}
          >
            <header className="sheet-head">
              <h3>放弃 {questDoomed.length} 条任务</h3>
              <span className="spacer" />
              <button className="btn" data-tone="ghost" disabled={manageBusy} onClick={() => setQuestDoomed(null)}>
                返回
              </button>
            </header>
            <div className="sheet-body">
              <p className="hint">
                要删的是这些任务在本存档里的<strong>完成记录行</strong>：
                <span className="mono-wrap">{questDoomed.join("、")}</span>
                。改前的每一行整份写进操作审计，但任务里的进度找不回来。
              </p>
              <p className="hint" data-tone="err">
                放弃之后角色可以重新接同一条任务；服务端那边要重新选角色才会看到。
              </p>
              <div className="row">
                <span className="spacer" />
                <button className="btn" data-tone="ghost" disabled={manageBusy} onClick={() => setQuestDoomed(null)}>
                  取消
                </button>
                <button
                  className="btn"
                  data-tone="danger"
                  disabled={manageBusy || !canQuest}
                  onClick={abandonQuests}
                >
                  {manageBusy ? "写入中…" : "确认放弃"}
                </button>
              </div>
            </div>
          </div>
        </div>
      ) : null}

      {doomed ? (
        <div className="scrim" onMouseDown={() => setDoomed(null)}>
          <div
            className="sheet sheet--narrow"
            role="dialog"
            aria-modal="true"
            onMouseDown={(event) => event.stopPropagation()}
          >
            <header className="sheet-head">
              <h3>删掉这一行背包物品</h3>
              <span className="spacer" />
              <button className="btn" data-tone="ghost" disabled={manageBusy} onClick={() => setDoomed(null)}>
                返回
              </button>
            </header>
            <div className="sheet-body">
              <p className="hint">
                要删的是 <strong>{doomed.name}</strong>（{bagLabel(doomed)} 位置 {doomed.slot}
                {doomed.group > 0 ? ` 组 ${doomed.group}` : ""}，模板 {doomed.template}，数量 {doomed.count}）。
              </p>
              <p className="hint" data-tone="err">
                这一行从存档里去掉，删掉前那一份完整记录会写进操作审计（改前 / 改后都在），
                但物品本身找不回来。期限与强化属性跟着这一行走，不留残壳。
              </p>
              <div className="row">
                <span className="spacer" />
                <button className="btn" data-tone="ghost" disabled={manageBusy} onClick={() => setDoomed(null)}>
                  取消
                </button>
                <button
                  className="btn"
                  data-tone="danger"
                  disabled={manageBusy || !canManage}
                  onClick={() => void deleteBagRow()}
                >
                  {manageBusy ? "删除中…" : "确认删除"}
                </button>
              </div>
            </div>
          </div>
        </div>
      ) : null}
    </>
  );
}
