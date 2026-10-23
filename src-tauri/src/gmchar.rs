//! GM 的角色详情与背包浏览（只读，一个字节都不写）。
//!
//! 口径抄自 Go gmtool：`handleCharacter`（`/api/character`）与 `bridge_inventory.go`
//! 的 `readGMInventory / gmInventoryEntry / bagEntry`。背包行的 `revision` 是那一行
//! 字段按键名排序重新序列化后的 SHA-256（Go 同算法），后面的「背包编辑」拿它做乐观锁：
//! 数量或属性只要变过，修订号就对不上，编辑直接被拒而不是覆盖别人的改动。
//! 名字表里查不到的物品保留「物品 N」占位名（Go 同口径），分类留空。

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Local};
use gmbuild::gmindex::{self, Catalog, Kind};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::gmgrant::BagRules;
use crate::paths::Root;

const BAG_VERSION: &str = "ordinary-bag-v1";
/// Go readGMInventory 依次展开的容器：普通三张表 + 时装(1)/宠物(7) 两个特殊空间。
/// 浏览与编辑共用这一个顺序，编辑才认得「第 li 张表」在 state 里的哪条小表。
const LIST_ORDER: [(&str, u8); 5] = [
    ("items", 0),
    ("equipment", 0),
    ("worn", 0),
    ("special_equipment", 1),
    ("special_equipment", 7),
];

#[derive(Serialize)]
pub struct GmCharacterHead {
    pub id: i64,
    pub account_id: i64,
    pub wire_id: i64,
    pub name: String,
    pub profession: i64,
    /// 由客户端职业名表推导（gmindex 的 class_of_role_full）。
    pub class_name: String,
    pub advancement: u8,
    pub awakening: u8,
    pub level: u8,
    pub experience: u64,
    pub gold: u32,
    pub created_at: String,
}

/// Go handleCharacter 里那份 summary：缺键按零值，与原结构体的反序列化一致。
#[derive(Serialize)]
pub struct GmCharacterSummary {
    pub level: u8,
    pub experience: u64,
    pub skill_points: [u16; 2],
    pub technique_points: [u16; 2],
    pub currency_slot2: u32,
    pub advancement: u8,
    pub awakening: u8,
    /// 原生装备栏标志：第 0/1/4 位分别是辅助装备、魔法石、耳环。
    pub equipment_slot_flags: u8,
    /// 原样透传的属性表（键是 PVF 段名，值是浮点或整数）。
    pub attributes: Value,
    pub learned_skills: Value,
    pub skill_slots: Value,
}

#[derive(Debug, Serialize)]
pub struct GmBagRow {
    pub collection: String,
    pub space: u8,
    pub slot: u16,
    pub group: u8,
    pub template: u32,
    pub name: String,
    pub count: u32,
    pub worn: bool,
    pub revision: String,
    pub can_edit_count: bool,
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub stack_limit: u32,
    /// 装备行的当前耐久（`equipment` / `worn` 两张表才有）。
    /// 读侧只是把这一键取出来给界面，写侧（`equipment.durability`）拿它当乐观锁的一半。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub durability: Option<u32>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub type_label: String,
    pub rarity: i32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub rarity_label: String,
    pub level: i32,
}

#[derive(Serialize)]
pub struct GmCharacterDetail {
    pub character: GmCharacterHead,
    pub cera: i64,
    pub summary: GmCharacterSummary,
    pub bag: Vec<GmBagRow>,
    /// 背包容器解不出来时不整体失败：照 Go 的错误原文报给界面。
    pub bag_error: String,
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

/// Go `json.Unmarshal` 进 byte/uint16/uint32/uint64：null、非数字、越界都算失败。
fn as_number(v: &Value, max: u64) -> Option<u64> {
    let n = v
        .as_u64()
        .or_else(|| v.as_i64().filter(|i| *i >= 0).map(|i| i as u64))?;
    (n <= max).then_some(n)
}

/// summary 用的宽松版：解不出来按零值，与「缺键即零值」同口径。
fn as_uint(v: Option<&Value>, max: u64) -> u64 {
    v.and_then(|x| as_number(x, max)).unwrap_or(0)
}

fn as_pair(v: Option<&Value>) -> [u16; 2] {
    let mut out = [0u16; 2];
    if let Some(Value::Array(items)) = v {
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = items.get(i).and_then(|x| x.as_u64()).unwrap_or(0).min(u16::MAX as u64) as u16;
        }
    }
    out
}

/// Go readGMInventory 的容器展开：返回 (容器名, 空间, 原始行)。
/// 这一步刻意只认容器边界，不碰行内字段，行由 `bag_row_of` 逐条校验。
fn read_containers(state: &Value) -> Result<Vec<(String, u8, Vec<Value>)>, String> {
    let top = state
        .as_object()
        .ok_or_else(|| "角色存档格式无效".to_string())?;
    let mut out = Vec::new();
    let Some(raw_bag) = top.get("inventory") else {
        return Ok(out);
    };
    if raw_bag.is_null() {
        return Err("角色没有可识别的背包存档".into());
    }
    let bag = raw_bag
        .as_object()
        .ok_or_else(|| "角色没有可识别的背包存档".to_string())?;
    if bag.get("version").and_then(|v| v.as_str()) != Some(BAG_VERSION) {
        return Err("不支持的背包存档版本".into());
    }
    // Go 先把普通三张表进列表，才碰 special_equipment（那张表解不开要单独报「时装或宠物…」），
    // 所以特殊空间的名单在第一次用到时才展开。
    let mut special: BTreeMap<String, Value> = BTreeMap::new();
    let mut special_opened = false;
    for (name, space) in LIST_ORDER {
        let raw = if name == "special_equipment" {
            if !special_opened {
                match bag.get("special_equipment") {
                    None | Some(Value::Null) => {}
                    Some(v) => {
                        let map = v
                            .as_object()
                            .ok_or_else(|| "时装或宠物容器格式无效".to_string())?;
                        for (k, item) in map {
                            special.insert(k.clone(), item.clone());
                        }
                    }
                }
                special_opened = true;
            }
            special.get(&space.to_string())
        } else {
            bag.get(name)
        };
        let rows = match raw {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(rows)) => rows.clone(),
            Some(_) => return Err(format!("背包容器 {name} 格式无效")),
        };
        out.push((name.to_string(), space, rows));
    }
    Ok(out)
}

