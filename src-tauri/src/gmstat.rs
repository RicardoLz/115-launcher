//! 等级 / 转职 / 觉醒 / 转职分支改写（route C 写侧）。
//!
//! 算法口径抄服务端那三件套：`ApplyGain`（`progression_state.go`）、`WireAdvancement`
//! 与 `ApplyAdvancement`（`advancement.go`）、`ApplyAwakening`（`awakening.go`），
//! 界面话术与前置校验抄 Go GM 的 `editCharacter`（`bridge_character.go`）。
//! 数据来自离线产物 `gm-growth.json`（`gmbuild build-growth` 直读 PVF 生成）加那份
//! 已经部署的 `experience.compat90.json` 里的 `level_cap`，运行期不碰 PVF。
//!
//! Go 在 `ApplyGain` 里还要 `Professions.Source.Checksum == Catalog.Source.Checksum`
//! （职业成长表与经验表同源）。这一份产物把两张表放在同一次构建里，等式由构造成立；
//! 剩下的两道身份门与 Go 同口径，且都必须过：
//!
//! 1. `characters.config_version` 等于存档契约身份（`sha256("dfolan-save-contract:v1")`），
//!    或者等于产物那次构建的归档校验（`BindGMIdentity` 允许的那条旧身份）；
//! 2. `state.source_sha256` 等于本职业 `.chr` 原文的 SHA-256（产物里的 `raw_sha256`）。
//!
//! 觉醒授予技能的成员判定与 `[required level]` 等级门，Go 用的是那份学习目录
//! （`Learning.index[profession][id]`）；本路线把它折进产物的 `awakening_levels`
//! —— 键缺席即「该技能不在本职业技能索引里」，与 Go 的 `!ok` 同一判据。
//!
//! 三处刻意不同，都写在这里而不是藏在代码里：
//!
//! - Go 把 `State` 结构体整份重编后合并回原 JSON，这里只写这次真正改动的键
//!   （等级/经验/属性/技能点，或转职/觉醒/技能账本），其余字段与其余容器原封不动 ——
//!   存档里那些服务端自己的私有键不该被我们重排。
//! - Go 读档时任何字段类型不对都整份 `Unmarshal` 失败，这里按字段宽松读取：
//!   这一笔要写的字段严格校验，其余（背包、任务、技能槽）不认识就留着不动。
//! - Go 的 `ApplyGain` 末尾跑 `EntryAddition`，其中 `knownSkills` 的技能投影在
//!   `Learning == nil` 时是空转（本路线正是这一档），真正生效的只有属性原生界内
//!   与已学技能非零这两道；`EquipmentFame`/`EquipmentSlotFlags`/穿戴明细解码不复现。

use std::collections::BTreeMap;
use std::path::Path;

use gmbuild::growth::{GrowthFile, GrowthProfession};
use gmbuild::gmindex::Catalog;
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::paths::Root;

