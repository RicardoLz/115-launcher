//! 停靠层：一棵二分树（叶子=面板/卡片，内节点=分割）。开发者页与 GM 页共用这一套。
//! 这里只有纯数据与纯函数，命中测试用 CSS 像素，DOM 一律不碰。
//! 泛化成任意字符串 key：开发者页用 PanelKey，GM 页用 GmKey，各自的默认树各放各的。

export type PanelKey = "log" | "mode" | "mods" | "modkit" | "memory";

export const PANEL_KEYS: PanelKey[] = ["log", "mode", "mods", "modkit", "memory"];

/// GM 页九张卡的 key：与旧布局的左右两列一一对应（左列 search/results，右列其余七块）。
export type GmKey =
  | "search"
  | "results"
  | "target"
  | "detail"
  | "bag"
  | "stats"
  | "quests"
  | "materials"
  | "records";

export const GM_KEYS: GmKey[] = [
  "search",
  "results",
  "target",
  "detail",
  "bag",
  "stats",
  "quests",
  "materials",
  "records",
];

export type Dir = "v" | "h";
export type Edge = "top" | "bottom" | "left" | "right";

export type DockNode<K extends string = PanelKey> =
  | { t: "leaf"; k: K }
  | { t: "split"; dir: Dir; r: number; a: DockNode<K>; b: DockNode<K> };

export type SplitNode<K extends string = PanelKey> = Extract<DockNode<K>, { t: "split" }>;

/// 拖到的位置：edge=null 表示落在某块面板正中＝两块交换。
export type Zone<K extends string = PanelKey> = { k: K; edge: Edge | null };

/// 默认摆法：左边一整列给运行日志（它最吃高度），右边 2×2 放其余四块。
/// 五块平铺会挤到每块都放不下正文，所以日志单独占一侧。
export const DEFAULT_DOCK: DockNode = {
  t: "split",
  dir: "h",
  r: 0.44,
  a: { t: "leaf", k: "log" },
  b: {
    t: "split",
    dir: "v",
    r: 0.5,
    a: {
      t: "split",
      dir: "h",
      r: 0.5,
      a: { t: "leaf", k: "mode" },
      b: { t: "leaf", k: "mods" },
    },
    b: {
      t: "split",
      dir: "h",
      r: 0.5,
      a: { t: "leaf", k: "modkit" },
      b: { t: "leaf", k: "memory" },
    },
  },
};

/// GM 页默认摆法：上半照旧版两列 —— 左列查询与结果各占一半，右列上排两张（发放目标、物品详情），
/// 下排 2×2（背包 / 数值 / 任务 / 仓库）；整条底栏给发放记录，它是这张页里唯一一行五列的宽表，
/// 挤在右下角只能看见标题。九块全平铺会挤到没一块能读，先分好层级。
export const DEFAULT_GM_DOCK: DockNode<GmKey> = {
  t: "split",
  dir: "v",
  r: 0.8,
  a: {
    t: "split",
    dir: "h",
    r: 0.5,
    a: {
      t: "split",
      dir: "v",
      r: 0.5,
      a: { t: "leaf", k: "search" },
      b: { t: "leaf", k: "results" },
    },
    b: {
      t: "split",
      dir: "v",
      r: 0.42,
      a: {
        t: "split",
        dir: "h",
        r: 0.5,
        a: { t: "leaf", k: "target" },
        b: { t: "leaf", k: "detail" },
      },
      b: {
        t: "split",
        dir: "v",
        r: 0.5,
        a: {
          t: "split",
          dir: "h",
          r: 0.5,
          a: { t: "leaf", k: "bag" },
          b: { t: "leaf", k: "stats" },
        },
        b: {
          t: "split",
          dir: "h",
          r: 0.5,
          a: { t: "leaf", k: "quests" },
          b: { t: "leaf", k: "materials" },
        },
      },
    },
  },
  b: { t: "leaf", k: "records" },
};

const EDGE_DIR: Record<Edge, Dir> = { top: "v", bottom: "v", left: "h", right: "h" };
const EDGE_FIRST: Record<Edge, boolean> = { top: true, left: true, bottom: false, right: false };

export function leaves<K extends string>(node: DockNode<K> | null): K[] {
  if (!node) return [];
  return node.t === "leaf" ? [node.k] : [...leaves(node.a), ...leaves(node.b)];
}

/// 摘掉一个面板并把空掉的分割塌掉。
export function strip<K extends string>(node: DockNode<K> | null, k: K): DockNode<K> | null {
  if (!node) return null;
  if (node.t === "leaf") return node.k === k ? null : node;
  const a = strip(node.a, k);
  const b = strip(node.b, k);
  if (!a) return b;
  if (!b) return a;
  return a === node.a && b === node.b ? node : { ...node, a, b };
}

