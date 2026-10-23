//! 方案 C 的读取侧：直接读 gm-tool/configs 下五个 JSON 产物，复刻 Go gmtool
//! （index.go / itemquery.go）的「查询 / 筛选」语义。
//!
//! 本模块**不碰 PVF**，只依赖 serde —— 启动器不带 `build` feature 也能链入。
//! 条目字符串走 `Arc`，所以 `Catalog` 本身是 `Send + Sync`，调用方直接共享它、
//! 不再另存一份自有副本；旁表（部位/职业/品质/名字）在 `load` 里逐份
//! 「解析 → 套用 → 释放」，运行期只留条目本体。
//! 语义来源（逐条对齐）：
//! - `LoadItemIndex → loadItemsIndexFile → absorb → refineEquipment → finish`
//! - `ItemIndex.Search / match / inType`（空条件低等级档、数字 ID 前缀、通配锚定、前缀优先两遍）
//! - `slotOfEquipmentCell / slotGroupOf / classifyStackable / stackableDisplayLabel / rarityLabel`
//! - `parseItemQuery / matchWildcard`（'*' 任意长度、两端锚定、逐字节小写比较）
//! - 旁表缺失全部容忍回落（loadSlotMap/loadJobsMap/loadRarityMap 的口径）。

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// 常量表（抄自 index.go）
// ---------------------------------------------------------------------------

const NAME_PREFIX: &str = "name_";
const GROW_PREFIX: &str = "growtype_name_";
const RARITY_PREFIX: &str = "common_rarity_";

pub const TYPE_CONSUMABLE: &str = "consumable";
pub const TYPE_MATERIAL: &str = "material";
pub const TYPE_QUEST: &str = "quest";
pub const TYPE_PROFESSION: &str = "profession";
pub const TYPE_EMBLEM: &str = "emblem";
pub const TYPE_EQUIPMENT: &str = "equipment";
pub const TYPE_OTHER: &str = "other";

const SLOT_OTHER: &str = "其它装备";
const SLOT_AVATAR: &str = "时装";
const GROUP_OTHER: &str = "其它";
const GROUP_AVATAR: &str = "时装";
const AVATAR_SUFFIX: &str = " avatar]";

/// Go defaultRarityLabels；names.client.json 的 common_rarity_0..8 与之一致。
pub const RARITY_LABELS: [&str; 9] = ["普通", "高级", "稀有", "神器", "史诗", "勇者", "传说", "神话", "太初"];

pub fn rarity_label(v: i32) -> &'static str {
    if v < 0 || v as usize >= RARITY_LABELS.len() {
        return "";
    }
    RARITY_LABELS[v as usize]
}

pub fn is_valid_rarity(v: i32) -> bool {
    (0..RARITY_LABELS.len() as i32).contains(&v)
}

/// 14 个现役装备位（cell → 部位、大类），与 equipmentSlotCells 逐行一致。
static EQUIPMENT_SLOT_CELLS: [(&str, &str, &str); 14] = [
    ("[weapon]", "武器", "武器"),
    ("[title name]", "称号", "称号"),
    ("[coat]", "上衣", "防具"),
    ("[shoulder]", "头肩", "防具"),
    ("[pants]", "下装", "防具"),
    ("[shoes]", "鞋", "防具"),
    ("[waist]", "腰带", "防具"),
    ("[amulet]", "项链", "首饰"),
    ("[wrist]", "手镯", "首饰"),
    ("[ring]", "戒指", "首饰"),
    ("[support]", "辅助装备", "特殊装备"),
    ("[magic stone]", "魔法石", "特殊装备"),
    ("[support weapon]", "辅助武器", "特殊装备"),
    ("[earring]", "耳环", "特殊装备"),
];

/// Go slotOfEquipmentCell：已知 14 位走表，`[xxx avatar]` 归时装，其余「其它装备」。
pub fn slot_of_equipment_cell(cell: &str) -> (&'static str, &'static str) {
    for (c, slot, group) in EQUIPMENT_SLOT_CELLS.iter() {
        if *c == cell {
            return (slot, group);
        }
    }
    if cell.ends_with(AVATAR_SUFFIX) {
        return (SLOT_AVATAR, GROUP_AVATAR);
    }
    (SLOT_OTHER, GROUP_OTHER)
}

/// Go slotGroupOf：按部位名回查大类。
pub fn slot_group_of(slot: &str) -> &'static str {
    for (_, s, group) in EQUIPMENT_SLOT_CELLS.iter() {
        if *s == slot {
            return group;
        }
    }
    match slot {
        SLOT_AVATAR => GROUP_AVATAR,
        SLOT_OTHER => GROUP_OTHER,
        _ => GROUP_OTHER,
    }
}

/// Go orderedTypes：页签固定顺序与中文标签。
static ORDERED_TYPES: [(&str, &str); 7] = [
    (TYPE_CONSUMABLE, "消耗品"),
    (TYPE_MATERIAL, "材料"),
    (TYPE_QUEST, "任务"),
    (TYPE_PROFESSION, "副职业"),
    (TYPE_EMBLEM, "徽章"),
    (TYPE_EQUIPMENT, "装备"),
    (TYPE_OTHER, "其它"),
];

pub fn type_label(key: &str) -> &'static str {
    for (k, label) in ORDERED_TYPES.iter() {
        if *k == key {
            return label;
        }
    }
    // Go labelOf 兜底返回 key 本身；这里为 &'static 输入返回 '' 之外的最近似：
    // 只有 ORDERED_TYPES 的 7 个 key 会作为筛选/展示 key 出现。
    ""
}

/// Go stackableTypeKeys（线性表 15 条足够快）。
static STACKABLE_TYPE_KEYS: &[(&str, &str)] = &[
    ("[waste]", TYPE_CONSUMABLE),
    ("[throw]", TYPE_CONSUMABLE),
    ("[booster]", TYPE_CONSUMABLE),
    ("[booster selection]", TYPE_CONSUMABLE),
    ("[booster random]", TYPE_CONSUMABLE),
    ("[cera package]", TYPE_CONSUMABLE),
    ("[usable cera package]", TYPE_CONSUMABLE),
    ("[etc]", TYPE_CONSUMABLE),
    ("[contract]", TYPE_CONSUMABLE),
    ("[recipe]", TYPE_CONSUMABLE),
    ("[material]", TYPE_MATERIAL),
    ("[upgrade limit cube]", TYPE_MATERIAL),
    ("[quest]", TYPE_QUEST),
    ("[quest receive]", TYPE_QUEST),
    ("[material expert job]", TYPE_PROFESSION),
    ("[avatar emblem]", TYPE_EMBLEM),
];

/// Go classifyStackable：未列出的 stack_type 一律按消耗品兜底，不做猜测归类。
pub fn classify_stackable(stack_type: &str) -> &'static str {
    for (k, v) in STACKABLE_TYPE_KEYS.iter() {
        if *k == stack_type {
            return v;
        }
    }
    TYPE_CONSUMABLE
}