/// Go `savecontract.Identity()`：存档契约身份，写死在 `characters.config_version` 里。
pub(crate) fn save_identity() -> String {
    let mut h = Sha256::new();
    h.update(b"dfolan-save-contract:v1");
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn is_hex64(v: &str) -> bool {
    v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Go `ApplyAwakening` 的等级门：阶段 1/2/3 各要 50/75/100 级。
const AWAKENING_LEVELS: [u8; 4] = [0, 50, 75, 100];

/// 一次养成改写要用的那份产物 + 等级上限。
pub struct Context {
    pub growth: GrowthFile,
    /// `experience.compat90.json` 的 `level_cap`：Go 的 `Rules.LevelCap`。
    pub level_cap: u8,
}

/// `progression.LoadRules` 的完整性检查：这一批只用到 `level_cap`，
/// 但那五个字段缺一项就是「这份兼容规则不是那一份」，照 Go 整份拒。
fn read_rules(dir: &Path) -> Result<u8, String> {
    let path = dir.join("experience.compat90.json");
    let raw = std::fs::read(&path).map_err(|e| format!("读取 {}：{}", path.display(), e))?;
    let v: Value =
        serde_json::from_slice(&raw).map_err(|e| format!("解析 {}：{}", path.display(), e))?;
    let cap = v
        .get("level_cap")
        .and_then(|x| x.as_u64())
        .filter(|n| (2..=u8::MAX as u64).contains(n))
        .ok_or_else(|| "经验兼容规则缺少 2 至 255 之间的 level_cap".to_string())? as u8;
    if v.get("model").and_then(|x| x.as_str()) != Some("reference90-solo-v1") {
        return Err("经验兼容规则不是 reference90-solo-v1".into());
    }
    let sha = v.get("reference_sha256").and_then(|x| x.as_str()).unwrap_or("");
    let mult = v.get("named_multiplier").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let penalty = v.get("monster_minus_character_level_rates").and_then(|x| x.as_object());
    if sha.len() != 64 || !(mult > 0.0) || penalty.map(|m| m.is_empty()).unwrap_or(true) {
        return Err("经验兼容规则不完整".into());
    }
    Ok(cap)
}

impl Context {
    /// 两份都得读到才开工：宁可拒绝写等级，也不要在没有成长表的情况下硬写。
    pub fn load(root: &Root) -> Result<Context, String> {
        let dir = gmbuild::gmindex::find_configs(&root.path)
            .ok_or_else(|| "未找到 GM 产物目录（gm-tool/configs）".to_string())?;
        let growth = gmbuild::growth::load_growth(&dir)?;
        let level_cap = read_rules(&dir)?;
        Ok(Context { growth, level_cap })
    }

    /// Go `BindGMIdentity` + `SaveIdentity()`：当前契约身份，或「存档来源精确等于
    /// 当前归档」的那条旧身份。一个进程只服务同一存档身份，Go 是绑一次，
    /// 这里是每次写前按这一行的 `config_version` 判。
    fn identity_ok(&self, config_version: &str) -> bool {
        config_version == save_identity()
            || (is_hex64(&self.growth.source.checksum) && config_version == self.growth.source.checksum)
    }

    /// Go `characterCatalog` 那三道门：存档身份、职业存在、来源指纹。
    /// 话术与 Go 一致，因为它就是这一批唯一的拒绝点。
    pub(crate) fn profession(&self, config_version: &str, profession: i64, state: &Value) -> Result<&GrowthProfession, String> {
        let mismatch = || "当前 PVF 职业目录与角色存档不匹配，已拒绝修改".to_string();
        if !self.identity_ok(config_version) {
            return Err(mismatch());
        }
        let prof = (0..=u8::MAX as i64)
            .contains(&profession)
            .then(|| self.growth.professions.get(&(profession as u8)))
            .flatten()
            .ok_or_else(mismatch)?;
        let source = state.get("source_sha256").and_then(|v| v.as_str()).unwrap_or("");
        if source != prof.raw_sha256 {
            return Err(mismatch());
        }
        Ok(prof)
    }
}

// ---------------------------------------------------------------------------
// 存档字段读取（Go 把它们解进 State 结构体：缺键或 null 是零值，形状不对整笔拒）
// ---------------------------------------------------------------------------

fn as_int(v: &Value, max: u64) -> Option<u64> {
    let n = v
        .as_u64()
        .or_else(|| v.as_i64().filter(|i| *i >= 0).map(|i| i as u64))?;
    (n <= max).then_some(n)
}

pub(crate) fn read_uint(state: &Value, key: &str, max: u64) -> Result<u64, String> {
    match state.get(key) {
        None | Some(Value::Null) => Ok(0),
        Some(v) => as_int(v, max).ok_or_else(|| format!("角色存档字段 {key} 不是 {max} 以内的整数")),
    }
}

/// Go 的 `[2]uint16`：多余元素丢掉，缺的元素留零，`null` 元素按零值。
fn read_pair(state: &Value, key: &str) -> Result<[u16; 2], String> {
    let mut out = [0u16; 2];
    match state.get(key) {
        None | Some(Value::Null) => {}
        Some(Value::Array(items)) => {
            for (i, raw) in items.iter().take(2).enumerate() {
                if raw.is_null() {
                    continue;
                }
                let n = as_int(raw, u16::MAX as u64)
                    .ok_or_else(|| format!("角色存档字段 {key} 第 {} 格不是 uint16", i + 1))?;
                out[i] = n as u16;
            }
        }
        Some(_) => return Err(format!("角色存档字段 {key} 不是数组")),
    }
    Ok(out)
}

fn map_of(doc: &mut Value) -> Result<&mut Map<String, Value>, String> {
    doc.as_object_mut().ok_or_else(|| "角色存档格式无效".to_string())
}

/// Go `State.Attributes map[string]float32`：缺席或 null 就是 nil（ApplyGain 据此拒绝），
/// 值不是数则整笔拒（Go 解不进 float32）。
fn read_attributes(state: &Value) -> Result<BTreeMap<String, f64>, String> {
    match state.get("attributes") {
        None | Some(Value::Null) => Err("missing source character attributes".into()),
        Some(Value::Object(fields)) => {
            let mut out = BTreeMap::new();
            for (k, v) in fields {
                let n = v
                    .as_f64()
                    .or_else(|| v.as_i64().map(|i| i as f64))
                    .ok_or_else(|| format!("属性 {k} 不是数值"))?;
                out.insert(k.clone(), n as f32 as f64);
            }
            Ok(out)
        }
        Some(_) => Err("missing source character attributes".into()),
    }
}

/// 与 Go 的 `float32` 编码同口径：先取 f32 的最短十进制表示再落成数，
/// 免得 0.1 这种值被 f64 展开成 0.10000000149011612。
fn number_of(v: f32) -> Value {
    serde_json::from_str(&format!("{v}")).unwrap_or_else(|_| Value::from(v as f64))
}

// ---------------------------------------------------------------------------
// EntryAddition 的属性/技能门（Go 只在 ApplyGain 末尾跑它，输出丢掉、错误照抛）
// ---------------------------------------------------------------------------

/// 逐字抄 Go 的结构体字面量求值顺序：后写的越界会覆盖前一次的 `failure`，
/// 所以「最后一条越界」才是真正报出来的那句。
struct Bounds {
    failure: Option<String>,
}

impl Bounds {
    fn scaled(&mut self, attrs: &BTreeMap<String, f64>, name: &str, max: f64) {
        let n = attrs.get(name).copied().unwrap_or(0.0) * 10.0;
        if !(n.is_finite()) || n < 0.0 || n > max {
            self.failure = Some(format!("attribute {name} out of native bounds"));
        }
    }

    /// 元素抗性、回复速度、受击恢复、跳跃力可为负，走这一档（int16）。
    fn signed(&mut self, attrs: &BTreeMap<String, f64>, name: &str) {
        let n = attrs.get(name).copied().unwrap_or(0.0) * 10.0;
        if !(n.is_finite()) || n < i16::MIN as f64 || n > i16::MAX as f64 {
            self.failure = Some(format!("attribute {name} out of signed native bounds"));
        }
    }
}

/// Go `knownSkills`：已学技能里出现 0 号 id 或 0 品级即判「invalid learned skill state」。
fn check_learned(doc: &Value) -> Result<(), String> {
    let trees = match doc.get("learned_skills") {
        None | Some(Value::Null) => return Ok(()),
        Some(Value::Array(list)) => list.clone(),
        Some(_) => return Err("角色存档字段 learned_skills 不是数组".into()),
    };
    for tree in trees.iter().take(2) {
        match tree {
            Value::Null => {}
            Value::Object(fields) => {
                for (key, level) in fields {
                    let id = key
                        .parse::<u32>()
                        .map_err(|_| format!("已学技能 id {key} 不在存档范围内"))?;
                    let rank = as_int(level, u8::MAX as u64)
                        .ok_or_else(|| "invalid learned skill state".to_string())?;
                    if id == 0 || rank == 0 {
                        return Err("invalid learned skill state".into());
                    }
                }
            }
            _ => return Err("角色存档字段 learned_skills 不是数组".into()),
        }
    }
    Ok(())
}

/// Go `EntryAddition` 里那串原生单位换算：只核对界内，不投影（见模块头第三条差异）。
fn entry_addition_gate(doc: &Value) -> Result<(), String> {
    let source = doc.get("source_sha256").and_then(|v| v.as_str()).unwrap_or("");
    if source.is_empty() {
        return Err("missing source character data".into());
    }
    let attrs = match doc.get("attributes") {
        Some(Value::Object(fields)) => {
            let mut out = BTreeMap::new();
            for (k, v) in fields {
                if let Some(n) = v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)) {
                    out.insert(k.clone(), n);
                }
            }
            out
        }
        _ => BTreeMap::new(),
    };
    let (u32_max, i32_max, u16_max) = (u32::MAX as f64, i32::MAX as f64, u16::MAX as f64);
    let mut b = Bounds { failure: None };
    b.scaled(&attrs, "[hp max]", u32_max);
    b.scaled(&attrs, "[mp max]", u32_max);
    b.scaled(&attrs, "[physical attack]", u16_max);
    b.scaled(&attrs, "[physical defense]", u16_max);
    b.scaled(&attrs, "[magical attack]", u16_max);
    b.scaled(&attrs, "[magical defense]", u16_max);
    for name in ["[fire resistance]", "[water resistance]", "[dark resistance]", "[light resistance]"] {
        b.signed(&attrs, name);
    }
    b.scaled(&attrs, "[inventory limit]", i32_max);
    b.signed(&attrs, "[hp regen speed]");
    b.signed(&attrs, "[mp regen speed]");
    b.scaled(&attrs, "[move speed]", u32_max);
    b.scaled(&attrs, "[attack speed]", u16_max);
    b.scaled(&attrs, "[cast speed]", u16_max);
    b.signed(&attrs, "[hit recovery]");
    b.signed(&attrs, "[jump power]");
    b.scaled(&attrs, "[weight]", i32_max);
    if let Some(e) = b.failure {
        return Err(e);
    }
    check_learned(doc)
}

// ---------------------------------------------------------------------------
// 转职 / 觉醒：WireAdvancement 与那两笔字段写
// ---------------------------------------------------------------------------

/// Go `State.WireAdvancement()`：分支 ≤15、阶段 ≤3，且阶段非零时分支必须已选。
fn wire_advancement(state: &Value) -> Result<(), String> {
    let adv = read_uint(state, "advancement", u8::MAX as u64)? as u8;
    let stage = read_uint(state, "awakening", u8::MAX as u64)? as u8;
    if adv > 15 || stage > 3 || (stage != 0 && adv == 0) {
        return Err("invalid advancement/stage".into());
    }
    Ok(())
}

/// Go `ApplyAdvancement`：换分支即回到未觉醒，只落 `advancement` + `awakening` 两键。
/// 返回 `false` 表示这一笔是幂等空转（Go 原样返回旧存档）。
/// Go 在这里的 `advancement source mismatch` 门（职业存在 + 存档身份 + 来源指纹）
/// 由 `Context::profession` 在分派之前统一判掉，四档动作同一口径，不在这里重判。
fn apply_advancement(doc: &mut Value, prof: &GrowthProfession, target: u8) -> Result<bool, String> {
    wire_advancement(doc)?;
    if target == 0 || target > 15 {
        return Err("invalid advancement target".into());
    }
    let adv = read_uint(doc, "advancement", u8::MAX as u64)? as u8;
    let stage = read_uint(doc, "awakening", u8::MAX as u64)? as u8;
    // Go 把空转判在身份门之前：已在该分支且未觉醒时连门都不必过。
    if target == adv && stage == 0 {
        return Ok(false);
    }
    if prof.advancement_growth.get(&target).map(|g| g.is_empty()).unwrap_or(true) {
        return Err("profession cannot advance to that branch".into());
    }
    let fields = map_of(doc)?;
    fields.insert("advancement".into(), Value::from(target as u64));
    fields.insert("awakening".into(), Value::from(0u64));
    Ok(true)
}