/// 两块面板交换位置（只在两块都在停靠区时用）。
function swap<K extends string>(node: DockNode<K>, from: K, to: K): DockNode<K> {
  if (node.t === "leaf") {
    if (node.k === from) return { t: "leaf", k: to };
    if (node.k === to) return { t: "leaf", k: from };
    return node;
  }
  return { ...node, a: swap(node.a, from, to), b: swap(node.b, from, to) };
}

function replaceLeaf<K extends string>(
  node: DockNode<K>,
  k: K,
  next: DockNode<K>,
): DockNode<K> | null {
  if (node.t === "leaf") return node.k === k ? next : null;
  const a = replaceLeaf(node.a, k, next);
  if (a) return { ...node, a };
  const b = replaceLeaf(node.b, k, next);
  return b ? { ...node, b } : null;
}

/// 没有落点就接在末尾，两块面板竖着排。
function append<K extends string>(node: DockNode<K> | null, k: K): DockNode<K> {
  const fresh: DockNode<K> = { t: "leaf", k };
  const base = strip(node, k);
  if (!base) return fresh;
  return { t: "split", dir: "v", r: 0.5, a: base, b: fresh };
}

/// 落点决定形状：正中=交换，四条边=沿该边分割，落在自己身上=不动。
export function place<K extends string>(
  node: DockNode<K> | null,
  k: K,
  zone: Zone<K> | null,
): DockNode<K> {
  if (!node) return { t: "leaf", k };
  if (zone && zone.k === k) return node;
  if (!zone) return append(node, k);
  // 交换只两块都在停靠区时才成立；从独立窗口归位回来时落点是空档，直接接末尾。
  if (zone.edge === null) {
    const both = leaves(node).includes(zone.k) && leaves(node).includes(k);
    return both ? swap(node, zone.k, k) : append(node, k);
  }

  const base = strip(node, k);
  if (!base) return { t: "leaf", k };
  if (!leaves(base).includes(zone.k)) return append(base, k);

  const fresh: DockNode<K> = { t: "leaf", k };
  const target: DockNode<K> = { t: "leaf", k: zone.k };
  const first = EDGE_FIRST[zone.edge];
  const branch: DockNode<K> = {
    t: "split",
    dir: EDGE_DIR[zone.edge],
    r: 0.5,
    a: first ? fresh : target,
    b: first ? target : fresh,
  };
  return replaceLeaf(base, zone.k, branch) ?? branch;
}

/// 拖动分割条：用「从根往下走 a/b」的路径认分割节点。
/// 不能按对象引用认——第一次改比例就会换掉那个对象，之后的移动就找不到它了。
export function retune<K extends string>(node: DockNode<K>, path: number[], r: number): DockNode<K> {
  if (node.t !== "split") return node;
  if (path.length === 0) return node.r === r ? node : { ...node, r };
  const [head, ...rest] = path;
  if (head === 0) {
    const a = retune(node.a, rest, r);
    return a === node.a ? node : { ...node, a };
  }
  const b = retune(node.b, rest, r);
  return b === node.b ? node : { ...node, b };
}

const NEAR = 0.3;

/// 命中测试：先看是不是落在正中（交换），否则取最近的一条边。
export function zoneAt<K extends string>(
  rects: Array<{ k: K; rect: { left: number; top: number; right: number; bottom: number } }>,
  x: number,
  y: number,
): Zone<K> | null {
  let best: Zone<K> | null = null;
  let bestNear = Infinity;
  for (const { k, rect } of rects) {
    if (x < rect.left || x > rect.right || y < rect.top || y > rect.bottom) continue;
    const w = rect.right - rect.left;
    const h = rect.bottom - rect.top;
    const dTop = y - rect.top;
    const dBottom = rect.bottom - y;
    const dLeft = x - rect.left;
    const dRight = rect.right - x;
    if (dTop > h * NEAR && dBottom > h * NEAR && dLeft > w * NEAR && dRight > w * NEAR) {
      return { k, edge: null };
    }
    const edges: Array<[Edge, number]> = [
      ["top", dTop],
      ["bottom", dBottom],
      ["left", dLeft],
      ["right", dRight],
    ];
    const nearest = edges.reduce((a, b) => (b[1] < a[1] ? b : a));
    if (nearest[1] < bestNear) {
      bestNear = nearest[1];
      best = { k, edge: nearest[0] };
    }
  }
  return best;
}