/// Go stackableDisplayTypes：悬停展示的细分中文标签表（65 条）。
static STACKABLE_DISPLAY_TYPES: &[(&str, &str)] = &[
    ("[waste]", "消耗品"),
    ("[material]", "材料"),
    ("[booster]", "礼盒"),
    ("[booster selection]", "自选礼盒"),
    ("[booster random]", "随机礼盒"),
    ("[cera booster]", "商城礼盒"),
    ("[cera package]", "商城礼包"),
    ("[usable cera package]", "可使用商城礼包"),
    ("[grouped random box]", "组合随机礼盒"),
    ("[avatar emblem]", "装扮徽章"),
    ("[auto enchant]", "附魔宝珠"),
    ("[enchant waste]", "附魔道具"),
    ("[recipe]", "配方"),
    ("[material expert job]", "副职业材料"),
    ("[contract]", "契约"),
    ("[dye]", "染色剂"),
    ("[feed]", "宠物饲料"),
    ("[throw]", "投掷道具"),
    ("[town and dungeon]", "城镇与地下城道具"),
    ("[expert town potion]", "副职业药剂"),
    ("[town status potion]", "城镇药剂"),
    ("[teleport potion]", "传送药剂"),
    ("[npc teleport potion]", "NPC传送药剂"),
    ("[unlimited waste]", "不限次数消耗品"),
    ("[unlimited town and dungeon]", "不限次数城镇与地下城道具"),
    ("[etc]", "特殊功能道具"),
    ("[upgradable legacy]", "升级抽奖道具"),
    ("[quest]", "任务物品"),
    ("[disguise]", "变身道具"),
    ("[legacy]", "旧式袖珍罐"),
    ("[dungeon and life recipe]", "生活制作图"),
    ("[only effect]", "特效道具"),
    ("[getitem use action]", "获取动作道具"),
    ("[upgrade limit cube]", "限定升级晶体"),
    ("[achievement item]", "成就道具"),
    ("[rune]", "符文"),
    ("[quest receive]", "任务接取道具"),
    ("[random disguise]", "随机变身道具"),
    ("[virtual]", "虚拟道具"),
    ("[scale effect]", "缩放特效道具"),
    ("[multi upgradable legacy]", "多重升级抽奖道具"),
    ("[dungeon and life booster]", "生活礼盒"),
    ("[primeval legacy item change tickets]", "太初袖珍罐物品转换券"),
    ("[monster tarot card]", "怪物塔罗牌"),
    ("[flag gem]", "守护珠"),
    ("[waste instant item]", "即时消耗品"),
    ("[guild contents]", "公会道具"),
    ("[consumption]", "消耗型道具"),
    ("[comsumption]", "消耗型道具（旧类型）"),
    ("[smart drop]", "智能掉落道具"),
    ("[set]", "套装道具"),
    ("[creature expitem]", "宠物经验道具"),
    ("[stackable]", "通用堆叠道具"),
    ("[global effect]", "全局特效道具"),
    ("[booster item crafting]", "礼盒制作道具"),
    ("[creature]", "宠物道具"),
    ("[remodel lottery]", "改造抽奖道具"),
    ("[stackable legacy]", "堆叠袖珍罐"),
    ("[unlimited etc]", "不限次数功能道具"),
    ("[apc legacy]", "APC袖珍罐"),
    ("[epic equipment transform]", "史诗装备转换道具"),
    ("[multi upgradable legacy bonus cera]", "多重升级抽奖点券奖励"),
    ("[item upgrad buff usual upgrade]", "物品升级增益道具"),
    ("[material instant item]", "即时材料道具"),
];

/// Go stackableDisplayLabel：空 = 未标注类型；表外 = 「未识别类型 <key>」。
pub fn stackable_display_label(key: &str) -> Cow<'static, str> {
    for (k, label) in STACKABLE_DISPLAY_TYPES.iter() {
        if *k == key {
            return Cow::Borrowed(label);
        }
    }
    if key.is_empty() {
        return Cow::Borrowed("未标注类型");
    }
    Cow::Owned(format!("未识别类型 {key}"))
}

/// Go levelBands：max=0 表示无上限。
pub static LEVEL_BANDS: [(&str, &str, i32, i32); 7] = [
    ("~20", "≤20", 0, 20),
    ("21-50", "21-50", 21, 50),
    ("51-80", "51-80", 51, 80),
    ("81-100", "81-100", 81, 100),
    ("101-110", "101-110", 101, 110),
    ("111-115", "111-115", 111, 115),
    ("116+", "116+", 116, 0),
];

// ---------------------------------------------------------------------------
// 物品模型
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Equipment,
    Stackable,
    Avatar,
    Unknown,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Equipment => "equipment",
            Kind::Stackable => "stackable",
            Kind::Avatar => "avatar",
            Kind::Unknown => "other",
        }
    }
    fn parse(s: &str) -> Kind {
        match s {
            "equipment" => Kind::Equipment,
            "stackable" => Kind::Stackable,
            "avatar" => Kind::Avatar,
            _ => Kind::Unknown,
        }
    }
}

/// 一件可检索的物品。label 类字段全部是 &'static / 派生查表，避免 67 万行重复堆串；
/// 名称类用 `Arc`：条目本体即最终形态，调用方共享引用而不是再抄一份。
pub struct GmItem {
    pub id: u32,
    pub name: Arc<str>,
    pub name_en: Arc<str>,
    pub kind: Kind,
    pub grade: i32,
    pub rarity: i32,
    pub level: i32,
    /// 装备位（"武器"/"上衣"/…"时装"/"其它装备"）；stackable 为空。
    pub slot: &'static str,
    pub group: &'static str,
    /// 页签分类 key（stackable）或部位名（equipment，经 applySlot 覆盖）。
    pub type_key: &'static str,
    /// 原始类型文本：stackable 的 [stackable type]，equipment 的 [equipment type] cell。
    pub type_text: Option<Arc<str>>,
    pub stack_limit: u32,
    pub durability: u32,
    /// 只对「有职业限制」的物品为 Some；None = 通用（Go 的 [all] 约定）。
    pub jobs: Option<Arc<[String]>>,
}

impl GmItem {
    pub fn grantable(&self) -> bool {
        // Go push 里 Grantable 恒为 true，Stackable 由 kind 决定。
        true
    }
    pub fn stackable(&self) -> bool {
        self.kind == Kind::Stackable
    }
    /// 时装判据。现网这一代 `items.index.json` 的 `kind` 只有 stackable/equipment 两种值，
    /// 时装是部位表（`equipment.slots.json`）把 `slot` 覆盖成「时装」那一格认出来的，
    /// 所以 `kind` 那一支只算兜底；服务端把这一族放进 `special_equipment` 的 space 1。
    pub fn avatar(&self) -> bool {
        self.kind == Kind::Avatar || self.slot == SLOT_AVATAR
    }
    fn id_key(&self) -> String {
        self.id.to_string()
    }
}

// ---------------------------------------------------------------------------
// 产物结构
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawItem {
    id: u32,
    #[serde(default)]
    name: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    grade: i32,
    // Go 解码前先置 Rarity: -1，缺字段即保持 -1。
    #[serde(default = "neg_one")]
    rarity: i32,
    #[serde(default)]
    stack_type: String,
    #[serde(default)]
    stackable_type: String,
    #[serde(default)]
    stack_limit: u32,
    #[serde(default)]
    durability: u32,
}