/// Go `forState`/`fillVariationSlots` 那一档：第三次觉醒要把 VP 面板铺满固定宽度
/// （三行强化 + 五行演进）。
fn fill_variation_slots(doc: &mut Value) -> Result<(), String> {
    let mut rows: [Map<String, Value>; 2] = [Map::new(), Map::new()];
    match doc.get("skill_variations") {
        None | Some(Value::Null) => {}
        Some(Value::Array(list)) => {
            for (i, raw) in list.iter().take(2).enumerate() {
                let fields = raw
                    .as_object()
                    .ok_or_else(|| "角色存档字段 skill_variations 格式无效".to_string())?;
                for (key, value) in fields {
                    rows[i].insert(key.clone(), value.clone());
                }
            }
        }
        Some(_) => return Err("角色存档字段 skill_variations 不是数组".into()),
    }
    let mut enhance = Map::new();
    enhance.insert("ID".into(), Value::from(0u64));
    enhance.insert("Choice".into(), Value::from(3u64));
    enhance.insert("Status".into(), Value::from(2u64));
    enhance.insert("Empty".into(), Value::from(0u64));
    let mut evolve = Map::new();
    evolve.insert("ID".into(), Value::from(0u64));
    evolve.insert("Choice".into(), Value::from(3u64));
    evolve.insert("Status".into(), Value::from(0u64));
    evolve.insert("Empty".into(), Value::from(1u64));

    // Go 的补位：强化（intension）三行、演进（option）五行，缺一律铺空白槽。
    let pads = [
        ("intensions", 3usize, enhance),
        ("options", 5usize, evolve),
    ];
    for row in rows.iter_mut() {
        // preset 为 0 时 Go 的 omitempty 会整个丢掉这一键，这里同口径。
        if row.get("preset").and_then(|v| v.as_u64()).unwrap_or(0) == 0 {
            row.remove("preset");
        }
        for (key, want, blank) in &pads {
            let mut list = match row.remove(*key) {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(items)) => {
                    let mut out = Vec::with_capacity(items.len());
                    for raw in items {
                        // Go 解进 protocol.SkillVariation：只认那四个键（大小写不敏感），其余丢掉。
                        let fields = raw
                            .as_object()
                            .ok_or_else(|| "技能变异记录格式无效".to_string())?;
                        let mut record = Map::new();
                        for (name, max) in [
                            ("ID", u16::MAX as u64),
                            ("Choice", u32::MAX as u64),
                            ("Status", u8::MAX as u64),
                            ("Empty", u8::MAX as u64),
                        ] {
                            let held = fields
                                .iter()
                                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                                .map(|(_, v)| v);
                            let n = match held {
                                None | Some(Value::Null) => 0,
                                Some(v) => as_int(v, max)
                                    .ok_or_else(|| format!("技能变异字段 {name} 无效"))?,
                            };
                            record.insert(name.to_string(), Value::from(n));
                        }
                        out.push(Value::Object(record));
                    }
                    out
                }
                Some(_) => return Err(format!("技能变异字段 {key} 不是数组")),
            };
            while list.len() < *want {
                list.push(Value::Object(blank.clone()));
            }
            if !list.is_empty() {
                row.insert(key.to_string(), Value::Array(list));
            }
        }
    }
    let arrays = rows.map(|row| Value::Object(row));
    let fields = map_of(doc)?;
    fields.insert("skill_variations".into(), Value::Array(arrays.to_vec()));
    Ok(())
}

/// Go `ApplyAwakening`：阶段只能一级一级往上走，跨过的那一档把该分支该阶段的
/// 觉醒技能按 `[required level]` 逐条授予。返回是否真的写了存档。
/// 与 `apply_advancement` 同理，Go 的 `awakening source mismatch` 门已由
/// `Context::profession` 统一判掉。
fn apply_awakening(doc: &mut Value, prof: &GrowthProfession, stage: u8) -> Result<bool, String> {
    wire_advancement(doc)?;
    let adv = read_uint(doc, "advancement", u8::MAX as u64)? as u8;
    let held = read_uint(doc, "awakening", u8::MAX as u64)? as u8;
    let level = read_uint(doc, "level", u8::MAX as u64)? as u8;
    if stage < 1 || stage > 3 || stage > held + 1 || adv == 0 {
        return Err("awakening must progress sequentially".into());
    }
    if stage <= held {
        return Ok(false);
    }
    if level < AWAKENING_LEVELS[stage as usize] {
        return Err("awakening level requirement not met".into());
    }
    let grants = match prof.awakening_skills.get(&adv).and_then(|m| m.get(&stage)) {
        Some(g) if !g.is_empty() && g.len() % 2 == 0 => g.clone(),
        _ => return Err("source awakening skills missing".into()),
    };
    // Go 的 `[2]map[uint16]byte`：`null` 是 nil map，只有真被授予时才实例化成 `{}`。
    let mut trees: [Option<BTreeMap<u16, u8>>; 2] = [None, None];
    match doc.get("learned_skills") {
        None | Some(Value::Null) => {}
        Some(Value::Array(list)) => {
            for (i, raw) in list.iter().take(2).enumerate() {
                let fields = match raw {
                    Value::Null => continue,
                    Value::Object(fields) => fields,
                    _ => return Err("角色存档字段 learned_skills 不是数组".into()),
                };
                let mut map = BTreeMap::new();
                for (key, value) in fields {
                    let id = key
                        .parse::<u16>()
                        .map_err(|_| format!("已学技能 id {key} 不在存档范围内"))?;
                    let rank = as_int(value, u8::MAX as u64)
                        .ok_or_else(|| format!("已学技能 {id} 的品级无效"))? as u8;
                    map.insert(id, rank);
                }
                trees[i] = Some(map);
            }
        }
        Some(_) => return Err("角色存档字段 learned_skills 不是数组".into()),
    }
    for pair in grants.chunks(2) {
        let (id, rank) = (pair[0], pair[1]);
        let entry = (1..=u16::MAX as i32).contains(&id)
            .then(|| (1..=255i32).contains(&rank).then_some(id as u16))
            .flatten();
        let required = match entry.and_then(|key| prof.awakening_levels.get(&key)) {
            Some(levels) => levels,
            None => return Err("invalid awakening skill grant".into()),
        };
        if required.len() != 1 {
            return Err("awakening skill level missing".into());
        }
        // 原文里那条 85 级的技能在二觉时只挂账不授予（Go 同口径）。
        if (level as i32) < required[0] {
            continue;
        }
        for tree in trees.iter_mut() {
            let map = tree.get_or_insert_with(BTreeMap::new);
            if map.get(&(id as u16)).copied().unwrap_or(0) < rank as u8 {
                map.insert(id as u16, rank as u8);
            }
        }
    }
    // 技巧点要在借走 `doc` 之前读（Go 用的是解进结构体的那一份）。
    let points = if stage == 3 { Some(read_pair(doc, "technique_points")?) } else { None };
    let fields = map_of(doc)?;
    fields.insert("awakening".into(), Value::from(stage as u64));
    fields.insert(
        "learned_skills".into(),
        Value::Array(
            trees
                .map(|t| match t {
                    None => Value::Null,
                    Some(map) => serde_json::to_value(map).unwrap_or(Value::Object(Map::new())),
                })
                .to_vec(),
        ),
    );
    if let Some(mut points) = points {
        // 第三次觉醒坐实 VP 面板：技巧点第一格记 5，并把变异槽铺满。
        points[0] = 5;
        fields.insert(
            "technique_points".into(),
            Value::Array(points.iter().map(|p| Value::from(*p as u64)).collect()),
        );
        fill_variation_slots(doc)?;
    }
    Ok(true)
}