/// Go gmInventoryEntry：只认 slot/template/group/amount 四个字段，
/// 重复键、null 值、类型不对都整行报错（宁可不显示，也不给半个可信的背包）。
fn bag_row_of(collection: &str, space: u8, raw: &Value) -> Result<GmBagRow, String> {
    let fields = raw
        .as_object()
        .ok_or_else(|| "背包物品记录格式无效".to_string())?;
    let mut entry = GmBagRow {
        collection: collection.to_string(),
        space,
        slot: 0,
        group: 0,
        template: 0,
        name: String::new(),
        count: 1,
        worn: collection == "worn",
        revision: String::new(),
        can_edit_count: false,
        stack_limit: 0,
        durability: None,
        kind: String::new(),
        type_label: String::new(),
        rarity: 0,
        rarity_label: String::new(),
        level: 0,
    };
    let mut seen: Vec<String> = Vec::new();
    for (key, value) in fields {
        let name = key.to_lowercase();
        // 与 Go 一致：只有这四个字段会被认，其它键既不校验也不参与重复判定，
        // 但整行原样进修订号。
        let field = match name.as_str() {
            "slot" | "template" | "group" => name.as_str(),
            "amount" if collection == "items" => "amount",
            _ => continue,
        };
        let ok = match field {
            "slot" => as_number(value, u16::MAX as u64).map(|n| entry.slot = n as u16).is_some(),
            "template" => as_number(value, u32::MAX as u64).map(|n| entry.template = n as u32).is_some(),
            "group" => as_number(value, u8::MAX as u64).map(|n| entry.group = n as u8).is_some(),
            _ => as_number(value, u32::MAX as u64).map(|n| entry.count = n as u32).is_some(),
        };
        if !ok || seen.iter().any(|s| s == &name) {
            return Err(format!("背包物品字段 {key} 无效或重复"));
        }
        seen.push(name);
    }
    let has = |k: &str| seen.iter().any(|s| s == k);
    if !has("slot") || entry.template == 0 || entry.count == 0 || (collection == "items" && !has("amount")) {
        return Err("背包物品缺少位置、模板或数量".into());
    }
    entry.name = format!("物品 {}", entry.template);
    entry.revision = revision_of(fields);
    // 耐久刻意不并进上面那个「只认四键」的循环：那一圈还兼管重复与合法性判定，
    // 而 Go 的 bagEntry 读耐久却不因为它重复就报错。解不出来就当这一行没有耐久字段，
    // 界面不给输入，写侧也拒绝改这一行。
    if matches!(collection, "equipment" | "worn") {
        entry.durability = fields
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("durability"))
            .and_then(|(_, v)| as_number(v, u16::MAX as u64))
            .map(|n| n as u32);
    }
    Ok(entry)
}

/// Go `json.Marshal(map[string]json.RawMessage)` 的规范形：键名升序、无空白。
/// 修订号只在本工具的一次读→一次写之间自洽，所以这里用 serde_json 的紧凑输出。
/// 写侧（编辑与它的测试）也走这一个函数，免得两处算出两个指纹。
pub(crate) fn revision_of(fields: &serde_json::Map<String, Value>) -> String {
    let sorted: BTreeMap<&String, &Value> = fields.iter().collect();
    let canonical = serde_json::to_vec(&sorted).unwrap_or_default();
    let mut h = Sha256::new();
    h.update(&canonical);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Go bagEntry：物品库里有就补名称、分类与品质。
fn enrich(cat: &Catalog, missing_stack_limit: u32, row: &mut GmBagRow) {
    let Some(item) = cat.get(row.template) else {
        return;
    };
    if !item.name.is_empty() {
        row.name = item.name.to_string();
    }
    row.kind = item.kind.as_str().to_string();
    row.type_label = gmindex::type_label(item.type_key).to_string();
    row.rarity = item.rarity;
    row.rarity_label = gmindex::rarity_label(item.rarity).to_string();
    row.level = item.level;
    if row.collection == "items" && item.kind == Kind::Stackable {
        // Go 还回落服务端 loot 目录与背包规则；产物里没带上限时用规则的缺省值。
        row.stack_limit = if item.stack_limit != 0 { item.stack_limit } else { missing_stack_limit };
        row.can_edit_count = row.stack_limit > 0;
    }
}

fn summary_of(state: &Value) -> GmCharacterSummary {
    GmCharacterSummary {
        level: as_uint(state.get("level"), u8::MAX as u64) as u8,
        experience: as_uint(state.get("experience"), u64::MAX),
        skill_points: as_pair(state.get("skill_points")),
        technique_points: as_pair(state.get("technique_points")),
        currency_slot2: as_uint(state.get("currency_slot2"), u32::MAX as u64) as u32,
        advancement: as_uint(state.get("advancement"), u8::MAX as u64) as u8,
        awakening: as_uint(state.get("awakening"), u8::MAX as u64) as u8,
        equipment_slot_flags: as_uint(state.get("equipment_slot_flags"), u8::MAX as u64) as u8,
        attributes: state.get("attributes").cloned().unwrap_or(Value::Null),
        learned_skills: state.get("learned_skills").cloned().unwrap_or(Value::Null),
        skill_slots: state.get("skill_slots").cloned().unwrap_or(Value::Null),
    }
}

fn gold_of(state: &Value) -> u32 {
    as_uint(state.get("inventory").and_then(|v| v.get("gold")), u32::MAX as u64) as u32
}

/// 只读连接：浏览不该因为服务端在跑而失败，也不该留下任何锁。
pub(crate) fn open_ro(db: &Path) -> Result<rusqlite::Connection, String> {
    rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("打不开存档（{}）：{e}", db.display()))
}

/// `/api/character` 的本体：账号 + 角色双重定位（Go 只认角色 id，这里更严一档），
/// 背包解不出来只记 `bag_error`，角色头部照旧返回。
pub fn detail(root: &Root, cat: &Catalog, account: i64, character: i64) -> Result<GmCharacterDetail, String> {
    let db = crate::saves::current_db(root)
        .ok_or_else(|| "存档还没有 SQLite 档：先启动一次游戏".to_string())?;
    // 规则文件只用来补「产物里没带上限」的可堆叠数量，读不到就按 0（该行为不可改数量）。
    let missing = gmindex::find_configs(&root.path)
        .and_then(|dir| BagRules::load(&dir).ok())
        .map(|rules| rules.missing_stack_limit())
        .unwrap_or(0);
    describe(&db, cat, missing, account, character)
}

pub(crate) fn describe(
    db: &Path,
    cat: &Catalog,
    missing_stack_limit: u32,
    account: i64,
    character: i64,
) -> Result<GmCharacterDetail, String> {
    if account <= 0 || character <= 0 {
        return Err("请选择账号和角色".into());
    }
    let conn = open_ro(db)?;
    let row = conn.query_row(
        "SELECT id,account_id,wire_id,name,profession,state,created_at FROM characters \
         WHERE account_id=?1 AND id=?2 AND deleted_at IS NULL",
        rusqlite::params![account, character],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Vec<u8>>(5)?,
                r.get::<_, i64>(6)?,
            ))
        },
    );
    let (id, account_id, wire_id, name, profession, state_raw, created_at) = match row {
        Ok(v) => v,
        Err(e) => return Err(format!("角色不存在：{e}")),
    };
    let state: Value =
        serde_json::from_slice(&state_raw).map_err(|e| format!("角色存档不是可识别的 JSON：{e}"))?;

    let mut bag_error = String::new();
    let mut bag: Vec<GmBagRow> = Vec::new();
    match read_containers(&state) {
        Ok(containers) => {
            for (collection, space, rows) in containers {
                for raw in rows {
                    match bag_row_of(&collection, space, &raw) {
                        Ok(mut entry) => {
                            enrich(cat, missing_stack_limit, &mut entry);
                            bag.push(entry);
                        }
                        Err(e) => {
                            bag_error = e;
                            bag.clear();
                            break;
                        }
                    }
                }
                if !bag_error.is_empty() {
                    break;
                }
            }
        }
        Err(e) => bag_error = e,
    }

    let summary = summary_of(&state);
    let cera = conn
        .query_row(
            "SELECT coalesce((SELECT cera FROM account_currency WHERE account_id=?1), 0)",
            rusqlite::params![account_id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0);
    let head = GmCharacterHead {
        class_name: cat.class_of_role_full(profession, summary.advancement as i64, summary.awakening as i64),
        id,
        account_id,
        wire_id,
        name,
        profession,
        advancement: summary.advancement,
        awakening: summary.awakening,
        level: summary.level,
        experience: summary.experience,
        gold: gold_of(&state),
        created_at: DateTime::from_timestamp_micros(created_at)
            .map(|t| t.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| format!("{created_at}")),
    };
    Ok(GmCharacterDetail { character: head, cera, summary, bag, bag_error })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn gm_character_detail(
    state: tauri::State<'_, crate::commands::AppState>,
    account: i64,
    character: i64,
) -> Result<GmCharacterDetail, String> {
    let shared = state.gm.clone();
    let root = state.root.clone();
    crate::commands::join(move || {
        let catalog = shared.ensure(&root)?;
        detail(&root, &catalog, account, character)
    })
    .await
}

// ---------------------------------------------------------------------------
// 背包编辑（Go editGMInventory：定位 → 乐观锁核对 → 改数量或删行 → 整份 state 回写）
// ---------------------------------------------------------------------------

/// 一次背包编辑的入参。`revision` 与 `expected_amount` 是乐观锁的两把钥匙：
/// 浏览时看到什么就必须交回什么，这一行在中间被服务端改过则整笔拒掉，不覆盖别人的改动。
/// `durability` 为真时走装备耐久那一档：`expected_durability` 是界面读到的当前耐久（第三把钥匙），
/// `amount` 是要写进去的新耐久，`durability_cap` 是装备目录里这件装备的耐久上限。
pub(crate) struct BagEdit {
    pub delete: bool,
    pub durability: bool,
    pub collection: String,
    pub space: u8,
    pub slot: u16,
    pub group: u8,
    pub template: u32,
    pub revision: String,
    pub expected_amount: u32,
    pub expected_durability: Option<u32>,
    pub durability_cap: Option<u32>,
    pub amount: u32,
}

/// 把「第 li 张表」换算成 state 里那条真正要改的小数组。
fn rows_slot<'a>(state: &'a mut Value, collection: &str, space: u8) -> Option<&'a mut Vec<Value>> {
    let bag = state.get_mut("inventory")?.as_object_mut()?;
    if collection != "special_equipment" {
        return bag.get_mut(collection)?.as_array_mut();
    }
    bag.get_mut("special_equipment")?
        .as_object_mut()?
        .get_mut(&space.to_string())?
        .as_array_mut()
}