fn neg_one() -> i32 {
    -1
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct ItemsFile {
    #[serde(default)]
    count: usize,
    #[serde(default)]
    items: Vec<RawItem>,
    #[serde(default)]
    source: String,
}

#[derive(Deserialize)]
struct SlotsFile {
    #[serde(default)]
    source: String,
    #[serde(default)]
    built_at: String,
    #[serde(default)]
    slots: HashMap<String, SlotRaw>,
}

#[derive(Deserialize)]
struct SlotRaw {
    #[serde(default)]
    cell: String,
    #[serde(default)]
    level: i32,
}

#[derive(Deserialize)]
struct JobsFile {
    #[serde(default)]
    source: String,
    #[serde(default)]
    built_at: String,
    #[serde(default)]
    jobs: HashMap<String, Vec<String>>,
}

#[derive(Deserialize)]
struct RarityFile {
    #[serde(default)]
    source: String,
    #[serde(default)]
    built_at: String,
    #[serde(default)]
    rarity: HashMap<String, i32>,
}

// 三张旁表各留一条指纹给界面；条目本体用完即释放，所以要在释放前取。
impl SlotsFile {
    fn info(&self) -> TableInfo {
        TableInfo {
            present: true,
            entries: self.slots.len(),
            source: self.source.clone(),
            built_at: self.built_at.clone(),
        }
    }
}

impl JobsFile {
    fn info(&self) -> TableInfo {
        TableInfo {
            present: true,
            entries: self.jobs.len(),
            source: self.source.clone(),
            built_at: self.built_at.clone(),
        }
    }
}

impl RarityFile {
    fn info(&self) -> TableInfo {
        TableInfo {
            present: true,
            entries: self.rarity.len(),
            source: self.source.clone(),
            built_at: self.built_at.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct TableInfo {
    pub present: bool,
    pub entries: usize,
    pub source: String,
    pub built_at: String,
}

impl TableInfo {
    /// 旁表缺失（或非法 JSON 按缺失处理）时的指纹。
    fn absent() -> TableInfo {
        TableInfo { present: false, entries: 0, source: String::new(), built_at: String::new() }
    }
}

/// 展示给界面的产物指纹。
#[derive(Clone, Debug, Serialize)]
pub struct CatalogStatus {
    pub dir: String,
    pub items_source: String,
    pub count: usize,
    pub equipment: usize,
    pub stackable: usize,
    pub slots: TableInfo,
    pub jobs: TableInfo,
    pub rarity: TableInfo,
    pub names: TableInfo,
}

pub struct Catalog {
    items: Vec<GmItem>,
    by_id: HashMap<u32, usize>,
    pub status: CatalogStatus,
    /// 空条件时 Go defaultGate 的命中下标，装载时算一次供每次查询复用。
    default_hits: Vec<usize>,
    /// 客户端职业名表（只留 growtype_name_* 那几百条），用来推导职业显示名。
    growtype: BTreeMap<i64, String>,
    class_names: ClassNames,
    facet_types: Vec<usize>,    // ORDERED_TYPES 下标 → 件数（装备桶含全部 kind==equipment）
    facet_slots: Vec<usize>,    // EQUIPMENT_SLOT_CELLS 下标 + 时装/其它装备 两个补位
    facet_rarities: Vec<usize>, // 0..8
    facet_bands: Vec<usize>,    // LEVEL_BANDS 下标
}

#[derive(Clone, Debug, Serialize)]
pub struct Facet {
    pub key: String,
    pub label: String,
    pub count: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Facets {
    pub types: Vec<Facet>,
    pub slots: Vec<Facet>,
    pub rarities: Vec<Facet>,
    pub level_bands: Vec<Facet>,
}

// ---------------------------------------------------------------------------
// 职业显示名（抄自 gmtool bridge_character.go 的 gmProfessionNames
// 与 classnames.go 的 buildClassNames / classOfRoleFull）
// ---------------------------------------------------------------------------

/// Go gmProfessionNames：职业大类 → 各转职槽基名（下标即 advancement）。
const PROFESSION_NAMES: [&[&str]; 17] = [
    &["鬼剑士", "剑魂", "鬼泣", "狂战士", "阿修罗", "剑影"],
    &["格斗家", "气功师", "散打", "街霸", "柔道家"],
    &["神枪手", "漫游枪手", "枪炮师", "机械师", "弹药专家", "合金战士"],
    &["魔法师", "元素师", "召唤师", "战斗法师", "魔道学者", "小魔女"],
    &["圣职者", "圣骑士", "蓝拳圣使", "驱魔师", "复仇者"],
    &["神枪手", "漫游枪手", "枪炮师", "机械师", "弹药专家", "协战师"],
    &["暗夜使者", "刺客", "死灵术士", "忍者", "影舞者"],
    &["格斗家", "气功师", "散打", "街霸", "柔道家"],
    &["魔法师", "元素爆破师", "冰结师", "血法师", "逐风者", "次元行者"],
    &["黑暗武士", "黑暗武士", "黑暗武士", "黑暗武士", "黑暗武士", "黑暗武士"],
    &["缔造者", "缔造者", "缔造者", "缔造者", "缔造者", "缔造者"],
    &["鬼剑士", "驭剑士", "暗殿骑士", "契魔者", "流浪武士", "刃影"],
    &["守护者", "精灵骑士", "混沌魔灵", "帕拉丁", "龙骑士"],
    &["魔枪士", "征战者", "决战者", "狩猎者", "暗枪士"],
    &["圣职者", "圣骑士", "异端审判者", "巫女", "诱魔者"],
    &["枪剑士", "暗刃", "特工", "战线佣兵", "源能专家"],
    &["弓箭手", "缪斯", "旅人", "猎人", "妖护使", "奇美拉"],
];

/// Go thirdAwakeningPrefixes：三次觉醒名的前缀（新职业用职业系名）。
const THIRD_AWAKENING_PREFIXES: [&str; 10] =
    ["极诣", "重霄", "聆风", "知源", "光启", "归元", "皓曦", "苍暮", "隐夜", "千魂"];

/// Go femaleFamilies：.chr 带 at 前缀的职业大类，其余按男性系取偏移。
const FEMALE_FAMILIES: [u8; 5] = [5, 7, 8, 11, 14];

/// Go staleClassNames：旧表里的过时速称 → 客户端 115 当前用名（不改名就匹配不上块首）。
const STALE_CLASS_NAMES: [(&str, &str); 8] = [
    ("圣职者", "光职者"),
    ("圣骑士", "光明骑士"),
    ("蓝拳圣使", "蓝拳使者"),
    ("复仇者", "惩戒者"),
    ("刺客", "暗星"),
    ("死灵术士", "黑夜术士"),
    ("血法师", "猩红法师"),
    ("异端审判者", "正义审判者"),
];

fn stale_name(old: &str) -> Option<&'static str> {
    STALE_CLASS_NAMES.iter().find(|(o, _)| *o == old).map(|(_, n)| *n)
}

/// (职业大类, 转职槽, 觉醒阶段) → 显示名，由客户端名字表推导。
#[derive(Default)]
pub struct ClassNames {
    table: HashMap<u8, Vec<[String; 4]>>,
}

impl ClassNames {
    /// Go classNames.name：越界与空名都返回 None，调用方回落到基名。
    fn name(&self, family: i64, slot: i64, awakening: i64) -> Option<&str> {
        let rows = self.table.get(&(family as u8))?;
        if slot < 0 || slot as usize >= rows.len() {
            return None;
        }
        let stage = if awakening < 0 || awakening > 3 { 0 } else { awakening as usize };
        let n = rows[slot as usize][stage].as_str();
        (!n.is_empty()).then_some(n)
    }
}

/// Go buildClassNames：按「块首是家族表里的基名」切分支，块内顺序即觉醒阶段。
/// 键不连续不能作为切分依据（30..39 连续却含漫游枪手与枪炮师两个分支）。
fn build_class_names(growtype: &BTreeMap<i64, String>) -> ClassNames {
    let keys: Vec<i64> = growtype
        .iter()
        .filter(|(_, v)| !v.is_empty() && !v.starts_with("//"))
        .map(|(k, _)| *k)
        .collect();
    if keys.is_empty() {
        return ClassNames::default();
    }
    let at = |k: i64| growtype.get(&k).map(String::as_str).unwrap_or("");

    let mut base_names: HashSet<&str> = HashSet::new();
    for slots in PROFESSION_NAMES {
        for s in slots {
            base_names.insert(s);
            if let Some(now) = stale_name(s) {
                base_names.insert(now);
            }
        }
    }

    let mut blocks: Vec<Vec<i64>> = Vec::new();
    let mut cur: Vec<i64> = Vec::new();
    for &k in &keys {
        if !cur.is_empty() && base_names.contains(at(k)) {
            blocks.push(std::mem::take(&mut cur));
        }
        cur.push(k);
    }
    if !cur.is_empty() {
        blocks.push(cur);
    }

    let mut block_of: HashMap<&str, usize> = HashMap::new();
    for (bi, b) in blocks.iter().enumerate() {
        block_of.entry(at(b[0])).or_insert(bi);
    }

    let mut table: HashMap<u8, Vec<[String; 4]>> = HashMap::new();
    for (family, slots) in PROFESSION_NAMES.iter().enumerate() {
        let female = FEMALE_FAMILIES.contains(&(family as u8));
        let mut rows = Vec::with_capacity(slots.len());
        for &slot in slots.iter() {
            let base = stale_name(slot).unwrap_or(slot);
            let Some(&bi) = block_of.get(base) else {
                // 拿不到块就沿用旧表基名，至少界面不空。
                rows.push([slot.to_string(), String::new(), String::new(), String::new()]);
                continue;
            };
            let block = &blocks[bi];
            let stages = &block[1..];
            let mut out = [base.to_string(), String::new(), String::new(), String::new()];
            // 只有「基名 + 男两阶段 + 女两阶段」= 5 键的块才说明男女阶段分列，
            // 更长的块（后面混着别的分支）不能偏移。
            let off = if female && block.len() == 5 { 2 } else { 0 };
            if stages.len() > off {
                out[1] = at(stages[off]).to_string();
            }
            if stages.len() > off + 1 {
                out[2] = at(stages[off + 1]).to_string();
            }
            // 三次觉醒先按「<前缀>·<基名>」全局找（最可靠）；块内第 4 键只在整块恰好 4 键时才认。
            for pre in THIRD_AWAKENING_PREFIXES {
                if let Some(k) = find_third_awakening(&keys, growtype, &format!("{pre}·{base}"), female)
                {
                    out[3] = at(k).to_string();
                    break;
                }
            }
            if out[3].is_empty() && stages.len() > off + 2 && block.len() == 4 {
                out[3] = at(stages[off + 2]).to_string();
            }
            rows.push(out);
        }
        table.insert(family as u8, rows);
    }
    ClassNames { table }
}

/// Go classOfRoleFull + classOfRole + classOf 三级回落。
fn class_of_role_full(
    growtype: &BTreeMap<i64, String>,
    names: &ClassNames,
    profession: i64,
    advancement: i64,
    awakening: i64,
) -> String {
    if let Some(n) = names.name(profession, advancement, awakening) {
        return n.to_string();
    }
    if profession >= 0 && (profession as usize) < PROFESSION_NAMES.len() {
        let slots = PROFESSION_NAMES[profession as usize];
        if advancement >= 0 && (advancement as usize) < slots.len() && !slots[advancement as usize].is_empty() {
            return slots[advancement as usize].to_string();
        }
    }
    growtype
        .get(&profession)
        .map(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("职业 {profession}"))
}

/// Go findThirdAwakening：同名时男取先出现、女取后出现（女阶段排在男之后）。
fn find_third_awakening(
    keys: &[i64],
    growtype: &BTreeMap<i64, String>,
    want: &str,
    female: bool,
) -> Option<i64> {
    let mut hit = None;
    for &k in keys {
        if growtype.get(&k).map(String::as_str) != Some(want) {
            continue;
        }
        if female {
            hit = Some(k);
            continue;
        }
        return Some(k);
    }
    hit
}

// ---------------------------------------------------------------------------
// 加载
// ---------------------------------------------------------------------------

/// Go resolvePaths 的独立发布位（<root>/gm-tool/configs），再加当前整合包的源码位。
/// 拆分布局（根=服务端）时产物在旁支 `../源码/*/gm-tool/configs`，与 config.rs 里
/// `discover_client` 找「../客户端/DFO」是同一个先例。
pub fn find_configs(root: &Path) -> Option<PathBuf> {
    let ok = |d: &Path| d.join("items.index.json").is_file();
    let scan_source_dir = |source_dir: &Path| -> Option<PathBuf> {
        let rd = std::fs::read_dir(source_dir).ok()?;
        let mut cands: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        cands.sort();
        cands
            .into_iter()
            .map(|c| c.join("gm-tool").join("configs"))
            .find(|d| ok(d))
    };
    let direct = root.join("gm-tool").join("configs");
    if ok(&direct) {
        return Some(direct);
    }
    if let Some(found) = scan_source_dir(&root.join("源码")) {
        return Some(found);
    }
    let parent = root.parent()?;
    scan_source_dir(&parent.join("源码"))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let b = std::fs::read(path).map_err(|e| format!("读取 {}：{}", path.display(), e))?;
    serde_json::from_slice(&b).map_err(|e| format!("解析 {}：{}", path.display(), e))
}

/// 名字表只留 GM 用得到的三类前缀键（Go loadNames 的裁剪口径）。
struct NameTable(HashMap<String, String>);

impl<'de> Deserialize<'de> for NameTable {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{MapAccess, Visitor};
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = HashMap<String, String>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("key -> 文本 的名字表")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut out = HashMap::with_capacity(map.size_hint().unwrap_or(0) / 2 + 1);
                while let Some((k, v)) = map.next_entry::<String, String>()? {
                    if k.starts_with(NAME_PREFIX)
                        || k.starts_with(GROW_PREFIX)
                        || k.starts_with(RARITY_PREFIX)
                    {
                        out.insert(k, v);
                    }
                }
                Ok(out)
            }
        }
        deserializer.deserialize_map(V).map(NameTable)
    }
}

/// Go loadNames：文件缺失或非法 JSON 都回落空表（非法时警告一句）。
fn load_names(path: &Path) -> HashMap<String, String> {
    match std::fs::read(path) {
        Ok(b) => match serde_json::from_slice::<NameTable>(&b) {
            Ok(t) => t.0,
            Err(e) => {
                eprintln!("警告：名字表 {} 不是合法 JSON（{e}），本次用物品库自带名称", path.display());
                HashMap::new()
            }
        },
        Err(_) => HashMap::new(),
    }
}

/// Go loadSlotMap/loadJobsMap/loadRarityMap：缺失即空表，非法 JSON 才警告。
fn load_optional<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let b = std::fs::read(path).ok()?;
    match serde_json::from_slice::<T>(&b) {
        Ok(v) => Some(v),
        Err(e) => {
            eprintln!("警告：{} 不是合法 JSON（{e}），本次按缺失处理", path.display());
            None
        }
    }
}