// ---------------------------------------------------------------------------
// 等级：AddExperience + 按成长表补属性
// ---------------------------------------------------------------------------

/// Go `progression.AddExperience`。
fn add_experience(ctx: &Context, level: u8, total: u64, gain: u64) -> Result<(u8, u64, u32), String> {
    let cap = ctx.level_cap;
    let thresholds = &ctx.growth.thresholds;
    if level == 0 || level > cap || (cap as usize) - 1 > thresholds.len() || u64::MAX - total < gain {
        return Err("invalid experience ledger or cap".into());
    }
    if level > 1 && total < thresholds[level as usize - 2] {
        return Err("level exceeds cumulative experience".into());
    }
    let mut out_level = level;
    let out_exp = total + gain;
    let mut sp_gain = 0u32;
    while out_level < cap && out_exp >= thresholds[out_level as usize - 1] {
        out_level += 1;
        let sp = ctx
            .growth
            .skill_points
            .get(&(out_level as u16))
            .ok_or_else(|| "missing crossed-level SP row".to_string())?;
        sp_gain += *sp as u32;
    }
    Ok((out_level, out_exp, sp_gain))
}

/// Go `ProgressionService.ApplyGain`：把 `gain` 这点经验记进账本，
/// 每升一级按当前分支的成长表补属性，跨过的每一级补发技能点。
fn apply_gain(
    doc: &mut Value,
    ctx: &Context,
    prof: &GrowthProfession,
    gain: u64,
) -> Result<(), String> {
    let level = read_uint(doc, "level", u8::MAX as u64)? as u8;
    let total = read_uint(doc, "experience", u64::MAX)?;
    let adv = read_uint(doc, "advancement", u8::MAX as u64)? as u8;
    let mut attributes = read_attributes(doc)?;
    let mut points = read_pair(doc, "skill_points")?;

    let growth: &BTreeMap<String, f32> = if adv == 0 {
        &prof.base_growth
    } else {
        match prof.advancement_growth.get(&adv) {
            Some(g) if !g.is_empty() => g,
            // Go 在这里还有一条只有剑圣试点档才走的回落（`SwordmasterGrowth`）；
            // 产物里没有那一份表，本批也不给未证来源开后门，照「成长表为空」拒。
            _ => return Err("missing source profession growth".into()),
        }
    };
    if growth.is_empty() {
        return Err("missing source profession growth".into());
    }
    let (new_level, new_exp, sp_gain) = add_experience(ctx, level, total, gain)?;
    let levels = new_level as i64 - level as i64;
    if levels > 0 {
        if prof.base_growth.is_empty() {
            return Err("missing source base profession growth".into());
        }
        for (name, rate) in growth.iter() {
            let held = attributes.get(name).copied().unwrap_or(0.0);
            let next =
                (f64::round(held * 10.0) + levels as f64 * f64::round(*rate as f64 * 10.0)) / 10.0;
            attributes.insert(name.clone(), next as f32 as f64);
        }
    }
    for slot in points.iter_mut() {
        let next = *slot as u32 + sp_gain;
        if next > u16::MAX as u32 {
            return Err("skill point ledger overflow".into());
        }
        *slot = next as u16;
    }
    let fields = map_of(doc)?;
    fields.insert("level".into(), Value::from(new_level as u64));
    fields.insert("experience".into(), Value::from(new_exp));
    fields.insert(
        "skill_points".into(),
        Value::Array(points.iter().map(|p| Value::from(*p as u64)).collect()),
    );
    let mut attrs = Map::new();
    for (name, value) in attributes.iter() {
        attrs.insert(name.clone(), number_of(*value as f32));
    }
    fields.insert("attributes".into(), Value::Object(attrs));
    Ok(())
}

// ---------------------------------------------------------------------------
// 四个动作的分派（话术与前置校验抄 Go editCharacter）
// ---------------------------------------------------------------------------

const BAD_GROWTH_TARGET: &str = "目标等级必须高于当前等级且不超过服务器上限";
const EXPERIENCE_PASSED: &str = "当前经验已超过目标等级，请先检查存档";
const NEED_ADVANCEMENT: &str = "请先完成转职";
const STAGE_TOO_LOW: &str = "不能降低已完成的觉醒阶段";
const BRANCH_RANGE: &str = "转职分支必须在 1 至 15 之间";
const STAGE_RANGE: &str = "觉醒阶段必须在 1 至 3 之间";
const BRANCH_STAGE_INVALID: &str = "请选择有效的转职分支和觉醒阶段";
const BRANCH_UNSUPPORTED: &str = "当前职业不支持该转职分支";
const STAGE_LOWER: &str = "当前分支不能降低已完成的觉醒阶段";

/// 觉醒等级门那句话的原文（Go 用 `fmt.Errorf` 带级数）。
fn awakening_gate_text(stage: usize) -> String {
    format!("所选觉醒需要达到 {} 级", AWAKENING_LEVELS[stage])
}

/// 养成动作白名单：`gmgrant::normalize_manage` 与这里共用，避免两处口径漂移。
pub const GROWTH_ACTIONS: [&str; 4] = ["level", "advancement", "awakening", "profession"];

pub fn is_growth_action(action: &str) -> bool {
    GROWTH_ACTIONS.contains(&action)
}

/// 一次养成改写：返回（回写用的整份 state 字节，审计回执）。
/// `raw` 是库里那份原字节，空转时原样交回，保证写下去的也是同一份。
pub(crate) fn apply(
    ctx: &Context,
    raw: &[u8],
    profession: i64,
    config_version: &str,
    action: &str,
    value: u32,
    stage: Option<u8>,
) -> Result<(Vec<u8>, Value), String> {
    let state: Value =
        serde_json::from_slice(raw).map_err(|e| format!("角色存档不是合法 JSON：{e}"))?;
    // Go 的 characterCatalog 在这四档动作之前就把三道门跑完（成长目录与存档不同源直接拒）。
    let prof = ctx.profession(config_version, profession, &state)?;
    if !state.is_object() {
        return Err("角色存档格式无效".into());
    }
    let mut doc = state;
    let before = doc.clone();
    match action {
        "level" => {
            let level = read_uint(&doc, "level", u8::MAX as u64)? as u8;
            let total = read_uint(&doc, "experience", u64::MAX)?;
            let target = value as usize;
            if value <= level as u32
                || value > ctx.level_cap as u32
                || value < 2
                || target - 2 >= ctx.growth.thresholds.len()
            {
                return Err(BAD_GROWTH_TARGET.into());
            }
            let threshold = ctx.growth.thresholds[target - 2];
            if threshold < total {
                return Err(EXPERIENCE_PASSED.into());
            }
            apply_gain(&mut doc, ctx, prof, threshold - total)?;
            // Go 末尾拿 EntryAddition 当验收：属性越界或技能账本坏了整笔拒。
            entry_addition_gate(&doc)?;
        }
        "advancement" => {
            if value > 15 {
                return Err(BRANCH_RANGE.into());
            }
            // Go 在这里有一条「分支号没变就直接交回旧存档」的短路，
            // ApplyAdvancement 内部的幂等门覆盖同一语义，不必重复。
            apply_advancement(&mut doc, prof, value as u8)?;
        }
        "awakening" => {
            if value > 3 {
                return Err(STAGE_RANGE.into());
            }
            let adv = read_uint(&doc, "advancement", u8::MAX as u64)? as u8;
            let held = read_uint(&doc, "awakening", u8::MAX as u64)? as u8;
            let level = read_uint(&doc, "level", u8::MAX as u64)? as u8;
            if adv == 0 {
                return Err(NEED_ADVANCEMENT.into());
            }
            if value < held as u32 {
                return Err(STAGE_TOO_LOW.into());
            }
            if level < AWAKENING_LEVELS[value as usize] {
                return Err(awakening_gate_text(value as usize));
            }
            let mut next = held + 1;
            while next <= value as u8 {
                if !apply_awakening(&mut doc, prof, next)? {
                    break;
                }
                next += 1;
            }
        }
        "profession" => {
            let stage = stage.ok_or_else(|| BRANCH_STAGE_INVALID.to_string())?;
            if stage > 3 || value > 15 {
                return Err(BRANCH_STAGE_INVALID.into());
            }
            let level = read_uint(&doc, "level", u8::MAX as u64)? as u8;
            if level < AWAKENING_LEVELS[stage as usize] {
                return Err(awakening_gate_text(stage as usize));
            }
            if prof
                .advancement_growth
                .get(&(value as u8))
                .map(|g| g.is_empty())
                .unwrap_or(true)
            {
                return Err(BRANCH_UNSUPPORTED.into());
            }
            let held = read_uint(&doc, "awakening", u8::MAX as u64)? as u8;
            let adv = read_uint(&doc, "advancement", u8::MAX as u64)? as u8;
            let mut start = held;
            if adv != value as u8 {
                apply_advancement(&mut doc, prof, value as u8)?;
                start = 0;
            } else if stage < held {
                return Err(STAGE_LOWER.into());
            }
            let mut next = start + 1;
            while next <= stage {
                apply_awakening(&mut doc, prof, next)?;
                next += 1;
            }
        }
        other => return Err(format!("不支持的角色养成操作：{other}")),
    }
    if doc == before {
        return Ok((raw.to_vec(), receipt(action, value, stage, &before, &before)));
    }
    let bytes = serde_json::to_vec(&doc).map_err(|e| e.to_string())?;
    Ok((bytes, receipt(action, value, stage, &before, &doc)))
}