/// 返回（回写用的整份 state 字节，审计回执）。
/// 与 Go 一致：只重写被碰的那一行的 amount 键，其余字段、其余行原封不动，
/// 所以期限（expire_time）与装备属性（record/refine）都会跟着留下。
pub(crate) fn apply_bag_edit(
    state: &Value,
    cat: &Catalog,
    missing: u32,
    edit: &BagEdit,
) -> Result<(Vec<u8>, Value), String> {
    let mut hit: Option<(usize, usize, GmBagRow)> = None;
    for (li, (collection, space, rows)) in read_containers(state)?.into_iter().enumerate() {
        if collection != edit.collection || space != edit.space {
            continue;
        }
        for (ri, raw) in rows.iter().enumerate() {
            let mut entry = bag_row_of(&collection, space, raw)?;
            enrich(cat, missing, &mut entry);
            if entry.slot != edit.slot || entry.group != edit.group {
                continue;
            }
            if hit.is_some() {
                return Err("同一位置存在重复物品，拒绝修改".into());
            }
            hit = Some((li, ri, entry));
        }
    }
    let (li, ri, target) = match hit {
        Some(v) => v,
        None => return Err("物品或数量已变化，请刷新背包后重新操作".into()),
    };
    // 装备那一档不吃 `expected_amount`：那一行的数量既不是界面看的东西，也不是它改的东西，
    // 强行让界面补一个「1」只会多一个错的地方。整行的指纹仍然由 revision + 当前耐久两把钥匙盯着。
    let stale = target.template != edit.template
        || target.revision != edit.revision
        || (!edit.durability && target.count != edit.expected_amount);
    if stale {
        return Err("物品或数量已变化，请刷新背包后重新操作".into());
    }
    if edit.durability {
        // Go 的「装备行只改耐久，template 与 slot 一律不给改，也不新建行」那一档。
        if !matches!(edit.collection.as_str(), "equipment" | "worn") {
            return Err("只有装备栏与已穿戴的行可以改耐久".into());
        }
        let Some(cur) = target.durability else {
            return Err("这一行没有耐久字段，不是装备行".into());
        };
        if edit.expected_durability != Some(cur) {
            return Err("耐久已变化，请刷新背包后重新操作".into());
        }
        // 上限只认装备目录（`equipment.current37.json` 的 `[durability]`）：
        // 查不到就拒，不放开成「65535 以内随便填」，那是写一个游戏里不存在的耐久值。
        let cap = match edit.durability_cap {
            Some(cap) if cap > 0 => cap,
            _ => return Err("装备目录里查不到这件装备的耐久上限".into()),
        };
        if edit.amount == 0 || edit.amount > cap {
            return Err(format!("耐久须为 1 至 {cap}"));
        }
        if edit.amount == cur {
            return Err("耐久没有变化".into());
        }
    } else if !edit.delete {
        if edit.collection != "items"
            || !target.can_edit_count
            || edit.amount == 0
            || edit.amount > target.stack_limit
        {
            return Err(format!("只能修改可堆叠物品，数量须为 1 至 {}", target.stack_limit));
        }
        if edit.amount == target.count {
            return Err("数量没有变化".into());
        }
    }

    let (collection, space) = LIST_ORDER[li];
    let mut doc = state.clone();
    let rows = rows_slot(&mut doc, collection, space)
        .ok_or_else(|| "物品或数量已变化，请刷新背包后重新操作".to_string())?;
    let (before, after) = {
        let row = rows
            .get_mut(ri)
            .ok_or_else(|| "物品或数量已变化，请刷新背包后重新操作".to_string())?;
        let before = row.clone();
        if edit.delete {
            (before, Value::Null)
        } else {
            let map = row
                .as_object_mut()
                .ok_or_else(|| "背包物品记录格式无效".to_string())?;
            // 真实存档用的是大写 Amount：按键名大小写不敏感找到那一个键，只换它的值。
            // 耐久同理（存档里是小写 durability），其余键（期限、record、refine）一律跟着留下。
            let want = if edit.durability { "durability" } else { "amount" };
            if let Some(key) = map.keys().find(|k| k.eq_ignore_ascii_case(want)).cloned() {
                map.insert(key, Value::from(edit.amount));
            }
            (before, row.clone())
        }
    };
    if edit.delete {
        rows.remove(ri);
    }
    let receipt = serde_json::json!({
        "action": if edit.durability {
            "equipment.durability"
        } else if edit.delete {
            "inventory.delete"
        } else {
            "inventory.amount"
        },
        "item": serde_json::to_value(&target).map_err(|e| e.to_string())?,
        "before": before,
        "after": after,
    });
    let bytes = serde_json::to_vec(&doc).map_err(|e| e.to_string())?;
    Ok((bytes, receipt))
}

// ---------------------------------------------------------------------------
// 数值改写（Go editCharacter 里不需要服务端成长目录的那三档）
// ---------------------------------------------------------------------------

/// Go 的装备栏掩码：只允许开辅助装备、魔法石、耳环三位（`value & ^0x13 != 0` 即拒）。
const EQUIP_SLOT_MASK: u32 = 0x13;