/// Go itemNameUnusable：空串/替换符/??/指向 .equ|.stk 的路径/无任何字母数字，都算不可用。
fn item_name_unusable(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() || t.contains('\u{FFFD}') || t.contains("??") {
        return true;
    }
    let lower = go_lower(t);
    if (lower.contains('/') || lower.contains('\\'))
        && (lower.ends_with(".equ") || lower.ends_with(".stk"))
    {
        return true;
    }
    !t.chars().any(|c| c.is_alphabetic() || c.is_numeric())
}

/// 与 Go strings.ToLower 对齐的逐 rune 简单映射；全 ASCII 快路径零分配。
/// 汉字原样返回 Cow::Borrowed。
pub fn go_lower(s: &str) -> Cow<'_, str> {
    if s.is_ascii() {
        if s.bytes().any(|b| b.is_ascii_uppercase()) {
            return Cow::Owned(s.to_ascii_lowercase());
        }
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    let mut changed = false;
    for c in s.chars() {
        let l = c.to_lowercase().next().unwrap_or(c);
        if l != c {
            changed = true;
        }
        out.push(l);
    }
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(s)
    }
}

/// 目录装载：items.index.json 必须存在，其余旁表全部容忍缺失。
/// 旁表逐份处理 —— 解析 → 套用到条目 → 取指纹 → 立刻释放，四张表不再同时常驻；
/// 留下的只有条目本体和几行小字符串。
pub fn load(dir: &Path) -> Result<Catalog, String> {
    let ItemsFile { items: raw_items, source: items_source, .. } =
        read_json(&dir.join("items.index.json"))?;
    let names = load_names(&dir.join("names.client.json"));
    let names_entries = names.len();
    // 职业名推导只用 growtype_name_* 这一小撮键（几百条），先摘出来，整张名字表照旧用完即弃。
    let mut growtype: BTreeMap<i64, String> = BTreeMap::new();
    for (k, v) in &names {
        if let Some(rest) = k.strip_prefix(GROW_PREFIX) {
            if let Ok(n) = rest.parse::<i64>() {
                growtype.insert(n, v.clone());
            }
        }
    }

    // absorb + push（文件本身按 ID 升序，重复 ID 先到者得，与 Go byID 判重一致）
    let mut out: Vec<GmItem> = Vec::with_capacity(raw_items.len());
    let mut by_id: HashMap<u32, usize> = HashMap::with_capacity(raw_items.len());
    for raw in raw_items {
        if by_id.contains_key(&raw.id) {
            continue;
        }
        let kind = Kind::parse(&raw.kind);
        let artifact_name: Arc<str> = Arc::from(raw.name.as_str());
        let display = names
            .get(&format!("{NAME_PREFIX}{}", raw.id))
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty());
        let mut name = display.map(|s| Arc::from(s)).unwrap_or_else(|| artifact_name.clone());
        if item_name_unusable(&name) {
            name = Arc::from(format!("名称待解析（{}）", raw.id).as_str());
        }
        let (slot, group, type_key, type_text) = match kind {
            Kind::Stackable => {
                let stack_type = if raw.stack_type.is_empty() { &raw.stackable_type } else { &raw.stack_type };
                ("", "", classify_stackable(stack_type), Some(Arc::from(stack_type.as_str())))
            }
            Kind::Equipment => (SLOT_OTHER, GROUP_OTHER, SLOT_OTHER, None),
            Kind::Avatar => (SLOT_AVATAR, GROUP_AVATAR, SLOT_AVATAR, None),
            Kind::Unknown => ("", "", TYPE_OTHER, Some(Arc::from(raw.kind.as_str()))),
        };
        by_id.insert(raw.id, out.len());
        out.push(GmItem {
            id: raw.id,
            name,
            name_en: artifact_name,
            kind,
            grade: raw.grade,
            rarity: raw.rarity,
            level: 0,
            slot,
            group,
            type_key,
            type_text,
            stack_limit: raw.stack_limit,
            durability: raw.durability,
            jobs: None,
        });
    }
    drop(names);

    // refineEquipment：部位表覆盖（无服务端装备目录那一步，Go 里 gear 为 nil 时同样只走 slotMap）。
    let slots = load_optional::<SlotsFile>(&dir.join("equipment.slots.json"));
    let slots_info = slots.as_ref().map(SlotsFile::info).unwrap_or_else(TableInfo::absent);
    {
        let mut cells: HashMap<&str, Arc<str>> = HashMap::new();
        for it in out.iter_mut() {
            if it.kind != Kind::Equipment && it.kind != Kind::Avatar {
                continue;
            }
            if let Some(e) = slots.as_ref().and_then(|f| f.slots.get(&it.id_key())) {
                apply_slot(it, &e.cell, e.level, &mut cells);
            }
        }
    }
    drop(slots);

    // finish 第一步：品质旁表覆盖。
    let rarity = load_optional::<RarityFile>(&dir.join("item.rarity.json"));
    let rarity_info = rarity.as_ref().map(RarityFile::info).unwrap_or_else(TableInfo::absent);
    for it in out.iter_mut() {
        if let Some(r) = rarity.as_ref().and_then(|f| f.rarity.get(&it.id_key())) {
            it.rarity = *r;
        }
    }
    drop(rarity);

    // finish 第二步：职业旁表覆盖。
    let jobs = load_optional::<JobsFile>(&dir.join("equipment.jobs.json"));
    let jobs_info = jobs.as_ref().map(JobsFile::info).unwrap_or_else(TableInfo::absent);
    for it in out.iter_mut() {
        let restricted = jobs.as_ref().and_then(|f| f.jobs.get(&it.id_key()));
        if let Some(j) = restricted.filter(|v| !v.is_empty()) {
            it.jobs = Some(Arc::from(j.as_slice()));
        }
    }
    drop(jobs);

    // finish 第三步：分桶计数（此时只剩条目本体）。
    let mut equipment = 0usize;
    let mut stackable = 0usize;
    let mut facet_types = vec![0usize; ORDERED_TYPES.len()];
    let mut facet_slots = vec![0usize; EQUIPMENT_SLOT_CELLS.len() + 2];
    let mut facet_rarities = vec![0usize; RARITY_LABELS.len()];
    let mut facet_bands = vec![0usize; LEVEL_BANDS.len()];
    for it in out.iter() {
        if it.kind == Kind::Equipment {
            equipment += 1;
        }
        if it.kind == Kind::Stackable {
            stackable += 1;
        }
        let bucket = match it.kind {
            Kind::Equipment => ORDERED_TYPES.iter().position(|(k, _)| *k == TYPE_EQUIPMENT).unwrap(),
            _ => ORDERED_TYPES
                .iter()
                .position(|(k, _)| *k == if it.type_key.is_empty() { TYPE_OTHER } else { it.type_key })
                .unwrap_or(ORDERED_TYPES.len() - 1),
        };
        facet_types[bucket] += 1;
        if matches!(it.kind, Kind::Equipment | Kind::Avatar) {
            if let Some(i) = EQUIPMENT_SLOT_CELLS.iter().position(|(_, s, _)| *s == it.slot) {
                facet_slots[i] += 1;
            } else if it.slot == SLOT_AVATAR {
                facet_slots[EQUIPMENT_SLOT_CELLS.len()] += 1;
            } else {
                facet_slots[EQUIPMENT_SLOT_CELLS.len() + 1] += 1;
            }
        }
        if is_valid_rarity(it.rarity) {
            facet_rarities[it.rarity as usize] += 1;
        }
        if it.level > 0 {
            for (i, (_, _, min, max)) in LEVEL_BANDS.iter().enumerate() {
                if (*min == 0 || it.level >= *min) && (*max == 0 || it.level <= *max) {
                    facet_bands[i] += 1;
                    break;
                }
            }
        }
    }

    // 空条件命中集用同一份谓词算出来，之后每次「不带条件」的查询直接复用。
    let default_hits = search_indices(out.iter().map(GmView::from), &Query::default());

    let class_names = build_class_names(&growtype);
    let status = CatalogStatus {
        dir: dir.display().to_string(),
        items_source,
        count: out.len(),
        equipment,
        stackable,
        slots: slots_info,
        jobs: jobs_info,
        rarity: rarity_info,
        names: TableInfo {
            present: names_entries > 0,
            entries: names_entries,
            source: String::new(),
            built_at: String::new(),
        },
    };
    Ok(Catalog {
        items: out,
        by_id,
        status,
        default_hits,
        growtype,
        class_names,
        facet_types,
        facet_slots,
        facet_rarities,
        facet_bands,
    })
}