/// Go `editCharacter` 的回执形状：动作、值、阶段，加上改前与改后那两份完整存档。
fn receipt(action: &str, value: u32, stage: Option<u8>, before: &Value, after: &Value) -> Value {
    let mut out = Map::new();
    out.insert("action".into(), Value::from(action));
    out.insert("value".into(), Value::from(value as u64));
    out.insert(
        "stage".into(),
        stage.map(|s| Value::from(s as u64)).unwrap_or(Value::Null),
    );
    out.insert("before".into(), before.clone());
    out.insert("after".into(), after.clone());
    Value::Object(out)
}

// ---------------------------------------------------------------------------
// 界面用的可选清单（Go characterOptions 的养成那半）
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct GmBranchOption {
    pub branch: u8,
    /// 由客户端职业名表推导（Go 那边是写死的分支名表，同一份内容的来源不同）。
    pub name: String,
    /// 该分支可用的觉醒阶段：0 表示「未转职状态本身」，后面跟着产物里有授予表的 1..=3。
    pub stages: Vec<u8>,
}

#[derive(Serialize)]
pub struct GmGrowOptions {
    pub level: u8,
    pub experience: u64,
    pub level_cap: u8,
    pub advancement: u8,
    pub awakening: u8,
    /// 升到下一级还要多少经验；已在满级时给 0。
    pub next_level_experience: u64,
    pub branches: Vec<GmBranchOption>,
    /// 产物里的成长表指纹，界面上用它说明「这次写的是哪一份构建」。
    pub source_checksum: String,
    pub profession: i64,
    pub class_name: String,
}