/// 返回（回写用的整份 state 字节，审计回执）。
/// 点数组与 Go 同：`skill_points` / `technique_points` 是 [2]uint16 的账本，
/// 服务端升级时也是两格各加同一份点数，这里照抄，不自己发明「只加可用那格」。
pub(crate) fn apply_numeric_edit(
    state: &Value,
    action: &str,
    value: u32,
) -> Result<(Vec<u8>, Value), String> {
    let mut doc = state.clone();
    let map = doc
        .as_object_mut()
        .ok_or_else(|| "角色存档格式无效".to_string())?;
    let (field, before, after) = if action == "equipment.unlock" {
        if value & !EQUIP_SLOT_MASK != 0 {
            return Err("只能开启辅助装备、魔法石和耳环栏".into());
        }
        let held = map.get("equipment_slot_flags").cloned().unwrap_or(Value::from(0));
        let flags = held.as_u64().unwrap_or(0) as u32 | value;
        if flags > u8::MAX as u32 {
            return Err("装备栏标志超出存档范围".into());
        }
        ("equipment_slot_flags", held, Value::from(flags as u64))
    } else {
        let field = if action == "sp" {
            "skill_points"
        } else {
            "technique_points"
        };
        let held = map.get(field).cloned().unwrap_or(Value::from(0));
        let mut points = [0u32; 2];
        match &held {
            Value::Array(list) => {
                for (i, raw) in list.iter().take(2).enumerate() {
                    points[i] = as_uint(Some(raw), u16::MAX as u64) as u32;
                }
            }
            Value::Null => {}
            other => points[0] = as_uint(Some(other), u16::MAX as u64) as u32,
        }
        // Go：逐格判 `value > 65535 - 现有`，任何一格放不下就整笔拒。
        if points.iter().any(|p| value > u16::MAX as u32 - *p) {
            return Err(if action == "sp" {
                "技能点超过存档上限".into()
            } else {
                "技巧点超过存档上限".into()
            });
        }
        let after = serde_json::json!([points[0] + value, points[1] + value]);
        (field, held, after)
    };
    map.insert(field.to_string(), after.clone());
    let receipt = serde_json::json!({
        "action": action,
        "value": value,
        "field": field,
        "before": before,
        "after": after,
    });
    let bytes = serde_json::to_vec(&doc).map_err(|e| e.to_string())?;
    Ok((bytes, receipt))
}