/// Go applySlot：部位/大类/分类 key 全用表里的 cell 文本，等级只补 0 值。
fn apply_slot<'a>(
    it: &mut GmItem,
    cell: &'a str,
    level: i32,
    cells: &mut HashMap<&'a str, Arc<str>>,
) {
    let (slot, group) = slot_of_equipment_cell(cell);
    it.slot = slot;
    it.group = group;
    it.type_key = slot;
    let shared = cells.entry(cell).or_insert_with(|| Arc::from(cell)).clone();
    if it.type_text.is_none() {
        it.type_text = Some(shared);
    }
    if it.level == 0 && level > 0 {
        it.level = level;
    }
}

// ---------------------------------------------------------------------------
// 查询（Go ItemQuery 语义）
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct Query {
    pub q: String,
    pub type_: String,
    pub slot: String,
    pub level_min: i32,
    pub level_max: i32,
    pub rarity: Option<i32>,
}

#[derive(Clone, Debug)]
struct ItemQuery {
    empty: bool,
    wildcard: bool,
    /// 小写子串或通配模式；empty 时无意义。
    text: String,
}

/// Go parseItemQuery：空串或纯 '*' 视为无关键字。
fn parse_item_query(raw: &str) -> ItemQuery {
    let raw = raw.trim();
    if raw.is_empty() || raw.chars().all(|c| c == '*') {
        return ItemQuery { empty: true, wildcard: false, text: String::new() };
    }
    let lowered = go_lower(raw).into_owned();
    if raw.contains('*') {
        return ItemQuery { empty: false, wildcard: true, text: lowered };
    }
    ItemQuery { empty: false, wildcard: false, text: lowered }
}

