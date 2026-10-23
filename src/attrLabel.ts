/**
 * 角色存档 `attributes` 那张表里，键名 → 中文的对照。
 *
 * 口径抄的是旧 Go 工具 README「属性面板」那一节，不是服务端 `internal/inventory/symbol_display.go`
 * ——那张表是给**装备符号**显示用的（把 `[physical attack]` 写成「力量」、速度类还按千分位换算），
 * 跟角色面板的用词与读法都对不上。旧工具的规矩是：
 * - 默认只铺一组**精选核心属性**（HP/MP 上限、物理/魔法攻击力、物理/魔法防御力、攻击/施放/移动速度、
 *   跳跃力、硬直、负重上限），其余（抗性、恢复速度、物品栏负重等）折叠进「展开全部属性」。
 * - 属性名用中文，内部键（`[hp max]` 这种）只放在鼠标悬停提示里。
 * - 对照表里没有的键标「未汉化」原样列出，不猜也不隐藏。
 * - **数值一律是存档原值，不做换算**；千分位这件事只在下面那句界面说明里点一句，不替界面改数。
 *
 * 中文用词逐条对照旧工具列出的客户端 `string/*.uv.str` 原文：「物理防御力」「魔法防御力」「火/光/暗属性
 * 抗性」「HP上限」「攻击速度」「跳跃力」那一族。已知钉不死的一条：`[water resistance]`——旧工具写客户端
 * 原文是「火/冰/光/暗属性抗性」，存档键却是 water，而本工作区唯一能读的客户端文本投影
 * （`gmbuild/out/names.client.json`，只有 `name_*` 物品名）里火/光/暗各出现一次、水与冰零出现。
 * 这里按存档键写作「水属性抗性」，游戏里那一行很可能写作「冰属性抗性」。它在折叠组，不参与任何写入判定。
 *
 * 键集与分组都对着本机真档核过（只读取 `characters.state`，4 个角色）：每档固定 15 条，外加 1~4 条抗性；
 * `[weight]`、`[jump speed]` 每条档都有但对照表里没有，走「未汉化」那一档。
 */
export interface AttrLabel {
  label: string;
  /** 精选核心属性：默认就铺出来；非精选的要等「展开全部属性」。 */
  core: boolean;
}

/** 键名 → 中文。界面上的先后照这里登记的顺序：精选组在前，折叠组在后，未汉化最后。 */
export const ATTR_LABELS: Record<string, AttrLabel> = {
  "[hp max]": { label: "HP上限", core: true },
  "[mp max]": { label: "MP上限", core: true },
  "[physical attack]": { label: "物理攻击力", core: true },
  "[magical attack]": { label: "魔法攻击力", core: true },
  "[physical defense]": { label: "物理防御力", core: true },
  "[magical defense]": { label: "魔法防御力", core: true },
  "[attack speed]": { label: "攻击速度", core: true },
  "[cast speed]": { label: "施放速度", core: true },
  "[move speed]": { label: "移动速度", core: true },
  "[jump power]": { label: "跳跃力", core: true },
  "[hit recovery]": { label: "硬直", core: true },
  "[inventory limit]": { label: "负重上限", core: true },
  "[fire resistance]": { label: "火属性抗性", core: false },
  "[water resistance]": { label: "水属性抗性", core: false },
  "[light resistance]": { label: "光属性抗性", core: false },
  "[dark resistance]": { label: "暗属性抗性", core: false },
  "[hp regen speed]": { label: "HP恢复速度", core: false },
  "[mp regen speed]": { label: "MP恢复速度", core: false },
};

const ORDER = Object.keys(ATTR_LABELS);

export interface AttrRow {
  key: string;
  /** 存档原值，界面直接铺这一个，不换算。 */
  value: number;
  label: string;
  translated: boolean;
  core: boolean;
}

/**
 * 把 `summary.attributes`（Rust 侧原样透传的存档对象）排成界面要的行列。
 * 只认真数字：`null` 丢掉而不是当成 0（`Number(null)` 是 0，硬换算会把一个空键变成一条真属性）。
 */
export function attributeRows(attrs: unknown): AttrRow[] {
  if (!attrs || typeof attrs !== "object" || Array.isArray(attrs)) return [];
  const rows: AttrRow[] = [];
  for (const [key, value] of Object.entries(attrs as Record<string, unknown>)) {
    if (typeof value !== "number" || !Number.isFinite(value)) continue;
    const known = ATTR_LABELS[key];
    rows.push(
      known
        ? { key, value, label: known.label, translated: true, core: known.core }
        : { key, value, label: key, translated: false, core: false },
    );
  }
  const rank = (r: AttrRow) => {
    const i = ORDER.indexOf(r.key);
    return i < 0 ? ORDER.length : i;
  };
  return rows.sort((a, b) => rank(a) - rank(b) || a.key.localeCompare(b.key));
}