/// SP「按当前等级重算为满值」：写下去的是**服务端把这个角色升到当前等级本该拿到的总量**，
/// 判据来自离线产物 `gm-growth.json` 的 `skill_points`（`etc/sptable.etc` 烘出来的每级授予）。
///
/// 求和从二级起、两格各写同一份，两条都是从服务端的算法抄的：
/// - `AddGrowthExperience`（`growth_rules.go:174`）只在**跨过**某一级时查表，
///   一级不发点，所以 1 级的满值就是 0；某一级在表里没有行则整笔拒（Go 原文
///   `missing crossed-level SP row`）。
/// - `progression_state.go:56` 升级时把同一份点数加到两格，这里也两格同写。
///
/// 刻意不套 `characterCatalog` 那三道身份门：这一张表是全局的 `catalog.Progression`，
/// 服务端算升级奖励时不看职业、也不看存档来源指纹。
pub(crate) fn apply_sp_recalc(
    state: &Value,
    skill_points: &BTreeMap<u16, u16>,
    level_cap: u8,
) -> Result<(Vec<u8>, Value), String> {
    let mut doc = state.clone();
    let map = doc
        .as_object_mut()
        .ok_or_else(|| "角色存档格式无效".to_string())?;
    let level = as_uint(map.get("level"), u8::MAX as u64) as u8;
    if level == 0 || level > level_cap {
        return Err(format!("当前等级 {level} 不在成长表范围内（上限 {level_cap}）"));
    }
    let mut total: u32 = 0;
    for crossed in 2..=u32::from(level) {
        let grant = skill_points
            .get(&(crossed as u16))
            .ok_or_else(|| format!("成长产物缺少第 {crossed} 级的技能点"))?;
        total += u32::from(*grant);
    }
    // 与 `apply_numeric_edit` 同一把尺子：落不进 uint16 就整笔拒，不截断。
    if total > u16::MAX as u32 {
        return Err("技能点超过存档上限".into());
    }
    let held = map.get("skill_points").cloned().unwrap_or(Value::from(0));
    let after = serde_json::json!([total, total]);
    map.insert("skill_points".to_string(), after.clone());
    let receipt = serde_json::json!({
        "action": "sp.recalculate",
        "field": "skill_points",
        "level": level as u64,
        // 求和跨过几级：1 级是 0 级（服务端 1 级不发点），5 级是 4 级（2..=5）。
        "levels_summed": level.saturating_sub(1) as u64,
        "total": total as u64,
        "before": held,
        "after": after,
    });
    let bytes = serde_json::to_vec(&doc).map_err(|e| e.to_string())?;
    Ok((bytes, receipt))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state(json: &str) -> Value {
        serde_json::from_str(json).expect("test state json")
    }

    #[test]
    fn containers_follow_go_order_and_tolerate_nulls() {
        let s = state(
            r#"{"inventory":{"version":"ordinary-bag-v1","gold":7,
            "items":[{"slot":65,"Template":10418035,"Amount":3,"expire_time":2147483647}],
            "equipment":[{"slot":9,"template":401040091,"durability":48}],
            "worn":[{"slot":12,"template":101040829,"durability":48,"refine":8}],
            "special_equipment":{"1":null,"7":[{"slot":0,"template":10001}]},"unknown_key":1}}"#,
        );
        let lists = read_containers(&s).expect("containers");
        let shape: Vec<(String, u8, usize)> = lists
            .iter()
            .map(|(c, sp, rows)| (c.clone(), *sp, rows.len()))
            .collect();
        assert_eq!(
            shape,
            vec![
                ("items".to_string(), 0, 1),
                ("equipment".to_string(), 0, 1),
                ("worn".to_string(), 0, 1),
                ("special_equipment".to_string(), 1, 0),
                ("special_equipment".to_string(), 7, 1),
            ]
        );
    }

    #[test]
    fn container_errors_match_go_words() {
        assert_eq!(read_containers(&state("[]")).unwrap_err(), "角色存档格式无效");
        assert_eq!(read_containers(&state(r#"{"level":1}"#)).unwrap().len(), 0);
        assert_eq!(
            read_containers(&state(r#"{"inventory":{"version":"other"}}"#)).unwrap_err(),
            "不支持的背包存档版本"
        );
        assert_eq!(
            read_containers(&state(r#"{"inventory":{"version":"ordinary-bag-v1","items":7}}"#))
                .unwrap_err(),
            "背包容器 items 格式无效"
        );
        assert_eq!(
            read_containers(&state(
                r#"{"inventory":{"version":"ordinary-bag-v1","special_equipment":7}}"#
            ))
            .unwrap_err(),
            "时装或宠物容器格式无效"
        );
    }

    #[test]
    fn row_needs_slot_template_and_amount_for_items() {
        // 装备行没有 amount 也算合法（Go 把数量默认成 1）。
        let e = bag_row_of(
            "equipment",
            0,
            &state(r#"{"slot":9,"template":401040091,"durability":48}"#),
        )
        .expect("equipment row");
        assert_eq!((e.slot, e.template, e.count), (9, 401040091, 1));
        assert_eq!(e.durability, Some(48), "装备行要把耐久带给界面");
        assert!(!e.worn);
        // 耐久只在装备两张表里认；items 行即使带这一键也不给（界面不给耐久输入）。
        assert_eq!(
            bag_row_of("items", 0, &state(r#"{"slot":5,"template":1,"amount":2,"durability":9}"#))
                .unwrap()
                .durability,
            None
        );
        // 解不出数字（超 u16、字符串、null）就当这一行没有耐久，不报错也不给改。
        assert_eq!(
            bag_row_of("equipment", 0, &state(r#"{"slot":9,"template":1,"durability":"48"}"#))
                .unwrap()
                .durability,
            None
        );
        assert_eq!(
            bag_row_of("equipment", 0, &state(r#"{"slot":9,"template":1,"durability":65536}"#))
                .unwrap()
                .durability,
            None
        );
        assert!(bag_row_of("items", 0, &state(r#"{"slot":65,"Template":1}"#)).is_err());
        assert!(bag_row_of("items", 0, &state(r#"{"template":1,"amount":2}"#)).is_err());
        assert!(bag_row_of("items", 0, &state(r#"{"slot":5,"template":1,"amount":0}"#)).is_err());
        // Go 只要求 slot 键在场，不校验它非零（快速浏览比写入侧宽松）。
        assert_eq!(bag_row_of("items", 0, &state(r#"{"slot":0,"template":1,"amount":2}"#)).unwrap().slot, 0);
        // 大小写同键算重复（报出的键名取决于遍历先后，Go 那边是 map 随机序，不钉键名）；
        // null 值也算无效。
        let dup = bag_row_of("items", 0, &state(r#"{"slot":5,"Slot":6,"template":1,"amount":2}"#))
            .unwrap_err();
        assert!(dup.starts_with("背包物品字段 ") && dup.ends_with(" 无效或重复"), "{dup}");
        assert_eq!(
            bag_row_of("items", 0, &state(r#"{"slot":5,"template":1,"amount":null}"#)).unwrap_err(),
            "背包物品字段 amount 无效或重复"
        );
        assert!(bag_row_of("worn", 0, &state(r#"{"slot":12,"template":1,"group":1}"#))
            .expect("worn")
            .worn);
    }

    #[test]
    fn revision_is_stable_and_field_sensitive() {
        let a = revision_of(state(r#"{"slot":5,"Template":1,"Amount":2}"#).as_object().unwrap());
        let b = revision_of(state(r#"{"Amount":2,"Template":1,"slot":5}"#).as_object().unwrap());
        let c = revision_of(state(r#"{"slot":5,"Template":1,"Amount":3}"#).as_object().unwrap());
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn summary_uses_go_zero_defaults() {
        let s = summary_of(&state(
            r#"{"level":70,"experience":1234567890123,"advancement":4,"awakening":3,
            "skill_points":[10,20],"technique_points":[1],"attributes":{"[hp max]":490},
            "learned_skills":[null,null],
            "inventory":{"version":"ordinary-bag-v1","gold":11123456}}"#,
        ));
        assert_eq!(s.level, 70);
        assert_eq!(s.experience, 1234567890123);
        assert_eq!((s.advancement, s.awakening), (4, 3));
        assert_eq!(s.skill_points, [10, 20]);
        // 只有一项的数组补零，而不是整份存档报错。
        assert_eq!(s.technique_points, [1, 0]);
        assert_eq!(s.currency_slot2, 0);
        assert_eq!(s.attributes["[hp max]"], 490);
        assert_eq!(s.learned_skills, state("[null,null]"));

        let bare = summary_of(&state(r#"{"inventory":{}}"#));
        assert_eq!((bare.level, bare.advancement, bare.awakening), (0, 0, 0));
        assert_eq!(bare.attributes, Value::Null);
    }

    #[test]
    fn missing_bag_is_not_fatal_for_the_head() {
        // inventory 缺版本 → 只有 bag_error，头部照常。
        let s = state(r#"{"level":1,"inventory":{"gold":5}}"#);
        assert_eq!(read_containers(&s).unwrap_err(), "不支持的背包存档版本");
        assert_eq!(summary_of(&s).level, 1);
    }

    /// 真库形状（test2 那一行）端到端：blob state、大写 Template/Amount、
    /// special_equipment 里的 null、物品库查不到的占位名。
    #[test]
    fn describe_reads_a_real_shaped_save() {
        let dir = std::env::temp_dir().join(format!("gmchar-{}", std::process::id()));
        let cfg = dir.join("configs");
        std::fs::create_dir_all(&cfg).expect("tmp dirs");
        std::fs::write(
            cfg.join("items.index.json"),
            r#"{"items":[
                {"id":10418035,"kind":"stackable","stack_type":"[material]","stack_limit":1000,"name":"合成材料"},
                {"id":401040091,"kind":"equipment","name":"合成武器"}]}"#,
        )
        .expect("items fixture");
        let cat = gmindex::load(&cfg).expect("fixture catalog");

        let db = dir.join("t.sqlite3");
        let _ = std::fs::remove_file(&db);
        {
            let conn = rusqlite::Connection::open(&db).expect("fixture db");
            conn.execute_batch(
                r#"CREATE TABLE characters(id INTEGER PRIMARY KEY,account_id INTEGER,wire_id INTEGER,
                        name TEXT,profession INTEGER,state BLOB,created_at INTEGER,deleted_at INTEGER);
                   CREATE TABLE account_currency(account_id INTEGER PRIMARY KEY,cera INTEGER);
                   INSERT INTO account_currency VALUES(1,295722);
                   INSERT INTO characters VALUES(2,1,2,'test2',5,CAST('{"level":1,"advancement":4,
                        "skill_points":[0,0],"technique_points":[0,0],"learned_skills":[null,null],
                        "inventory":{"version":"ordinary-bag-v1","gold":11123456,
                        "items":[{"slot":65,"Template":10418035,"Amount":3,"expire_time":2147483647}],
                        "equipment":[{"slot":9,"template":401040091,"durability":48}],
                        "worn":[{"slot":12,"template":101040829,"durability":48,"refine":8}],
                        "special_equipment":{"1":null,"7":[{"slot":3,"template":10001}]}}}' AS BLOB),
                        1791989989694000,NULL);
                   INSERT INTO characters VALUES(9,1,9,'删掉的',0,CAST('{}' AS BLOB),1,2);"#,
            )
            .expect("fixture rows");
        }

        let d = describe(&db, &cat, 0, 1, 2).expect("detail");
        assert_eq!(d.character.name, "test2");
        // 名字表缺席时按 (大类 5, 转职槽 4) 回落家族表基名。
        assert_eq!(d.character.class_name, "弹药专家");
        assert_eq!(d.character.gold, 11123456);
        assert_eq!(d.cera, 295722);
        assert!(d.bag_error.is_empty());
        assert_eq!(d.summary.skill_points, [0, 0]);
        assert_eq!(d.summary.awakening, 0);

        let shape: Vec<(String, u8, u32)> =
            d.bag.iter().map(|r| (r.collection.clone(), r.space, r.template)).collect();
        assert_eq!(
            shape,
            vec![
                ("items".to_string(), 0, 10418035),
                ("equipment".to_string(), 0, 401040091),
                ("worn".to_string(), 0, 101040829),
                ("special_equipment".to_string(), 7, 10001),
            ]
        );
        let items = &d.bag[0];
        assert_eq!(items.name, "合成材料");
        assert_eq!(items.count, 3);
        assert_eq!(items.stack_limit, 1000);
        assert!(items.can_edit_count);
        assert_eq!(items.kind, "stackable");
        let equip = &d.bag[1];
        assert_eq!(equip.name, "合成武器");
        assert_eq!(equip.count, 1);
        assert!(!equip.can_edit_count);
        // 物品库查不到的行保留「物品 N」占位名，不猜分类。
        assert_eq!(d.bag[2].name, "物品 101040829");
        assert!(d.bag[2].worn);
        assert!(d.bag[2].kind.is_empty());
        for r in &d.bag {
            assert_eq!(r.revision.len(), 64);
        }
        assert!(!d.character.created_at.is_empty());

        // 软删的角色与别人的账号都读不到（Go 只按 id，这里更严）。
        assert!(describe(&db, &cat, 0, 1, 9).is_err());
        assert!(describe(&db, &cat, 0, 2, 2).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 编辑用的物品库夹具：可堆叠 10418035（上限 1000）+ 装备 401040091。
    fn edit_fixture(tag: &str) -> (std::path::PathBuf, Catalog) {
        let dir = std::env::temp_dir().join(format!("gmchar-edit-{tag}-{}", std::process::id()));
        let cfg = dir.join("configs");
        std::fs::create_dir_all(&cfg).expect("tmp dirs");
        std::fs::write(
            cfg.join("items.index.json"),
            r#"{"items":[
                {"id":10418035,"kind":"stackable","stack_type":"[material]","stack_limit":1000,"name":"合成材料"},
                {"id":401040091,"kind":"equipment","name":"合成武器"}]}"#,
        )
        .expect("items fixture");
        let cat = gmindex::load(&cfg).expect("fixture catalog");
        (dir, cat)
    }

    /// 界面上那一行：浏览侧怎么算出来，编辑侧就怎么交回来（乐观锁的两把钥匙）。
    fn view(collection: &str, space: u8, raw: &Value, cat: &Catalog) -> GmBagRow {
        let mut row = bag_row_of(collection, space, raw).expect("行可解析");
        enrich(cat, 0, &mut row);
        row
    }

    fn edit_of(row: &GmBagRow, amount: u32, delete: bool) -> BagEdit {
        BagEdit {
            delete,
            durability: false,
            collection: row.collection.clone(),
            space: row.space,
            slot: row.slot,
            group: row.group,
            template: row.template,
            revision: row.revision.clone(),
            expected_amount: row.count,
            expected_durability: None,
            durability_cap: None,
            amount,
        }
    }

    /// 耐久那一档的入参：第三把钥匙是界面读到的当前耐久，上限由装备目录给。
    fn durability_edit(row: &GmBagRow, amount: u32, cap: Option<u32>) -> BagEdit {
        BagEdit {
            durability: true,
            expected_durability: row.durability,
            durability_cap: cap,
            ..edit_of(row, amount, false)
        }
    }

    /// test2 那一行的形状：大写 Template/Amount、带期限、别的容器与未知键都在。
    fn edit_state() -> Value {
        state(
            r#"{"level":1,"creature_loyalty_fraction":0.5,"inventory":{"version":"ordinary-bag-v1","gold":11223456,
                "items":[{"slot":65,"Template":10418035,"Amount":3,"expire_time":2147483647},{"slot":66,"Template":1,"Amount":9}],
                "equipment":[{"slot":9,"template":401040091,"durability":48}],
                "worn":[{"slot":12,"template":101040829,"durability":48,"refine":8}],
                "special_equipment":{"1":null,"7":[{"slot":3,"template":10001}]}},"unknown_top":7}"#,
        )
    }

    #[test]
    fn amount_edit_keeps_expiry_and_every_other_field() {
        let (dir, cat) = edit_fixture("amount");
        let s = edit_state();
        let row = view("items", 0, &s["inventory"]["items"][0], &cat);
        assert!(row.can_edit_count);
        let (bytes, receipt) = apply_bag_edit(&s, &cat, 0, &edit_of(&row, 7, false)).expect("改数量");
        let after: Value = serde_json::from_slice(&bytes).expect("回写的是合法 JSON");
        let hit = &after["inventory"]["items"][0];
        assert_eq!(hit["Amount"], 7, "数量改掉");
        assert_eq!(hit["expire_time"], 2147483647, "期限必须留着");
        assert_eq!(hit["Template"], 10418035);
        // 别的行、别的容器、连 state 里的未知键都不许被顺手重写掉。
        assert_eq!(after["inventory"]["items"][1]["Amount"], 9);
        assert_eq!(after["inventory"]["gold"], 11223456);
        assert_eq!(after["unknown_top"], 7);
        assert_eq!(after["creature_loyalty_fraction"], 0.5);
        assert_eq!(after["inventory"]["worn"][0]["refine"], 8);
        assert_eq!(after["inventory"]["special_equipment"]["1"], Value::Null);
        // 回执与 Go 同形：action / item / before / after。
        assert_eq!(receipt["action"], "inventory.amount");
        assert_eq!(receipt["before"]["Amount"], 3);
        assert_eq!(receipt["after"]["Amount"], 7);
        assert_eq!(receipt["item"]["revision"], row.revision);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_refuses_stale_locks_and_uneditable_rows() {
        let (dir, cat) = edit_fixture("refuse");
        let s = edit_state();
        let row = view("items", 0, &s["inventory"]["items"][0], &cat);

        // 修订号不对 ⇒ 这一行在我们看过之后被改过。
        let mut stale = edit_of(&row, 7, false);
        stale.revision = "0".repeat(64);
        assert_eq!(
            apply_bag_edit(&s, &cat, 0, &stale).unwrap_err(),
            "物品或数量已变化，请刷新背包后重新操作"
        );
        // 原数量不对 ⇒ 同上（模板不对也走这一条）。
        let mut stale = edit_of(&row, 7, false);
        stale.expected_amount = 4;
        assert_eq!(
            apply_bag_edit(&s, &cat, 0, &stale).unwrap_err(),
            "物品或数量已变化，请刷新背包后重新操作"
        );
        let mut stale = edit_of(&row, 7, false);
        stale.template = 999;
        assert!(apply_bag_edit(&s, &cat, 0, &stale).is_err());
        // 找不到那一格（槽位被别的东西占了）也走同一条话。
        let mut gone = edit_of(&row, 7, false);
        gone.slot = 6500;
        assert!(apply_bag_edit(&s, &cat, 0, &gone).is_err());

        // 装备不可改数量；超上限也不行；改成同一个数也不行。
        let equip = view("equipment", 0, &s["inventory"]["equipment"][0], &cat);
        assert_eq!(
            apply_bag_edit(&s, &cat, 0, &edit_of(&equip, 5, false)).unwrap_err(),
            "只能修改可堆叠物品，数量须为 1 至 0"
        );
        assert_eq!(
            apply_bag_edit(&s, &cat, 0, &edit_of(&row, 1001, false)).unwrap_err(),
            "只能修改可堆叠物品，数量须为 1 至 1000"
        );
        assert_eq!(
            apply_bag_edit(&s, &cat, 0, &edit_of(&row, 0, false)).unwrap_err(),
            "只能修改可堆叠物品，数量须为 1 至 1000"
        );
        assert_eq!(
            apply_bag_edit(&s, &cat, 0, &edit_of(&row, 3, false)).unwrap_err(),
            "数量没有变化"
        );

        // 物品库查不到 ⇒ 没有上限，也就不可改（宁可不给改，也不写个没依据的数）。
        let unknown = view("items", 0, &state(r#"{"slot":70,"Template":590722598,"Amount":1}"#), &cat);
        assert!(!unknown.can_edit_count);
        assert!(apply_bag_edit(
            &state(r#"{"inventory":{"version":"ordinary-bag-v1","items":[{"slot":70,"Template":590722598,"Amount":1}]}}"#),
            &cat,
            0,
            &edit_of(&unknown, 2, false)
        )
        .is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn durability_edit_keeps_every_other_equipment_field() {
        let (dir, cat) = edit_fixture("durability");
        let s = edit_state();
        // 已穿戴那一行带 refine：改耐久必须只动 durability 一键。
        let row = view("worn", 0, &s["inventory"]["worn"][0], &cat);
        assert_eq!(row.durability, Some(48));
        let (bytes, receipt) =
            apply_bag_edit(&s, &cat, 0, &durability_edit(&row, 120, Some(120))).expect("改耐久");
        let after: Value = serde_json::from_slice(&bytes).expect("回写的是合法 JSON");
        let hit = &after["inventory"]["worn"][0];
        assert_eq!(hit["durability"], 120, "耐久改掉");
        assert_eq!(hit["refine"], 8, "精炼记录必须留着");
        assert_eq!(hit["template"], 101040829);
        assert_eq!(hit["slot"], 12);
        // 别的容器与 state 里的未知键同样不许被顺手重写。
        assert_eq!(after["inventory"]["equipment"][0]["durability"], 48);
        assert_eq!(after["inventory"]["items"][0]["Amount"], 3);
        assert_eq!(after["inventory"]["gold"], 11223456);
        assert_eq!(after["unknown_top"], 7);
        assert_eq!(receipt["action"], "equipment.durability");
        assert_eq!(receipt["before"]["durability"], 48);
        assert_eq!(receipt["after"]["durability"], 120);
        assert_eq!(receipt["item"]["revision"], row.revision);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn durability_edit_refuses_everything_it_cannot_prove() {
        let (dir, cat) = edit_fixture("durability-refuse");
        let s = edit_state();
        let row = view("worn", 0, &s["inventory"]["worn"][0], &cat);

        // items 行没有耐久这一档：那一行改的是数量，不是耐久。
        let items_row = view("items", 0, &s["inventory"]["items"][0], &cat);
        assert_eq!(
            apply_bag_edit(&s, &cat, 0, &durability_edit(&items_row, 120, Some(120))).unwrap_err(),
            "只有装备栏与已穿戴的行可以改耐久"
        );

        // 装备行没有 durability 键 ⇒ 界面不给输入，写侧也拒。
        let plain = state(
            r#"{"inventory":{"version":"ordinary-bag-v1","equipment":[{"slot":9,"Template":401040091}]}}"#,
        );
        let plain_row = view("equipment", 0, &plain["inventory"]["equipment"][0], &cat);
        assert_eq!(plain_row.durability, None);
        assert_eq!(
            apply_bag_edit(&plain, &cat, 0, &durability_edit(&plain_row, 120, Some(120))).unwrap_err(),
            "这一行没有耐久字段，不是装备行"
        );

        // 第三把钥匙不对：这一行在我们看过之后耐久变了。
        let mut stale = durability_edit(&row, 120, Some(120));
        stale.expected_durability = Some(47);
        assert_eq!(
            apply_bag_edit(&s, &cat, 0, &stale).unwrap_err(),
            "耐久已变化，请刷新背包后重新操作"
        );

        // 上限只认装备目录：查不到就整条拒，不放开成「随便填」。
        assert_eq!(
            apply_bag_edit(&s, &cat, 0, &durability_edit(&row, 120, None)).unwrap_err(),
            "装备目录里查不到这件装备的耐久上限"
        );
        assert_eq!(
            apply_bag_edit(&s, &cat, 0, &durability_edit(&row, 120, Some(0))).unwrap_err(),
            "装备目录里查不到这件装备的耐久上限"
        );

        // 0、超上限、改完还是同一个数，三条都拒；耐久为 0 的坏装备照样可以修好。
        assert_eq!(
            apply_bag_edit(&s, &cat, 0, &durability_edit(&row, 0, Some(120))).unwrap_err(),
            "耐久须为 1 至 120"
        );
        assert_eq!(
            apply_bag_edit(&s, &cat, 0, &durability_edit(&row, 121, Some(120))).unwrap_err(),
            "耐久须为 1 至 120"
        );
        assert_eq!(
            apply_bag_edit(&s, &cat, 0, &durability_edit(&row, 48, Some(120))).unwrap_err(),
            "耐久没有变化"
        );
        let broken = state(
            r#"{"inventory":{"version":"ordinary-bag-v1","equipment":[{"slot":9,"Template":401040091,"durability":0}]}}"#,
        );
        let broken_row = view("equipment", 0, &broken["inventory"]["equipment"][0], &cat);
        assert_eq!(broken_row.durability, Some(0));
        let (bytes, _) = apply_bag_edit(&broken, &cat, 0, &durability_edit(&broken_row, 120, Some(120)))
            .expect("打坏的装备（耐久 0）可以修");
        let after: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(after["inventory"]["equipment"][0]["durability"], 120);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_removes_one_row_from_its_own_container() {
        let (dir, cat) = edit_fixture("delete");
        let s = edit_state();

        let row = view("items", 0, &s["inventory"]["items"][0], &cat);
        let (bytes, receipt) = apply_bag_edit(&s, &cat, 0, &edit_of(&row, 0, true)).expect("删一行");
        let after: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(after["inventory"]["items"].as_array().unwrap().len(), 1);
        assert_eq!(after["inventory"]["items"][0]["slot"], 66);
        assert_eq!(after["inventory"]["worn"].as_array().unwrap().len(), 1);
        assert_eq!(receipt["action"], "inventory.delete");
        assert_eq!(receipt["after"], Value::Null, "Go 删行时 after 就是 null");
        assert_eq!(receipt["before"]["Amount"], 3, "删前的记录要留在审计里");

        // 特殊空间也能删：那一行挂在 special_equipment["7"] 下面。
        let pet = view("special_equipment", 7, &s["inventory"]["special_equipment"]["7"][0], &cat);
        let (bytes, _) = apply_bag_edit(&s, &cat, 0, &edit_of(&pet, 0, true)).expect("删时装/宠物行");
        let after: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(after["inventory"]["special_equipment"]["7"].as_array().unwrap().len(), 0);
        assert_eq!(after["inventory"]["special_equipment"]["1"], Value::Null);

        // 同一格有两行：拒绝修改（改哪一行都不该由我们猜）。
        let dup = state(
            r#"{"inventory":{"version":"ordinary-bag-v1","items":[
                {"slot":5,"Template":1,"Amount":2},{"slot":5,"Template":1,"Amount":3}]}}"#,
        );
        let row = view("items", 0, &dup["inventory"]["items"][0], &cat);
        assert_eq!(
            apply_bag_edit(&dup, &cat, 0, &edit_of(&row, 9, true)).unwrap_err(),
            "同一位置存在重复物品，拒绝修改"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 夹具照 `gmstat::test_context` 那张表：2/3/4/5 级各发 3/4/5/6 点，等级上限 5。
    fn sp_table() -> std::collections::BTreeMap<u16, u16> {
        std::collections::BTreeMap::from([(2u16, 3u16), (3u16, 4u16), (4u16, 5u16), (5u16, 6u16)])
    }

    fn recalc(json: &str, cap: u8) -> (Value, Value) {
        let (bytes, receipt) = apply_sp_recalc(&state(json), &sp_table(), cap).unwrap();
        (serde_json::from_slice(&bytes).unwrap(), receipt)
    }

    #[test]
    fn sp_recalc_sums_only_the_levels_actually_crossed() {
        // 3 级：服务端跨过的是 2 级和 3 级，所以 3 + 4 = 7，两格各写 7。
        let (doc, receipt) = recalc(r#"{"level":3,"skill_points":[1,200]}"#, 5);
        assert_eq!(doc["skill_points"], serde_json::json!([7, 7]));
        assert_eq!(receipt["after"], serde_json::json!([7, 7]));
        assert_eq!(receipt["before"], serde_json::json!([1, 200]));
        assert_eq!(receipt["level"], 3);
        assert_eq!(receipt["levels_summed"], 2);
        assert_eq!(receipt["total"], 7);

        // 1 级的满值就是 0：一级不发点，所以这一键会把两格清零，不是「加满」。
        let (doc, receipt) = recalc(r#"{"level":1,"skill_points":[9,9]}"#, 5);
        assert_eq!(doc["skill_points"], serde_json::json!([0, 0]));
        assert_eq!(receipt["levels_summed"], 0);

        // 上限 5 级：3+4+5+6 = 18。
        let (doc, _) = recalc(r#"{"level":5}"#, 5);
        assert_eq!(doc["skill_points"], serde_json::json!([18, 18]));
    }

    #[test]
    fn sp_recalc_touches_only_that_one_column() {
        let json = r#"{"level":4,"experience":1200,"skill_points":[1,1],"technique_points":[2,2],
            "equipment_slot_flags":1,"attributes":{"物理攻击力":12.5},"learned_skills":[null,null],
            "inventory":{"version":"ordinary-bag-v1","gold":100,"items":[]}}"#;
        let (doc, _) = recalc(json, 5);
        assert_eq!(doc["skill_points"], serde_json::json!([12, 12]), "跨过 2/3/4 三级");
        assert_eq!(doc["technique_points"], serde_json::json!([2, 2]), "TP 一格都不碰");
        assert_eq!(doc["experience"], 1200);
        assert_eq!(doc["equipment_slot_flags"], 1);
        assert_eq!(doc["attributes"]["物理攻击力"], 12.5);
        assert_eq!(doc["inventory"]["gold"], 100);
        assert!(doc.get("gm_locked_sp").is_none(), "不自创服务端不认的键");
        // 顶层键集合原样保留：既不新增也不丢。
        let before: Value = serde_json::from_str(json).unwrap();
        let mut keys: Vec<&String> = doc.as_object().unwrap().keys().collect();
        keys.sort();
        let mut old: Vec<&String> = before.as_object().unwrap().keys().collect();
        old.sort();
        assert_eq!(keys, old);
    }

    #[test]
    fn sp_recalc_refuses_what_the_growth_table_cannot_answer() {
        // 等级超出成长表上限（Go 的 `Rules.LevelCap`）：不截断、不按上限凑。
        assert!(apply_sp_recalc(&state(r#"{"level":6}"#), &sp_table(), 5)
            .unwrap_err()
            .contains("不在成长表范围内"));
        // 0 级是坏档，不是「还没发点」。
        assert!(apply_sp_recalc(&state(r#"{"level":0}"#), &sp_table(), 5).is_err());
        // 跨过的某一级在表里没有行 ⇒ 与服务端同口径整笔拒（Go：missing crossed-level SP row）。
        let hole = std::collections::BTreeMap::from([(2u16, 3u16), (4u16, 5u16)]);
        assert_eq!(
            apply_sp_recalc(&state(r#"{"level":3,"skill_points":[7,7]}"#), &hole, 5).unwrap_err(),
            "成长产物缺少第 3 级的技能点"
        );
    }

    #[test]
    fn sp_recalc_refuses_when_the_total_does_not_fit_a_uint16() {
        let big = std::collections::BTreeMap::from([(2u16, 60_000u16), (3u16, 60_000u16)]);
        assert_eq!(
            apply_sp_recalc(&state(r#"{"level":3,"skill_points":[1,1]}"#), &big, 5).unwrap_err(),
            "技能点超过存档上限"
        );
    }

    /// 只在设了 `DFO_PROBE_CONFIGS`（GM 产物目录）时执行：拿整合包里那份真 `gm-growth.json`
    /// 把「按等级算满」的得数核一遍。右边的期望值是本轮用 `node` 独立扫同一份产物求和得到的，
    /// 不是这条 Rust 循环自己算出来的，所以两边不等就是解析或求和口径漂了。
    /// 跑法：`DFO_PROBE_CONFIGS=<产物目录> cargo test --lib live_sp_recalc_on_real_growth -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_sp_recalc_on_real_growth() {
        let Ok(dir) = std::env::var("DFO_PROBE_CONFIGS") else {
            eprintln!("跳过：未设 DFO_PROBE_CONFIGS");
            return;
        };
        let growth = gmbuild::growth::load_growth(Path::new(&dir)).expect("成长产物读不出");
        println!("产物指纹 {}，每级授予 {} 档", growth.source.checksum, growth.skill_points.len());
        for (level, want) in [(1u8, 0u32), (70, 7470), (115, 19320), (150, 32670)] {
            let (bytes, receipt) = apply_sp_recalc(
                &state(&format!(r#"{{"level":{level}}}"#)),
                &growth.skill_points,
                u8::MAX,
            )
            .unwrap();
            let doc: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(doc["skill_points"], serde_json::json!([want, want]), "Lv.{level} 满值不符");
            assert_eq!(receipt["total"], u64::from(want));
            println!("Lv.{level} → 满值 {want}（跨过 {} 级）", receipt["levels_summed"].as_u64().unwrap());
        }
    }

    /// 只在同时设了 `DFO_PROBE_CONFIGS`（GM 产物目录）与 `DFO_PROBE_DB`（dfolan.sqlite3）时执行：
    /// 拿真存档的字节把只读链路走一遍。连接是 READ_ONLY，且只 SELECT，一个字节都不写。
    /// 跑法：
    /// `DFO_PROBE_CONFIGS=<产物目录> DFO_PROBE_DB=<sqlite 档> cargo test --lib live_detail_reads_real_pack -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_detail_reads_real_pack() {
        let (Ok(dir), Ok(db)) = (
            std::env::var("DFO_PROBE_CONFIGS"),
            std::env::var("DFO_PROBE_DB"),
        ) else {
            eprintln!("跳过：未设 DFO_PROBE_CONFIGS / DFO_PROBE_DB");
            return;
        };
        let cat = gmindex::load(&std::path::PathBuf::from(&dir)).expect("产物加载失败");
        let missing = BagRules::load(Path::new(&dir))
            .map(|r| r.missing_stack_limit())
            .unwrap_or(0);
        let db = Path::new(&db);
        let conn = open_ro(db).expect("真库存只读打不开");
        let mut stmt = conn
            .prepare("SELECT account_id,id,name FROM characters WHERE deleted_at IS NULL ORDER BY id")
            .expect("schema 查询失败");
        let ids: Vec<(i64, i64, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .expect("角色查询失败")
            .filter_map(Result::ok)
            .collect();
        drop(stmt);
        println!("角色 {} 个：{:?}", ids.len(), ids);
        for (account, id, name) in &ids {
            match describe(db, &cat, missing, *account, *id) {
                Ok(d) => println!(
                    "账号 {} 角色 {} {} | 职业码 {} → {} | 转职 {} 觉醒 {} Lv.{} | 经验 {} 金币 {} 点券 {} | 背包 {} 行 | 错误 {:?}",
                    account,
                    id,
                    d.character.name,
                    d.character.profession,
                    d.character.class_name,
                    d.character.advancement,
                    d.character.awakening,
                    d.character.level,
                    d.character.experience,
                    d.character.gold,
                    d.cera,
                    d.bag.len(),
                    d.bag_error
                ),
                Err(e) => println!("账号 {} 角色 {} {} | 读失败：{}", account, id, name, e),
            }
        }
        if let Some(d) = ids.first().and_then(|(a, i, _)| describe(db, &cat, missing, *a, *i).ok()) {
            for row in &d.bag {
                println!(
                    "  {} 空间{} 槽{} 组{} 模板{} 名{} 数量{} 可改={} 修订{}…",
                    row.collection,
                    row.space,
                    row.slot,
                    row.group,
                    row.template,
                    row.name,
                    row.count,
                    row.can_edit_count,
                    &row.revision[..8]
                );
            }
        }
    }
}