/// Go matchWildcard：'*' 匹配任意长度（含空），模式两端锚定，线性回溯。
pub fn match_wildcard(pattern: &str, s: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), s.as_bytes());
    let (mut pi, mut si) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while si < t.len() {
        if pi < p.len() && p[pi] == b'*' {
            star = pi;
            mark = si;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[si] {
            pi += 1;
            si += 1;
        } else if star != usize::MAX {
            mark += 1;
            pi = star + 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

/// Go itemQuery.matchesItem 的通配分支：逐字段匹配，id 也算。
fn wildcard_matches_item(it: &GmView<'_>, pattern: &str) -> bool {
    match_wildcard(pattern, &go_lower(it.name))
        || match_wildcard(pattern, &go_lower(it.name_en))
        || match_wildcard(pattern, &it.id.to_string())
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// 搜索谓词的借用视图：条目本体和调用方自己的行结构都能映射过来，
/// 这样「一套语义」只有一个实现（Go 侧同样是 index.Search 直接吃条目）。
#[derive(Clone, Copy)]
pub struct GmView<'a> {
    pub id: u32,
    pub name: &'a str,
    pub name_en: &'a str,
    pub kind: Kind,
    pub grade: i32,
    pub rarity: i32,
    pub level: i32,
    pub slot: &'a str,
    pub type_key: &'a str,
}

impl<'a> From<&'a GmItem> for GmView<'a> {
    fn from(it: &'a GmItem) -> GmView<'a> {
        GmView {
            id: it.id,
            name: &it.name,
            name_en: &it.name_en,
            kind: it.kind,
            grade: it.grade,
            rarity: it.rarity,
            level: it.level,
            slot: it.slot,
            type_key: it.type_key,
        }
    }
}

/// Go inType：分类三层语义（页签 / 装备大类 / 装备位）。
pub fn in_type_of(it: &GmView<'_>, type_key: &str) -> bool {
    if type_key.is_empty() {
        return true;
    }
    if it.kind == Kind::Equipment {
        if type_key == TYPE_EQUIPMENT {
            return true;
        }
        if it.slot == type_key || it.type_key == type_key {
            return true;
        }
        return slot_group_of(it.slot) == type_key;
    }
    let key = if it.type_key.is_empty() { TYPE_OTHER } else { it.type_key };
    key == type_key
}

/// Go match：分类、部位、等级（给等级条件时排除未知等级）、品级。
pub fn item_matches(it: &GmView<'_>, f: &Query) -> bool {
    if !in_type_of(it, f.type_.as_str()) {
        return false;
    }
    if !f.slot.is_empty() {
        if it.kind != Kind::Equipment {
            return false;
        }
        let mut want = f.slot.as_str();
        if want.starts_with('[') {
            let (s, _) = slot_of_equipment_cell(want);
            if !s.is_empty() {
                want = s;
            }
        }
        if it.slot != want {
            return false;
        }
    }
    if f.level_min > 0 || f.level_max > 0 {
        if it.level <= 0 {
            return false;
        }
        if f.level_min > 0 && it.level < f.level_min {
            return false;
        }
        if f.level_max > 0 && it.level > f.level_max {
            return false;
        }
    }
    if let Some(r) = f.rarity {
        if it.rarity != r {
            return false;
        }
    }
    true
}

/// Go defaultGate 里「一个条件都没给」的判据。目录装载时预计算空条件命中也走这一条，
/// 免得判据在两处各写一遍再漂移。
pub fn is_default_query(raw: &Query) -> bool {
    parse_item_query(&raw.q).empty
        && raw.type_.trim().is_empty()
        && raw.slot.trim().is_empty()
        && raw.level_min == 0
        && raw.level_max == 0
        && raw.rarity.is_none()
}

/// Go Search 主体：对任意可重播的借用视图序列返回命中下标，顺序与 Go 一致。
/// 收迭代器而不是切片，是为了让 67 万条目录不必每次查询先物化一份视图数组；
/// 两遍命中的分支靠 `Clone` 把同一个迭代器重播，对拍测试用 `iter().copied()` 走切片。
pub fn search_indices<'a, I>(items: I, raw: &Query) -> Vec<usize>
where
    I: Clone + IntoIterator<Item = GmView<'a>>,
{
    let q = raw.q.trim().to_string();
    let parsed = parse_item_query(&q);
    let q_eff = if parsed.empty { String::new() } else { q.clone() };
    let type_ = raw.type_.trim().to_string();
    let slot = raw.slot.trim().to_string();
    let default_gate = is_default_query(raw);
    let f = Query {
        q: q_eff.clone(),
        type_,
        slot,
        level_min: raw.level_min,
        level_max: raw.level_max,
        rarity: raw.rarity,
    };

    let mut out: Vec<usize> = Vec::new();
    if parsed.wildcard {
        for (i, it) in items.clone().into_iter().enumerate() {
            if item_matches(&it, &f) && wildcard_matches_item(&it, &parsed.text) {
                out.push(i);
            }
        }
        return out;
    }
    if q_eff.is_empty() || is_digits(&q_eff) {
        for (i, it) in items.into_iter().enumerate() {
            if !item_matches(&it, &f) {
                continue;
            }
            if default_gate {
                // Go：一个条件都没给时只列低等级、最可能想发的一批。
                if it.level > 10 || it.grade > 3 {
                    continue;
                }
            } else if !q_eff.is_empty() && !it.id.to_string().starts_with(&q_eff) {
                continue;
            }
            out.push(i);
        }
        return out;
    }
    let needle = parsed.text.as_str();
    let mut hit: HashMap<u32, bool> = HashMap::new();
    for pass in 0..2 {
        for (i, it) in items.clone().into_iter().enumerate() {
            if !item_matches(&it, &f) || hit.get(&it.id).copied().unwrap_or(false) {
                continue;
            }
            let n = go_lower(it.name);
            let ne = go_lower(it.name_en);
            let ok = if pass == 0 {
                n.starts_with(needle) || ne.starts_with(needle)
            } else {
                n.contains(needle) || ne.contains(needle)
            };
            if ok {
                hit.insert(it.id, true);
                out.push(i);
            }
        }
    }
    out
}

impl Catalog {
    pub fn get(&self, id: u32) -> Option<&GmItem> {
        self.by_id.get(&id).map(|i| &self.items[*i])
    }

    /// Go classOfRoleFull：推导表优先，回落到家族表基名，最后才是「职业 N」。
    /// Go 的 classOf 还会再查中文/英文名字表，那两个产物本工具不生成，
    /// 少一层回落只会让极少数角色的名字退化成基名，不会报错。
    pub fn class_of_role_full(&self, profession: i64, advancement: i64, awakening: i64) -> String {
        class_of_role_full(&self.growtype, &self.class_names, profession, advancement, awakening)
    }

    /// 全量条目（按 ID 升序）。
    pub fn iter(&self) -> impl Iterator<Item = &GmItem> {
        self.items.iter()
    }

    /// 全量命中（按 Go Search 的返回顺序），分页/截断由调用方做。
    pub fn search(&self, raw: Query) -> Vec<&GmItem> {
        if is_default_query(&raw) {
            // 装载时算好的那一批；判据仍然是 search_indices 那一份。
            return self.default_hits.iter().map(|i| &self.items[*i]).collect();
        }
        let hits = search_indices(self.items.iter().map(GmView::from), &raw);
        hits.into_iter().map(|i| &self.items[i]).collect()
    }

    pub fn facets(&self) -> Facets {
        let types = ORDERED_TYPES
            .iter()
            .enumerate()
            .map(|(i, (key, label))| Facet {
                key: (*key).to_string(),
                label: (*label).to_string(),
                count: self.facet_types[i],
            })
            .collect();
        let mut slots: Vec<Facet> = EQUIPMENT_SLOT_CELLS
            .iter()
            .enumerate()
            .map(|(i, (_, slot, _))| Facet {
                key: (*slot).to_string(),
                label: (*slot).to_string(),
                count: self.facet_slots[i],
            })
            .collect();
        slots.push(Facet {
            key: SLOT_AVATAR.to_string(),
            label: SLOT_AVATAR.to_string(),
            count: self.facet_slots[EQUIPMENT_SLOT_CELLS.len()],
        });
        slots.push(Facet {
            key: SLOT_OTHER.to_string(),
            label: SLOT_OTHER.to_string(),
            count: self.facet_slots[EQUIPMENT_SLOT_CELLS.len() + 1],
        });
        let rarities = RARITY_LABELS
            .iter()
            .enumerate()
            .map(|(i, label)| Facet {
                key: i.to_string(),
                label: (*label).to_string(),
                count: self.facet_rarities[i],
            })
            .collect();
        let level_bands = LEVEL_BANDS
            .iter()
            .enumerate()
            .map(|(i, (key, label, _, _))| Facet {
                key: (*key).to_string(),
                label: (*label).to_string(),
                count: self.facet_bands[i],
            })
            .collect();
        Facets { types, slots, rarities, level_bands }
    }
}

// ---------------------------------------------------------------------------
// 单测
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_anchor_and_star() {
        assert!(match_wildcard("*巨剑*", "gnb 巨剑-改"));
        assert!(match_wildcard("巨剑*", "巨剑时代"));
        assert!(!match_wildcard("巨剑*", "旧巨剑"));
        assert!(!match_wildcard("*100", "10018"));
        assert!(match_wildcard("*100", "90100"));
        assert!(!match_wildcard("100", "10018"));
        assert!(match_wildcard("100*", "10018"));
        assert!(match_wildcard("**", "任何东西"));
    }

    #[test]
    fn slot_cell_lookup() {
        assert_eq!(slot_of_equipment_cell("[weapon]"), ("武器", "武器"));
        assert_eq!(slot_of_equipment_cell("[magic stone]"), ("魔法石", "特殊装备"));
        assert_eq!(slot_of_equipment_cell("[hat avatar]"), ("时装", "时装"));
        assert_eq!(slot_of_equipment_cell("[cross]"), ("其它装备", "其它"));
        assert_eq!(slot_group_of("上衣"), "防具");
        assert_eq!(slot_group_of("武器"), "武器");
    }

    #[test]
    fn classify_and_display_labels() {
        assert_eq!(classify_stackable("[waste]"), TYPE_CONSUMABLE);
        assert_eq!(classify_stackable("[material]"), TYPE_MATERIAL);
        assert_eq!(classify_stackable("[never-seen]"), TYPE_CONSUMABLE);
        assert_eq!(stackable_display_label("[booster selection]"), "自选礼盒");
        assert_eq!(stackable_display_label(""), "未标注类型");
        assert_eq!(stackable_display_label("[x]"), "未识别类型 [x]");
    }

    #[test]
    fn lower_helper() {
        assert_eq!(go_lower("ABC dеf"), "abc dеf");
        assert_eq!(go_lower("巨剑+12"), "巨剑+12");
    }

    #[test]
    fn unusable_name_shapes() {
        assert!(item_name_unusable(""));
        assert!(item_name_unusable("??"));
        assert!(item_name_unusable("equipment/abc.equ"));
        assert!(item_name_unusable("-- ** --"));
        assert!(!item_name_unusable("(旧) 古老的羊毛背心"));
    }

    #[test]
    fn side_tables_missing_are_tolerated() {
        let dir = std::env::temp_dir().join(format!("gmindex-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("items.index.json"),
            "{\"count\":3,\"items\":[\n{\"id\":961,\"name\":\"测试药水\",\"kind\":\"stackable\",\"grade\":0,\"rarity\":-1,\"stack_type\":\"[waste]\",\"stack_limit\":100},\n{\"id\":10018,\"name\":\"(旧) 古老的羊毛背心\",\"kind\":\"equipment\",\"grade\":0,\"rarity\":-1},\n{\"id\":10019,\"name\":\"时装盒\",\"kind\":\"avatar\",\"grade\":0,\"rarity\":-1}\n],\"source\":\"abc\"}"
                .as_bytes(),
        )
        .unwrap();
        let cat = load(&dir).unwrap();
        assert_eq!(cat.status.count, 3);
        assert_eq!(cat.status.slots.entries, 0);
        let it = cat.get(10018).unwrap();
        assert_eq!(it.slot, SLOT_OTHER);
        assert_eq!(it.level, 0);
        // 无旁表：时装仍占时装桶，其余物品按 [all]（jobs=None）处理。
        assert_eq!(cat.get(961).unwrap().type_key, TYPE_CONSUMABLE);
        assert_eq!(cat.get(10019).unwrap().slot, SLOT_AVATAR);
        // defaultGate：没给条件只列低等级档（level<=10 && grade<=3）。
        let all = cat.search(Query::default());
        assert_eq!(all.len(), 3);
        // 名字回落：无 names.client.json 时沿用物品库自带名称。
        assert_eq!(&*cat.get(961).unwrap().name, "测试药水");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 时装判据 `avatar()` 的两条来源：现网这一代 `items.index.json` 的 `kind` 只有
    /// stackable/equipment 两种值，时装**全靠旁表把 `slot` 覆盖成「时装」**来认；
    /// `kind` 标 avatar 只算兜底。普通部位（`[weapon]`、没进旁表的）都不算。
    #[test]
    fn avatar_predicate_reads_the_slot_overlay_not_only_kind() {
        let dir = std::env::temp_dir().join(format!("gmindex-avatar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("items.index.json"),
            "{\"count\":5,\"items\":[\n{\"id\":961,\"name\":\"测试药水\",\"kind\":\"stackable\",\"grade\":0,\"rarity\":-1,\"stack_type\":\"[waste]\"},\n{\"id\":10019,\"name\":\"时装盒\",\"kind\":\"avatar\",\"grade\":0,\"rarity\":-1},\n{\"id\":39000,\"name\":\"帽子时装\",\"kind\":\"equipment\",\"grade\":0,\"rarity\":-1},\n{\"id\":29201,\"name\":\"发型时装\",\"kind\":\"equipment\",\"grade\":0,\"rarity\":-1},\n{\"id\":10018,\"name\":\"一把武器\",\"kind\":\"equipment\",\"grade\":0,\"rarity\":-1}\n]}"
                .as_bytes(),
        )
        .unwrap();
        std::fs::write(
            dir.join("equipment.slots.json"),
            "{\"source\":\"s\",\"built_at\":\"b\",\"slots\":{\"39000\":{\"cell\":\"[hat avatar]\",\"level\":0},\"29201\":{\"cell\":\"[hair avatar]\",\"level\":0},\"10018\":{\"cell\":\"[weapon]\",\"level\":10}}}"
                .as_bytes(),
        )
        .unwrap();
        let cat = load(&dir).unwrap();
        assert_eq!(cat.get(39000).unwrap().slot, SLOT_AVATAR);
        assert!(cat.get(39000).unwrap().avatar(), "旁表认出的是时装，kind 却是 equipment");
        assert!(cat.get(29201).unwrap().avatar());
        assert!(cat.get(10019).unwrap().avatar(), "kind 标 avatar 那一支也要认");
        assert!(!cat.get(10018).unwrap().avatar(), "普通部位不能算时装");
        assert!(!cat.get(961).unwrap().avatar(), "可堆叠更不能算时装");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_semantics_prefix_then_substring_and_digits() {        let dir = std::env::temp_dir().join(format!("gmindex-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("items.index.json"),
            "{\"count\":4,\"items\":[\n{\"id\":500,\"name\":\"力量药水\",\"kind\":\"stackable\",\"grade\":0,\"rarity\":-1,\"stack_type\":\"[waste]\"},\n{\"id\":5001,\"name\":\"巨剑-力量之戒\",\"kind\":\"equipment\",\"grade\":0,\"rarity\":-1},\n{\"id\":10018,\"name\":\"力量腰带\",\"kind\":\"equipment\",\"grade\":0,\"rarity\":-1},\n{\"id\":20018,\"name\":\"史诗肩甲\",\"kind\":\"equipment\",\"grade\":0,\"rarity\":4}\n]}"
                .as_bytes(),
        )
        .unwrap();
        std::fs::write(
            dir.join("equipment.slots.json"),
            "{\"source\":\"s\",\"built_at\":\"b\",\"slots\":{\"5001\":{\"cell\":\"[weapon]\",\"level\":10},\"10018\":{\"cell\":\"[waist]\",\"level\":20},\"20018\":{\"cell\":\"[shoulder]\",\"level\":40}}}"
                .as_bytes(),
        )
        .unwrap();
        let cat = load(&dir).unwrap();

        let by_prefix = cat.search(Query { q: "力量".into(), ..Default::default() });
        // 前缀命中优先：力量药水(500)、力量腰带(10018)；子串命中：巨剑-力量之戒。
        assert_eq!(
            by_prefix.iter().map(|i| i.id).collect::<Vec<_>>(),
            vec![500, 10018, 5001]
        );

        let digits = cat.search(Query { q: "500".into(), ..Default::default() });
        assert_eq!(digits.iter().map(|i| i.id).collect::<Vec<_>>(), vec![500, 5001]);

        let weapon = cat.search(Query { slot: "[weapon]".into(), ..Default::default() });
        assert_eq!(weapon.len(), 1);
        assert_eq!(weapon[0].slot, "武器");
        assert_eq!(weapon[0].level, 10);

        let band = cat.search(Query { level_min: 15, level_max: 25, ..Default::default() });
        assert_eq!(band.iter().map(|i| i.id).collect::<Vec<_>>(), vec![10018]);

        let rarity4 = cat.search(Query { rarity: Some(4), ..Default::default() });
        assert_eq!(rarity4.len(), 1);

        let wildcard = cat.search(Query { q: "*腰".into(), ..Default::default() });
        assert!(wildcard.iter().all(|i| !i.name.contains("腰带")));
        assert_eq!(cat.search(Query { q: "*腰带".into(), ..Default::default() }).len(), 1);

        let facets = cat.facets();
        assert_eq!(facets.types.iter().find(|f| f.key == TYPE_EQUIPMENT).unwrap().count, 3);
        assert_eq!(facets.slots.iter().find(|f| f.key == "武器").unwrap().count, 1);
        assert_eq!(facets.rarities.iter().find(|f| f.key == "4").unwrap().count, 1);

        // 视图切片路径必须与 Catalog::search（内部走惰性迭代器）逐下标一致。
        let views: Vec<GmView<'_>> = cat.iter().map(GmView::from).collect();
        for raw in [
            Query { q: "力量".into(), ..Default::default() },
            Query { q: "500".into(), ..Default::default() },
            Query { slot: "[weapon]".into(), ..Default::default() },
            Query { q: "*腰带".into(), ..Default::default() },
            Query::default(),
        ] {
            let via_view = search_indices(views.iter().copied(), &raw);
            let via_catalog = cat.search(raw).iter().map(|i| i.id).collect::<Vec<_>>();
            assert_eq!(
                via_view.iter().map(|i| views[*i].id).collect::<Vec<_>>(),
                via_catalog
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn grow(pairs: &[(i64, &str)]) -> BTreeMap<i64, String> {
        pairs.iter().map(|(k, v)| (*k, v.to_string())).collect()
    }

    #[test]
    fn class_names_blocks_and_third_awakening() {
        // classnames.go 注释里的实测形状：30 漫游枪手 + 男两阶段 + 女两阶段，整块 5 键。
        let cn = build_class_names(&grow(&[
            (7, "//待定"),
            (8, ""),
            (30, "漫游枪手"),
            (31, "枪神"),
            (32, "掠天之翼"),
            (33, "沾血蔷薇"),
            (34, "绯红玫瑰"),
            (400, "刃影"),
            (401, "斩夜"),
            (402, "夜皇"),
            (403, "极诣·刃影"),
        ]));
        assert_eq!(cn.name(2, 1, 1), Some("枪神"));
        assert_eq!(cn.name(2, 1, 2), Some("掠天之翼"));
        // 女性系共用基名时取块内偏移 2（只有 5 键块才说明男女阶段分列）。
        assert_eq!(cn.name(5, 1, 1), Some("沾血蔷薇"));
        assert_eq!(cn.name(5, 1, 2), Some("绯红玫瑰"));
        // 三次觉醒名不在本块也要按「极诣·刃影」全局命中。
        assert_eq!(cn.name(11, 5, 3), Some("极诣·刃影"));
        assert_eq!(cn.name(11, 5, 1), Some("斩夜"));
        // 占位与空名不进块；查不到块的家族槽仍填旧表基名，所以名字非空。
        assert_eq!(cn.name(9, 0, 0), Some("黑暗武士"));
        assert_eq!(cn.name(9, 3, 1), None);
        // 越界：觉醒阶段按 0、槽位越界与家族越界都无名字。
        assert_eq!(cn.name(2, 1, 9), Some("漫游枪手"));
        assert_eq!(cn.name(2, 9, 0), None);
        assert_eq!(cn.name(99, 0, 0), None);
    }

    #[test]
    fn class_name_falls_back_to_family_table() {
        let empty = BTreeMap::new();
        let none = ClassNames::default();
        let full = build_class_names(&grow(&[(400, "刃影"), (401, "斩夜")]));
        // 名字表缺席时按 gmProfessionNames 的 (大类, 转职槽) 取基名。
        assert_eq!(class_of_role_full(&empty, &none, 11, 5, 0), "刃影");
        assert_eq!(class_of_role_full(&empty, &none, 0, 1, 0), "剑魂");
        // 推导表命中时优先于基名。
        assert_eq!(class_of_role_full(&empty, &full, 11, 5, 1), "斩夜");
        // 槽位越界回落到 growtype 大类名，再不行才编号。
        assert_eq!(class_of_role_full(&grow(&[(11, "女鬼剑士")]), &none, 11, 9, 0), "女鬼剑士");
        assert_eq!(class_of_role_full(&empty, &none, 99, 0, 0), "职业 99");
    }
}