/// 只读取可选清单：换分支要哪些等级、每一段有没有觉醒授予表，全从产物算。
pub fn options(
    ctx: &Context,
    cat: &Catalog,
    db: &Path,
    account: i64,
    character: i64,
) -> Result<GmGrowOptions, String> {
    if account <= 0 || character <= 0 {
        return Err("请选择账号和角色".into());
    }
    let conn = crate::gmchar::open_ro(db)?;
    let (profession, config_version, state_raw) = conn
        .query_row(
            "SELECT profession,config_version,state FROM characters \
             WHERE account_id=?1 AND id=?2 AND deleted_at IS NULL",
            rusqlite::params![account, character],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Vec<u8>>(2)?)),
        )
        .map_err(|e| format!("角色不存在：{e}"))?;
    let state: Value =
        serde_json::from_slice(&state_raw).map_err(|e| format!("角色存档不是可识别的 JSON：{e}"))?;
    ctx.profession(&config_version, profession, &state)?;
    let level = read_uint(&state, "level", u8::MAX as u64)? as u8;
    let experience = read_uint(&state, "experience", u64::MAX)?;
    let advancement = read_uint(&state, "advancement", u8::MAX as u64)? as u8;
    let awakening = read_uint(&state, "awakening", u8::MAX as u64)? as u8;
    // 门槛表第 k 格是「站到 k+2 级」的累计经验，所以下一级取 thresholds[level-1]。
    let next_level_experience = if level >= ctx.level_cap {
        0
    } else {
        ctx.growth
            .thresholds
            .get(usize::from(level).saturating_sub(1))
            .copied()
            .unwrap_or(0)
            .saturating_sub(experience)
    };
    let prof = ctx
        .growth
        .professions
        .get(&(profession as u8))
        .ok_or_else(|| "当前 PVF 职业目录与角色存档不匹配，已拒绝修改".to_string())?;
    let mut branches = Vec::new();
    for (branch, growth) in &prof.advancement_growth {
        if *branch == 0 || *branch > 15 || growth.is_empty() {
            continue;
        }
        let mut stages = vec![0u8];
        for stage in 1..=3u8 {
            let grants = prof
                .awakening_skills
                .get(branch)
                .and_then(|m| m.get(&stage))
                .cloned()
                .unwrap_or_default();
            if grants.is_empty() || grants.len() % 2 != 0 {
                break;
            }
            stages.push(stage);
        }
        branches.push(GmBranchOption {
            branch: *branch,
            name: cat.class_of_role_full(profession, i64::from(*branch), 0),
            stages,
        });
    }
    Ok(GmGrowOptions {
        level,
        experience,
        level_cap: ctx.level_cap,
        advancement,
        awakening,
        next_level_experience,
        branches,
        source_checksum: ctx.growth.source.checksum.clone(),
        profession,
        class_name: cat.class_of_role_full(profession, i64::from(advancement), i64::from(awakening)),
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn gm_growth_options(
    state: tauri::State<'_, crate::commands::AppState>,
    account: i64,
    character: i64,
) -> Result<GmGrowOptions, String> {
    let shared = state.gm.clone();
    let root = state.root.clone();
    crate::commands::join(move || {
        let catalog = shared.ensure(&root)?;
        let ctx = Context::load(&root)?;
        let db = crate::saves::current_db(&root)
            .ok_or_else(|| "存档还没有 SQLite 档：先启动一次游戏".to_string())?;
        options(&ctx, &catalog, &db, account, character)
    })
    .await
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

/// 夹具里 `.chr` 原文指纹与归档校验共用这一枚 64 位 hex，两条身份门都能过。
#[cfg(test)]
pub(crate) const TEST_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// 等级上限 5、门槛四档、每级 SP 3/4/5/6；职业 0 有分支 1、2。
/// 分支 1 的三段觉醒：1 段授 100，2 段授 101(75 级) 与 102(85 级)，3 段授 103(100 级)。
/// 发放事务侧（`gmgrant` 的端到端测试）共用这一份，免得两边夹具漂移。
#[cfg(test)]
pub(crate) fn test_context() -> Context {
    use gmbuild::growth::{GrowthProfession, SourceSnapshot};
    let mut base = std::collections::BTreeMap::new();
    base.insert("[hp max]".to_string(), 12.5f32);
    let mut advanced = std::collections::BTreeMap::new();
    advanced.insert("[hp max]".to_string(), 20.0f32);
    let mut stages1 = std::collections::BTreeMap::new();
    stages1.insert(1u8, vec![100i32, 1]);
    stages1.insert(2u8, vec![101i32, 1, 102i32, 1]);
    stages1.insert(3u8, vec![103i32, 1]);
    let mut stages2 = std::collections::BTreeMap::new();
    stages2.insert(1u8, vec![100i32, 2]);
    let mut awakening_skills = std::collections::BTreeMap::new();
    awakening_skills.insert(1u8, stages1);
    awakening_skills.insert(2u8, stages2);
    let mut awakening_levels = std::collections::BTreeMap::new();
    awakening_levels.insert(100u16, vec![50i32]);
    awakening_levels.insert(101u16, vec![75i32]);
    awakening_levels.insert(102u16, vec![85i32]);
    awakening_levels.insert(103u16, vec![100i32]);
    let growth = GrowthFile {
        source: SourceSnapshot {
            format: "dfo_fixture".into(),
            path: "Script.pvf".into(),
            size: 1,
            checksum: TEST_HASH.into(),
            built_at: "2026-10-21T00:00:00Z".into(),
        },
        thresholds: vec![100, 250, 500, 900],
        skill_points: std::collections::BTreeMap::from([
            (2u16, 3u16),
            (3u16, 4u16),
            (4u16, 5u16),
            (5u16, 6u16),
        ]),
        professions: std::collections::BTreeMap::from([(
            0u8,
            GrowthProfession {
                path: "character/swordman/swordman.chr".into(),
                raw_sha256: TEST_HASH.into(),
                job: "[swordman]".into(),
                base_growth: base,
                advancement_growth: std::collections::BTreeMap::from([(1u8, advanced.clone()), (2u8, advanced)]),
                awakening_skills,
                awakening_levels,
            },
        )]),
    };
    Context { growth, level_cap: 5 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const HASH: &str = TEST_HASH;

    fn ctx() -> Context {
        test_context()
    }

    /// 一份「服务端刚建好的 1 级角色」：属性表在、技能账本空、未转职。
    fn state(level: u8, exp: u64, tail: &str) -> String {
        format!(
            r#"{{"level":{level},"experience":{exp},"attributes":{{"[hp max]":100}},"skill_points":[0,0],"learned_skills":[null,null],"source_sha256":"{HASH}",{tail}}}"#
        )
    }

    /// 走一遍写侧，身份用当前契约（Go 的 SaveIdentity()）。
    fn attempt(action: &str, value: u32, stage: Option<u8>, json: &str) -> Result<(Vec<u8>, Value), String> {
        run(&save_identity(), action, value, stage, json)
    }

    fn run(
        config_version: &str,
        action: &str,
        value: u32,
        stage: Option<u8>,
        json: &str,
    ) -> Result<(Vec<u8>, Value), String> {
        apply(&ctx(), json.as_bytes(), 0, config_version, action, value, stage)
    }

    fn wrote(action: &str, value: u32, stage: Option<u8>, json: &str) -> Value {
        let (bytes, _) = attempt(action, value, stage, json).expect("应当写入");
        serde_json::from_slice(&bytes).unwrap()
    }

    fn refused(action: &str, value: u32, stage: Option<u8>, json: &str) -> String {
        attempt(action, value, stage, json).unwrap_err()
    }

    // ---- 身份门 ----

    #[test]
    fn identity_and_source_gates_refuse_every_action() {
        let json = state(1, 0, r#""advancement":0,"source_sha256":"deadbeef""#);
        // state.source_sha256 与产物那份 .chr 指纹不符。
        assert_eq!(
            run(&save_identity(), "level", 3, None, &json).unwrap_err(),
            "当前 PVF 职业目录与角色存档不匹配，已拒绝修改"
        );
        // config_version 既不是当前契约、也不是产物归档校验。
        let json = state(1, 0, r#""advancement":0"#);
        assert_eq!(
            run("0000000000000000000000000000000000000000000000000000000000000000", "level", 3, None, &json)
                .unwrap_err(),
            "当前 PVF 职业目录与角色存档不匹配，已拒绝修改"
        );
        // BindGMIdentity 允许的那条旧身份：等于产物归档校验，且是 64 位 hex。
        assert!(wrote("level", 3, None, &json)["level"] == 3);
        // 职业码在产物里没有。
        assert_eq!(
            apply(&ctx(), json.as_bytes(), 7, &save_identity(), "level", 3, None).unwrap_err(),
            "当前 PVF 职业目录与角色存档不匹配，已拒绝修改"
        );
    }

    // ---- 等级 ----

    #[test]
    fn level_uses_thresholds_and_base_growth() {
        let out = wrote("level", 3, None, &state(1, 0, r#""advancement":0"#));
        assert_eq!(out["level"], 3);
        assert_eq!(out["experience"], 250);
        // 跨过 2、3 两级：3 + 4 点技能点，两格账本都补。
        assert_eq!(out["skill_points"], serde_json::json!([7, 7]));
        // (round(100*10) + 2*round(12.5*10)) / 10 = 125
        assert_eq!(out["attributes"]["[hp max]"], 125);
        // 未动过的键一个字节都不许变。
        assert_eq!(out["source_sha256"], HASH);
        assert_eq!(out["learned_skills"], serde_json::json!([null, null]));
    }

    #[test]
    fn level_uses_branch_growth_once_advanced() {
        let out = wrote("level", 3, None, &state(1, 0, r#""advancement":1"#));
        // 已转职走 AdvancementGrowth[1]（每级 20）：(1000 + 2*200)/10 = 140
        assert_eq!(out["attributes"]["[hp max]"], 140);
    }

    #[test]
    fn level_to_cap_chains_every_crossed_row() {
        let out = wrote("level", 5, None, &state(1, 0, r#""advancement":0"#));
        assert_eq!(out["level"], 5);
        assert_eq!(out["experience"], 900);
        assert_eq!(out["skill_points"][0], 18);
        assert_eq!(out["attributes"]["[hp max]"], 150);
    }

    #[test]
    fn level_prechecks_match_go_wording() {
        let json = state(3, 0, r#""advancement":0"#);
        assert_eq!(refused("level", 3, None, &json), "目标等级必须高于当前等级且不超过服务器上限");
        assert_eq!(refused("level", 2, None, &json), "目标等级必须高于当前等级且不超过服务器上限");
        assert_eq!(refused("level", 6, None, &json), "目标等级必须高于当前等级且不超过服务器上限");
        // 等级 2 的门槛是 100，经验已经 300 ⇒ 存档经验跑到了目标等级之后。
        assert_eq!(
            refused("level", 2, None, &state(1, 300, r#""advancement":0"#)),
            "当前经验已超过目标等级，请先检查存档"
        );
    }

    #[test]
    fn level_domain_refusals() {
        // 属性表缺席 = Go 的 state.Attributes == nil。
        let no_attrs =
            format!(r#"{{"level":1,"experience":0,"skill_points":[0,0],"source_sha256":"{HASH}","advancement":0}}"#);
        assert_eq!(refused("level", 3, None, &no_attrs), "missing source character attributes");
        // 跨过的那一级没有 SP 行。
        let mut c = ctx();
        c.growth.skill_points.remove(&3u16);
        assert_eq!(
            apply(&c, state(1, 0, r#""advancement":0"#).as_bytes(), 0, &save_identity(), "level", 3, None)
                .unwrap_err(),
            "missing crossed-level SP row"
        );
        // 技能点两格都会溢出。
        let loaded = format!(
            r#"{{"level":1,"experience":0,"attributes":{{"[hp max]":100}},"skill_points":[65530,65530],"learned_skills":[null,null],"source_sha256":"{HASH}","advancement":0}}"#
        );
        assert_eq!(refused("level", 5, None, &loaded), "skill point ledger overflow");
        // 已转职但产物里那一段成长是空的。
        let mut c = ctx();
        c.growth
            .professions
            .get_mut(&0)
            .unwrap()
            .advancement_growth
            .insert(3u8, BTreeMap::new());
        assert_eq!(
            apply(&c, state(1, 0, r#""advancement":3"#).as_bytes(), 0, &save_identity(), "level", 3, None)
                .unwrap_err(),
            "missing source profession growth"
        );
    }

    #[test]
    fn entry_addition_gate_bounds_and_skill_ledger() {
        // 属性 ×10 后要落在原生单位内：hp max 是 u32，抗性是有符号 int16。
        let big = format!(
            r#"{{"level":1,"experience":0,"attributes":{{"[hp max]":{}}},"skill_points":[0,0],"learned_skills":[null,null],"source_sha256":"{HASH}","advancement":0}}"#,
            f64::from(u32::MAX) / 10.0 + 10.0
        );
        assert_eq!(
            refused("level", 3, None, &big),
            "attribute [hp max] out of native bounds"
        );
        let resist = format!(
            r#"{{"level":1,"experience":0,"attributes":{{"[hp max]":100,"[fire resistance]":{}}},"skill_points":[0,0],"learned_skills":[null,null],"source_sha256":"{HASH}","advancement":0}}"#,
            f64::from(i16::MAX) / 10.0 + 10.0
        );
        assert_eq!(
            refused("level", 3, None, &resist),
            "attribute [fire resistance] out of signed native bounds"
        );
        // 已学账本里 0 号技能或 0 品级都算坏档（Go knownSkills 的判据）。
        let broken = format!(
            r#"{{"level":1,"experience":0,"attributes":{{"[hp max]":100}},"skill_points":[0,0],"learned_skills":[{{"0":1}},null],"source_sha256":"{HASH}","advancement":0}}"#
        );
        assert_eq!(refused("level", 3, None, &broken), "invalid learned skill state");
        // 满级门那条：source_sha256 空着就是 Go 的「missing source character data」。
        let no_source = format!(
            r#"{{"level":1,"experience":0,"attributes":{{"[hp max]":100}},"skill_points":[0,0],"learned_skills":[null,null],"source_sha256":"","advancement":0}}"#
        );
        assert_eq!(
            run(&save_identity(), "level", 3, None, &no_source).unwrap_err(),
            "当前 PVF 职业目录与角色存档不匹配，已拒绝修改"
        );
        assert_eq!(
            entry_addition_gate(&serde_json::from_str::<Value>(&no_source).unwrap()).unwrap_err(),
            "missing source character data"
        );
    }

    // ---- 转职 ----

    #[test]
    fn advancement_writes_only_two_keys_and_resets_awakening() {
        let json = state(48, 0, r#""advancement":0,"awakening":0"#);
        let out = wrote("advancement", 1, None, &json);
        assert_eq!(out["advancement"], 1);
        assert_eq!(out["awakening"], 0);
        // 未转职的成长表与等级账本都不该被这一笔碰到。
        assert_eq!(out["level"], 48);
        assert_eq!(out["attributes"]["[hp max]"], 100);

        // 换分支：已在分支 1、二觉，选分支 2 回到未觉醒（Go 同语义）。
        let json = state(80, 0, r#""advancement":1,"awakening":2,"learned_skills":[{"100":1},{"100":1}],"technique_points":[5,0]"#);
        let out = wrote("advancement", 2, None, &json);
        assert_eq!(out["advancement"], 2);
        assert_eq!(out["awakening"], 0);
        assert_eq!(out["technique_points"][0], 5, "换分支不动技巧点账本，Go 也只落两键");

        // 幂等空转：已在该分支且未觉醒 ⇒ 原字节交回。
        let json = state(48, 0, r#""advancement":1,"awakening":0"#);
        let (bytes, receipt) = attempt("advancement", 1, None, &json).unwrap();
        assert_eq!(bytes, json.as_bytes());
        assert_eq!(receipt["before"], receipt["after"]);
    }

    #[test]
    fn advancement_refusals() {
        let json = state(48, 0, r#""advancement":0"#);
        assert_eq!(refused("advancement", 16, None, &json), "转职分支必须在 1 至 15 之间");
        assert_eq!(refused("advancement", 0, None, &json), "invalid advancement target");
        assert_eq!(
            refused("advancement", 9, None, &json),
            "profession cannot advance to that branch"
        );
        // 存档本身形状坏了（觉醒非零却没转职）：WireAdvancement 先拒。
        assert_eq!(
            refused("advancement", 1, None, &state(48, 0, r#""advancement":0,"awakening":2"#)),
            "invalid advancement/stage"
        );
    }

    // ---- 觉醒 ----

    #[test]
    fn awakening_grants_level_met_skills_in_both_trees() {
        let json = state(50, 0, r#""advancement":1,"awakening":0"#);
        let out = wrote("awakening", 1, None, &json);
        assert_eq!(out["awakening"], 1);
        assert_eq!(out["learned_skills"][0]["100"], 1);
        assert_eq!(out["learned_skills"][1]["100"], 1);
        // 一觉不碰技巧点与 VP 面板（那是三觉的事）。
        assert!(out.get("technique_points").is_none());
        assert!(out.get("skill_variations").is_none());

        // 二觉：85 级那条在本场（80 级）只挂账不授予。
        let json = state(80, 0, r#""advancement":1,"awakening":1,"learned_skills":[{"100":1},{"100":1}]"#);
        let out = wrote("awakening", 2, None, &json);
        assert_eq!(out["awakening"], 2);
        assert_eq!(out["learned_skills"][0]["101"], 1);
        assert_eq!(out["learned_skills"][0].get("102"), None);

        // 一次点到三觉：逐段走，最后铺满 VP 面板并把技巧点记成 5。
        let json = state(100, 0, r#""advancement":1,"awakening":0"#);
        let out = wrote("awakening", 3, None, &json);
        assert_eq!(out["awakening"], 3);
        assert_eq!(out["technique_points"], serde_json::json!([5, 0]));
        assert_eq!(out["learned_skills"][0]["103"], 1);
        let variations = out["skill_variations"].as_array().unwrap();
        assert_eq!(variations.len(), 2);
        assert_eq!(variations[0]["intensions"].as_array().unwrap().len(), 3);
        assert_eq!(variations[0]["options"].as_array().unwrap().len(), 5);
        assert_eq!(variations[0]["intensions"][0], serde_json::json!({"ID":0,"Choice":3,"Status":2,"Empty":0}));
        assert_eq!(variations[0]["options"][0], serde_json::json!({"ID":0,"Choice":3,"Status":0,"Empty":1}));
        assert_eq!(variations[0].get("preset"), None, "preset 为 0 时 Go 的 omitempty 丢键");
    }

    #[test]
    fn awakening_keeps_existing_variation_rows_and_pads_to_width() {
        let json = state(
            100,
            0,
            r#""advancement":1,"awakening":2,"learned_skills":[{"100":1},{"100":1}],"technique_points":[0,3],"skill_variations":[{"preset":1,"intensions":[{"id":9,"choice":1,"status":2,"empty":0}],"options":[]},{"preset":0,"intensions":[],"options":[]}]"#,
        );
        let out = wrote("awakening", 3, None, &json);
        let variations = out["skill_variations"].as_array().unwrap();
        assert_eq!(variations[0]["preset"], 1);
        // 已有那行按四个键重排（Go 解进结构体再编回来），大小写不敏感。
        assert_eq!(
            variations[0]["intensions"][0],
            serde_json::json!({"ID":9,"Choice":1,"Status":2,"Empty":0})
        );
        assert_eq!(variations[0]["intensions"].as_array().unwrap().len(), 3);
        assert!(variations[1].get("preset").is_none(), "第二棵树 preset 为 0 ⇒ 丢键");
        assert_eq!(variations[1]["options"].as_array().unwrap().len(), 5);
        assert_eq!(out["technique_points"][1], 3, "只改第一格");
    }

    #[test]
    fn awakening_refusals() {
        assert_eq!(
            refused("awakening", 1, None, &state(50, 0, r#""advancement":0"#)),
            "请先完成转职"
        );
        assert_eq!(refused("awakening", 4, None, &state(50, 0, r#""advancement":1"#)), "觉醒阶段必须在 1 至 3 之间");
        assert_eq!(
            refused("awakening", 1, None, &state(49, 0, r#""advancement":1"#)),
            "所选觉醒需要达到 50 级"
        );
        assert_eq!(
            refused("awakening", 2, None, &state(99, 0, r#""advancement":1,"awakening":3"#)),
            "不能降低已完成的觉醒阶段"
        );
        // 同段重放：不改字节。
        let json = state(50, 0, r#""advancement":1,"awakening":1,"learned_skills":[{"100":1},{"100":1}]"#);
        let (bytes, _) = attempt("awakening", 1, None, &json).unwrap();
        assert_eq!(bytes, json.as_bytes());

        // 产物里那一段没有授予表。
        assert_eq!(
            refused("awakening", 2, None, &state(100, 0, r#""advancement":2"#)),
            "source awakening skills missing"
        );
        // 授予的技能不在本职业技能索引里（Go 的 Learning !ok）。
        let mut c = ctx();
        c.growth
            .professions
            .get_mut(&0)
            .unwrap()
            .awakening_skills
            .get_mut(&2)
            .unwrap()
            .insert(1u8, vec![999i32, 1]);
        assert_eq!(
            apply(&c, state(50, 0, r#""advancement":2"#).as_bytes(), 0, &save_identity(), "awakening", 1, None)
                .unwrap_err(),
            "invalid awakening skill grant"
        );
        // [required level] 不是一个数。
        let mut c = ctx();
        c.growth
            .professions
            .get_mut(&0)
            .unwrap()
            .awakening_levels
            .insert(100u16, vec![50i32, 60i32]);
        assert_eq!(
            apply(&c, state(50, 0, r#""advancement":1"#).as_bytes(), 0, &save_identity(), "awakening", 1, None)
                .unwrap_err(),
            "awakening skill level missing"
        );
    }

    // ---- 转职分支一步到位 ----

    #[test]
    fn profession_switches_branch_then_walks_awakening() {
        let json = state(100, 0, r#""advancement":0,"awakening":0"#);
        let out = wrote("profession", 1, Some(3), &json);
        assert_eq!(out["advancement"], 1);
        assert_eq!(out["awakening"], 3);
        assert_eq!(out["technique_points"][0], 5);
        assert_eq!(out["learned_skills"][0]["103"], 1);

        // 换分支 + 只到一觉：先落分支（觉醒归零），再走一段。
        let json = state(60, 0, r#""advancement":2,"awakening":1,"learned_skills":[{"100":2},{"100":2}]"#);
        let out = wrote("profession", 1, Some(1), &json);
        assert_eq!(out["advancement"], 1);
        assert_eq!(out["awakening"], 1);
        // 分支 1 的一觉授 100 品级 1，分支 2 那一份是品级 2 ⇒ 取高者按新分支重算。
        assert_eq!(out["learned_skills"][0]["100"], 2, "已有品级更高时不降级");

        // 同分支降阶段：拒。
        assert_eq!(
            refused("profession", 1, Some(1), &state(100, 0, r#""advancement":1,"awakening":2"#)),
            "当前分支不能降低已完成的觉醒阶段"
        );
        // 同分支同阶段：空转，字节不动。
        let json = state(100, 0, r#""advancement":1,"awakening":1,"learned_skills":[{"100":1},{"100":1}]"#);
        let (bytes, _) = attempt("profession", 1, Some(1), &json).unwrap();
        assert_eq!(bytes, json.as_bytes());
    }

    #[test]
    fn profession_refusals() {
        let json = state(100, 0, r#""advancement":0"#);
        assert_eq!(refused("profession", 1, None, &json), "请选择有效的转职分支和觉醒阶段");
        assert_eq!(refused("profession", 1, Some(4), &json), "请选择有效的转职分支和觉醒阶段");
        assert_eq!(refused("profession", 16, Some(0), &json), "请选择有效的转职分支和觉醒阶段");
        assert_eq!(refused("profession", 9, Some(0), &json), "当前职业不支持该转职分支");
        assert_eq!(
            refused("profession", 1, Some(2), &state(60, 0, r#""advancement":0"#)),
            "所选觉醒需要达到 75 级"
        );
        // 阶段 0 没有等级门（Go 的表第 0 格是 0）。
        let out = wrote("profession", 1, Some(0), &state(20, 0, r#""advancement":0"#));
        assert_eq!(out["advancement"], 1);
        assert_eq!(out["awakening"], 0);
    }

    #[test]
    fn unknown_action_is_refused() {
        assert!(refused("hp", 1, None, &state(1, 0, r#""advancement":0"#)).starts_with("不支持的角色养成操作"));
        assert!(is_growth_action("level") && is_growth_action("profession"));
        assert!(!is_growth_action("sp") && !is_growth_action("inventory.amount"));
    }

    #[test]
    fn receipt_carries_both_archives_and_go_field_names() {
        let json = state(1, 0, r#""advancement":0"#);
        let (_, receipt) = attempt("level", 3, None, &json).unwrap();
        assert_eq!(receipt["action"], "level");
        assert_eq!(receipt["value"], 3);
        assert!(receipt["stage"].is_null(), "非 profession 动作不带阶段");
        assert_eq!(receipt["before"]["level"], 1);
        assert_eq!(receipt["after"]["level"], 3);
        let (_, receipt) = attempt("profession", 1, Some(1), &state(50, 0, r#""advancement":0"#)).unwrap();
        assert_eq!(receipt["stage"], 1);
    }

    /// 只读彩排：真库（或它的复制件）过一遍两道身份门，把每个角色的可选清单打出来。
    /// 不开写事务、不领租约，跑坏不了存档。
    /// 跑法：`DFO_REHEARSAL_ROOT=<路径> cargo test --lib live_growth -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_growth_options_on_real_pack() {
        let Ok(root_dir) = std::env::var("DFO_REHEARSAL_ROOT") else {
            eprintln!("跳过：未设 DFO_REHEARSAL_ROOT");
            return;
        };
        let root = Root {
            path: std::path::PathBuf::from(&root_dir),
            source: "彩排目录",
        };
        let ctx = Context::load(&root).expect("养成产物读取失败");
        eprintln!(
            "产物就绪：上限 {} 级，门槛 {} 档，职业 {} 个，归档校验 {}",
            ctx.level_cap,
            ctx.growth.thresholds.len(),
            ctx.growth.professions.len(),
            ctx.growth.source.checksum
        );
        let shared = crate::gm::GmShared::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(240);
        let catalog = loop {
            match shared.ensure(&root) {
                Ok(c) => break c,
                Err(stage) if stage == "加载中" || stage == "未加载" => {
                    assert!(std::time::Instant::now() < deadline, "产物加载超时");
                    std::thread::sleep(std::time::Duration::from_millis(400));
                }
                Err(stage) => panic!("产物加载失败：{stage}"),
            }
        };
        let db = crate::saves::current_db(&root).expect("彩排库里没有 SQLite 档");
        let chars = crate::gmgrant::characters(&root).unwrap();
        assert!(!chars.is_empty(), "彩排库里没有存活角色");
        let mut ok = 0usize;
        for c in &chars {
            match options(&ctx, &catalog, &db, c.account_id, c.id) {
                Ok(o) => {
                    ok += 1;
                    eprintln!(
                        "{}（账号 {}）{} Lv{}/{} 经验 {} 分支 {} 觉醒 {}，可选分支 {:?}，下一级还差 {}",
                        c.name,
                        c.account_id,
                        o.class_name,
                        o.level,
                        o.level_cap,
                        o.experience,
                        o.advancement,
                        o.awakening,
                        o.branches.iter().map(|b| b.branch).collect::<Vec<_>>(),
                        o.next_level_experience
                    );
                }
                Err(e) => eprintln!("{}（账号 {}）：门拒 — {e}", c.name, c.account_id),
            }
        }
        assert_eq!(ok, chars.len(), "有角色过不了身份门，产物与这批存档不同源");
    }
}
