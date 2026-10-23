//! 发放批：启动器直写 SQLite 存档的发放链路。
//!
//! 复刻 Go GM（servercompat）的事务契约：流水号幂等闸门（admin_grants 主键）、
//! 审计回执、账号所有权校验、ordinary-bag-v1 背包写入（金币 / 点券 / 账号晶体仓库 /
//! 可堆叠 / 时装与宠物专用容器 / 普通装备耐久）、租约文件尽力互斥。
//! 运行期不碰 PVF：可堆叠口径取启动器同款 items.index.json（与查询同一代际），
//! 装备口径取 configs 里现成的 equipment.current37.json，并沿用它与
//! inventory.current37.json 的同源校验（两件都是 Go 代际的导出）。
//! 三处刻意偏离，都朝安全一侧：
//!  - Go 的「发奖目录与背包规则同批次」校验被换成逐条存在性校验 —— 索引已换 Rust
//!    代际，天然不同源，照抄只会让所有发放都失败；
//!  - Go 的 ReadBag/SaveBag 会把 inventory 里未建模的字段抹掉（例如服务端新写入的
//!    creature_loyalty_updated_at），这里用 flatten 原样保留；
//!  - 限时可堆叠的过期戳（stackableAwardExpiry）要读 PVF 周期表，运行期拿不到，
//!    一律按永久（expire_time 缺省）发放。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use rusqlite::{params, Transaction, TransactionBehavior};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::commands::AppState;
use crate::gm::GmShared;
use crate::paths::Root;

const OPERATOR: &str = "launcher";
const DEFAULT_REASON: &str = "启动器发放";
const LEASE_TTL: std::time::Duration = std::time::Duration::from_secs(60);
/// 单进程内的发放互斥：租约文件是跨进程的，这一层只挡自家窗口连点。
static GRANT_BUSY: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// serde 小工具：与 Go 的 json 标签逐字对齐所需的谓词与映射键编解码
// ---------------------------------------------------------------------------

pub(crate) fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}
fn is_zero_u16(v: &u16) -> bool {
    *v == 0
}
pub(crate) fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}
fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}
fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}
fn is_empty_vec<T>(v: &Vec<T>) -> bool {
    v.is_empty()
}
fn is_blank_blob(v: &Option<Blob>) -> bool {
    v.as_ref().is_none_or(|b| b.0.is_empty())
}

/// Go 的 `map[byte][]BagEquipment` / `map[uint16]uint32` 在 JSON 里键是**十进制字符串**；
/// serde 默认把整数键编成字符（`u0001`），必须换成十进制串，否则服务端读不回。
/// （作为 `#[serde(with = "...")]` 模块使用，函数名即 serialize/deserialize。）
mod decimal_map_keys {
    use super::*;

    pub fn serialize<K, V, S>(map: &BTreeMap<K, V>, s: S) -> Result<S::Ok, S::Error>
    where
        K: std::fmt::Display + std::cmp::Ord,
        V: Serialize,
        S: Serializer,
    {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(Some(map.len()))?;
        for (k, v) in map {
            m.serialize_entry(&k.to_string(), v)?;
        }
        m.end()
    }

    pub fn deserialize<'de, K, V, D>(d: D) -> Result<BTreeMap<K, V>, D::Error>
    where
        K: std::str::FromStr + std::cmp::Ord,
        K::Err: std::fmt::Display,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        let raw = HashMap::<String, V>::deserialize(d)?;
        let mut out = BTreeMap::new();
        for (k, v) in raw {
            out.insert(k.parse::<K>().map_err(serde::de::Error::custom)?, v);
        }
        Ok(out)
    }

    /// 给 `Option<&BTreeMap<..>>` 字段用（Some 才写、且键要编成十进制串）。
    pub fn serialize_opt<K, V, S>(map: &Option<&BTreeMap<K, V>>, s: S) -> Result<S::Ok, S::Error>
    where
        K: std::fmt::Display + std::cmp::Ord,
        V: Serialize,
        S: Serializer,
    {
        match map {
            Some(m) => serialize(m, s),
            None => s.serialize_none(),
        }
    }
}

/// `[]byte` + omitempty 的等价物：Go 编码为 base64 字符串；解码同时接受
/// 字符串与数字数组（后者按字节还原），保证读得进任何写法、写得出 Go 形状。
#[derive(Clone, Debug, PartialEq)]
pub struct Blob(Vec<u8>);

impl Blob {
    fn len(&self) -> usize {
        self.0.len()
    }
    fn le_u32_at(&self, i: usize) -> Option<u32> {
        let b: &[u8] = &self.0;
        if i + 4 > b.len() {
            return None;
        }
        Some(u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]))
    }
}

/// `Option<Blob>` 的字段级编解码（新类型直接 impl Option 的 Trait 会与孤儿规则冲突）。
mod blob_field {
    use super::*;

    pub fn serialize<S: Serializer>(v: &Option<Blob>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(b) if !b.0.is_empty() => s.serialize_str(&b64_encode(&b.0)),
            _ => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Blob>, D::Error> {
        let v = Option::<Value>::deserialize(d)?;
        match v {
            None => Ok(None),
            Some(Value::String(s)) => b64_decoded(&s)
                .map(|b| Some(Blob(b)))
                .ok_or_else(|| serde::de::Error::custom("invalid base64 byte field")),
            Some(Value::Array(xs)) => {
                let mut out = Vec::with_capacity(xs.len());
                for x in xs {
                    let n = x.as_u64().ok_or_else(|| {
                        serde::de::Error::custom("invalid byte array element")
                    })?;
                    out.push(n as u8);
                }
                Ok(Some(Blob(out)))
            }
            Some(_) => Err(serde::de::Error::custom("invalid byte field")),
        }
    }
}

const B64_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64_CHARS[(n >> 18) as usize & 63] as char);
        out.push(B64_CHARS[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64_CHARS[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64_CHARS[n as usize & 63] as char } else { '=' });
    }
    out
}

fn b64_decoded(raw: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    if raw.len() % 4 != 0 {
        return None;
    }
    let bytes = raw.as_bytes();
    let pads = bytes.iter().rev().take_while(|c| **c == b'=').count();
    if pads > 2 {
        return None;
    }
    let body = &bytes[..bytes.len() - pads];
    if body.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut nbits: u32 = 0;
    for c in body {
        acc = (acc << 6) | val(*c)? as u32;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push(((acc >> nbits) & 0xff) as u8);
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// 背包规则（inventory.current37.json，Go 侧 LoadBagRules 的口径）
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
pub struct BagRules {
    model: String,
    source: String,
    slots: BTreeMap<String, [u16; 2]>,
    missing_stack_limit: u32,
    #[serde(default)]
    equipment_slots: Option<[u16; 2]>,
    #[serde(default)]
    quick_slots: Option<[u16; 2]>,
}

fn zero_range(v: &[u16; 2]) -> bool {
    v[0] == 0 && v[1] == 0
}

impl BagRules {
    fn from_json(raw: &str) -> Result<BagRules, String> {
        let r: BagRules = serde_json::from_str(raw)
            .map_err(|e| format!("解析背包规则失败：{e}"))?;
        if r.model != "reference90-bag-v1" || r.missing_stack_limit == 0 || r.source.len() != 64 {
            return Err("invalid bag policy".into());
        }
        // 槽位区间互不重叠：与服务端同一份校验，读坏了就拒绝发放。
        let mut seen: HashSet<u16> = HashSet::new();
        let mut take = |name: &str, v: &[u16; 2], allow_zero: bool| -> Result<(), String> {
            if zero_range(v) {
                return Ok(());
            }
            if (!allow_zero && v[0] == 0) || v[0] > v[1] || v[1] > 65534 {
                return Err(format!("invalid bag slot range: {name}"));
            }
            for n in v[0]..=v[1] {
                if !seen.insert(n) {
                    return Err("overlapping bag ranges".into());
                }
            }
            Ok(())
        };
        if let Some(q) = r.quick_slots {
            take("quick", &q, true)?;
            if let Some(e) = r.equipment_slots {
                if !zero_range(&e) && q[1] >= e[0] {
                    return Err("quick slots overlap the equipment bag".into());
                }
            }
        }
        if let Some(e) = r.equipment_slots {
            take("equipment", &e, false)?;
        }
        for (k, v) in &r.slots {
            take(k, v, false)?;
        }
        Ok(r)
    }

    pub(crate) fn missing_stack_limit(&self) -> u32 {
        self.missing_stack_limit
    }

    pub(crate) fn load(dir: &Path) -> Result<BagRules, String> {
        let path = dir.join("inventory.current37.json");
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| format!("缺少背包规则 {}：{e}", path.display()))?;
        BagRules::from_json(&raw)
    }
}

// ---------------------------------------------------------------------------
// 装备目录（equipment.current37.json，只服务装备发放的分类与耐久）
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
struct Token {
    #[serde(rename = "type", default)]
    kind: u8,
    #[serde(default)]
    value: i32,
    #[serde(default)]
    text: String,
}

#[derive(Clone, Debug, Deserialize)]
struct EquipRow {
    #[serde(rename = "ID")]
    id: u32,
    #[serde(rename = "Fields", default)]
    fields: HashMap<String, Vec<Token>>,
}

#[derive(Deserialize)]
struct EquipFile {
    source: EquipSource,
    rows: Vec<EquipRow>,
}

#[derive(Deserialize)]
struct EquipSource {
    checksum: String,
}

#[derive(Clone, Debug)]
pub struct EquipCatalog {
    by_id: HashMap<u32, EquipRow>,
}

fn durability_optional(kind: &str) -> bool {
    [
        "[amulet]",
        "[wrist]",
        "[ring]",
        "[earring]",
        "[support]",
        "[magic stone]",
        "[support weapon]",
        "[title name]",
        "[talisman]",
        "[creature]",
        "[amalgamation stone]",
        "[oath]",
        "[primer]",
        "[charm]",
        "[artifact red]",
        "[artifact blue]",
        "[artifact green]",
    ]
    .contains(&kind)
}

/// Go EquipmentBagSpace 的口径：后缀 avatar 归时装容器，宠物与宠物装备归 space 7。
fn equipment_bag_space(kind: &str) -> u8 {
    if kind.ends_with(" avatar]") {
        return 1;
    }
    match kind {
        "[creature]" | "[creature skin]" | "[artifact red]" | "[artifact blue]"
        | "[artifact green]" => 7,
        _ => 0,
    }
}

/// Go importTarget：从 [import script] 的路径取目标模板号（文件名就是模板 id）。
fn import_target(cells: &[Token]) -> Option<u32> {
    for t in cells {
        if t.kind != 6 {
            continue;
        }
        let name = match t.text.rsplit_once('/') {
            Some((_, tail)) => tail,
            None => t.text.as_str(),
        };
        let name = name.strip_suffix(".equ").unwrap_or(name);
        if let Ok(n) = name.parse::<u32>() {
            if n != 0 {
                return Some(n);
            }
        }
    }
    None
}

impl EquipCatalog {
    fn from_json(raw: &str, expect_source: &str) -> Result<EquipCatalog, String> {
        let file: EquipFile =
            serde_json::from_str(raw).map_err(|e| format!("解析装备目录失败：{e}"))?;
        if file.source.checksum != expect_source {
            // 与 Go LoadEquipmentCatalog 同一条闸：目录与背包规则必须同源。
            return Err("装备目录与背包规则不同源，拒绝对这份存档发放装备".into());
        }
        let mut by_id = HashMap::new();
        for r in file.rows {
            if r.id != 0 {
                by_id.insert(r.id, r);
            }
        }
        Ok(EquipCatalog { by_id })
    }

    fn load(dir: &Path, expect_source: &str) -> Result<EquipCatalog, String> {
        let path = dir.join("equipment.current37.json");
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| format!("缺少装备目录 {}：{e}", path.display()))?;
        EquipCatalog::from_json(&raw, expect_source)
    }

    /// Go definitionResolved：跟随 [import script] 链补 [rarity]/[equipment type]/[durability]。
    fn definition_resolved(&self, id: u32, depth: u8) -> Result<EquipRow, String> {
        let row = self
            .by_id
            .get(&id)
            .ok_or_else(|| format!("装备 {id} 不在装备目录里"))?;
        if depth >= 8 {
            return Err(format!("equipment import chain too deep at {id}"));
        }
        let target = import_target(row.fields.get("[import script]").map(|v| &v[..]).unwrap_or(&[]));
        let Some(target) = target.filter(|t| *t != id) else {
            return Ok(row.clone());
        };
        let Ok(base) = self.definition_resolved(target, depth + 1) else {
            // 基础取不到时退化为自身，保持原有报错行为（与服务端一致）。
            return Ok(row.clone());
        };
        let mut fields = row.fields.clone();
        for key in ["[rarity]", "[equipment type]", "[durability]"] {
            let has_own = fields.get(key).is_some_and(|v| !v.is_empty());
            if !has_own {
                if let Some(tokens) = base.fields.get(key) {
                    if !tokens.is_empty() {
                        fields.insert(key.to_string(), tokens.clone());
                    }
                }
            }
        }
        Ok(EquipRow { id, fields })
    }

    /// Go EquipmentCatalog.Reward：发放行的耐久与准入。
    fn reward(&self, id: u32) -> Result<u16, String> {
        let row = self.definition_resolved(id, 0)?;
        let rarity = row.fields.get("[rarity]").map(|v| &v[..]).unwrap_or(&[]);
        let kind = row.fields.get("[equipment type]").map(|v| &v[..]).unwrap_or(&[]);
        if rarity.len() != 1 || rarity[0].kind != 0 || rarity[0].value < 0 || kind.is_empty() {
            return Err("special equipment reward requires additional source state".into());
        }
        let dur = row.fields.get("[durability]").map(|v| &v[..]).unwrap_or(&[]);
        if dur.is_empty() {
            if durability_optional(&kind[0].text) {
                return Ok(0);
            }
            return Err("missing source equipment durability".into());
        }
        if dur.len() != 1 || dur[0].kind != 0 || dur[0].value < 0 || dur[0].value > 65535 {
            return Err("invalid equipment durability".into());
        }
        Ok(dur[0].value as u16)
    }

    /// 分类用：[equipment type] 首项文本（也走继承链，与 Go Grant 一致）。
    fn kind_text(&self, id: u32) -> Result<Option<String>, String> {
        let row = self.definition_resolved(id, 0)?;
        let kind = row.fields.get("[equipment type]").map(|v| &v[..]).unwrap_or(&[]);
        Ok(kind.first().map(|t| t.text.clone()))
    }
}

// ---------------------------------------------------------------------------
// ordinary-bag-v1
// ---------------------------------------------------------------------------

/// Go BagItem：slot/expire_time 小写，Template/Amount 无标签（大写序列化），
/// alias 兼容服务端小写行；未知键经 flatten 原样往返。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BagItem {
    #[serde(alias = "Slot")]
    pub slot: u16,
    #[serde(rename = "Template", alias = "template", default)]
    pub template: u32,
    #[serde(rename = "Amount", alias = "amount")]
    pub amount: u32,
    #[serde(
        alias = "ExpireTime",
        default,
        skip_serializing_if = "is_zero_u32"
    )]
    pub expire_time: u32,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Go BagEquipment：显式小写标签（实机行就是这个形状），record 等为 base64 []byte。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BagEquipment {
    #[serde(alias = "Slot")]
    pub slot: u16,
    #[serde(alias = "Template", default)]
    pub template: u32,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub group: u8,
    #[serde(default)]
    pub durability: u16,
    #[serde(default, with = "blob_field", skip_serializing_if = "is_blank_blob")]
    pub record: Option<Blob>,
    #[serde(
        rename = "avatar_options",
        default,
        with = "blob_field",
        skip_serializing_if = "is_blank_blob"
    )]
    pub avatar_options: Option<Blob>,
    #[serde(
        rename = "avatar_sockets",
        default,
        with = "blob_field",
        skip_serializing_if = "is_blank_blob"
    )]
    pub avatar_sockets: Option<Blob>,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub period: u32,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub refine: u8,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bag {
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub expansion: u8,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub gold: u32,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub coin: u32,
    /// Go 无 omitempty：空背包也序列化成 `[]`。
    #[serde(default)]
    pub items: Vec<BagItem>,
    #[serde(default, skip_serializing_if = "is_empty_vec")]
    pub equipment: Vec<BagEquipment>,
    #[serde(default, skip_serializing_if = "is_empty_vec")]
    pub worn: Vec<BagEquipment>,
    #[serde(
        rename = "special_equipment",
        default,
        serialize_with = "decimal_map_keys::serialize",
        deserialize_with = "decimal_map_keys::deserialize",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub special: BTreeMap<u8, Vec<BagEquipment>>,
    #[serde(
        rename = "expand_equip_flags",
        default,
        skip_serializing_if = "is_zero_u8"
    )]
    pub expand_equip_flags: u8,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Bag {
    fn default() -> Self {
        Bag {
            expansion: 0,
            version: "ordinary-bag-v1".into(),
            gold: 0,
            coin: 0,
            items: Vec::new(),
            equipment: Vec::new(),
            worn: Vec::new(),
            special: BTreeMap::new(),
            expand_equip_flags: 0,
            extra: Map::new(),
        }
    }
}

const EGG_HATCH: [(u32, u32); 12] = [
    (63006, 63000),
    (63007, 63003),
    (63013, 63008),
    (63014, 63009),
    (63015, 63010),
    (63016, 63011),
    (63017, 63012),
    (63018, 63019),
    (63022, 63023),
    (63024, 63025),
    (63028, 63027),
    (63031, 63032),
];

fn egg_hatch(template: u32) -> Option<u32> {
    EGG_HATCH
        .iter()
        .find(|(from, _)| *from == template)
        .map(|(_, to)| *to)
}

const RECORD_SIZE: usize = 181;

/// Go BagEquipment.ValidateRecord。record 空串/空数组等价于缺省（Go 的 []byte 空片）。
fn validate_record(it: &BagEquipment) -> Result<(), String> {
    if let Some(rec) = &it.record {
        if rec.len() != 0 && rec.len() != RECORD_SIZE {
            return Err("invalid equipment instance record".into());
        }
        if rec.len() == RECORD_SIZE
            && rec.le_u32_at(2).is_some_and(|t| t != it.template)
        {
            return Err("equipment instance template mismatch".into());
        }
    }
    for ext in [&it.avatar_options, &it.avatar_sockets] {
        if ext.as_ref().is_some_and(|b| b.len() > 4096) {
            return Err("equipment extension too large".into());
        }
    }
    Ok(())
}

/// Go ReadBag：从角色 state 解出背包（含银币折叠、蛋位改写与全部行校验）。
pub fn read_bag(state: &Value) -> Result<Bag, String> {
    let fields = state
        .as_object()
        .ok_or_else(|| "character state is not an object".to_string())?;
    let mut b: Bag = match fields.get("inventory") {
        Some(raw) => {
            let b: Bag =
                serde_json::from_value(raw.clone()).map_err(|_| "不支持的背包存档版本".to_string())?;
            if b.version != "ordinary-bag-v1" {
                return Err("unsupported saved inventory".into());
            }
            b
        }
        None => Bag::default(),
    };
    if b.expansion > 2 {
        return Err("背包扩展档位超出客户端范围".into());
    }
    // 银币行折叠进 coin 字段（Go 同口径：溢出时静默丢行，这里保持同样行为）。
    let mut kept: Vec<BagItem> = Vec::with_capacity(b.items.len());
    for i in b.items.drain(..) {
        if i.template == 1 {
            if let Some(sum) = b.coin.checked_add(i.amount) {
                b.coin = sum;
            }
            continue;
        }
        kept.push(i);
    }
    b.items = kept;
    let mut seen: HashSet<u16> = HashSet::new();
    for i in &b.items {
        if i.slot == 0 || i.slot == 1 || i.template == 0 || i.amount == 0 || !seen.insert(i.slot) {
            return Err("invalid saved inventory row".into());
        }
    }
    for i in &b.equipment {
        validate_record(i)?;
        if i.slot == 0 || i.template == 0 || !seen.insert(i.slot) {
            return Err("invalid saved equipment".into());
        }
    }
    let mut worn_seen: HashSet<u32> = HashSet::new();
    for idx in 0..b.worn.len() {
        {
            let i = &b.worn[idx];
            validate_record(i)?;
            if i.slot > 47 || i.template == 0 {
                return Err("invalid saved worn equipment".into());
            }
            if i.group > 1 || (i.group == 1 && i.slot > 11) {
                return Err("invalid worn equipment group".into());
            }
            let key = (i.group as u32) << 16 | i.slot as u32;
            if !worn_seen.insert(key) {
                return Err("invalid saved worn equipment".into());
            }
        }
        if b.worn[idx].slot == 26 {
            if let Some(hatched) = egg_hatch(b.worn[idx].template) {
                b.worn[idx].template = hatched;
            }
        }
    }
    for (space, rows) in &b.special {
        if *space != 1 && *space != 7 {
            return Err("unsupported equipment inventory".into());
        }
        let mut seen2: HashSet<u16> = HashSet::new();
        for i in rows {
            validate_record(i)?;
            if i.template == 0 || !seen2.insert(i.slot) {
                return Err("invalid special equipment".into());
            }
        }
    }
    Ok(b)
}

/// Go SaveBag：只替换 state 的 inventory 键，其余顶层键原样保留。
pub fn save_bag(state: &Value, bag: &Bag) -> Result<Vec<u8>, String> {
    let mut fields = state
        .as_object()
        .cloned()
        .ok_or_else(|| "character state is not an object".to_string())?;
    fields.insert(
        "inventory".into(),
        serde_json::to_value(bag).map_err(|e| e.to_string())?,
    );
    serde_json::to_vec(&Value::Object(fields)).map_err(|e| e.to_string())
}

fn normalize_stackable_type(s: &str) -> String {
    s.replace('`', "").trim().to_lowercase()
}

/// Go Bag.Add 的口径（金币/银币短路 + 可堆叠落格）；期限固定 0（见模块头偏差说明）。
pub fn bag_add(
    b: &mut Bag,
    rules: &BagRules,
    stack_type: Option<&str>,
    stack_limit: u32,
    id: u32,
    amount: u32,
) -> Result<u16, String> {
    if amount == 0 {
        return Err("invalid inventory award".into());
    }
    if id == 0 {
        let sum = (b.gold as u64) + (amount as u64);
        if sum > u32::MAX as u64 {
            return Err("gold overflow".into());
        }
        b.gold += amount;
        return Ok(0);
    }
    if id == 1 {
        let sum = (b.coin as u64) + (amount as u64);
        if sum > u32::MAX as u64 {
            return Err("coin overflow".into());
        }
        b.coin += amount;
        return Ok(1);
    }
    let typ = stack_type.unwrap_or_default();
    let slots = match rules.slots.get(typ) {
        Some(v) if !zero_range(v) => *v,
        _ => {
            if typ.to_lowercase().contains("material") {
                [121, 176]
            } else {
                [65, 120]
            }
        }
    };
    let limit = if stack_limit > 0 {
        stack_limit
    } else if normalize_stackable_type(typ).contains("unlimited") {
        i32::MAX as u32
    } else {
        rules.missing_stack_limit
    };
    if amount > limit {
        return Err("award exceeds stack limit".into());
    }
    let mut occupied: HashSet<u16> = HashSet::new();
    for i in &b.equipment {
        occupied.insert(i.slot);
    }
    for i in &b.items {
        occupied.insert(i.slot);
    }
    for row in b.items.iter_mut() {
        if row.template == id
            && row.slot >= slots[0]
            && row.slot <= slots[1]
            && (row.amount as u64) + (amount as u64) <= limit as u64
        {
            row.amount += amount;
            return Ok(row.slot);
        }
    }
    for n in slots[0]..=slots[1] {
        if occupied.insert(n) {
            b.items.push(BagItem {
                slot: n,
                template: id,
                amount,
                expire_time: 0,
                extra: Map::new(),
            });
            return Ok(n);
        }
    }
    Err("bag category is full".into())
}

/// Go Bag.addSpecialAward：专用容器（时装 210、宠物与宠物装备 140）。
pub fn add_special_award(b: &mut Bag, space: u8, id: u32, amount: u32) -> Result<Vec<u16>, String> {
    let capacity: u16 = if space == 7 { 140 } else { 210 };
    if amount == 0 || amount > capacity as u32 {
        return Err("专用背包发放数量超出容量".into());
    }
    let rows = b.special.entry(space).or_default();
    let mut occupied: HashSet<u16> = HashSet::new();
    for item in rows.iter() {
        occupied.insert(item.slot);
    }
    let mut slots: Vec<u16> = Vec::new();
    for slot in 0..capacity {
        if slots.len() as u32 == amount {
            break;
        }
        if occupied.insert(slot) {
            slots.push(slot);
        }
    }
    if slots.len() as u32 != amount {
        return Err("时装或宠物背包空间不足".into());
    }
    for slot in slots.clone() {
        rows.push(BagEquipment {
            slot,
            template: id,
            group: 0,
            durability: 0,
            record: None,
            avatar_options: None,
            avatar_sockets: None,
            period: 0,
            refine: 0,
            extra: Map::new(),
        });
    }
    Ok(slots)
}

/// Go Bag.AddEquipment：占用集是 items ∪ equipment，行按升序补齐。
pub fn add_equipment(
    b: &mut Bag,
    rules: &BagRules,
    id: u32,
    count: u32,
    durability: u16,
) -> Result<Vec<u16>, String> {
    let slots = rules.equipment_slots.unwrap_or([0, 0]);
    if slots[0] == 0 || slots[0] > slots[1] || count == 0 || count > (slots[1] - slots[0] + 1) as u32
    {
        return Err("invalid equipment award".into());
    }
    let mut occupied: HashSet<u16> = HashSet::new();
    for i in &b.items {
        occupied.insert(i.slot);
    }
    for i in &b.equipment {
        occupied.insert(i.slot);
    }
    let mut available: Vec<u16> = Vec::new();
    for n in slots[0]..=slots[1] {
        if available.len() as u32 == count {
            break;
        }
        if occupied.insert(n) {
            available.push(n);
        }
    }
    if available.len() as u32 != count {
        return Err("equipment bag is full".into());
    }
    for n in available.clone() {
        b.equipment.push(BagEquipment {
            slot: n,
            template: id,
            group: 0,
            durability,
            record: None,
            avatar_options: None,
            avatar_sockets: None,
            period: 0,
            refine: 0,
            extra: Map::new(),
        });
    }
    Ok(available)
}

// ---------------------------------------------------------------------------
// 账号晶体仓库（account_material_storage）
// ---------------------------------------------------------------------------

const ACCOUNT_MATERIAL_SLOTS: [(u32, u16); 19] = [
    (3033, 363),
    (3034, 364),
    (3035, 365),
    (3036, 366),
    (3037, 367),
    (3262, 368),
    (10100115, 369),
    (10100116, 370),
    (10099773, 371),
    (10099774, 372),
    (10099775, 373),
    (10158124, 374),
    (10361512, 375),
    (10361513, 376),
    (10361514, 377),
    (10361515, 378),
    (10361516, 379),
    (10415190, 0),
    (10415191, 1),
];

const ACCOUNT_MATERIALS_VERSION: &str = "account-materials-v1";

fn account_materials_version() -> String {
    ACCOUNT_MATERIALS_VERSION.to_string()
}

fn account_material_slot(template: u32) -> Option<u16> {
    ACCOUNT_MATERIAL_SLOTS
        .iter()
        .find(|(t, _)| *t == template)
        .map(|(_, s)| *s)
}

#[derive(Clone, Debug)]
pub struct AccountMaterials {
    pub version: String,
    pub counts: BTreeMap<u16, u32>,
}

#[derive(Serialize)]
struct AccountMaterialsWire<'a> {
    version: &'a str,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "decimal_map_keys::serialize_opt"
    )]
    counts: Option<&'a BTreeMap<u16, u32>>,
}

impl Serialize for AccountMaterials {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        AccountMaterialsWire {
            version: &self.version,
            counts: (!self.counts.is_empty()).then_some(&self.counts),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for AccountMaterials {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            // Go 是先 NewAccountMaterials()（Version 预置 v1）再 Unmarshal 进同一个结构，
            // 所以「没有 version 这个 key」保留预置值而不是空串。
            #[serde(default = "account_materials_version")]
            version: String,
            #[serde(default, deserialize_with = "decimal_map_keys::deserialize")]
            counts: BTreeMap<u16, u32>,
        }
        let w = Wire::deserialize(d)?;
        Ok(AccountMaterials {
            version: w.version,
            counts: w.counts,
        })
    }
}

#[derive(Clone, Serialize)]
struct AccountMaterialDelta {
    slot: u16,
    template: u32,
    amount: u32,
}

impl AccountMaterials {
    /// Go ReadAccountMaterials：容忍空档，校验版本与槽位，合并旧槽 381/382。
    fn from_raw(raw: Option<&[u8]>) -> Result<AccountMaterials, String> {
        let mut m = AccountMaterials {
            version: ACCOUNT_MATERIALS_VERSION.into(),
            counts: BTreeMap::new(),
        };
        if let Some(bytes) = raw.filter(|b| !b.is_empty() && *b != b"null") {
            m = serde_json::from_slice(bytes)
                .map_err(|_| "unsupported account material storage".to_string())?;
        }
        if m.version != ACCOUNT_MATERIALS_VERSION {
            return Err("unsupported account material storage".into());
        }
        for (from, to) in [(381u16, 0u16), (382, 1)] {
            if let Some(n) = m.counts.remove(&from) {
                let cur = m.counts.get(&to).copied().unwrap_or(0);
                let sum = (cur as u64) + (n as u64);
                if sum > u32::MAX as u64 {
                    return Err("account material overflow".into());
                }
                m.counts.insert(to, cur + n);
            }
        }
        for slot in m.counts.keys() {
            if !ACCOUNT_MATERIAL_SLOTS.iter().any(|(_, s)| s == slot) {
                return Err("invalid account material slot".into());
            }
        }
        m.counts.retain(|_, n| *n != 0);
        Ok(m)
    }

    fn add(&mut self, template: u32, amount: u32) -> Result<u16, String> {
        let slot = account_material_slot(template)
            .ok_or_else(|| "not an account material template".to_string())?;
        if amount == 0 {
            return Err("invalid account material amount".into());
        }
        let cur = self.counts.get(&slot).copied().unwrap_or(0);
        let sum = (cur as u64) + (amount as u64);
        if sum > u32::MAX as u64 {
            return Err("account material overflow".into());
        }
        self.counts.insert(slot, sum as u32);
        Ok(slot)
    }

    fn save(&self) -> Result<Vec<u8>, String> {
        if self.version != "account-materials-v1" {
            return Err("unsupported account material storage".into());
        }
        serde_json::to_vec(self).map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------------------
// 发放请求 / 回执（字段顺序与 Go json.Marshal 逐字对齐，流水号重试按字节比对）
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GrantItem {
    pub template: u32,
    pub amount: u32,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct GrantRequest {
    id: String,
    account_id: i64,
    #[serde(rename = "character_id", skip_serializing_if = "is_zero_i64")]
    character_id: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    delivery: String,
    #[serde(skip_serializing_if = "is_zero_i64")]
    cera: i64,
    #[serde(skip_serializing_if = "is_zero_u32")]
    gold: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    items: Vec<GrantItem>,
    reason: String,
    operator: String,
}

/// Go AwardReceipt 无标签 ⇒ 大写字段名，且没有 omitempty。
#[derive(Serialize)]
struct AwardReceipt {
    #[serde(rename = "Template")]
    template: u32,
    #[serde(rename = "Amount")]
    amount: u32,
    #[serde(rename = "Slots")]
    slots: Vec<u16>,
}

#[derive(Default, Serialize)]
struct Receipt {
    grant: String,
    #[serde(skip_serializing_if = "is_zero_u64")]
    cera_balance: u64,
    #[serde(rename = "gold_added", skip_serializing_if = "is_zero_u32")]
    gold: u32,
    #[serde(rename = "gold_after", skip_serializing_if = "is_zero_u32")]
    gold_after: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    items: Vec<AwardReceipt>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    materials: Vec<AccountMaterialDelta>,
    /// 这一笔覆盖 `characters.state` 之前落地的改前原文（绝对路径）。
    /// 没有覆盖那一列的动作（任务、仓库）与备份被跳过时留空、不写进 JSON。
    #[serde(skip_serializing_if = "String::is_empty")]
    state_backup: String,
}

// ---------------------------------------------------------------------------
// 管理租约（<库文件>.admin-guard，与服务端同一份文件协议）
// ---------------------------------------------------------------------------

fn guard_path(db: &Path) -> PathBuf {
    let mut s = db.as_os_str().to_os_string();
    s.push(".admin-guard");
    PathBuf::from(s)
}

fn read_lease(path: &Path) -> (u32, bool) {
    let Ok(raw) = std::fs::read(path) else {
        return (0, false);
    };
    let holder = String::from_utf8_lossy(&raw)
        .trim()
        .parse::<u32>()
        .unwrap_or(0);
    let age = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
        .unwrap_or_default();
    (holder, age < LEASE_TTL)
}

/// O_CREATE|O_EXCL 建租约：这一步本身就是跨进程互斥点（Go createLease 同口径）。
fn try_create_lease(path: &Path) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    f.write_all(std::process::id().to_string().as_bytes())?;
    f.flush()
}

struct LeaseGuard {
    path: Option<PathBuf>,
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        // 只删自己创建的那份（服务端同规则：删除是持有者的动作）。
        if let Some(p) = &self.path {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// HoldAdminGuard(editing=true) 的语义：尽力互斥。拿得到就占住，拿不到也放行。
fn take_lease_best_effort(db: &Path) -> LeaseGuard {
    let path = guard_path(db);
    let (_holder, held) = read_lease(&path);
    if held {
        return LeaseGuard { path: None };
    }
    match try_create_lease(&path) {
        Ok(()) => LeaseGuard { path: Some(path) },
        Err(_) => LeaseGuard { path: None },
    }
}

// ---------------------------------------------------------------------------
// 事务本体
// ---------------------------------------------------------------------------

const SQL_CLAIM: &str = "INSERT INTO admin_grants(grant_id,account_id,character_id,request,receipt,operator,reason) \
 VALUES(?1,?2,?3,?4,CAST('{}' AS BLOB),?5,?6) ON CONFLICT(grant_id) DO NOTHING";
const SQL_REPLAY: &str = "SELECT request=?2,receipt FROM admin_grants WHERE grant_id=?1";
const SQL_CERA_ENSURE: &str = "INSERT INTO account_currency(account_id,cera) VALUES(?1,0) ON CONFLICT (account_id) DO NOTHING";
const SQL_CERA_UPDATE: &str = "UPDATE account_currency SET cera=cera+?2,updated_at=(CAST((julianday('now') - 2440587.5) * 86400000000 AS INTEGER)) \
 WHERE account_id=?1 RETURNING cera";
const SQL_CERA_READ: &str = "SELECT coalesce((SELECT cera FROM account_currency WHERE account_id=?1), 0)";
const SQL_CHARACTER: &str = "SELECT id,account_id,state FROM characters WHERE account_id=?1 AND id=?2 AND deleted_at IS NULL";
/// 养成写侧的第二读：同一行的 profession 与 config_version（身份门要用）。
const SQL_STATE_WRITE: &str = "UPDATE characters SET state=?2 WHERE id=?1";
const SQL_MATERIALS_ENSURE: &str = "INSERT INTO account_material_storage(account_id) VALUES(?1) ON CONFLICT DO NOTHING";
const SQL_MATERIALS_READ: &str = "SELECT counts FROM account_material_storage WHERE account_id=?1";
const SQL_MATERIALS_SAVE: &str = "UPDATE account_material_storage SET counts=?2,updated_at=(CAST((julianday('now') - 2440587.5) * 86400000000 AS INTEGER)) WHERE account_id=?1";
const SQL_RECEIPT_WRITE: &str = "UPDATE admin_grants SET receipt=?2 WHERE grant_id=?1";

/// 分类结果：可堆叠自带落格口径（类型串 + 堆叠上限），装备按容器分流。
#[derive(Clone, PartialEq)]
pub enum GrantPlan {
    Stackable { type_text: Option<String>, stack_limit: u32 },
    Equipment,
    Special(u8),
}

/// 背包侧的发放执行物：金币 + 逐件物品（材料分流 → 可堆叠 → 装备容器），
/// 口径与 Go Receipt / Awarder.Grant 一致；错误同样带 `item N: ` 前缀。
fn apply_to_character(
    tx: &Transaction<'_>,
    g: &GrantRequest,
    state: &Value,
    rules: &BagRules,
    equip: &mut Option<EquipCatalog>,
    equip_dir: &Path,
    classify: &dyn Fn(u32) -> Result<GrantPlan, String>,
) -> Result<(Vec<u8>, Receipt), String> {
    let mut bag = read_bag(state)?;
    let mut r = Receipt {
        grant: g.id.clone(),
        ..Default::default()
    };
    let mut materials: Option<AccountMaterials> = None;
    if g.gold != 0 {
        bag_add(&mut bag, rules, None, 0, 0, g.gold)?;
        r.gold = g.gold;
        r.gold_after = bag.gold;
    }
    for it in &g.items {
        if it.template == 0 || it.amount == 0 {
            return Err("an item grant needs a real template and amount".into());
        }
        if account_material_slot(it.template).is_some() {
            // Go：账号材料模板必须仍是 stackable 定义，否则整笔拒绝。
            let stackable = matches!(classify(it.template)?, GrantPlan::Stackable { .. });
            if !stackable {
                return Err(format!("账号材料 {} 缺少有效物品定义", it.template));
            }
            if materials.is_none() {
                tx.execute(SQL_MATERIALS_ENSURE, params![g.account_id])
                    .map_err(|e| e.to_string())?;
                let raw: Vec<u8> = tx
                    .query_row(SQL_MATERIALS_READ, params![g.account_id], |row| row.get(0))
                    .map_err(|e| e.to_string())?;
                materials = Some(AccountMaterials::from_raw(Some(&raw))?);
            }
            let m = materials.as_mut().expect("just initialized");
            let slot = m
                .add(it.template, it.amount)
                .map_err(|e| format!("账号材料 {}：{e}", it.template))?;
            r.materials.push(AccountMaterialDelta {
                slot,
                template: it.template,
                amount: it.amount,
            });
            continue;
        }
        match classify(it.template)? {
            GrantPlan::Stackable { type_text, stack_limit } => {
                let slot = bag_add(
                    &mut bag,
                    rules,
                    type_text.as_deref(),
                    stack_limit,
                    it.template,
                    it.amount,
                )
                .map_err(|e| format!("item {}: {e}", it.template))?;
                r.items.push(AwardReceipt {
                    template: it.template,
                    amount: it.amount,
                    slots: vec![slot],
                });
            }
            GrantPlan::Special(space) => {
                let slots = add_special_award(&mut bag, space, it.template, it.amount)
                    .map_err(|e| format!("item {}: {e}", it.template))?;
                r.items.push(AwardReceipt {
                    template: it.template,
                    amount: it.amount,
                    slots,
                });
            }
            GrantPlan::Equipment => {
                if equip.is_none() {
                    *equip = Some(EquipCatalog::load(equip_dir, &rules.source)?);
                }
                let catalog = equip.as_ref().expect("loaded above");
                let durability = catalog
                    .reward(it.template)
                    .map_err(|e| format!("item {}: {e}", it.template))?;
                let slots = add_equipment(&mut bag, rules, it.template, it.amount, durability)
                    .map_err(|e| format!("item {}: {e}", it.template))?;
                r.items.push(AwardReceipt {
                    template: it.template,
                    amount: it.amount,
                    slots,
                });
            }
        }
    }
    if let Some(m) = &materials {
        let raw = m.save()?;
        tx.execute(SQL_MATERIALS_SAVE, params![g.account_id, raw])
            .map_err(|e| e.to_string())?;
    }
    let updated = save_bag(state, &bag)?;
    tx.execute(SQL_STATE_WRITE, params![g.character_id, updated])
        .map_err(|e| e.to_string())?;
    Ok((updated, r))
}

// ---------------------------------------------------------------------------
// 命令层
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
pub struct InventoryStatus {
    pub editable: bool,
    pub server_running: bool,
    pub management_busy: bool,
    pub message: String,
    pub db_path: String,
}

#[derive(Serialize)]
pub struct GmCharacter {
    pub id: i64,
    pub account_id: i64,
    pub username: String,
    pub name: String,
    pub profession: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct GmGrantResult {
    pub applied: bool,
    pub grant_id: String,
    pub message: String,
    pub note: String,
    pub receipt: Value,
}

fn open_rw(db: &Path) -> Result<rusqlite::Connection, String> {
    let conn = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("打不开存档（{}）：{e}", db.display()))?;
    conn.pragma_update(None, "busy_timeout", 5000)
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

fn current_db(root: &Root) -> Option<PathBuf> {
    crate::saves::current_db(root)
}

/// 覆盖 `characters.state` 之前，把**改前那一份**原样落一份文件（旧工具同口径）。
/// 为什么还要一份文件、`admin_grants.receipt` 里不是已经有 `before` 了：回执是**这一笔改了的行的结构化前后**，
/// 要照着它还原得先懂每种动作的 JSON 形状；这一份是那一列的原文，改回去就是把它写回 `characters.state`。
/// 写不出来就返回 Err，调用方回滚整笔——不许吞，也不许「备份没成功但也照样写」。
/// `None` 只给测试用：生产两条链路（`grant` / `manage`）都带落点。
fn backup_state_before(
    dir: Option<&Path>,
    account_id: i64,
    character_id: i64,
    before: &[u8],
) -> Result<Option<PathBuf>, String> {
    let Some(dir) = dir else {
        return Ok(None);
    };
    std::fs::create_dir_all(dir).map_err(|e| format!("角色存档备份目录建不出来：{e}"))?;
    // 文件名只用数字（账号 / 角色 / 时戳）：流水号与角色名都能带路径字符，别把它们拼进路径。
    let target = crate::saves::unique_backup_path(
        dir,
        "role",
        &format!("-{account_id}-{character_id}"),
        "json",
    );
    std::fs::write(&target, before).map_err(|e| {
        let _ = std::fs::remove_file(&target);
        format!("角色存档备份失败，整笔操作已取消：{e}")
    })?;
    Ok(Some(target))
}

/// 这一档会不会覆盖 `characters.state`。写法故意是**排除式**：任务五档与仓库改数明确不碰那一列
/// （见 `gmquest::apply` 与 `materials.set`），邮件只往 `character_mail` 插一行（见 `gmmail::send`），
/// 其余——包括以后新增的动作——一律先备份。
/// 宁可多留一份没人用的文件，也不能少一份能救命的。
fn writes_character_state(action: &str) -> bool {
    !crate::gmquest::is_quest_action(action)
        && action != "materials.set"
        && action != "mail.send"
}

/// 把备份落点盖进管理动作的回执（JSON 对象才盖；不是对象就说明这一档的回执形状有问题，不硬塞）。
fn stamp_state_backup(receipt: &mut Value, path: Option<&PathBuf>) {
    let Some(path) = path else {
        return;
    };
    if let Value::Object(map) = receipt {
        map.insert(
            "state_backup".to_string(),
            Value::String(path.display().to_string()),
        );
    }
}

pub fn inventory_status(root: &Root) -> InventoryStatus {
    let Some(db) = current_db(root) else {
        return InventoryStatus {
            editable: false,
            server_running: false,
            management_busy: false,
            message: "存档还没有 SQLite 档：先启动一次游戏，服务端会自己建库".into(),
            db_path: String::new(),
        };
    };
    let path = guard_path(&db);
    let (holder, held) = read_lease(&path);
    if GRANT_BUSY.load(Ordering::SeqCst) || (held && holder == std::process::id()) {
        return InventoryStatus {
            editable: false,
            server_running: false,
            management_busy: true,
            message: "另一项管理操作正在保存，请稍后重试".into(),
            db_path: db.display().to_string(),
        };
    }
    if held {
        return InventoryStatus {
            editable: false,
            server_running: true,
            management_busy: false,
            message: "检测到游戏服务端正在运行，请停止服务端后再修改背包；SQLite 存档文件无需停止"
                .into(),
            db_path: db.display().to_string(),
        };
    }
    InventoryStatus {
        editable: true,
        server_running: false,
        management_busy: false,
        message: String::new(),
        db_path: db.display().to_string(),
    }
}

pub fn characters(root: &Root) -> Result<Vec<GmCharacter>, String> {
    let db = current_db(root).ok_or_else(|| "存档还没有 SQLite 档：先启动一次游戏".to_string())?;
    let conn = rusqlite::Connection::open_with_flags(
        &db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("打不开存档（{}）：{e}", db.display()))?;
    let mut stmt = conn
        .prepare("SELECT c.id,c.account_id,a.username,c.name,c.profession FROM characters c \
                  JOIN accounts a ON a.id=c.account_id WHERE c.deleted_at IS NULL ORDER BY c.id")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(GmCharacter {
                id: row.get(0)?,
                account_id: row.get(1)?,
                username: row.get(2)?,
                name: row.get(3)?,
                profession: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// 发放流水一行：只读 `admin_grants`。界面默认只看流水号 / 动作 / 角色 / 时间，
/// `request` 与 `receipt` 要展开才铺（`receipt` 对管理动作是整行改前改后，能挺长）。
#[derive(Debug, Serialize)]
pub struct GmGrantRecord {
    pub grant_id: String,
    pub account_id: i64,
    /// 0 = 这笔是账号级的（只发点券时可以不选角色）。
    pub character_id: i64,
    /// 角色改名/删号或这一笔没角色时留空，界面退回显示角色号。
    pub character_name: String,
    /// 请求体里的 `action`；发放请求没有这一键，写 `grant`。
    pub action: String,
    pub operator: String,
    /// 建表默认值就是微秒时戳，原样交给界面格式化。
    pub created_at: i64,
    /// 请求体原文（`request` 列，那一份才是权威）。
    pub request: String,
    pub receipt: String,
}

/// 一次列多少笔：与旧工具同口径的「最近 100 条」，不做分页。
pub const GRANT_LOG_LIMIT: i64 = 100;

/// 动作键藏在哪一层：`request` 那列整份是 `GrantRequest` 的 JSON，它没有 `action` 键；
/// 管理操作把整份 `ManageInput` 序列化后塞进它的 `reason` 键里（`manage()` 就是这么拼的，
/// `reason` 列与这一个键同值），所以往里探一层就够。两处都没有 ⇒ 这是一笔发放，写 `grant`。
/// 只探一层，不递归：宁可少认一个动作，也不让一行脏数据把命令挂住。
fn grant_action(request: &Value) -> String {
    if let Some(action) = request.get("action").and_then(Value::as_str) {
        return action.to_string();
    }
    let nested = request.get("reason").and_then(Value::as_str).unwrap_or_default();
    if let Ok(doc) = serde_json::from_str::<Value>(nested) {
        if let Some(action) = doc.get("action").and_then(Value::as_str) {
            return action.to_string();
        }
    }
    "grant".to_string()
}

/// 读最近 `GRANT_LOG_LIMIT` 笔，新的在前。表不在这份存档里就报错，不静给空表——
/// 空表在这里和「一笔都没发过」长得一模一样，而这两件事该说的话完全不同。
pub fn grant_log(conn: &rusqlite::Connection) -> Result<Vec<GmGrantRecord>, String> {
    let present: i64 = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='admin_grants')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if present == 0 {
        return Err("这份存档里没有发放流水表 admin_grants（服务端审计表，由服务端建）".into());
    }
    let mut stmt = conn
        .prepare(
            "SELECT g.grant_id,g.account_id,g.character_id,c.name,g.operator, \
                    CAST(g.request AS TEXT),CAST(g.receipt AS TEXT),g.created_at \
             FROM admin_grants g LEFT JOIN characters c ON c.id=g.character_id \
             ORDER BY g.created_at DESC,g.rowid DESC LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([GRANT_LOG_LIMIT], |row| {
            let character_id: Option<i64> = row.get(2)?;
            let name: Option<String> = row.get(3)?;
            let request: String = row.get(5)?;
            // `request`/`receipt` 建表时是带 json_valid 约束的 BLOB，`CAST(... AS TEXT)` 只为拿原文——
            // 别的工具也可能按 TEXT 存。解不动就当脏数据交给界面，不因为一行把整条命令顶失败。
            let doc = serde_json::from_str::<Value>(&request).unwrap_or(Value::Null);
            Ok(GmGrantRecord {
                grant_id: row.get(0)?,
                account_id: row.get(1)?,
                character_id: character_id.unwrap_or(0),
                character_name: name.unwrap_or_default(),
                action: grant_action(&doc),
                operator: row.get(4)?,
                created_at: row.get(7)?,
                request,
                receipt: row.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// 与 `characters` 同一把只读钥匙：不领租约、不写一个字节，服务端在跑也读得动。
pub fn grant_log_at(root: &Root) -> Result<Vec<GmGrantRecord>, String> {
    let db = current_db(root).ok_or_else(|| "存档还没有 SQLite 档：先启动一次游戏".to_string())?;
    let conn = rusqlite::Connection::open_with_flags(
        &db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("打不开存档（{}）：{e}", db.display()))?;
    grant_log(&conn)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn gm_grant_log(state: tauri::State<'_, AppState>) -> Result<Vec<GmGrantRecord>, String> {
    let root = state.root.clone();
    crate::commands::join(move || grant_log_at(&root)).await
}

/// 发放本体：校验 → 租约 → 单事务（幂等闸门 / 点券 / 角色背包 / 审计回执）。
pub fn grant(root: &Root, shared: &GmShared, input: &GrantInput) -> Result<GmGrantResult, String> {
    let busy = BusyGuard::acquire()?;
    let id = input.id.trim().to_string();
    if id.len() < 16 || id.len() > 100 || !id.starts_with("launcher-") {
        return Err("发放缺少有效的操作流水号".into());
    }
    if input.account <= 0 {
        return Err("缺少账号".into());
    }
    if input.items.len() > 100 || input.cera < 0 {
        return Err("请选择有效的发放内容，单次最多 100 种物品".into());
    }
    if input.gold == 0 && input.cera == 0 && input.items.is_empty() {
        return Err("请选择有效的发放内容，单次最多 100 种物品".into());
    }
    for it in &input.items {
        if it.template == 0 || it.amount == 0 {
            return Err("物品及数量必须大于零".into());
        }
    }
    let has_bag_payout = input.gold != 0 || !input.items.is_empty();
    if has_bag_payout && input.character <= 0 {
        return Err("金币与物品发放必须选定角色".into());
    }
    let reason = {
        let t = input.reason.trim();
        if t.is_empty() {
            DEFAULT_REASON.to_string()
        } else {
            t.to_string()
        }
    };

    let g = GrantRequest {
        id: id.clone(),
        account_id: input.account,
        character_id: input.character,
        delivery: String::new(),
        cera: input.cera,
        gold: input.gold,
        items: input.items.clone(),
        reason,
        operator: OPERATOR.to_string(),
    };
    let request = serde_json::to_vec(&g).map_err(|e| e.to_string())?;

    let catalog = shared.ensure(root)?;
    let dir = gmbuild::gmindex::find_configs(&root.path).ok_or_else(|| "未找到 GM 产物目录".to_string())?;
    let rules = BagRules::load(&dir)?;
    let db = current_db(root).ok_or_else(|| "存档还没有 SQLite 档：先启动一次游戏".to_string())?;
    let _lease = take_lease_best_effort(&db);
    let mut conn = open_rw(&db)?;

    // 分类：物品库直接给出 stackable（含 type/limit）；其余走装备目录字段分容器。
    // 装备目录懒加载一次，供分类与耐久共用（Go 在发奖器里持有同一份）。
    let equip_cache: Mutex<Option<EquipCatalog>> = Mutex::new(None);
    let classify = |template: u32| -> Result<GrantPlan, String> {
        classify_with(&catalog, &equip_cache, &dir, &rules, template)
    };
    let result = grant_inner(
        &mut conn,
        &dir,
        &rules,
        &g,
        &request,
        &classify,
        Some(&crate::saves::role_state_backup_dir(root)),
    );
    drop(busy);
    result
}

fn classify_with(
    catalog: &gmbuild::gmindex::Catalog,
    equip_cache: &Mutex<Option<EquipCatalog>>,
    dir: &Path,
    rules: &BagRules,
    template: u32,
) -> Result<GrantPlan, String> {
    let item = catalog
        .get(template)
        .ok_or_else(|| format!("物品 {template} 不在物品库里"))?;
    if item.kind == gmbuild::gmindex::Kind::Stackable {
        return Ok(GrantPlan::Stackable {
            type_text: item.type_text.as_ref().map(|t| t.to_string()),
            stack_limit: item.stack_limit,
        });
    }
    let mut guard = equip_cache.lock().unwrap();
    if guard.is_none() {
        *guard = Some(EquipCatalog::load(dir, &rules.source)?);
    }
    let kind = guard
        .as_ref()
        .expect("loaded above")
        .kind_text(template)?
        .ok_or_else(|| "物品缺少装备类型".to_string())?;
    match equipment_bag_space(&kind) {
        s @ (1 | 7) => Ok(GrantPlan::Special(s)),
        _ => Ok(GrantPlan::Equipment),
    }
}

/// 幂等闸门的两种结局。
enum Claim {
    Fresh,
    Replay(Vec<u8>),
}

/// 先领流水号再付任何东西（发放与管理共用这一份，避免两条链路对幂等的理解漂移）：
/// 领到 = Fresh；号已在册且请求字节逐字相同 = Replay(原回执)；
/// 号已用于别的请求，或目标账号/角色未知 = Err（回滚由调用方做，它才持有事务）。
fn claim_flow(
    tx: &rusqlite::Transaction<'_>,
    g: &GrantRequest,
    request: &[u8],
    bad_target: &str,
    mismatch: &str,
) -> Result<Claim, String> {
    let character_arg: Option<i64> = if g.character_id != 0 { Some(g.character_id) } else { None };
    let claimed = tx
        .execute(
            SQL_CLAIM,
            params![g.id, g.account_id, character_arg, request, g.operator, g.reason],
        )
        .map_err(|e| format!("{bad_target}：{e}"))?;
    if claimed != 0 {
        return Ok(Claim::Fresh);
    }
    let (same, stored): (i64, Vec<u8>) = tx
        .query_row(SQL_REPLAY, params![g.id, request], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| e.to_string())?;
    if same == 0 {
        return Err(mismatch.to_string());
    }
    Ok(Claim::Replay(stored))
}

/// 发放本体：单事务里幂等闸门 → 点券 → 角色背包 → 审计回执。
pub(crate) fn grant_inner(
    conn: &mut rusqlite::Connection,
    dir: &Path,
    rules: &BagRules,
    g: &GrantRequest,
    request: &[u8],
    classify: &dyn Fn(u32) -> Result<GrantPlan, String>,
    state_backup: Option<&Path>,
) -> Result<GmGrantResult, String> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;

    let claim = claim_flow(&tx, g, request, "发放目标不是已知账号/角色", "流水号已用于不同发放");
    let replay = match claim {
        Ok(Claim::Fresh) => None,
        Ok(Claim::Replay(stored)) => Some(stored),
        Err(e) => {
            tx.rollback().ok();
            return Err(e);
        }
    };
    if let Some(stored) = replay {
        tx.commit().map_err(|e| e.to_string())?;
        let receipt: Value = serde_json::from_slice(&stored).unwrap_or(Value::Null);
        return Ok(GmGrantResult {
            applied: false,
            grant_id: g.id.clone(),
            message: "该笔发放已存在，未重复发放。".into(),
            note: ONLINE_NOTE.into(),
            receipt,
        });
    }

    let cera_after: u64;
    if g.cera != 0 {
        tx.execute(SQL_CERA_ENSURE, params![g.account_id])
            .map_err(|e| e.to_string())?;
        let new_balance: i64 = match tx.query_row(
            SQL_CERA_UPDATE,
            params![g.account_id, g.cera],
            |row| row.get(0),
        ) {
            Ok(v) => v,
            Err(e) => {
                tx.rollback().ok();
                return Err(format!("点券调整被拒（余额会被扣成负数？）：{e}"));
            }
        };
        if new_balance < 0 {
            tx.rollback().ok();
            return Err("点券调整被拒（余额会被扣成负数？）".into());
        }
        cera_after = new_balance as u64;
    } else {
        cera_after = tx
            .query_row(SQL_CERA_READ, params![g.account_id], |row| row.get::<_, i64>(0))
            .unwrap_or(0)
            .max(0) as u64;
    }

    let mut receipt = Receipt::default();
    if g.character_id != 0 {
        let row: (i64, Vec<u8>) = match tx.query_row(
            SQL_CHARACTER,
            params![g.account_id, g.character_id],
            |row| Ok((row.get(0)?, row.get(2)?)),
        ) {
            Ok(r) => r,
            Err(e) => {
                tx.rollback().ok();
                return Err(format!("发放角色不属于这个账号：{e}"));
            }
        };
        let state: Value = match serde_json::from_slice::<Value>(&row.1) {
            Ok(v) => v,
            Err(e) => {
                tx.rollback().ok();
                return Err(format!("角色存档不是合法 JSON：{e}"));
            }
        };
        // 耐久用的装备目录在这里懒加载；分类侧另有自己的缓存（同一份文件，读两次不贵）。
        let mut equip_slot: Option<EquipCatalog> = None;
        // 与写侧任何一笔同样：先把这一列的改前原文落文件，落不出来整笔拒（事务里此刻只有
        // `claim_flow` 那个未提交的 INSERT，回滚之后库里不留痕）。
        let backup = match backup_state_before(state_backup, g.account_id, g.character_id, &row.1) {
            Ok(v) => v,
            Err(e) => {
                tx.rollback().ok();
                return Err(e);
            }
        };
        let (_updated, r) = match apply_to_character(
            &tx,
            g,
            &state,
            rules,
            &mut equip_slot,
            dir,
            classify,
        ) {
            Ok(v) => v,
            Err(e) => {
                tx.rollback().ok();
                return Err(e);
            }
        };
        receipt = r;
        if let Some(path) = backup {
            receipt.state_backup = path.display().to_string();
        }
    }
    // 与 Go 相同：最终回执覆盖 grant 与 cera_balance 两格再落审计。
    receipt.grant = g.id.clone();
    receipt.cera_balance = cera_after;
    let blob = serde_json::to_vec(&receipt).map_err(|e| e.to_string())?;
    tx.execute(SQL_RECEIPT_WRITE, params![g.id, blob.clone()])
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;

    let mut message = "发放成功。".to_string();
    if !receipt.materials.is_empty() {
        message = "发放成功，晶体与灵魂材料已存入账号晶体仓库。".into();
    }
    Ok(GmGrantResult {
        applied: true,
        grant_id: g.id.clone(),
        message,
        note: ONLINE_NOTE.into(),
        receipt: serde_json::from_slice(&blob).map_err(|e| e.to_string())?,
    })
}

const ONLINE_NOTE: &str =
    "游戏内的角色如果正在线上，需要重新选择角色（或小退再进）才能看到新的金币/物品。";

struct BusyGuard(());
impl BusyGuard {
    fn acquire() -> Result<BusyGuard, String> {
        if GRANT_BUSY
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("另一项管理操作正在保存，请稍后重试".into());
        }
        Ok(BusyGuard(()))
    }
}
impl Drop for BusyGuard {
    fn drop(&mut self) {
        GRANT_BUSY.store(false, Ordering::SeqCst);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GrantInput {
    pub id: String,
    pub account: i64,
    #[serde(default)]
    pub character: i64,
    #[serde(default)]
    pub gold: u32,
    #[serde(default)]
    pub cera: i64,
    #[serde(default)]
    pub items: Vec<GrantItem>,
    #[serde(default)]
    pub reason: String,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn gm_inventory_status(
    state: tauri::State<'_, AppState>,
) -> Result<InventoryStatus, String> {
    let root = state.root.clone();
    crate::commands::block(move || inventory_status(&root)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn gm_characters(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<GmCharacter>, String> {
    let root = state.root.clone();
    crate::commands::join(move || characters(&root)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn gm_grant(
    id: String,
    account: i64,
    character: Option<i64>,
    gold: Option<u32>,
    cera: Option<i64>,
    items: Option<Vec<GrantItem>>,
    reason: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<GmGrantResult, String> {
    let root = state.root.clone();
    let shared = state.gm.clone();
    let input = GrantInput {
        id,
        account,
        character: character.unwrap_or(0),
        gold: gold.unwrap_or(0),
        cera: cera.unwrap_or(0),
        items: items.unwrap_or_default(),
        reason: reason.unwrap_or_default(),
    };
    crate::commands::join(move || grant(&root, &shared, &input)).await
}

// ---------------------------------------------------------------------------
// 管理操作：背包数量/删除与账号晶体仓库（抄 Go cmd/gmtool/bridge_manage.go 的 manage）
// ---------------------------------------------------------------------------

const DEFAULT_MANAGE_REASON: &str = "启动器管理操作";

/// Go manageRequest 里这一批用得到的字段（任务、归档角色不在这一批）。
/// `Serialize` 出来的字符串整份进审计的 reason 列，与 Go 的 `json.Marshal(req)` 同形。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", default)]
pub struct ManageInput {
    pub action: String,
    pub id: String,
    pub account: i64,
    pub character: i64,
    pub template: u32,
    pub amount: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_amount: Option<u32>,
    /// `equipment.durability` 的第三把钥匙：界面读到的当前耐久。
    /// 与 `expected_amount`（那一行的数量）分开，是因为装备行的数量根本不是用户看的东西。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_durability: Option<u32>,
    pub reason: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub revision: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub collection: String,
    #[serde(skip_serializing_if = "is_zero_u8")]
    pub space: u8,
    #[serde(skip_serializing_if = "is_zero_u16")]
    pub slot: u16,
    #[serde(skip_serializing_if = "is_zero_u8")]
    pub group: u8,
    /// 转职分支改写（`profession`）那一条要带的觉醒阶段；其余动作不写这一键。
    /// 与 Go `characterEdit.Stage *byte json:"stage,omitempty"` 同形。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<u8>,
    /// `quests.*` 点名的任务 id（1 至 100 个）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub quests: Vec<u16>,
    /// `quests.mainline` 的目标等级：1 至角色当前等级。
    #[serde(skip_serializing_if = "is_zero_u8")]
    pub level: u8,
    /// 计划类动作（`quests.mainline` / `quests.prerequisites`）必须带预览算出的签名，
    /// 与事务内重算的那一份逐字相同才落库。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub plan: String,
    /// `mail.send` 的金币附件（0 = 不带金币）。刻意不复用 `GrantRequest.gold`：
    /// 那一格是「直接进背包」，这一格是「装进邮件由玩家领取」，审计里两份各记各的。
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub gold: u32,
    /// `mail.send` 的正文。上限是 `character_mail.body` 的 CHECK（512 字节），见 `gmmail`。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub body: String,
}

#[derive(Clone, Serialize)]
pub struct GmMaterialRow {
    pub slot: u16,
    pub template: u32,
    pub name: String,
    pub amount: u32,
}

/// Go bridge.go 的 materials 读侧：19 个仓库槽位逐个给出（没有也算 0），名字查物品库。
pub fn materials(root: &Root, cat: &gmbuild::gmindex::Catalog, account: i64) -> Result<Vec<GmMaterialRow>, String> {
    if account <= 0 {
        return Err("请选择账号".into());
    }
    let db = current_db(root).ok_or_else(|| "存档还没有 SQLite 档：先启动一次游戏".to_string())?;
    let conn = crate::gmchar::open_ro(&db).map_err(|e| e.to_string())?;
    let raw: Vec<u8> = conn
        .query_row(SQL_MATERIALS_READ, params![account], |row| row.get(0))
        .unwrap_or_default();
    // 没有这一行 = 仓库还没建，Go 那边按 ErrNoRows 走空档。
    let materials = AccountMaterials::from_raw((!raw.is_empty()).then_some(raw.as_slice()))?;
    Ok(ACCOUNT_MATERIAL_SLOTS
        .iter()
        .map(|(template, slot)| GmMaterialRow {
            slot: *slot,
            template: *template,
            name: cat
                .get(*template)
                .filter(|it| !it.name.is_empty())
                .map(|it| it.name.to_string())
                .unwrap_or_else(|| format!("材料 {template}")),
            amount: materials.counts.get(slot).copied().unwrap_or(0),
        })
        .collect())
}

/// 参数校验与规范化：流水号形状、目标、动作白名单、原物品快照四件套。
/// 单独成函数是因为它是纯的 —— 测试能逐条钉住拒绝话，不必造 Root 与产物。
pub(crate) fn normalize_manage(input: &ManageInput) -> Result<ManageInput, String> {
    let id = input.id.trim().to_string();
    if id.len() < 16 || id.len() > 100 || !id.starts_with("launcher-") {
        return Err("管理操作缺少有效的流水号".into());
    }
    if input.account <= 0 || input.character <= 0 {
        return Err("请选择账号和角色".into());
    }
    let bag_edit = input.action == "inventory.amount" || input.action == "inventory.delete";
    // 装备行改耐久：Go 的「装备行只改耐久」那一档，乐观锁比背包多一把钥匙（当前耐久）。
    let durability_edit = input.action == "equipment.durability";
    // Go editCharacter 里那一档「不需要成长目录」的数值改写：SP / TP / 装备栏解锁。
    let numeric = matches!(input.action.as_str(), "sp" | "tp" | "equipment.unlock");
    // 等级 / 转职 / 觉醒 / 转职分支：要成长产物，见 `gmstat`。
    let growth = crate::gmstat::is_growth_action(&input.action);
    // SP「按当前等级重算为满值」：不看用户填的数，判据全在成长产物的每级授予表里。
    let sp_recalc = input.action == "sp.recalculate";
    // 任务五个动作：判据全在离线产物 `gm-quests.json`，见 `gmquest`。
    let quest = crate::gmquest::is_quest_action(&input.action);
    // 邮件发放：金币 + 至多一件物品，只往 `character_mail` 插一行，见 `gmmail`。
    let mail = input.action == "mail.send";
    if !bag_edit
        && !durability_edit
        && input.action != "materials.set"
        && !numeric
        && !sp_recalc
        && !growth
        && !quest
        && !mail
    {
        return Err("不支持的管理操作".into());
    }
    // Go 的请求门：追溯前置也要走这一条，所以「恰好一个 id」留到事务里再判。
    if quest
        && input.action != "quests.mainline"
        && (input.quests.is_empty() || input.quests.len() > 100)
    {
        return Err("单次请选择 1 至 100 个任务".into());
    }
    if (numeric || growth) && input.amount == 0 {
        return Err("要写入的数值必须大于 0".into());
    }
    // 邮件的两条纯参数门，逐字对上 Go：`GMSystemMail` 的 `len(body) > 512`，
    // 以及「金币与物品都没填就没有可发的东西」。放在这里而不是事务里，
    // 是为了让被拒的流水号根本不落库（见 §2.3 那条口径）。
    if mail {
        if input.body.len() > crate::gmmail::MAX_BODY_BYTES {
            return Err("邮件正文过长".into());
        }
        if input.gold == 0 && input.template == 0 {
            return Err("邮件至少要带金币或物品".into());
        }
    }
    // Go 的请求门：profession 必须带阶段且 ≤3、分支 ≤15，否则整笔拒在领取之前。
    if input.action == "profession"
        && !matches!(input.stage, Some(stage) if stage <= 3 && (1..=15).contains(&input.amount))
    {
        return Err("请选择有效的转职分支和觉醒阶段".into());
    }
    // Go 把这三样合称「原物品快照」：少一样就说明界面拿的那一行不是此刻库里的行。
    if bag_edit
        && (input.template == 0
            || input.revision.len() != 64
            || input.expected_amount.unwrap_or(0) == 0)
    {
        return Err("背包操作缺少原物品快照，请刷新后重试".into());
    }
    // 耐久这一档同样的三样，外加「当前耐久」：它可能是 0（打坏的装备），所以不要求非 0，只要求带。
    if durability_edit
        && (input.template == 0
            || input.revision.len() != 64
            || input.expected_durability.is_none()
            || input.amount == 0)
    {
        return Err("装备耐久操作缺少原装备快照，请刷新后重试".into());
    }
    let mut out = input.clone();
    out.id = id;
    out.reason = {
        let t = input.reason.trim();
        if t.is_empty() {
            DEFAULT_MANAGE_REASON.to_string()
        } else {
            t.to_string()
        }
    };
    Ok(out)
}

/// 背包/材料编辑本体：校验 → 租约 → 单事务（幂等闸门 / 改存档或仓库 / 审计回执）。
pub fn manage(
    root: &Root,
    shared: &GmShared,
    input: &ManageInput,
) -> Result<GmGrantResult, String> {
    let busy = BusyGuard::acquire()?;
    let normalized = match normalize_manage(input) {
        Ok(v) => v,
        Err(e) => {
            drop(busy);
            return Err(e);
        }
    };
    let payload = serde_json::to_string(&normalized).map_err(|e| e.to_string())?;

    let g = GrantRequest {
        id: normalized.id.clone(),
        account_id: normalized.account,
        character_id: normalized.character,
        delivery: String::new(),
        cera: 0,
        gold: 0,
        items: Vec::new(),
        reason: payload,
        operator: OPERATOR.to_string(),
    };
    let request = serde_json::to_vec(&g).map_err(|e| e.to_string())?;

    let catalog = shared.ensure(root)?;
    let configs = gmbuild::gmindex::find_configs(&root.path);
    // 规则只用来补「产物里没带上限」的可堆叠数量，读不到按 0（那一行就不可改数量）。
    let rules = configs.as_ref().and_then(|dir| BagRules::load(dir).ok());
    let missing = rules
        .as_ref()
        .map(|rules| rules.missing_stack_limit())
        .unwrap_or(0);
    // 改耐久要先有装备目录给的上限：这一条是路线 C 的「产物缺就整条拒」，
    // 不放开成「65535 以内随便填」——那会写进一个游戏里不存在的耐久值。
    let durability_cap = if normalized.action == "equipment.durability" {
        match (configs.as_deref(), rules.as_ref()) {
            (Some(dir), Some(rules)) => Some(
                EquipCatalog::load(dir, &rules.source)?
                    .reward(normalized.template)
                    .map(u32::from)
                    .map_err(|e| format!("装备目录读不到这件装备的耐久上限：{e}"))?,
            ),
            _ => return Err("缺少装备目录：equipment.current37.json".into()),
        }
    } else {
        None
    };
    let db = current_db(root).ok_or_else(|| "存档还没有 SQLite 档：先启动一次游戏".to_string())?;
    // 养成四档要那份离线成长产物；读不到就整笔拒，不降级成「按经验硬写」。
    // 主线与追溯前置也要它（职业 `[job]` 段是主线资格判据之一），见 `gmquest`。
    let needs_grow = crate::gmstat::is_growth_action(&normalized.action)
        || normalized.action == "sp.recalculate"
        || matches!(normalized.action.as_str(), "quests.mainline" | "quests.prerequisites");
    let grow = needs_grow.then(|| crate::gmstat::Context::load(root)).transpose()?;
    let quest_file = crate::gmquest::is_quest_action(&normalized.action)
        .then(|| crate::gmquest::load(root))
        .transpose()?;
    // 改前备份的落点：与整库备份同一个根，`state/` 子目录（那些 JSON 不该冒充整库档）。
    // 目录不存在不要紧，`backup_state_before` 会建；建不出来 = 这一笔拒。
    let state_dir = crate::saves::role_state_backup_dir(root);
    let art = ManageArtifacts {
        grow: grow.as_ref(),
        quests: quest_file.as_ref(),
        durability_cap,
        state_backup: Some(&state_dir),
    };
    let _lease = take_lease_best_effort(&db);
    let mut conn = open_rw(&db)?;
    let result = manage_inner(&mut conn, &catalog, missing, &art, &g, &request, &normalized);
    drop(busy);
    result
}

/// `manage_inner` 写侧要用的运行期输入：离线产物、耐久上限、改前备份落点。
/// 产物缺哪份就整笔拒哪一档，不降级成「硬写」。
#[derive(Clone, Copy, Default)]
pub(crate) struct ManageArtifacts<'a> {
    pub grow: Option<&'a crate::gmstat::Context>,
    pub quests: Option<&'a gmbuild::quest::QuestFile>,
    /// 目标装备的耐久上限，来自 `equipment.current37.json` 的 `[durability]`（含 `[import script]` 继承链）。
    /// 只有 `equipment.durability` 这一条动作会用到；读不到就不给写，见 `manage`。
    pub durability_cap: Option<u32>,
    /// 覆盖 `characters.state` 前的改原文落点（`<storage>/backup/state/`）。
    /// `None` = 不备份，只有测试这么传；见 `backup_state_before`。
    pub state_backup: Option<&'a Path>,
}

/// 事务本体（测试直接喂内存库，不碰 Root 与产物）。
pub(crate) fn manage_inner(
    conn: &mut rusqlite::Connection,
    cat: &gmbuild::gmindex::Catalog,
    missing_stack_limit: u32,
    art: &ManageArtifacts<'_>,
    g: &GrantRequest,
    request: &[u8],
    input: &ManageInput,
) -> Result<GmGrantResult, String> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;

    let claim = claim_flow(&tx, g, request, "管理目标不是已知账号/角色", "流水号已用于不同操作");
    let replay = match claim {
        Ok(Claim::Fresh) => None,
        Ok(Claim::Replay(stored)) => Some(stored),
        Err(e) => {
            tx.rollback().ok();
            return Err(e);
        }
    };
    if let Some(stored) = replay {
        tx.commit().map_err(|e| e.to_string())?;
        let receipt = serde_json::from_slice(&stored).unwrap_or(Value::Null);
        return Ok(GmGrantResult {
            applied: false,
            grant_id: g.id.clone(),
            message: "该笔操作已存在，未重复执行。".into(),
            note: String::new(),
            receipt,
        });
    }

    // 与 Go 的 ApplyManagedGrant 同：任何管理操作都先取到这个角色（软删或别人的角色一律失败）。
    // 一次取齐 state / profession / config_version：任务写侧要用后两列判身份与职业，
    // 分成两次读会变成「按 A 版本校验、按 B 版本写入」。
    let role = match crate::gmquest::read_role(&tx, g.account_id, g.character_id) {
        Ok(v) => v,
        Err(e) => {
            tx.rollback().ok();
            return Err(format!("管理角色不属于这个账号：{e}"));
        }
    };
    if crate::gmstat::is_growth_action(&input.action) && art.grow.is_none() {
        tx.rollback().ok();
        return Err("养成产物未就绪：gm-growth.json".into());
    }
    // 改前留一份原文，且**在这之后任何写入都不再有回头路**：这一步在 `run_manage_action` 之前，
    // 事务里除了 `claim_flow` 的 INSERT 什么都没动，那个 INSERT 也还没提交，所以备份失败整笔回滚，
    // 库里连流水号都不留（被拒的流水号可以再用，见 §2.3）。
    let backup = if writes_character_state(&input.action) {
        match backup_state_before(art.state_backup, g.account_id, g.character_id, &role.state) {
            Ok(v) => v,
            Err(e) => {
                tx.rollback().ok();
                return Err(e);
            }
        }
    } else {
        None
    };
    // 任务这几档的产物缺口由 `run_manage_action` 自己拒（话术见 `gmquest::apply`），
    // 与养成档同一次事务、同一套拒绝即回滚。

    let body = run_manage_action(&tx, cat, missing_stack_limit, g, input, &role, art);
    let (mut receipt, message, note) = match body {
        Ok(v) => v,
        Err(e) => {
            tx.rollback().ok();
            return Err(e);
        }
    };
    stamp_state_backup(&mut receipt, backup.as_ref());
    let blob = serde_json::to_vec(&receipt).map_err(|e| e.to_string())?;
    tx.execute(SQL_RECEIPT_WRITE, params![g.id, blob.clone()])
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(GmGrantResult {
        applied: true,
        grant_id: g.id.clone(),
        message,
        note,
        receipt: serde_json::from_slice(&blob).map_err(|e| e.to_string())?,
    })
}

/// 各动作要做的事，返回（回执, 给界面的话, 补充说明）。
/// 背包的两条与 Go editGMInventory 的 message 逐字一致；仓库那条同 materials.set；
/// 养成四档与 Go editCharacter 的成功话逐字一致（「角色修改已保存，重新选择角色后生效」）。
fn run_manage_action(
    tx: &rusqlite::Transaction<'_>,
    cat: &gmbuild::gmindex::Catalog,
    missing_stack_limit: u32,
    g: &GrantRequest,
    input: &ManageInput,
    role: &crate::gmquest::Role,
    art: &ManageArtifacts<'_>,
) -> Result<(Value, String, String), String> {
    if let Some(ctx) = art.grow.filter(|_| crate::gmstat::is_growth_action(&input.action)) {
        // 等级 / 转职 / 觉醒 / 转职分支：算式全在 `gmstat`，这一层只负责落库与审计。
        let (bytes, receipt) = crate::gmstat::apply(
            ctx,
            &role.state,
            role.profession,
            &role.config_version,
            &input.action,
            input.amount,
            input.stage,
        )?;
        tx.execute(SQL_STATE_WRITE, params![g.character_id, bytes])
            .map_err(|e| format!("写回角色存档失败：{e}"))?;
        return Ok((
            receipt,
            "角色修改已保存，重新选择角色后生效".into(),
            String::new(),
        ));
    }
    if crate::gmquest::is_quest_action(&input.action) {
        // 任务五个动作：判据全在 `gm-quests.json`，落库只动 character_quests，
        // 不碰角色存档；奖励一律不发（回执里的 rewards:false 就是这个意思）。
        let Some(file) = art.quests else {
            return Err("任务产物未就绪：gm-quests.json".into());
        };
        return crate::gmquest::apply(tx, file, art.grow, role, input);
    }
    if input.action == "mail.send" {
        // 邮件发放：只往 `character_mail` 插一行，不碰角色存档（所以 `writes_character_state` 排除了它），
        // 也不写 `gm_mail`——那张表是旧管理台自己的队列，现役服务端一条 SQL 都不读它。
        let (receipt, message) = crate::gmmail::send(tx, cat, missing_stack_limit, g.character_id, input)?;
        return Ok((receipt, message, String::new()));
    }
    if input.action == "materials.set" {
        let slot = account_material_slot(input.template)
            .ok_or_else(|| "该物品不属于账号晶体仓库".to_string())?;
        tx.execute(SQL_MATERIALS_ENSURE, params![g.account_id])
            .map_err(|e| e.to_string())?;
        let raw: Vec<u8> = tx
            .query_row(SQL_MATERIALS_READ, params![g.account_id], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let before = AccountMaterials::from_raw(Some(&raw))?;
        let cur = before.counts.get(&slot).copied().unwrap_or(0);
        if let Some(exp) = input.expected_amount {
            if cur != exp {
                return Err(format!("材料数量已从 {exp} 变为 {cur}，请刷新后再修改"));
            }
        }
        let mut after = before.clone();
        if input.amount == 0 {
            after.counts.remove(&slot);
        } else {
            after.counts.insert(slot, input.amount);
        }
        let bytes = after.save()?;
        tx.execute(SQL_MATERIALS_SAVE, params![g.account_id, bytes.clone()])
            .map_err(|e| e.to_string())?;
        let receipt = serde_json::json!({
            "action": input.action,
            "slot": slot,
            "template": input.template,
            "before": serde_json::to_value(&before).unwrap_or(Value::Null),
            "after": serde_json::to_value(&after).unwrap_or(Value::Null),
        });
        return Ok((
            receipt,
            "仓库数量已保存；同账号共享，重新进入角色后同步游戏显示".into(),
            String::new(),
        ));
    }

    let state: Value = match serde_json::from_slice(&role.state) {
        Ok(v) => v,
        Err(e) => return Err(format!("角色存档不是合法 JSON：{e}")),
    };
    if input.action == "sp.recalculate" {
        // 满值 = 服务端把这个角色升到当前等级本该拿到的总量，判据只有产物里那张每级授予表。
        // 这一档不读用户填的数，所以 `normalize_manage` 不给它设「数值必须大于 0」那条门。
        let Some(ctx) = art.grow else {
            return Err("养成产物未就绪：gm-growth.json".into());
        };
        let (updated, receipt) =
            crate::gmchar::apply_sp_recalc(&state, &ctx.growth.skill_points, ctx.level_cap)?;
        tx.execute(SQL_STATE_WRITE, params![g.character_id, updated])
            .map_err(|e| format!("写回角色存档失败：{e}"))?;
        return Ok((
            receipt,
            "角色修改已保存，重新选择角色后生效".into(),
            String::new(),
        ));
    }
    if matches!(input.action.as_str(), "sp" | "tp" | "equipment.unlock") {
        let (updated, receipt) = crate::gmchar::apply_numeric_edit(&state, &input.action, input.amount)?;
        tx.execute(SQL_STATE_WRITE, params![g.character_id, updated])
            .map_err(|e| format!("写回角色存档失败：{e}"))?;
        return Ok((
            receipt,
            "角色修改已保存，重新选择角色后生效".into(),
            String::new(),
        ));
    }
    let delete = input.action == "inventory.delete";
    let edit = crate::gmchar::BagEdit {
        delete,
        durability: input.action == "equipment.durability",
        collection: input.collection.clone(),
        space: input.space,
        slot: input.slot,
        group: input.group,
        template: input.template,
        revision: input.revision.clone(),
        expected_amount: input.expected_amount.unwrap_or(0),
        expected_durability: input.expected_durability,
        durability_cap: art.durability_cap,
        amount: input.amount,
    };
    let (updated, receipt) = crate::gmchar::apply_bag_edit(&state, cat, missing_stack_limit, &edit)?;
    tx.execute(SQL_STATE_WRITE, params![g.character_id, updated])
        .map_err(|e| format!("写回角色存档失败：{e}"))?;
    let message = if edit.durability {
        "装备耐久已保存，原属性与精炼记录已保留"
    } else if delete {
        "物品已删除，删除前记录已保存到操作审计"
    } else {
        "物品数量已保存，原期限与属性已保留"
    };
    Ok((receipt, message.into(), ONLINE_NOTE.into()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn gm_materials(
    state: tauri::State<'_, AppState>,
    account: i64,
) -> Result<Vec<GmMaterialRow>, String> {
    let root = state.root.clone();
    let shared = state.gm.clone();
    crate::commands::join(move || {
        let catalog = shared.ensure(&root)?;
        materials(&root, &catalog, account)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn gm_manage(
    input: ManageInput,
    state: tauri::State<'_, AppState>,
) -> Result<GmGrantResult, String> {
    let root = state.root.clone();
    let shared = state.gm.clone();
    crate::commands::join(move || manage(&root, &shared, &input)).await
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "7ef2db59331f7e5b18b2f250b8b907526bf2c94b17a7312036cf599644d88e80";

    struct TmpDir(PathBuf);
    impl TmpDir {
        fn new(tag: &str) -> TmpDir {
            static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = SEQ.fetch_add(1, Ordering::SeqCst);
            let p = std::env::temp_dir()
                .join(format!("gmgrant-test-{}-{}-{}", tag, std::process::id(), n));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).expect("temp dir");
            TmpDir(p)
        }
        fn write(&self, name: &str, content: &str) {
            std::fs::write(self.0.join(name), content).expect("write");
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn rules() -> BagRules {
        BagRules::from_json(
            r#"{"model":"reference90-bag-v1","source":"7ef2db59331f7e5b18b2f250b8b907526bf2c94b17a7312036cf599644d88e80",
            "slots":{"[throw]":[65,120],"[material]":[121,176]},"missing_stack_limit":1000,
            "equipment_slots":[9,64],"quick_slots":[0,8]}"#,
        )
        .expect("rules")
    }

    // ---- 背包规则 ----

    #[test]
    fn bag_rules_real_pack_parses() {
        // 线上那份 inventory.current37.json 的原样形状（D:\115us\源码\…\gm-tool\configs）。
        let r = BagRules::from_json(
            r#"{"model":"reference90-bag-v1","source":"7ef2db59331f7e5b18b2f250b8b907526bf2c94b17a7312036cf599644d88e80",
               "slots":{"[throw]":[65,120],"[material]":[121,176]},"missing_stack_limit":1000,
               "equipment_slots":[9,64],"quick_slots":[0,8]}"#,
        )
        .expect("real pack rules");
        assert_eq!(r.missing_stack_limit, 1000);
        assert_eq!(r.equipment_slots, Some([9, 64]));
        assert_eq!(r.slots.get("[material]"), Some(&[121, 176]));
    }

    #[test]
    fn bag_rules_rejects_overlap_and_zero_start() {
        let base = r#"{"model":"reference90-bag-v1","source":""#;
        let bad = format!(
            "{base}{SRC}\",\"slots\":{{\"a\":[65,120],\"b\":[120,176]}},\"missing_stack_limit\":1000}}"
        );
        assert_eq!(BagRules::from_json(&bad).unwrap_err(), "overlapping bag ranges");
        let bad2 = format!(
            "{base}{SRC}\",\"slots\":{{\"a\":[0,10]}},\"missing_stack_limit\":1000}}"
        );
        assert!(BagRules::from_json(&bad2).unwrap_err().starts_with("invalid bag slot range"));
        let bad3 = r#"{"model":"other","source":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","slots":{},"missing_stack_limit":5}"#;
        assert_eq!(BagRules::from_json(bad3).unwrap_err(), "invalid bag policy");
    }

    // ---- base64 ----

    #[test]
    fn base64_roundtrip_matches_std() {
        for case in ["a", "ab", "abc", "abcd", "你好世界"] {
            let bytes = case.as_bytes();
            let enc = b64_encode(bytes);
            let dec = b64_decoded(&enc).expect("decode");
            assert_eq!(dec, bytes.to_vec(), "roundtrip {case:?}");
        }
        // 181 字节的记录编码后必须是 244 个字符（4×ceil(181/3)，末尾两个填充）、解码回到 181。
        let rec = vec![7u8; RECORD_SIZE];
        let enc = b64_encode(&rec);
        assert_eq!(enc.len(), 244);
        assert_eq!(b64_decoded(&enc).unwrap().len(), RECORD_SIZE);
        assert!(b64_decoded("abc").is_none());
        assert!(b64_decoded("ab!d").is_none());
    }

    // ---- ReadBag / SaveBag ----

    fn state_with_inventory(inv: &str) -> Value {
        serde_json::from_str(&format!(
            r#"{{"level":70,"inventory":{inv},"creature_loyalty_updated_at":123}}"#
        ))
        .unwrap()
    }

    #[test]
    fn read_fold_and_save_preserves_shape() {
        let state = state_with_inventory(
            r#"{"version":"ordinary-bag-v1","gold":5,"items":[
                 {"slot":2,"Template":1,"Amount":300},
                 {"slot":70,"template":3001,"amount":5,"unknown_field":{"a":1}}],
               "worn":[{"slot":26,"template":63006,"durability":1}],
               "special_equipment":{"1":[{"slot":0,"template":500001,"durability":0}]},
               "unknown_top":"keep-me"}"#,
        );
        let bag = read_bag(&state).expect("read");
        assert_eq!(bag.coin, 300, "银币行折叠");
        assert_eq!(bag.items.len(), 1);
        assert!(bag.items[0].extra.contains_key("unknown_field"));
        assert_eq!(bag.worn[0].template, 63000, "蛋位改写");
        assert!(bag.special.contains_key(&1));

        let out = save_bag(&state, &bag).expect("save");
        let saved: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(saved["level"], 70, "顶层键原样保留");
        assert_eq!(saved["creature_loyalty_updated_at"], 123);
        let inv = &saved["inventory"];
        assert_eq!(inv["items"][0]["Template"], 3001, "物品行按 GM 的 Go 结构体写大写 Template/Amount（服务端反序列化大小写不敏感）");
        assert_eq!(inv["items"][0]["Amount"], 5);
        assert_eq!(inv["coin"], 300);
        assert_eq!(inv["unknown_top"], "keep-me", "inventory 未知键 flatten 保留");
        assert_eq!(inv["items"][0]["unknown_field"]["a"], 1);
        assert!(inv["special_equipment"].get("1").is_some(), "专用容器键必须是十进制串");
        assert!(inv.get("equipment").is_none(), "Go 的 omitempty：空装备栏不写");
        assert!(inv["items"].is_array(), "Go 无 omitempty：items 恒为数组");
    }

    #[test]
    fn read_bag_rejects_bad_rows() {
        // 重复槽位
        let s = state_with_inventory(
            r#"{"version":"ordinary-bag-v1","gold":0,"items":[
                 {"slot":70,"Template":2,"Amount":1},{"slot":70,"Template":3,"Amount":1}]}"#,
        );
        assert_eq!(read_bag(&s).unwrap_err(), "invalid saved inventory row");
        // 槽位 1（金币行残留）
        let s = state_with_inventory(
            r#"{"version":"ordinary-bag-v1","gold":0,"items":[{"slot":1,"Template":2,"Amount":1}]}"#,
        );
        assert!(read_bag(&s).is_err());
        // 旧版本背包
        let s = state_with_inventory(r#"{"version":"ordinary-bag-v0","gold":0,"items":[]}"#);
        assert_eq!(read_bag(&s).unwrap_err(), "unsupported saved inventory");
        // record 长度不是 181
        let rec = b64_encode(&[9u8; 180]);
        let s = state_with_inventory(&format!(
            r#"{{"version":"ordinary-bag-v1","gold":0,"items":[],"equipment":[
                 {{"slot":9,"template":123,"durability":1,"record":"{rec}"}}]}}"#
        ));
        assert_eq!(read_bag(&s).unwrap_err(), "invalid equipment instance record");
        // record 内嵌模板号不符
        let mut body = vec![0u8; RECORD_SIZE];
        body[2..6].copy_from_slice(&999u32.to_le_bytes());
        let rec = b64_encode(&body);
        let s = state_with_inventory(&format!(
            r#"{{"version":"ordinary-bag-v1","gold":0,"items":[],"equipment":[
                 {{"slot":9,"template":123,"durability":1,"record":"{rec}"}}]}}"#
        ));
        assert_eq!(read_bag(&s).unwrap_err(), "equipment instance template mismatch");
        // 专用空间不是 1/7
        let s = state_with_inventory(
            r#"{"version":"ordinary-bag-v1","gold":0,"items":[],"special_equipment":{"2":[]}}"#,
        );
        assert_eq!(read_bag(&s).unwrap_err(), "unsupported equipment inventory");
    }

    // ---- Bag.Add ----

    #[test]
    fn bag_add_gold_coin_and_stackables() {
        let r = rules();
        let mut b = Bag::default();
        assert_eq!(bag_add(&mut b, &r, None, 0, 0, 500).unwrap(), 0);
        assert_eq!(b.gold, 500);
        assert!(bag_add(&mut b, &r, None, 0, 0, u32::MAX).is_err(), "金币溢出必须整笔拒");
        assert_eq!(bag_add(&mut b, &r, None, 0, 1, 7).unwrap(), 1);
        assert_eq!(b.coin, 7);

        // 落格：[throw] → 65..120
        assert_eq!(bag_add(&mut b, &r, Some("[throw]"), 0, 2001, 3).unwrap(), 65);
        assert_eq!(bag_add(&mut b, &r, Some("[throw]"), 0, 2001, 4).unwrap(), 65, "先叠已有行");
        assert_eq!(b.items[0].amount, 7);
        // material 类型名兜底 → 121..176
        assert_eq!(bag_add(&mut b, &r, Some("Material"), 0, 2002, 1).unwrap(), 121);
        // 超过堆叠上限直接拒
        assert_eq!(
            bag_add(&mut b, &r, Some("[throw]"), 5, 2003, 6).unwrap_err(),
            "award exceeds stack limit"
        );
        // 数量为一
        assert_eq!(bag_add(&mut b, &r, Some("[throw]"), 0, 2003, 0).unwrap_err(), "invalid inventory award");
    }

    #[test]
    fn bag_add_full_category_refuses() {
        let r = rules();
        let mut b = Bag::default();
        for n in 65u16..=120 {
            b.items.push(BagItem {
                slot: n,
                template: 9000 + n as u32,
                amount: 1,
                expire_time: 0,
                extra: Map::new(),
            });
        }
        assert_eq!(
            bag_add(&mut b, &r, Some("[throw]"), 0, 2001, 1).unwrap_err(),
            "bag category is full"
        );
    }

    #[test]
    fn add_equipment_and_special() {
        let r = rules();
        let mut b = Bag::default();
        assert_eq!(add_equipment(&mut b, &r, 12345, 2, 40).unwrap(), vec![9, 10]);
        assert_eq!(b.equipment[0].durability, 40);
        assert_eq!(
            add_equipment(&mut b, &r, 12345, 60, 1).unwrap_err(),
            "invalid equipment award",
            "count 超过窗口（9..=64 共 56 格）先撞数量守卫，与 Go 一致"
        );
        assert_eq!(
            add_equipment(&mut b, &r, 12345, 56, 1).unwrap_err(),
            "equipment bag is full",
            "count 恰好等于窗口，但 9/10 已被占 ⇒ 放不下"
        );
        // 时装容器：210 格、一次性最多 210
        let mut b2 = Bag::default();
        let slots = add_special_award(&mut b2, 1, 500001, 3).unwrap();
        assert_eq!(slots, vec![0, 1, 2]);
        assert_eq!(
            add_special_award(&mut b2, 1, 500001, 211).unwrap_err(),
            "专用背包发放数量超出容量"
        );
        let slots = add_special_award(&mut b2, 7, 600001, 140).unwrap();
        assert_eq!(slots.len(), 140);
        assert_eq!(b2.special[&7].len(), 140);
    }

    // ---- 装备目录 ----

    fn equip_file(rows: &str) -> String {
        format!(
            r#"{{"source":{{"checksum":"{SRC}"}},"rows":{rows}}}"#
        )
    }

    #[test]
    fn equipment_import_chain_inherits_fields() {
        let cat = EquipCatalog::from_json(
            &equip_file(
                r#"[{"ID":100050791,"Path":"a.equ","SHA256":"x","Fields":{
                      "[name]":[{"type":6,"value":0,"text":"[up]"}],
                      "[import script]":[{"type":6,"value":0,"text":"character/common/jacket/cloth/100050666.equ"}]}},
                    {"ID":100050666,"Path":"b.equ","SHA256":"y","Fields":{
                      "[rarity]":[{"type":0,"value":1,"text":""}],
                      "[equipment type]":[{"type":6,"value":0,"text":"[jacket]"}],
                      "[durability]":[{"type":0,"value":35,"text":""}]}}]"#,
            ),
            SRC,
        )
        .unwrap();
        assert_eq!(cat.reward(100050791).unwrap(), 35, "薄壳必须继承基础件耐久");
        assert_eq!(
            cat.kind_text(100050791).unwrap().as_deref(),
            Some("[jacket]"),
            "分类也要读继承后的字段"
        );
    }

    #[test]
    fn equipment_reward_durability_rules() {
        let cat = EquipCatalog::from_json(
            &equip_file(
                r#"[{"ID":1,"Path":"1.equ","SHA256":"x","Fields":{
                      "[rarity]":[{"type":0,"value":0,"text":""}],
                      "[equipment type]":[{"type":6,"value":0,"text":"[title name]"}]}},
                    {"ID":2,"Path":"2.equ","SHA256":"x","Fields":{
                      "[rarity]":[{"type":0,"value":0,"text":""}],
                      "[equipment type]":[{"type":6,"value":0,"text":"[weapon]"}]}},
                    {"ID":3,"Path":"3.equ","SHA256":"x","Fields":{
                      "[rarity]":[{"type":0,"value":0,"text":""}],
                      "[equipment type]":[{"type":6,"value":0,"text":"[weapon]"}],
                      "[durability]":[{"type":0,"value":70000,"text":""}]}}]"#,
            ),
            SRC,
        )
        .unwrap();
        assert_eq!(cat.reward(1).unwrap(), 0, "免耐久部位缺耐久 → 耐久 0 放行");
        assert_eq!(cat.reward(2).unwrap_err(), "missing source equipment durability");
        assert_eq!(cat.reward(3).unwrap_err(), "invalid equipment durability");
        assert!(cat.reward(4).unwrap_err().contains("不在装备目录"));
    }

    #[test]
    fn equipment_source_mismatch_gate() {
        let raw = equip_file("[]");
        let err = EquipCatalog::from_json(&raw, &"0".repeat(64)).unwrap_err();
        assert!(err.contains("不同源"), "{err}");
    }

    #[test]
    fn bag_space_routing() {
        assert_eq!(equipment_bag_space("[hat avatar]"), 1);
        assert_eq!(equipment_bag_space("[face avatar]"), 1);
        assert_eq!(equipment_bag_space("[creature]"), 7);
        assert_eq!(equipment_bag_space("[artifact red]"), 7);
        assert_eq!(equipment_bag_space("[weapon]"), 0);
    }

    // ---- 账号晶体仓库 ----

    #[test]
    fn account_materials_legacy_merge_and_shape() {
        let m = AccountMaterials::from_raw(Some(
            br#"{"version":"account-materials-v1","counts":{"381":10,"0":5,"363":2,"363":2}}"#,
        ))
        .unwrap();
        assert_eq!(m.counts.get(&0), Some(&15), "381 并进 0");
        assert_eq!(m.counts.get(&363), Some(&2));
        let saved = m.save().unwrap();
        let text = String::from_utf8(saved).unwrap();
        assert!(text.contains("\"363\":2"), "槽位键必须是十进制字符串: {text}");
        assert!(!text.contains("381"));

        assert!(AccountMaterials::from_raw(Some(br#"{"version":"v2"}"#)).is_err());
        assert!(
            AccountMaterials::from_raw(Some(br#"{"version":"account-materials-v1","counts":{"777":1}}"#))
                .unwrap_err()
                == "invalid account material slot"
        );
        let empty = AccountMaterials::from_raw(Some(b"null")).unwrap();
        assert_eq!(empty.save().unwrap(), br#"{"version":"account-materials-v1"}"#.to_vec());
    }

    // ---- 事务本体（内存库跑真实 schema） ----

    fn test_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(
            r#"
CREATE TABLE accounts (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 username TEXT NOT NULL UNIQUE,
 password_hash TEXT,
 development_only BOOLEAN NOT NULL DEFAULT 1,
 created_at TIMESTAMP NOT NULL DEFAULT (CAST((julianday('now') - 2440587.5) * 86400000000 AS INTEGER)),
 CHECK(development_only <> 0 OR password_hash IS NOT NULL));
CREATE TABLE characters (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 account_id BIGINT NOT NULL REFERENCES accounts(id),
 wire_id INTEGER NOT NULL CHECK(wire_id BETWEEN 1 AND 65534),
 name TEXT NOT NULL,
 profession INTEGER NOT NULL CHECK(profession BETWEEN 0 AND 255),
 create_request BLOB NOT NULL,
 config_version TEXT NOT NULL,
 state BLOB NOT NULL CHECK(json_valid(state)),
 created_at TIMESTAMP NOT NULL DEFAULT (CAST((julianday('now') - 2440587.5) * 86400000000 AS INTEGER)),
 deleted_at TIMESTAMP,
 max_fame INTEGER NOT NULL DEFAULT 0 CHECK(max_fame >= 0),
 roster_order BIGINT CHECK(roster_order > 0),
 fixed_slot SMALLINT NOT NULL DEFAULT 0 CHECK(fixed_slot BETWEEN 0 AND 255),
 UNIQUE(account_id,wire_id));
CREATE TABLE account_currency(
 account_id BIGINT PRIMARY KEY REFERENCES accounts(id),
 cera BIGINT NOT NULL DEFAULT 0 CHECK(cera>=0),
 updated_at TIMESTAMP NOT NULL DEFAULT (CAST((julianday('now') - 2440587.5) * 86400000000 AS INTEGER)));
CREATE TABLE account_material_storage (
 account_id BIGINT PRIMARY KEY REFERENCES accounts(id),
 counts BLOB NOT NULL DEFAULT (CAST('{}' AS BLOB)) CHECK(json_valid(counts)),
 updated_at TIMESTAMP NOT NULL DEFAULT (CAST((julianday('now') - 2440587.5) * 86400000000 AS INTEGER)));
CREATE TABLE admin_grants(
 grant_id TEXT PRIMARY KEY,
 account_id BIGINT NOT NULL REFERENCES accounts(id),
 character_id BIGINT REFERENCES characters(id),
 request BLOB NOT NULL CHECK(json_valid(request)),
 receipt BLOB NOT NULL CHECK(json_valid(receipt)),
 operator TEXT NOT NULL, reason TEXT NOT NULL,
 created_at TIMESTAMP NOT NULL DEFAULT (CAST((julianday('now') - 2440587.5) * 86400000000 AS INTEGER)));
CREATE TABLE character_quests (
 character_id BIGINT NOT NULL REFERENCES characters(id),
 quest_id INTEGER NOT NULL CHECK(quest_id BETWEEN 1 AND 65535),
 status TEXT NOT NULL CHECK(status IN ('accepted','completed')),
 progress BIGINT NOT NULL DEFAULT 0 CHECK(progress BETWEEN 0 AND 4294967295),
 config_version TEXT NOT NULL,
 progress_model TEXT NOT NULL DEFAULT 'legacy-zero',
 accepted_at TIMESTAMP NOT NULL DEFAULT (CAST((julianday('now') - 2440587.5) * 86400000000 AS INTEGER)),
 completed_at TIMESTAMP,
 PRIMARY KEY(character_id,quest_id));
CREATE TABLE character_mail (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 sender_id BIGINT REFERENCES characters(id),
 recipient_id BIGINT NOT NULL REFERENCES characters(id),
 sender_name TEXT NOT NULL, body TEXT NOT NULL,
 status SMALLINT NOT NULL DEFAULT 1 CHECK(status IN (1,2,3)),
 assets BLOB NOT NULL DEFAULT (CAST('[]' AS BLOB)) CHECK(json_type(assets)='array'),
 created_at TIMESTAMP NOT NULL DEFAULT (CAST((julianday('now') - 2440587.5) * 86400000000 AS INTEGER)),
 expires_at TIMESTAMP NOT NULL,
 deleted_at TIMESTAMP,
 CHECK(length(CAST(sender_name AS BLOB))<=29), CHECK(length(CAST(body AS BLOB))<=512));
CREATE INDEX character_mail_inbox ON character_mail(recipient_id,id) WHERE deleted_at IS NULL;
INSERT INTO accounts(id,username) VALUES(1,'player');
INSERT INTO characters(id,account_id,wire_id,name,profession,create_request,config_version,state)
 VALUES(10,1,1,'剑魂',0,x'00','v1',CAST('{"level":1,"inventory":{"version":"ordinary-bag-v1","gold":0,"items":[]}}' AS BLOB));
"#,
        )
        .unwrap();
        conn
    }

    fn req(id: &str, account: i64, character: i64, gold: u32, cera: i64, items: Vec<(u32, u32)>) -> GrantRequest {
        GrantRequest {
            id: id.into(),
            account_id: account,
            character_id: character,
            delivery: String::new(),
            cera,
            gold,
            items: items
                .into_iter()
                .map(|(template, amount)| GrantItem { template, amount })
                .collect(),
            reason: DEFAULT_REASON.into(),
            operator: OPERATOR.into(),
        }
    }

    fn fake_classify(table: HashMap<u32, GrantPlan>) -> impl Fn(u32) -> Result<GrantPlan, String> {
        move |t| table.get(&t).cloned().ok_or_else(|| format!("物品 {t} 不在物品库里"))
    }

    fn char_state(conn: &rusqlite::Connection) -> Value {
        let raw: Vec<u8> = conn
            .query_row("SELECT state FROM characters WHERE id=10", [], |r| r.get(0))
            .unwrap();
        serde_json::from_slice(&raw).unwrap()
    }

    fn grant_table() -> HashMap<u32, GrantPlan> {
        let mut t = HashMap::new();
        t.insert(
            2001,
            GrantPlan::Stackable { type_text: Some("[throw]".into()), stack_limit: 0 },
        );
        t.insert(3033, GrantPlan::Stackable { type_text: Some("[material]".into()), stack_limit: 0 });
        t
    }

    #[test]
    fn grant_idempotency_replay_and_mismatch() {
        let dir = TmpDir::new("idem");
        let r = rules();
        let mut conn = test_conn();
        let g = req("launcher-0001-0002", 1, 10, 100, 0, vec![]);
        let request = serde_json::to_vec(&g).unwrap();
        let cl = fake_classify(grant_table());

        let first = grant_inner(&mut conn, &dir.0, &r, &g, &request, &cl, None).unwrap();
        assert!(first.applied);
        assert_eq!(first.receipt["gold_added"], 100);
        // 回执落库必须是合法 JSON 且字段名与 Go 对齐（grant 恒在首位）。
        let stored: Vec<u8> = conn
            .query_row("SELECT receipt FROM admin_grants WHERE grant_id=?1", params![g.id], |row| row.get(0))
            .unwrap();
        let text = String::from_utf8(stored).unwrap();
        assert!(text.starts_with(r#"{"grant":"launcher-0001-0002""#), "{text}");

        // 同流水号同请求 ⇒ 回放，不二次支付。
        let replay = grant_inner(&mut conn, &dir.0, &r, &g, &request, &cl, None).unwrap();
        assert!(!replay.applied);
        assert_eq!(replay.message, "该笔发放已存在，未重复发放。");
        assert_eq!(char_state(&conn)["inventory"]["gold"], 100, "回放不得再叠金币");

        // 同流水号不同请求 ⇒ 拒绝。
        let g2 = req("launcher-0001-0002", 1, 10, 999, 0, vec![]);
        let request2 = serde_json::to_vec(&g2).unwrap();
        assert_eq!(
            grant_inner(&mut conn, &dir.0, &r, &g2, &request2, &cl, None).unwrap_err(),
            "流水号已用于不同发放"
        );
    }

    #[test]
    fn grant_writes_items_and_materials() {
        let dir = TmpDir::new("write");
        let r = rules();
        let mut conn = test_conn();
        conn.execute("INSERT INTO account_currency(account_id,cera) VALUES(1,50)", []).unwrap();
        let g = req(
            "launcher-0002-0007",
            1,
            10,
            200,
            0,
            vec![(2001, 3), (3033, 11)],
        );
        let request = serde_json::to_vec(&g).unwrap();
        let cl = fake_classify(grant_table());
        let out = grant_inner(&mut conn, &dir.0, &r, &g, &request, &cl, None).unwrap();
        assert!(out.applied);
        assert_eq!(out.receipt["items"][0]["Template"], 2001);
        assert_eq!(out.receipt["items"][0]["Slots"][0], 65);
        assert_eq!(out.receipt["materials"][0]["slot"], 363);
        assert!(out.message.contains("晶体"), "{}", out.message);

        let state = char_state(&conn);
        assert_eq!(state["inventory"]["gold"], 200);
        assert_eq!(state["inventory"]["items"][0]["Template"], 2001, "GM 写的行是大写形状");
        assert_eq!(state["inventory"]["items"][0]["Amount"], 3);
        let counts: Vec<u8> = conn
            .query_row("SELECT counts FROM account_material_storage WHERE account_id=1", [], |r| r.get(0))
            .unwrap();
        let counts: Value = serde_json::from_slice(&counts).unwrap();
        assert_eq!(counts["counts"]["363"], 11);
        assert_eq!(counts["version"], "account-materials-v1");
    }

    #[test]
    fn grant_cera_negative_refused_and_no_claim_left() {
        let dir = TmpDir::new("cera");
        let r = rules();
        let mut conn = test_conn();
        conn.execute("INSERT INTO account_currency(account_id,cera) VALUES(1,10)", []).unwrap();
        let g = req("launcher-0003-0001", 1, 0, 0, -100, vec![]);
        let request = serde_json::to_vec(&g).unwrap();
        let cl = fake_classify(grant_table());
        assert!(grant_inner(&mut conn, &dir.0, &r, &g, &request, &cl, None)
            .unwrap_err()
            .contains("点券调整被拒"));
        let n: i64 = conn
            .query_row("SELECT count(*) FROM admin_grants", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "失败必须连领取记录一起回滚");
        let cera: i64 = conn
            .query_row("SELECT cera FROM account_currency WHERE account_id=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cera, 10);

        // 正向加钱则一切照常，回执带余额。
        let g2 = req("launcher-0003-0002", 1, 0, 0, 25, vec![]);
        let request2 = serde_json::to_vec(&g2).unwrap();
        let out = grant_inner(&mut conn, &dir.0, &r, &g2, &request2, &cl, None).unwrap();
        assert!(out.applied);
        assert_eq!(out.receipt["grant"], g2.id);
        assert_eq!(out.receipt["cera_balance"], 35);
        assert!(!out.receipt.as_object().unwrap().contains_key("items"));
    }

    #[test]
    fn grant_partial_failure_rolls_back_everything() {
        let dir = TmpDir::new("rollback");
        let r = rules();
        let mut conn = test_conn();
        // 第一件能落，第二件（不在任何表里）必须让整笔回滚。
        let g = req("launcher-0004-0001", 1, 10, 500, 0, vec![(2001, 1), (9999, 1)]);
        let request = serde_json::to_vec(&g).unwrap();
        let cl = fake_classify(grant_table());
        assert!(grant_inner(&mut conn, &dir.0, &r, &g, &request, &cl, None)
            .unwrap_err()
            .contains("不在物品库里"));
        assert_eq!(char_state(&conn)["inventory"]["gold"], 0, "金币不得残留半笔");
        let n: i64 = conn
            .query_row("SELECT count(*) FROM admin_grants", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn grant_request_json_matches_go_marshal_shape() {
        let g = req("launcher-x", 7, 0, 0, 0, vec![]);
        assert_eq!(
            String::from_utf8(serde_json::to_vec(&g).unwrap()).unwrap(),
            r#"{"id":"launcher-x","account_id":7,"reason":"启动器发放","operator":"launcher"}"#
        );
        let g = req("launcher-x", 7, 10, 100, 5, vec![(2001, 3)]);
        assert_eq!(
            String::from_utf8(serde_json::to_vec(&g).unwrap()).unwrap(),
            r#"{"id":"launcher-x","account_id":7,"character_id":10,"cera":5,"gold":100,"items":[{"template":2001,"amount":3}],"reason":"启动器发放","operator":"launcher"}"#
        );
    }

    #[test]
    fn claim_rejects_unknown_account_or_character() {
        let dir = TmpDir::new("fk");
        let r = rules();
        let mut conn = test_conn();
        // 角色 999 根本不存在 ⇒ admin_grants 的外键在领取阶段就把它挡下（引用 characters(id)）。
        let cl = fake_classify(grant_table());
        let g = req("launcher-0005-0001", 404, 10, 1, 0, vec![]);
        let request = serde_json::to_vec(&g).unwrap();
        assert!(grant_inner(&mut conn, &dir.0, &r, &g, &request, &cl, None)
            .unwrap_err()
            .contains("发放目标不是已知账号/角色"));
        let g = req("launcher-0005-0002", 1, 999, 1, 0, vec![]);
        let request = serde_json::to_vec(&g).unwrap();
        assert!(grant_inner(&mut conn, &dir.0, &r, &g, &request, &cl, None)
            .unwrap_err()
            .contains("发放目标不是已知账号/角色"));
        // 角色存在但属于另一个账号：领取阶段外键放行（character_id 引用的是 characters(id)，不看归属），
        // 归属检查必须在事务内取角色时命中。
        conn.execute_batch(
            "INSERT INTO accounts(id,username) VALUES(2,'other');
             INSERT INTO characters(id,account_id,wire_id,name,profession,create_request,config_version,state)
               VALUES(20,2,1,'鬼泣',0,x'00','v1',CAST('{\"inventory\":{\"version\":\"ordinary-bag-v1\",\"gold\":0,\"items\":[]}}' AS BLOB));",
        )
        .unwrap();
        let g = req("launcher-0005-0003", 1, 20, 1, 0, vec![]);
        let request = serde_json::to_vec(&g).unwrap();
        assert!(grant_inner(&mut conn, &dir.0, &r, &g, &request, &cl, None)
            .unwrap_err()
            .contains("发放角色不属于这个账号"));
    }

    #[test]
    fn grant_equipment_path_uses_catalog_durability() {
        let dir = TmpDir::new("equip");
        dir.write(
            "equipment.current37.json",
            &equip_file(
                r#"[{"ID":77001,"Path":"a.equ","SHA256":"x","Fields":{
                      "[rarity]":[{"type":0,"value":1,"text":""}],
                      "[equipment type]":[{"type":6,"value":0,"text":"[weapon]"}],
                      "[durability]":[{"type":0,"value":42,"text":""}]}}]"#,
            ),
        );
        let mut table = grant_table();
        table.insert(77001, GrantPlan::Equipment);
        table.insert(
            77002,
            GrantPlan::Special(1),
        );
        let r = rules();
        let mut conn = test_conn();
        let g = req("launcher-0006-0001", 1, 10, 0, 0, vec![(77001, 1), (77002, 2)]);
        let request = serde_json::to_vec(&g).unwrap();
        let cl = fake_classify(table);
        let out = grant_inner(&mut conn, &dir.0, &r, &g, &request, &cl, None).unwrap();
        assert!(out.applied);
        let state = char_state(&conn);
        let inv = &state["inventory"];
        assert_eq!(inv["equipment"][0]["template"], 77001);
        assert_eq!(inv["equipment"][0]["durability"], 42);
        assert_eq!(inv["special_equipment"]["1"].as_array().unwrap().len(), 2, "时装进专用容器");
    }

    // ---- 管理操作：背包编辑与账号晶体仓库 ----

    /// 物品库夹具：只有那一枚可堆叠（上限 1000），够编辑侧用。
    fn manage_cat(tag: &str) -> (TmpDir, gmbuild::gmindex::Catalog) {
        let dir = TmpDir::new(tag);
        dir.write(
            "items.index.json",
            r#"{"items":[{"id":10418035,"kind":"stackable","stack_type":"[material]","stack_limit":1000,"name":"合成材料"}]}"#,
        );
        let cat = gmbuild::gmindex::load(&dir.0).expect("fixture catalog");
        (dir, cat)
    }

    fn bag_state(conn: &mut rusqlite::Connection, json: &str) {
        conn.execute(
            "UPDATE characters SET state=CAST(?1 AS BLOB) WHERE id=10",
            params![json],
        )
        .unwrap();
    }

    /// 命令层收到的那份 JSON，从这里过一遍就与 manage() 内部完全同形。
    fn manage_req(input: &ManageInput) -> (ManageInput, GrantRequest, Vec<u8>) {
        let normalized = normalize_manage(input).expect("参数合法");
        let g = GrantRequest {
            id: normalized.id.clone(),
            account_id: normalized.account,
            character_id: normalized.character,
            delivery: String::new(),
            cera: 0,
            gold: 0,
            items: Vec::new(),
            reason: serde_json::to_string(&normalized).unwrap(),
            operator: OPERATOR.into(),
        };
        let request = serde_json::to_vec(&g).unwrap();
        (normalized, g, request)
    }

    #[test]
    fn manage_bag_amount_round_trip_and_audit() {
        let (_dir, cat) = manage_cat("manage-amount");
        let mut conn = test_conn();
        let json = r#"{"level":1,"inventory":{"version":"ordinary-bag-v1","gold":100,
            "items":[{"slot":65,"Template":10418035,"Amount":3,"expire_time":2147483647}],
            "worn":[{"slot":12,"template":101040829,"refine":8}]}}"#;
        bag_state(&mut conn, json);
        let s: Value = serde_json::from_str(json).unwrap();
        let revision = crate::gmchar::revision_of(s["inventory"]["items"][0].as_object().unwrap());

        let input = ManageInput {
            action: "inventory.amount".into(),
            id: "launcher-0007-0001".into(),
            account: 1,
            character: 10,
            template: 10418035,
            amount: 7,
            expected_amount: Some(3),
            revision,
            collection: "items".into(),
            slot: 65,
            group: 0,
            ..Default::default()
        };
        let (n, g, request) = manage_req(&input);
        let out = manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap();
        assert!(out.applied);
        assert_eq!(out.message, "物品数量已保存，原期限与属性已保留");
        let state = char_state(&conn);
        assert_eq!(state["inventory"]["items"][0]["Amount"], 7);
        assert_eq!(state["inventory"]["items"][0]["expire_time"], 2147483647);
        assert_eq!(state["inventory"]["gold"], 100);
        assert_eq!(state["inventory"]["worn"][0]["refine"], 8);
        let kind: String = conn
            .query_row("SELECT typeof(state) FROM characters WHERE id=10", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kind, "blob", "回写必须是 BLOB，与服务端写的那些同形");

        // 审计两列：reason 存整份管理请求，receipt 存改前改后。
        let (reason, receipt): (String, Vec<u8>) = conn
            .query_row(
                "SELECT reason,receipt FROM admin_grants WHERE grant_id=?1",
                params![g.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(reason.contains(r#""action":"inventory.amount""#), "{reason}");
        let receipt: Value = serde_json::from_slice(&receipt).unwrap();
        assert_eq!(receipt["before"]["Amount"], 3);
        assert_eq!(receipt["after"]["Amount"], 7);

        // 同流水号同请求 ⇒ 回放原回执，不再动存档。
        let again = manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap();
        assert!(!again.applied);
        assert_eq!(again.message, "该笔操作已存在，未重复执行。");
        assert_eq!(again.receipt["after"]["Amount"], 7);

        // 同流水号换数量 ⇒ 拒（Go：流水号已用于不同操作）。
        let mut other = input.clone();
        other.amount = 9;
        let (n2, g2, request2) = manage_req(&other);
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g2, &request2, &n2).unwrap_err(),
            "流水号已用于不同操作"
        );

        // 拿着旧修订号再来一次 ⇒ 乐观锁拒，且存档不动。
        let mut stale = input.clone();
        stale.id = "launcher-0007-0002".into();
        stale.amount = 11;
        stale.revision = "f".repeat(64);
        let (n3, g3, request3) = manage_req(&stale);
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g3, &request3, &n3).unwrap_err(),
            "物品或数量已变化，请刷新背包后重新操作"
        );
        assert_eq!(char_state(&conn)["inventory"]["items"][0]["Amount"], 7);
    }

    #[test]
    fn manage_durability_round_trip_keeps_refine() {
        let (_dir, cat) = manage_cat("manage-durability");
        let mut conn = test_conn();
        let json = r#"{"level":1,"inventory":{"version":"ordinary-bag-v1","gold":100,
            "items":[{"slot":65,"Template":10418035,"Amount":3,"expire_time":2147483647}],
            "worn":[{"slot":12,"template":101040829,"durability":48,"refine":8}]}}"#;
        bag_state(&mut conn, json);
        let s: Value = serde_json::from_str(json).unwrap();
        let revision = crate::gmchar::revision_of(s["inventory"]["worn"][0].as_object().unwrap());

        let input = ManageInput {
            action: "equipment.durability".into(),
            id: "launcher-0011-0001".into(),
            account: 1,
            character: 10,
            template: 101040829,
            amount: 120,
            expected_durability: Some(48),
            revision,
            collection: "worn".into(),
            slot: 12,
            ..Default::default()
        };
        let (n, g, request) = manage_req(&input);
        let art = ManageArtifacts { durability_cap: Some(120), ..Default::default() };

        // 先撞两道拒绝门（此时存档还是 48，两把钥匙都对得上，报的就是要验的那一条）。
        // 上限来自装备目录：目录里查不到（产物缺 / 这件不是装备）就整笔拒，存档不动。
        let mut no_cap = input.clone();
        no_cap.id = "launcher-0011-0002".into();
        no_cap.amount = 200;
        let (n2, g2, request2) = manage_req(&no_cap);
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g2, &request2, &n2).unwrap_err(),
            "装备目录里查不到这件装备的耐久上限"
        );
        assert_eq!(char_state(&conn)["inventory"]["worn"][0]["durability"], 48);

        // 第三把钥匙不对（这一行的耐久在我们看过之后变了）⇒ 拒，存档不动。
        let mut stale = input.clone();
        stale.id = "launcher-0011-0003".into();
        stale.amount = 90;
        stale.expected_durability = Some(47);
        let (n3, g3, request3) = manage_req(&stale);
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &art, &g3, &request3, &n3).unwrap_err(),
            "耐久已变化，请刷新背包后重新操作"
        );
        assert_eq!(char_state(&conn)["inventory"]["worn"][0]["durability"], 48);

        let out = manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n).unwrap();
        assert!(out.applied);
        assert_eq!(out.message, "装备耐久已保存，原属性与精炼记录已保留");

        let state = char_state(&conn);
        assert_eq!(state["inventory"]["worn"][0]["durability"], 120);
        assert_eq!(state["inventory"]["worn"][0]["refine"], 8, "精炼记录必须跟着留下");
        assert_eq!(state["inventory"]["worn"][0]["slot"], 12);
        assert_eq!(state["inventory"]["items"][0]["Amount"], 3, "别的行一个字都不动");
        assert_eq!(state["inventory"]["items"][0]["expire_time"], 2147483647);

        let receipt: Value = conn
            .query_row(
                "SELECT receipt FROM admin_grants WHERE grant_id=?1",
                params![g.id],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .map(|b| serde_json::from_slice(&b).unwrap())
            .unwrap();
        assert_eq!(receipt["action"], "equipment.durability");
        assert_eq!(receipt["before"]["durability"], 48);
        assert_eq!(receipt["after"]["durability"], 120);
    }

    #[test]
    fn manage_delete_drops_only_the_target_row() {
        let (_dir, cat) = manage_cat("manage-delete");
        let mut conn = test_conn();
        let json = r#"{"level":1,"inventory":{"version":"ordinary-bag-v1","gold":100,
            "items":[{"slot":65,"Template":10418035,"Amount":3,"expire_time":2147483647}],
            "worn":[{"slot":12,"template":101040829,"refine":8},{"slot":13,"template":101040830}]}}"#;
        bag_state(&mut conn, json);
        let s: Value = serde_json::from_str(json).unwrap();
        let revision = crate::gmchar::revision_of(s["inventory"]["worn"][0].as_object().unwrap());
        let input = ManageInput {
            action: "inventory.delete".into(),
            id: "launcher-0010-0001".into(),
            account: 1,
            character: 10,
            template: 101040829,
            revision,
            expected_amount: Some(1),
            collection: "worn".into(),
            slot: 12,
            ..Default::default()
        };
        let (n, g, request) = manage_req(&input);
        let out = manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap();
        assert!(out.applied);
        assert_eq!(out.message, "物品已删除，删除前记录已保存到操作审计");
        assert_eq!(out.receipt["after"], Value::Null);
        assert_eq!(out.receipt["before"]["refine"], 8, "删除前的记录要留在审计里");

        let state = char_state(&conn);
        assert_eq!(state["inventory"]["worn"].as_array().unwrap().len(), 1);
        assert_eq!(state["inventory"]["worn"][0]["template"], 101040830);
        assert_eq!(state["inventory"]["items"].as_array().unwrap().len(), 1, "只删那一行");
        assert_eq!(state["inventory"]["gold"], 100);
    }

    #[test]
    fn manage_numeric_edits_touch_only_their_field() {
        let (_dir, cat) = manage_cat("manage-numeric");
        let mut conn = test_conn();
        let json = r#"{"level":5,"experience":1200,"skill_points":[10,4],"technique_points":[2,2],
            "equipment_slot_flags":1,"attributes":{"物理攻击力":12.5},
            "inventory":{"version":"ordinary-bag-v1","gold":100,"items":[]}}"#;
        bag_state(&mut conn, json);
        let numeric = |action: &str, value: u32, id: &str| ManageInput {
            action: action.into(),
            id: id.into(),
            account: 1,
            character: 10,
            amount: value,
            ..Default::default()
        };

        let (n, g, request) = manage_req(&numeric("sp", 5, "launcher-0011-0001"));
        let out = manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap();
        assert!(out.applied);
        assert_eq!(out.message, "角色修改已保存，重新选择角色后生效");
        assert_eq!(out.receipt["before"], serde_json::json!([10, 4]));
        assert_eq!(out.receipt["after"], serde_json::json!([15, 9]), "两格各加同一份点数（与服务端升级时同口径）");
        let state = char_state(&conn);
        assert_eq!(state["level"], 5);
        assert_eq!(state["technique_points"], serde_json::json!([2, 2]));
        assert_eq!(state["equipment_slot_flags"], 1);
        assert_eq!(state["inventory"]["gold"], 100);
        assert_eq!(state["attributes"]["物理攻击力"], 12.5);

        // 装备栏：只允许那三位，已有的位保留。
        let (n, g, request) = manage_req(&numeric("equipment.unlock", 0x12, "launcher-0011-0002"));
        manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap();
        assert_eq!(char_state(&conn)["equipment_slot_flags"], 0x13);

        // 越界掩码与超上限的点数都在写之前拒，存档一个字节都不动。
        let (n, g, request) = manage_req(&numeric("equipment.unlock", 0x20, "launcher-0011-0003"));
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap_err(),
            "只能开启辅助装备、魔法石和耳环栏"
        );
        let (n, g, request) = manage_req(&numeric("tp", 65534, "launcher-0011-0004"));
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap_err(),
            "技巧点超过存档上限"
        );
        assert_eq!(char_state(&conn)["equipment_slot_flags"], 0x13);
        assert_eq!(char_state(&conn)["technique_points"], serde_json::json!([2, 2]));
        let live: i64 = conn
            .query_row(
                "SELECT count(*) FROM admin_grants WHERE grant_id IN (?1,?2)",
                params!["launcher-0011-0003", "launcher-0011-0004"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live, 0, "被拒的那两笔不许留在册");

        // 数值为 0 在参数层就拒（Go：Value == 0 直接算参数无效）。
        assert_eq!(
            normalize_manage(&numeric("sp", 0, "launcher-0011-0005")).unwrap_err(),
            "要写入的数值必须大于 0"
        );

        // 同流水号同请求 ⇒ 回放，不会再叠一层点数。
        let (n, g, request) = manage_req(&numeric("sp", 5, "launcher-0011-0001"));
        let again = manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap();
        assert!(!again.applied);
        assert_eq!(again.message, "该笔操作已存在，未重复执行。");
        assert_eq!(char_state(&conn)["skill_points"], serde_json::json!([15, 9]));
    }

    #[test]
    fn manage_sp_recalculate_writes_the_level_total_and_refuses_without_the_table() {
        let (_dir, cat) = manage_cat("manage-sp-recalculate");
        let mut conn = test_conn();
        // 夹具表（`gmstat::test_context`）：2/3/4/5 级各发 3/4/5/6 点，上限 5 级 ⇒ 5 级满值 18。
        bag_state(
            &mut conn,
            r#"{"level":5,"experience":1500,"skill_points":[3,3],"technique_points":[2,2],
               "attributes":{"物理攻击力":12.5},
               "inventory":{"version":"ordinary-bag-v1","gold":100,"items":[]}}"#,
        );
        let input = ManageInput {
            action: "sp.recalculate".into(),
            id: "launcher-0012-0001".into(),
            account: 1,
            character: 10,
            ..Default::default()
        };
        // 这一档不读金额：0 不能撞上「要写入的数值必须大于 0」那条门。
        let normalized = normalize_manage(&input).unwrap();
        assert_eq!(normalized.amount, 0);
        // 覆盖的就是 `characters.state`，所以 #40 那条排除式谓词必须认它。
        assert!(super::writes_character_state("sp.recalculate"));

        let ctx = crate::gmstat::test_context();
        let art = ManageArtifacts {
            grow: Some(&ctx),
            ..Default::default()
        };
        let (n, g, request) = manage_req(&input);
        let out = manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n).unwrap();
        assert!(out.applied);
        assert_eq!(out.message, "角色修改已保存，重新选择角色后生效");
        assert_eq!(out.receipt["after"], serde_json::json!([18, 18]));
        assert_eq!(out.receipt["before"], serde_json::json!([3, 3]));
        assert_eq!(out.receipt["level"], 5);
        let state = char_state(&conn);
        assert_eq!(state["skill_points"], serde_json::json!([18, 18]));
        assert_eq!(state["level"], 5, "等级与经验一个字都不动（这一档只写技能点）");
        assert_eq!(state["experience"], 1500);
        assert_eq!(state["technique_points"], serde_json::json!([2, 2]));
        assert_eq!(state["attributes"]["物理攻击力"], 12.5);
        assert_eq!(state["inventory"]["gold"], 100);

        // 产物没就绪 ⇒ 在写之前拒，库里连流水号都不留。
        let (n, g, request) = manage_req(&ManageInput {
            action: "sp.recalculate".into(),
            id: "launcher-0012-0002".into(),
            account: 1,
            character: 10,
            ..Default::default()
        });
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap_err(),
            "养成产物未就绪：gm-growth.json"
        );
        assert_eq!(char_state(&conn)["skill_points"], serde_json::json!([18, 18]));
        let rows: i64 = conn
            .query_row(
                "SELECT count(*) FROM admin_grants WHERE grant_id=?1",
                params!["launcher-0012-0002"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 0, "被拒的那一笔不许在册");

        // 等级越出成长表（夹具上限 5 级）⇒ 拒在算式里，不凑数。
        bag_state(&mut conn, r#"{"level":6,"skill_points":[1,1]}"#);
        let (n, g, request) = manage_req(&ManageInput {
            action: "sp.recalculate".into(),
            id: "launcher-0012-0003".into(),
            account: 1,
            character: 10,
            ..Default::default()
        });
        assert!(manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n)
            .unwrap_err()
            .contains("不在成长表范围内"));
    }

    #[test]
    fn manage_materials_set_shares_the_same_locks() {
        let (_dir, cat) = manage_cat("manage-materials");
        let mut conn = test_conn();
        let base = ManageInput {
            action: "materials.set".into(),
            id: "launcher-0008-0001".into(),
            account: 1,
            character: 10,
            template: 3033,
            amount: 50,
            expected_amount: Some(0),
            ..Default::default()
        };
        let (n, g, request) = manage_req(&base);
        let out = manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap();
        assert!(out.applied);
        assert_eq!(
            out.message,
            "仓库数量已保存；同账号共享，重新进入角色后同步游戏显示"
        );
        let counts: Vec<u8> = conn
            .query_row(
                "SELECT counts FROM account_material_storage WHERE account_id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            String::from_utf8(counts).unwrap(),
            r#"{"version":"account-materials-v1","counts":{"363":50}}"#
        );

        // 原数量对不上 ⇒ 拒，而且流水号不许留在册（整笔回滚）。
        let stale = ManageInput {
            expected_amount: Some(1),
            id: "launcher-0008-0002".into(),
            ..base.clone()
        };
        let (n2, g2, request2) = manage_req(&stale);
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g2, &request2, &n2).unwrap_err(),
            "材料数量已从 1 变为 50，请刷新后再修改"
        );
        let live: i64 = conn
            .query_row("SELECT count(*) FROM admin_grants WHERE grant_id=?1", params![g2.id], |r| r.get(0))
            .unwrap();
        assert_eq!(live, 0);

        // 不是仓库材料的模板 ⇒ 拒。
        let wrong = ManageInput {
            template: 999999,
            id: "launcher-0008-0003".into(),
            ..base.clone()
        };
        let (n3, g3, request3) = manage_req(&wrong);
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g3, &request3, &n3).unwrap_err(),
            "该物品不属于账号晶体仓库"
        );

        // 数量写 0 ⇒ 清掉那一格（与 Go 的 delete(counts, slot) 同）。
        let zero = ManageInput {
            amount: 0,
            expected_amount: Some(50),
            id: "launcher-0008-0004".into(),
            ..base.clone()
        };
        let (n4, g4, request4) = manage_req(&zero);
        assert!(manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g4, &request4, &n4).unwrap().applied);
        let counts: Vec<u8> = conn
            .query_row(
                "SELECT counts FROM account_material_storage WHERE account_id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(String::from_utf8(counts).unwrap(), r#"{"version":"account-materials-v1"}"#);
    }

    #[test]
    fn manage_input_rules_match_go_preconditions() {
        let ok = ManageInput {
            action: "inventory.amount".into(),
            id: "launcher-0009-0001".into(),
            account: 1,
            character: 10,
            template: 5,
            amount: 7,
            expected_amount: Some(3),
            revision: "a".repeat(64),
            collection: "items".into(),
            ..Default::default()
        };
        assert_eq!(normalize_manage(&ok).unwrap().reason, DEFAULT_MANAGE_REASON);

        let mut bad = ok.clone();
        bad.id = "grant-0001".into();
        assert_eq!(normalize_manage(&bad).unwrap_err(), "管理操作缺少有效的流水号");

        bad = ok.clone();
        bad.character = 0;
        assert_eq!(normalize_manage(&bad).unwrap_err(), "请选择账号和角色");

        bad = ok.clone();
        bad.action = "quests.unknown".into();
        assert_eq!(normalize_manage(&bad).unwrap_err(), "不支持的管理操作");

        // 任务五个动作也进白名单了；带不上任务 id 才拒（Go 的 1..100 那一条）。
        bad = ok.clone();
        bad.action = "quests.complete".into();
        assert_eq!(normalize_manage(&bad).unwrap_err(), "单次请选择 1 至 100 个任务");

        bad = ok.clone();
        bad.revision = String::new();
        assert_eq!(
            normalize_manage(&bad).unwrap_err(),
            "背包操作缺少原物品快照，请刷新后重试"
        );

        bad = ok.clone();
        bad.expected_amount = Some(0);
        assert!(normalize_manage(&bad).is_err());

        // 仓库那条不吃快照三件套（Go：只有 bagEdit 才校验）。
        let m = ManageInput {
            action: "materials.set".into(),
            template: 3033,
            amount: 1,
            expected_amount: None,
            ..ok.clone()
        };
        assert!(normalize_manage(&m).is_ok());

        // 养成四档进白名单，且与 Go 一样吃「数值必须大于 0」这条。
        let grow = ManageInput {
            action: "level".into(),
            template: 0,
            amount: 30,
            expected_amount: None,
            revision: String::new(),
            collection: String::new(),
            ..ok.clone()
        };
        assert!(normalize_manage(&grow).is_ok());
        let mut zero = grow.clone();
        zero.amount = 0;
        assert_eq!(normalize_manage(&zero).unwrap_err(), "要写入的数值必须大于 0");

        // 只有 profession 需要阶段；缺阶段或阶段越界都拒在领取之前（Go 的请求门）。
        let branch = ManageInput { action: "profession".into(), amount: 1, ..grow.clone() };
        assert_eq!(normalize_manage(&branch).unwrap_err(), "请选择有效的转职分支和觉醒阶段");
        let mut bad_stage = branch.clone();
        bad_stage.stage = Some(4);
        assert_eq!(normalize_manage(&bad_stage).unwrap_err(), "请选择有效的转职分支和觉醒阶段");
        bad_stage.stage = Some(2);
        bad_stage.amount = 16;
        assert_eq!(normalize_manage(&bad_stage).unwrap_err(), "请选择有效的转职分支和觉醒阶段");
        bad_stage.amount = 2;
        assert!(normalize_manage(&bad_stage).is_ok());

        // 觉醒/转职分支那两条不吃背包三件套。
        let awaken = ManageInput { action: "awakening".into(), amount: 1, ..grow.clone() };
        assert!(normalize_manage(&awaken).is_ok());

        // 耐久那一档：三件套 + 「当前耐久」缺一不可。数量不是它看的字段，所以不吃 expected_amount。
        let dur = ManageInput {
            action: "equipment.durability".into(),
            template: 101040829,
            amount: 120,
            expected_amount: None,
            expected_durability: Some(48),
            revision: "b".repeat(64),
            collection: "worn".into(),
            ..ok.clone()
        };
        assert!(normalize_manage(&dur).is_ok());
        // 耐久 0 是合法当前值（打坏的装备照样要能改回来），所以只要求「带」。
        let mut broken = dur.clone();
        broken.expected_durability = Some(0);
        assert!(normalize_manage(&broken).is_ok());
        for drop in ["cur", "revision", "template", "amount"] {
            let mut bad_input = dur.clone();
            match drop {
                "cur" => bad_input.expected_durability = None,
                "revision" => bad_input.revision = String::new(),
                "template" => bad_input.template = 0,
                _ => bad_input.amount = 0,
            }
            assert_eq!(
                normalize_manage(&bad_input).unwrap_err(),
                "装备耐久操作缺少原装备快照，请刷新后重试",
                "漏掉 {drop}"
            );
        }
    }

    // ---- 养成四档的整链路（真事务 + 审计 + 幂等） ----

    /// 养成夹具：`config_version` 用当前存档契约身份，`source_sha256` 用产物那份 `.chr` 指纹。
    /// 背包容器留着，用来验这一笔只落改动的键、其余原封不动。
    fn grow_state(conn: &mut rusqlite::Connection, level: u8) {
        let json = format!(
            r#"{{"level":{level},"experience":0,"attributes":{{"[hp max]":100}},"skill_points":[0,0],"learned_skills":[null,null],"advancement":0,"awakening":0,"source_sha256":"{}","inventory":{{"version":"ordinary-bag-v1","gold":5,"items":[]}}}}"#,
            crate::gmstat::TEST_HASH
        );
        conn.execute(
            "UPDATE characters SET config_version=?1, state=CAST(?2 AS BLOB) WHERE id=10",
            params![crate::gmstat::save_identity(), json],
        )
        .unwrap();
    }

    fn grant_rows(conn: &rusqlite::Connection) -> i64 {
        conn.query_row("SELECT count(*) FROM admin_grants", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn manage_growth_level_writes_state_and_audits_both_archives() {
        let (_dir, cat) = manage_cat("manage-growth-level");
        let mut conn = test_conn();
        grow_state(&mut conn, 1);
        let ctx = crate::gmstat::test_context();

        let input = ManageInput {
            action: "level".into(),
            id: "launcher-0011-0001".into(),
            account: 1,
            character: 10,
            amount: 3,
            ..Default::default()
        };
        let (n, g, request) = manage_req(&input);
        let out = manage_inner(&mut conn, &cat, 0, &ManageArtifacts { grow: Some(&ctx), ..Default::default() }, &g, &request, &n).unwrap();
        assert!(out.applied);
        assert_eq!(out.message, "角色修改已保存，重新选择角色后生效");
        let state = char_state(&conn);
        assert_eq!(state["level"], 3);
        assert_eq!(state["experience"], 250);
        assert_eq!(state["skill_points"][0], 7, "跨 2、3 两级各授 3/4 点");
        assert_eq!(state["attributes"]["[hp max]"], 125);
        assert_eq!(state["inventory"]["gold"], 5, "这一笔只落改动的键，背包容器原封不动");
        let kind: String = conn
            .query_row("SELECT typeof(state) FROM characters WHERE id=10", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kind, "blob", "回写必须是 BLOB，与服务端写的那些同形");

        // 审计回执：整份改前改后，字段名与 Go editCharacter 的 details 对齐。
        let raw: Vec<u8> = conn
            .query_row("SELECT receipt FROM admin_grants WHERE grant_id=?1", params![g.id], |r| r.get(0))
            .unwrap();
        let receipt: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(receipt["action"], "level");
        assert_eq!(receipt["value"], 3);
        assert!(receipt["stage"].is_null(), "不带阶段的那三档 stage 落 null，与 Go 的 *byte 同形");
        assert_eq!(receipt["before"]["level"], 1);
        assert_eq!(receipt["after"]["level"], 3);

        // 同流水号重放 ⇒ 领号即回，等级不会涨两次。
        let again = manage_inner(&mut conn, &cat, 0, &ManageArtifacts { grow: Some(&ctx), ..Default::default() }, &g, &request, &n).unwrap();
        assert!(!again.applied);
        assert_eq!(again.message, "该笔操作已存在，未重复执行。");
        assert_eq!(char_state(&conn)["level"], 3);

        // 换个流水号重复同一笔 ⇒ 目标等级不高于当前，Go 那句拒绝；半行流水都不留。
        let mut repeat = input.clone();
        repeat.id = "launcher-0011-0002".into();
        let (n2, g2, request2) = manage_req(&repeat);
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &ManageArtifacts { grow: Some(&ctx), ..Default::default() }, &g2, &request2, &n2).unwrap_err(),
            "目标等级必须高于当前等级且不超过服务器上限"
        );
        assert_eq!(char_state(&conn)["level"], 3);
        assert_eq!(grant_rows(&conn), 1, "拒绝的那笔不留下流水行");
    }

    #[test]
    fn manage_growth_profession_threads_stage_and_allows_idempotent_replay() {
        let (_dir, cat) = manage_cat("manage-growth-profession");
        let mut conn = test_conn();
        grow_state(&mut conn, 1);
        let ctx = crate::gmstat::test_context();

        let input = ManageInput {
            action: "profession".into(),
            id: "launcher-0012-0001".into(),
            account: 1,
            character: 10,
            amount: 1,
            stage: Some(0),
            ..Default::default()
        };
        let (n, g, request) = manage_req(&input);
        let out = manage_inner(&mut conn, &cat, 0, &ManageArtifacts { grow: Some(&ctx), ..Default::default() }, &g, &request, &n).unwrap();
        assert!(out.applied);
        let state = char_state(&conn);
        assert_eq!(state["advancement"], 1);
        assert_eq!(state["awakening"], 0);

        // 阶段一路留在链路里：审计的 reason 是整份请求，receipt 是这份带阶段的回执。
        let reason: String = conn
            .query_row("SELECT reason FROM admin_grants WHERE grant_id=?1", params![g.id], |r| r.get(0))
            .unwrap();
        assert!(reason.contains(r#""stage":0"#), "{reason}");
        let raw: Vec<u8> = conn
            .query_row("SELECT receipt FROM admin_grants WHERE grant_id=?1", params![g.id], |r| r.get(0))
            .unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&raw).unwrap()["stage"], 0);

        // 已在该分支、阶段 0 ⇒ gmstat 把原字节交回；流水照留，存档不变（Go 先领号再空转）。
        let mut noop = input.clone();
        noop.id = "launcher-0012-0002".into();
        let (n2, g2, request2) = manage_req(&noop);
        let out = manage_inner(&mut conn, &cat, 0, &ManageArtifacts { grow: Some(&ctx), ..Default::default() }, &g2, &request2, &n2).unwrap();
        assert!(out.applied);
        assert_eq!(char_state(&conn)["advancement"], 1);
        assert_eq!(grant_rows(&conn), 2);

        // 等级还没到一觉的门 ⇒ 拒在这里，存档与流水都不动。
        let mut gated = input.clone();
        gated.id = "launcher-0012-0003".into();
        gated.stage = Some(1);
        let (n3, g3, request3) = manage_req(&gated);
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &ManageArtifacts { grow: Some(&ctx), ..Default::default() }, &g3, &request3, &n3).unwrap_err(),
            "所选觉醒需要达到 50 级"
        );
        assert_eq!(grant_rows(&conn), 2);
    }

    #[test]
    fn manage_growth_refuses_before_claim_when_gated() {
        let (_dir, cat) = manage_cat("manage-growth-gate");
        let mut conn = test_conn();
        grow_state(&mut conn, 1);
        let input = ManageInput {
            action: "awakening".into(),
            id: "launcher-0013-0001".into(),
            account: 1,
            character: 10,
            amount: 1,
            ..Default::default()
        };
        let (n, g, request) = manage_req(&input);

        // 产物未就绪：四档一律拒在领号之前，什么都不落。
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap_err(),
            "养成产物未就绪：gm-growth.json"
        );
        assert_eq!(grant_rows(&conn), 0);
        assert_eq!(char_state(&conn)["level"], 1);

        // 存档身份不符（别的 pack 的 config_version）：同一道 Go 拒绝话术。
        conn.execute("UPDATE characters SET config_version='other-pack' WHERE id=10", [])
            .unwrap();
        let ctx = crate::gmstat::test_context();
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &ManageArtifacts { grow: Some(&ctx), ..Default::default() }, &g, &request, &n).unwrap_err(),
            "当前 PVF 职业目录与角色存档不匹配，已拒绝修改"
        );
        assert_eq!(grant_rows(&conn), 0);

        // 身份门也接受产物那次构建的归档校验（Go `BindGMIdentity` 允许的那条旧身份）：
        // 这一份夹具的 checksum 正是那枚 64 位 hex，所以这一笔要能写进去。
        conn.execute("UPDATE characters SET config_version=?1 WHERE id=10", params![crate::gmstat::TEST_HASH])
            .unwrap();
        let mut legacy = input.clone();
        legacy.action = "level".into();
        legacy.id = "launcher-0013-0002".into();
        legacy.amount = 3;
        let (n2, g2, request2) = manage_req(&legacy);
        assert!(manage_inner(&mut conn, &cat, 0, &ManageArtifacts { grow: Some(&ctx), ..Default::default() }, &g2, &request2, &n2).is_ok());
        assert_eq!(char_state(&conn)["level"], 3);
    }

    // ---- 任务五个动作的整链路（真事务 + 审计 + 计划签名） ----

    /// 任务夹具：角色 50 级、`config_version` 用当前存档契约身份（任务这一侧只认它，
    /// 没有养成那侧的「归档校验值」旧身份分支）。背包留着，用来验任务这一笔
    /// 一个字节都不碰角色存档。
    fn quest_state(conn: &mut rusqlite::Connection) -> Value {
        let json = format!(
            r#"{{"level":50,"experience":0,"attributes":{{"[hp max]":100}},"skill_points":[0,0],"learned_skills":[null,null],"advancement":0,"awakening":0,"source_sha256":"{}","inventory":{{"version":"ordinary-bag-v1","gold":5,"items":[]}}}}"#,
            crate::gmstat::TEST_HASH
        );
        conn.execute(
            "UPDATE characters SET config_version=?1, state=CAST(?2 AS BLOB) WHERE id=10",
            params![crate::gmstat::save_identity(), json],
        )
        .unwrap();
        char_state(conn)
    }

    /// 库里摆一行任务。`model` 传空串取产物那条已认模型；传别的就是为了钉
    /// 「进度模型尚未闭环」那一条拒绝。
    fn quest_row(conn: &mut rusqlite::Connection, id: u16, status: &str, model: &str) {
        let model = if model.is_empty() { "single-meet-npc-remaining-v1" } else { model };
        conn.execute(
            "INSERT INTO character_quests(character_id,quest_id,status,progress,config_version,progress_model) \
             VALUES(10,?1,?2,3,?3,?4)",
            params![id, status, crate::gmstat::save_identity(), model],
        )
        .unwrap();
    }

    /// (状态, 进度, 进度模型)；没有行就是 None —— 放弃任务那条要看的就是 None。
    fn quest_line(conn: &rusqlite::Connection, id: u16) -> Option<(String, i64, String)> {
        use rusqlite::OptionalExtension;
        conn.query_row(
            "SELECT status,progress,progress_model FROM character_quests \
             WHERE character_id=10 AND quest_id=?1",
            params![id],
            |r| Ok((r.get(0)?, r.get::<_, i64>(1)?, r.get(2)?)),
        )
        .optional()
        .unwrap()
    }

    fn quest_closed_at(conn: &rusqlite::Connection, id: u16) -> Option<i64> {
        conn.query_row(
            "SELECT completed_at FROM character_quests WHERE character_id=10 AND quest_id=?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn quest_role(conn: &rusqlite::Connection) -> crate::gmquest::Role {
        crate::gmquest::read_role(conn, 1, 10).unwrap()
    }

    /// 界面那一侧带来的 revision：同一份算法，读的是同一批行。
    fn quest_head(conn: &rusqlite::Connection) -> String {
        let role = quest_role(conn);
        let states = crate::gmquest::read_states(conn, role.id).unwrap();
        crate::gmquest::quest_revision(&role, &states).unwrap()
    }

    /// 预览一次计划（与事务内重算同一入口），执行只要它那份签名。
    fn quest_plan(conn: &rusqlite::Connection, action: &str, target: u16, level: u8) -> crate::gmquest::GmQuestPlan {
        let file = crate::gmquest::test_quest_file();
        let ctx = crate::gmstat::test_context();
        let role = quest_role(conn);
        let states = crate::gmquest::read_states(conn, role.id).unwrap();
        if action == "quests.prerequisites" {
            crate::gmquest::prerequisite_plan(&file, &ctx, &role, &states, target).unwrap()
        } else {
            crate::gmquest::mainline_plan(&file, &ctx, &role, &states, level).unwrap()
        }
    }

    fn quest_input(action: &str, id: &str) -> ManageInput {
        ManageInput {
            action: action.into(),
            id: id.into(),
            account: 1,
            character: 10,
            ..Default::default()
        }
    }

    #[test]
    fn manage_quests_ready_complete_abandon_write_rows_and_audit() {
        let (_dir, cat) = manage_cat("manage-quests-single");
        let mut conn = test_conn();
        let state_before = quest_state(&mut conn);
        quest_row(&mut conn, 100, "accepted", "");
        quest_row(&mut conn, 101, "accepted", "");
        quest_row(&mut conn, 102, "accepted", "");
        quest_row(&mut conn, 105, "accepted", "");
        let file = crate::gmquest::test_quest_file();
        let art = ManageArtifacts {
            quests: Some(&file),
            ..Default::default()
        };

        // 1) 置为可交付：只有进度归零，状态仍是进行中。
        let input = ManageInput {
            revision: quest_head(&conn),
            quests: vec![100],
            ..quest_input("quests.ready", "launcher-0021-0001")
        };
        let (n, g, request) = manage_req(&input);
        let first = manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n).unwrap();
        assert!(first.applied);
        assert_eq!(first.message, "修改已保存并记录审计");
        assert_eq!(
            quest_line(&conn, 100).unwrap(),
            ("accepted".into(), 0, "single-meet-npc-remaining-v1".into())
        );
        // 回执带改前那一行整份，且 rewards 恒 false —— 任务这一侧从不发奖励。
        assert_eq!(first.receipt["action"], "quests.ready");
        assert_eq!(first.receipt["quests"][0], 100);
        assert_eq!(first.receipt["rewards"], false);
        assert_eq!(first.receipt["before"][0]["id"], 100);
        assert_eq!(first.receipt["before"][0]["progress"], 3);
        assert_eq!(char_state(&conn), state_before, "任务写侧一个字节都不碰角色存档");

        // 2) 标记完成：重复 id 只处理一次（`before` 只有一行），时间戳落下。
        let mut next = ManageInput {
            revision: quest_head(&conn),
            quests: vec![101, 101],
            ..quest_input("quests.complete", "launcher-0021-0002")
        };
        let (n, g, request) = manage_req(&next);
        let out = manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n).unwrap();
        assert_eq!(quest_line(&conn, 101).unwrap().0, "completed");
        assert!(quest_closed_at(&conn, 101).is_some());
        assert_eq!(out.receipt["before"].as_array().unwrap().len(), 1);
        assert_eq!(out.receipt["quests"].as_array().unwrap().len(), 2, "quests 原样回执界面带来的那份");

        // 3) 放弃：整行删掉，不是写成别的状态。
        next = ManageInput {
            revision: quest_head(&conn),
            quests: vec![102],
            ..quest_input("quests.abandon", "launcher-0021-0003")
        };
        let (n, g, request) = manage_req(&next);
        assert!(manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n).unwrap().applied);
        assert!(quest_line(&conn, 102).is_none());
        assert_eq!(grant_rows(&conn), 3);

        // 同流水号重放 ⇒ 领号即回，任务不会被动第二次。
        let again = manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n).unwrap();
        assert!(!again.applied);
        assert_eq!(again.message, "该笔操作已存在，未重复执行。");
        assert_eq!(again.receipt["action"], "quests.abandon");
        assert_eq!(grant_rows(&conn), 3);

        // 4) 带着过期的 revision 再来一笔：拒在乐观锁，库里那一行原封不动。
        let mut stale = ManageInput {
            revision: quest_head(&conn),
            quests: vec![105],
            ..quest_input("quests.ready", "launcher-0021-0004")
        };
        // 界面拿到 head 之后，别人动了这一行 —— 直接改库来模拟。
        conn.execute("UPDATE character_quests SET progress=1 WHERE character_id=10 AND quest_id=105", [])
            .unwrap();
        stale.revision = "0".repeat(64);
        let (n, g, request) = manage_req(&stale);
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n).unwrap_err(),
            "角色或任务已变化，请刷新任务后再操作"
        );
        assert_eq!(quest_line(&conn, 105).unwrap().1, 1, "拒绝的那笔不该把进度写成 0");
        assert_eq!(grant_rows(&conn), 3, "拒绝的那笔不留下流水行");
    }

    #[test]
    fn manage_quests_refusals_are_per_id_and_rollback_the_batch() {
        let (_dir, cat) = manage_cat("manage-quests-refusal");
        let mut conn = test_conn();
        quest_state(&mut conn);
        quest_row(&mut conn, 100, "accepted", "");
        quest_row(&mut conn, 400, "completed", "");
        quest_row(&mut conn, 105, "accepted", "legacy-zero");
        let file = crate::gmquest::test_quest_file();
        let art = ManageArtifacts {
            quests: Some(&file),
            ..Default::default()
        };

        // 逐条点名，话术与 Go 一致；「不在目录」判在「尚未接取」之前。
        for (quests, expected) in [
            (vec![999u16], "任务 999 不在当前目录"),
            (vec![104], "任务 104 尚未接取"),
            (vec![400], "任务 400 不处于可修改的进行中状态"),
            (vec![105], "任务 105 的进度模型尚未闭环，拒绝强制修改"),
        ] {
            let input = ManageInput {
                quests,
                ..quest_input("quests.complete", "launcher-0022-0001")
            };
            let (n, g, request) = manage_req(&input);
            assert_eq!(
                manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n).unwrap_err(),
                expected
            );
            assert_eq!(grant_rows(&conn), 0, "{expected}");
        }

        // 同批里前一条已写成功、后一条被拒 ⇒ 整笔事务回滚，一条也不留。
        let input = ManageInput {
            quests: vec![100, 400],
            ..quest_input("quests.complete", "launcher-0022-0002")
        };
        let (n, g, request) = manage_req(&input);
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n).unwrap_err(),
            "任务 400 不处于可修改的进行中状态"
        );
        assert_eq!(
            quest_line(&conn, 100).unwrap(),
            ("accepted".into(), 3, "single-meet-npc-remaining-v1".into()),
            "同批里已经写下去的那一条也得退回去"
        );
        assert_eq!(grant_rows(&conn), 0);

        // 存档身份是归档校验值：养成那侧接受，任务这侧整档拒（不带 revision 才走到身份门）。
        conn.execute("UPDATE characters SET config_version=?1 WHERE id=10", params![crate::gmstat::TEST_HASH])
            .unwrap();
        let input = ManageInput {
            quests: vec![100],
            ..quest_input("quests.complete", "launcher-0022-0003")
        };
        let (n, g, request) = manage_req(&input);
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n).unwrap_err(),
            "任务目录与角色版本不匹配"
        );
        assert_eq!(grant_rows(&conn), 0);
    }

    #[test]
    fn manage_quests_request_gate_and_missing_artifacts() {
        let (_dir, cat) = manage_cat("manage-quests-gate");
        let mut conn = test_conn();
        quest_state(&mut conn);
        quest_row(&mut conn, 100, "accepted", "");
        quest_row(&mut conn, 101, "accepted", "");
        let file = crate::gmquest::test_quest_file();
        let with_quests = ManageArtifacts {
            quests: Some(&file),
            ..Default::default()
        };

        // 请求门（Go 的 1..100）：追溯前置也要先过这一条，「恰好一个」留到事务里。
        let empty = quest_input("quests.prerequisites", "launcher-0023-0001");
        assert_eq!(normalize_manage(&empty).unwrap_err(), "单次请选择 1 至 100 个任务");
        let many = ManageInput {
            quests: (1..=101u16).collect(),
            ..quest_input("quests.complete", "launcher-0023-0002")
        };
        assert_eq!(normalize_manage(&many).unwrap_err(), "单次请选择 1 至 100 个任务");
        // 主线的任务数不受这一条管（它带的是目标等级）。
        assert!(normalize_manage(&ManageInput {
            level: 50,
            ..quest_input("quests.mainline", "launcher-0023-0003")
        })
        .is_ok());

        // 产物缺口各拒各的，话术与 `gmstat` 那三条养成档同形。
        let input = ManageInput {
            quests: vec![100],
            ..quest_input("quests.complete", "launcher-0023-0004")
        };
        let (n, g, request) = manage_req(&input);
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap_err(),
            "任务产物未就绪：gm-quests.json"
        );
        assert_eq!(grant_rows(&conn), 0);

        let mainline = ManageInput {
            level: 50,
            plan: "a".repeat(64),
            ..quest_input("quests.mainline", "launcher-0023-0005")
        };
        let (n, g, request) = manage_req(&mainline);
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &with_quests, &g, &request, &n).unwrap_err(),
            "养成产物未就绪：gm-growth.json",
            "主线要拿职业名做资格判据，成长产物缺了不能降级"
        );
        // 签名带错（含空）：一律「请重新预览」，一个字节不写。
        for plan in [String::new(), "a".repeat(64)] {
            let bad = ManageInput {
                level: 50,
                plan,
                ..quest_input("quests.mainline", "launcher-0023-0006")
            };
            let (n, g, request) = manage_req(&bad);
            assert_eq!(
                manage_inner(&mut conn, &cat, 0, &ManageArtifacts {
                    grow: Some(&crate::gmstat::test_context()),
                    quests: Some(&file),
                    ..Default::default()
                }, &g, &request, &n)
                    .unwrap_err(),
                "角色或任务已变化，请重新预览任务范围"
            );
        }
        // 追溯前置带两个 id：事务里才判「恰好一个」。
        let two = ManageInput {
            quests: vec![101, 102],
            plan: "b".repeat(64),
            ..quest_input("quests.prerequisites", "launcher-0023-0007")
        };
        let (n, g, request) = manage_req(&two);
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &ManageArtifacts {
                grow: Some(&crate::gmstat::test_context()),
                quests: Some(&file),
                ..Default::default()
            }, &g, &request, &n)
                .unwrap_err(),
            "追溯前置需要选择一个任务"
        );
        assert_eq!(grant_rows(&conn), 0);
    }

    #[test]
    fn manage_quests_mainline_marks_skip_rows_and_refuses_a_stale_preview() {
        let (_dir, cat) = manage_cat("manage-quests-mainline");
        let mut conn = test_conn();
        let state_before = quest_state(&mut conn);
        quest_row(&mut conn, 400, "completed", "");
        let file = crate::gmquest::test_quest_file();
        let ctx = crate::gmstat::test_context();
        let art = ManageArtifacts {
            grow: Some(&ctx),
            quests: Some(&file),
            ..Default::default()
        };

        let plan = quest_plan(&conn, "quests.mainline", 0, 50);
        // 与 `gmquest` 那批纯算法用例同一份选择：逐层闭合 100→101→102、103 的替代段、
        // 105/106（互斥同伴 300 自己被挡）、答案门取 200。
        assert_eq!(plan.quests, vec![100, 101, 102, 103, 105, 106, 200]);

        let input = ManageInput {
            level: 50,
            plan: plan.signature.clone(),
            ..quest_input("quests.mainline", "launcher-0024-0001")
        };
        let (n1, g1, request1) = manage_req(&input);
        let out = manage_inner(&mut conn, &cat, 0, &art, &g1, &request1, &n1).unwrap();
        assert!(out.applied);
        assert_eq!(out.message, "主线任务已按预览完成，未发放奖励；重新选择角色后生效");
        assert_eq!(out.receipt["action"], "quests.mainline");
        assert_eq!(out.receipt["rewards"], false);
        assert_eq!(out.receipt["plan"]["signature"], plan.signature);
        for id in &plan.quests {
            // 逐条 upsert：没接过的补一行 completed，落的就是 Go 那枚跳过标记模型。
            assert_eq!(
                quest_line(&conn, *id).unwrap(),
                ("completed".into(), 0, "gm-mainline-skip-v1".into()),
                "任务 {id}"
            );
            assert!(quest_closed_at(&conn, *id).is_some());
        }
        // 答案门的另一条、被挡的那批都不该有行；角色存档逐字不变。
        assert!(quest_line(&conn, 201).is_none());
        assert!(quest_line(&conn, 300).is_none());
        assert_eq!(char_state(&conn), state_before);

        // 同流水号重放照旧只领号即回，跳过行不再补第二次（时间戳也不重写）。
        let closed_at = quest_closed_at(&conn, 100);
        let replay = manage_inner(&mut conn, &cat, 0, &art, &g1, &request1, &n1).unwrap();
        assert!(!replay.applied);
        assert_eq!(replay.message, "该笔操作已存在，未重复执行。");
        assert_eq!(quest_closed_at(&conn, 100), closed_at);
        assert_eq!(grant_rows(&conn), 1);

        // 不重新预览就再点一次：库里已经是完成态 ⇒ 签名对不上，整笔拒，流水也不留。
        let again = ManageInput {
            level: 50,
            plan: plan.signature.clone(),
            ..quest_input("quests.mainline", "launcher-0024-0002")
        };
        let (n, g, request) = manage_req(&again);
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n).unwrap_err(),
            "角色或任务已变化，请重新预览任务范围"
        );
        assert_eq!(grant_rows(&conn), 1);

        // 全目录已完成 ⇒ 计划为空，带着这份合法签名执行也要拒在「没有符合条件的任务」。
        for id in file.quests.keys().copied() {
            if id <= u32::from(u16::MAX) && quest_line(&conn, id as u16).is_none() {
                quest_row(&mut conn, id as u16, "completed", "");
            }
        }
        let done = quest_plan(&conn, "quests.mainline", 0, 50);
        assert!(done.quests.is_empty());
        let empty = ManageInput {
            level: 50,
            plan: done.signature.clone(),
            ..quest_input("quests.mainline", "launcher-0024-0003")
        };
        let (n, g, request) = manage_req(&empty);
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n).unwrap_err(),
            "没有符合条件的未完成任务"
        );
        assert_eq!(grant_rows(&conn), 1);
    }

    #[test]
    fn manage_quests_mainline_refuses_when_the_player_finished_one_first() {
        let (_dir, cat) = manage_cat("manage-quests-race");
        let mut conn = test_conn();
        quest_state(&mut conn);
        quest_row(&mut conn, 400, "completed", "");
        let file = crate::gmquest::test_quest_file();
        let ctx = crate::gmstat::test_context();
        let art = ManageArtifacts {
            grow: Some(&ctx),
            quests: Some(&file),
            ..Default::default()
        };

        let plan = quest_plan(&conn, "quests.mainline", 0, 50);
        assert_eq!(plan.quests, vec![100, 101, 102, 103, 105, 106, 200]);
        // 预览之后、执行之前，玩家自己把 100 做完了。事务内重算的那一份已经没有它，
        // 签名随之改变 ⇒ 拒在预览失效这一条，界面必须回去重新预览。
        quest_row(&mut conn, 100, "completed", "");
        let input = ManageInput {
            level: 50,
            plan: plan.signature.clone(),
            ..quest_input("quests.mainline", "launcher-0025-0001")
        };
        let (n, g, request) = manage_req(&input);
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n).unwrap_err(),
            "角色或任务已变化，请重新预览任务范围"
        );
        assert_eq!(grant_rows(&conn), 0);
        // 组里其余的每一条都不许留下：整组撤销，不是「完成到哪算哪」。
        for id in &plan.quests {
            assert!(quest_line(&conn, *id).is_none() || *id == 100, "任务 {id} 该随事务一起退回去");
        }

        // 重新预览一次就写得进去了：计划短了一截，签名是新的。
        let again = quest_plan(&conn, "quests.mainline", 0, 50);
        assert_eq!(again.quests, vec![101, 102, 103, 105, 106, 200]);
        assert_ne!(again.signature, plan.signature);
        let fresh = ManageInput {
            level: 50,
            plan: again.signature.clone(),
            ..quest_input("quests.mainline", "launcher-0025-0002")
        };
        let (n, g, request) = manage_req(&fresh);
        assert!(manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n).unwrap().applied);
        assert_eq!(quest_line(&conn, 101).unwrap().0, "completed");
    }

    /// 跳过用的那条 upsert：`DO UPDATE ... WHERE` 不成立时 SQLite 报 0 行，
    /// 正是 Go `mainlineSkipSQL` 的 RowsAffected 口径，也是「任务状态发生变化，
    /// 整组操作已撤销」那句拒绝的唯一依据。合法的自算计划带不出这两种 0 行形状
    /// （别的存档身份会被归进 Blocked、已完成的本就进不了候选），所以这里直接打
    /// SQL 把计数钉住，而不是假装计划能造出它。
    #[test]
    fn mainline_skip_upsert_counts_affected_rows_like_go() {
        let mut conn = test_conn();
        quest_state(&mut conn);
        let identity = crate::gmstat::save_identity();
        let skip = |conn: &rusqlite::Connection, id: u16, version: &str| -> usize {
            conn.execute(
                crate::gmquest::SQL_MAINLINE_SKIP,
                params![10i64, id as i64, version],
            )
            .unwrap()
        };

        // 没接过的补一行 ⇒ 1 行，落 skipped 标记模型。
        assert_eq!(skip(&conn, 100, &identity), 1);
        assert_eq!(
            quest_line(&conn, 100).unwrap(),
            ("completed".into(), 0, "gm-mainline-skip-v1".into())
        );
        // 进行中且同存档身份 ⇒ 覆盖成完成，1 行。
        quest_row(&mut conn, 101, "accepted", "");
        assert_eq!(skip(&conn, 101, &identity), 1);
        assert_eq!(quest_line(&conn, 101).unwrap().0, "completed");
        // 早已完成 ⇒ WHERE 的 status 条件不成立，0 行、原行不动。
        assert_eq!(skip(&conn, 101, &identity), 0);
        // 那一行属于别的存档身份 ⇒ 版本条件不成立，0 行，更不能把它改写成本存档的。
        quest_row(&mut conn, 102, "accepted", "");
        conn.execute(
            "UPDATE character_quests SET config_version='别的存档身份' \
             WHERE character_id=10 AND quest_id=102",
            [],
        )
        .unwrap();
        assert_eq!(skip(&conn, 102, &identity), 0);
        assert_eq!(quest_line(&conn, 102).unwrap().0, "accepted");
    }

    #[test]
    fn manage_quests_prerequisites_keeps_the_target_row() {
        let (_dir, cat) = manage_cat("manage-quests-prereq");
        let mut conn = test_conn();
        quest_state(&mut conn);
        quest_row(&mut conn, 100, "completed", "");
        quest_row(&mut conn, 101, "accepted", "");
        quest_row(&mut conn, 102, "accepted", "");
        let file = crate::gmquest::test_quest_file();
        let ctx = crate::gmstat::test_context();
        let art = ManageArtifacts {
            grow: Some(&ctx),
            quests: Some(&file),
            ..Default::default()
        };

        let plan = quest_plan(&conn, "quests.prerequisites", 102, 0);
        assert_eq!(plan.quests, vec![101]);

        let input = ManageInput {
            quests: vec![102],
            plan: plan.signature.clone(),
            ..quest_input("quests.prerequisites", "launcher-0026-0001")
        };
        let (n, g, request) = manage_req(&input);
        let out = manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n).unwrap();
        assert!(out.applied);
        assert_eq!(
            out.message,
            "前置任务已按预览完成，未发放奖励，目标任务保留；重新选择角色后生效"
        );
        assert_eq!(out.receipt["action"], "quests.prerequisites");
        assert_eq!(out.receipt["rewards"], false);
        assert_eq!(quest_line(&conn, 101).unwrap().0, "completed");
        // 目标任务保留：玩家自己接着做完，才算真通关这一条。
        assert_eq!(
            quest_line(&conn, 102).unwrap(),
            ("accepted".into(), 3, "single-meet-npc-remaining-v1".into())
        );
        assert_eq!(grant_rows(&conn), 1);
    }

    // ---- 发放流水（只读 admin_grants）----

    /// 往流水表里塞一笔：`request`/`receipt` 按真表的形状写成 BLOB，`reason` 列填一句人话，
    /// 这样「动作键只在 request 里」这件事才测得出来——列同值的话，读错列也照样绿。
    fn seed_grant(conn: &rusqlite::Connection, id: &str, request: &str, at: i64, character: Option<i64>) {
        conn.execute(
            "INSERT INTO admin_grants(grant_id,account_id,character_id,request,receipt,operator,reason,created_at) \
             VALUES(?1,1,?2,CAST(?3 AS BLOB),CAST('{\"applied\":true}' AS BLOB),'launcher','补发活动奖励',?4)",
            params![id, character, request, at],
        )
        .unwrap();
    }

    #[test]
    fn grant_log_lists_newest_first_and_labels_the_action() {
        let conn = test_conn();
        // 发放请求没有 action 键 ⇒ 退回 grant。
        seed_grant(
            &conn,
            "launcher-gold",
            r#"{"id":"launcher-gold","account_id":1,"gold":100,"reason":"补发活动奖励","operator":"launcher"}"#,
            1000,
            Some(10),
        );
        // 管理请求：整份入参塞在 request 的 reason 键里（manage() 的真实拼法），动作键在那一层。
        seed_grant(
            &conn,
            "launcher-amount",
            r#"{"id":"launcher-amount","account_id":1,"character_id":10,"reason":"{\"action\":\"inventory.amount\",\"id\":\"launcher-amount\"}","operator":"launcher"}"#,
            2000,
            Some(10),
        );
        // 只发点券的那一笔可以不带角色：character_id 落 NULL，界面要能分清「账号级」。
        seed_grant(
            &conn,
            "launcher-cera",
            r#"{"id":"launcher-cera","account_id":1,"cera":50,"operator":"launcher"}"#,
            3000,
            None,
        );

        let rows = grant_log(&conn).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].grant_id, "launcher-cera", "新的在前");
        assert_eq!(rows[1].grant_id, "launcher-amount");
        assert_eq!(rows[2].grant_id, "launcher-gold");

        assert_eq!(rows[0].action, "grant");
        assert_eq!(rows[0].character_id, 0);
        assert_eq!(rows[0].character_name, "", "这一笔没有角色，不该借到名字");
        assert_eq!(rows[1].action, "inventory.amount");
        assert_eq!(rows[2].action, "grant");
        assert_eq!(rows[2].character_name, "剑魂", "角色名跟着流水一起给");

        // 请求体原样带回来（不是 reason 列），界面展开时看到的才是当时真发出去的那份。
        assert!(rows[2].request.contains("\"gold\":100"), "{}", rows[2].request);
        assert!(!rows[2].request.contains("剑魂"), "{}", rows[2].request);
        assert_eq!(rows[2].receipt, "{\"applied\":true}");
        assert_eq!(rows[2].created_at, 1000);
        assert_eq!(rows[2].operator, "launcher");

        // 建表的 CHECK(json_valid) 保证了它必是合法 JSON，但合法不等于「是个对象」——
        // 数组/标量照样解得动，`.get("action")` 为空，退回 grant 而不是把整条命令顶失败。
        seed_grant(&conn, "launcher-weird", "[1,2,3]", 4000, None);
        let rows = grant_log(&conn).unwrap();
        assert_eq!(rows[0].grant_id, "launcher-weird");
        assert_eq!(rows[0].action, "grant");
        assert_eq!(rows[0].request, "[1,2,3]");
    }

    #[test]
    fn grant_log_cuts_at_a_hundred_and_refuses_a_save_without_the_table() {
        let conn = test_conn();
        for i in 0..101 {
            seed_grant(&conn, &format!("launcher-{i:03}"), r#"{"id":"x"}"#, i, Some(10));
        }
        let rows = grant_log(&conn).unwrap();
        assert_eq!(rows.len(), GRANT_LOG_LIMIT as usize);
        assert_eq!(rows[0].grant_id, "launcher-100", "留的是最新那一百笔");
        assert_eq!(rows[rows.len() - 1].grant_id, "launcher-001", "最旧那笔被切掉");

        // 表不在 ⇒ 报错，不给空表：空表和「一笔都没发过」在界面上长得一样，该说的话完全不同。
        let bare = rusqlite::Connection::open_in_memory().unwrap();
        bare.execute_batch("CREATE TABLE characters(id INTEGER PRIMARY KEY,name TEXT);").unwrap();
        let err = grant_log(&bare).unwrap_err();
        assert!(err.contains("admin_grants"), "{err}");
    }

    // ---- 实机彩排：整条 grant() 打在「真库复制件」上 ----

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Peek {
        state_kind: String,
        gold: u64,
        item_total: u64,
        item_rows: usize,
        top_keys: Vec<String>,
        cera: i64,
        grants: i64,
    }

    /// 只读地把彩排库里这个角色的现状读成一个可比较的快照。
    fn peek(db: &std::path::Path, account: i64, character: i64, template: u32) -> Peek {
        let conn = rusqlite::Connection::open_with_flags(
            db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .expect("只读打开彩排库");
        let (kind, raw): (String, Vec<u8>) = conn
            .query_row(
                "SELECT typeof(state), state FROM characters WHERE id=?1",
                params![character],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("读角色 state");
        let state: Value = serde_json::from_slice(&raw).expect("state 必须是合法 JSON");
        let mut p = Peek {
            state_kind: kind,
            gold: 0,
            item_total: 0,
            item_rows: 0,
            top_keys: state
                .as_object()
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default(),
            cera: 0,
            grants: 0,
        };
        if let Some(inv) = state.get("inventory") {
            p.gold = inv["gold"].as_u64().unwrap_or(0);
            for row in inv.get("items").and_then(Value::as_array).into_iter().flatten() {
                if row["Template"].as_u64() == Some(u64::from(template)) {
                    p.item_rows += 1;
                    p.item_total += row["Amount"].as_u64().unwrap_or(0);
                }
            }
        }
        p.cera = conn
            .query_row(
                SQL_CERA_READ,
                params![account],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0);
        p.grants = conn
            .query_row("SELECT count(*) FROM admin_grants", [], |r| r.get(0))
            .unwrap();
        p
    }

    /// 只在设了 `DFO_REHEARSAL_ROOT` 时执行：那目录得是按整合包形状摆的彩排根
    /// （`server/work/dfo-lan/runtime/storage/` 放真库的复制件 + 同目录 local.json 指过去，
    /// `gm-tool/configs` 用 junction 指回真产物）。
    /// 跑法：`DFO_REHEARSAL_ROOT=<路径> cargo test --lib live_grant_rehearsal -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_grant_rehearsal_on_db_copy() {
        let Ok(root_dir) = std::env::var("DFO_REHEARSAL_ROOT") else {
            eprintln!("跳过：未设 DFO_REHEARSAL_ROOT");
            return;
        };
        // 真背包里就在用的堆叠件，代际与产物一致，分类必然走可堆叠分支。
        const ITEM: u32 = 10418035;
        const ID: &str = "launcher-rehearsal-20261021-a";
        let root = Root {
            path: PathBuf::from(&root_dir),
            source: "彩排目录",
        };
        let shared = GmShared::new();

        // 产物一百几十 MB，ensure 首轮只投后台加载并回阶段名，等它到「就绪」。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(240);
        loop {
            match shared.ensure(&root) {
                Ok(_) => break,
                Err(stage) if stage == "加载中" || stage == "未加载" => {
                    assert!(std::time::Instant::now() < deadline, "产物加载超时");
                    std::thread::sleep(std::time::Duration::from_millis(400));
                }
                Err(stage) => panic!("产物加载失败：{stage}"),
            }
        }

        let status = inventory_status(&root);
        assert!(status.editable, "彩排库不可写：{}", status.message);
        let chars = characters(&root).unwrap();
        assert!(!chars.is_empty(), "彩排库里没有存活角色");
        let target = &chars[0];
        let db = PathBuf::from(&status.db_path);
        let before = peek(&db, target.account_id, target.id, ITEM);

        let out = grant(
            &root,
            &shared,
            &GrantInput {
                id: ID.into(),
                account: target.account_id,
                character: target.id,
                gold: 123_456,
                cera: 7,
                items: vec![GrantItem { template: ITEM, amount: 3 }],
                reason: "实机彩排".into(),
            },
        )
        .expect("彩排发放失败");
        assert!(out.applied, "首次发放必须真付：{}", out.message);

        let after = peek(&db, target.account_id, target.id, ITEM);
        assert_eq!(after.gold, before.gold + 123_456, "金币入账数不符");
        assert_eq!(after.cera, before.cera + 7, "点券入账数不符");
        assert_eq!(after.item_total, before.item_total + 3, "物品总数不符");
        assert_eq!(after.state_kind, "blob", "写回的 state 必须仍是 BLOB（服务端就这么存）");
        assert_eq!(after.top_keys, before.top_keys, "state 顶层键集合被改动");
        assert_eq!(after.grants, before.grants + 1, "审计行没落");

        // 回放与拒绝都不许再动一个字节。回放能命中，本身就证明这次序列化的 request
        // 与落库那串逐字节相同（SQL_REPLAY 是 `request=?` 的字节比较）。
        let again = grant(
            &root,
            &shared,
            &GrantInput {
                id: ID.into(),
                account: target.account_id,
                character: target.id,
                gold: 123_456,
                cera: 7,
                items: vec![GrantItem { template: ITEM, amount: 3 }],
                reason: "实机彩排".into(),
            },
        )
        .unwrap();
        assert!(!again.applied, "{:?}", again.message);
        assert_eq!(again.message, "该笔发放已存在，未重复发放。");
        let clash = grant(
            &root,
            &shared,
            &GrantInput {
                id: ID.into(),
                account: target.account_id,
                character: target.id,
                gold: 999_999,
                cera: 7,
                items: vec![GrantItem { template: ITEM, amount: 3 }],
                reason: "实机彩排".into(),
            },
        );
        assert_eq!(clash.unwrap_err(), "流水号已用于不同发放");
        let settled = peek(&db, target.account_id, target.id, ITEM);
        assert_eq!(settled, after, "回放与拒绝不该改动任何一格");

        println!(
            "彩排通过：角色 {}（账号 {}）金币 {}→{}，点券 {}→{}，模板 {ITEM} 由 {} 行/{} 个变成 {} 行/{} 个，审计行 {}→{}",
            target.name,
            target.account_id,
            before.gold,
            after.gold,
            before.cera,
            after.cera,
            before.item_rows,
            before.item_total,
            after.item_rows,
            after.item_total,
            before.grants,
            after.grants,
        );
        println!("回执：{}", out.receipt);
    }

    // ---- 写存档前的改前备份（#40）----

    /// 落点目录里的文件名（排序）。目录不存在时返回空——「一个字节都没写」就是这么认的。
    fn backup_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|e| e.file_name().to_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn raw_state(conn: &rusqlite::Connection) -> Vec<u8> {
        conn.query_row("SELECT state FROM characters WHERE id=10", [], |r| r.get(0))
            .unwrap()
    }

    /// 备份这件事测的是「那一列的原文有没有逐字节留下来」，所以故意塞一个非数字键
    /// 与一位小数：重新序列化一次就会把它们改成 `12.5`→`12.5` 尚可、但键序与 `2147483647`
    /// 这类值都可能变，逐字节比较才是真的口径。
    const BACKUP_JSON: &str = r#"{"level":5,"experience":1200,"skill_points":[10,4],
        "technique_points":[2,2],"equipment_slot_flags":1,"attributes":{"物理攻击力":12.5},
        "inventory":{"version":"ordinary-bag-v1","gold":100,"items":[]}}"#;

    fn sp_input(id: &str) -> ManageInput {
        ManageInput {
            action: "sp".into(),
            id: id.into(),
            account: 1,
            character: 10,
            amount: 5,
            ..Default::default()
        }
    }

    #[test]
    fn manage_backs_up_the_state_before_overwriting_it() {
        let (_dir, cat) = manage_cat("manage-state-backup");
        let dst = TmpDir::new("manage-state-backup-dst");
        let mut conn = test_conn();
        bag_state(&mut conn, BACKUP_JSON);
        let before = raw_state(&conn);

        let (n, g, request) = manage_req(&sp_input("launcher-0009-0001"));
        let art = ManageArtifacts { state_backup: Some(&dst.0), ..Default::default() };
        let out = manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n).unwrap();
        assert!(out.applied);
        assert_eq!(char_state(&conn)["skill_points"], serde_json::json!([15, 9]));

        let names = backup_names(&dst.0);
        assert_eq!(names.len(), 1, "落点里只该有这一份：{names:?}");
        // 名字只带数字（账号 1、角色 10）：流水号与角色名都可能含路径字符，不进文件名。
        assert!(names[0].starts_with("role-1-10-") && names[0].ends_with(".json"), "{}", names[0]);
        let path = dst.0.join(&names[0]);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "备份必须是那一列的原文，逐字节，不是重新序列化过的 JSON"
        );
        // 落点写进回执：发放记录卡展开那一笔就能看到它。
        assert_eq!(out.receipt["state_backup"].as_str(), Some(path.display().to_string().as_str()));
    }

    #[test]
    fn manage_refuses_the_whole_write_when_the_backup_cannot_be_written() {
        let (_dir, cat) = manage_cat("manage-state-backup-fail");
        let blocked = TmpDir::new("manage-state-backup-blocked");
        // 落点的父目录是一个普通文件 ⇒ `create_dir_all` 必然失败，这就是「备份写不出来」。
        std::fs::write(blocked.0.join("blocker"), b"x").unwrap();
        let dst = blocked.0.join("blocker").join("state");
        let mut conn = test_conn();
        bag_state(&mut conn, BACKUP_JSON);
        let before = raw_state(&conn);

        let (n, g, request) = manage_req(&sp_input("launcher-0009-0002"));
        let art = ManageArtifacts { state_backup: Some(&dst), ..Default::default() };
        let err = manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n).unwrap_err();
        assert!(err.contains("备份"), "{err}");
        assert_eq!(raw_state(&conn), before, "备份没成，一个字节都不许写");
        assert_eq!(grant_rows(&conn), 0, "被拒的这一笔连流水号都不留（同号可以再用）");
        assert!(backup_names(&dst).is_empty());

        // 同一笔换个能写的落点重放：流水号没被占住，这一笔照旧能成。
        let ok = TmpDir::new("manage-state-backup-retry");
        let retry = ManageArtifacts { state_backup: Some(&ok.0), ..Default::default() };
        assert!(manage_inner(&mut conn, &cat, 0, &retry, &g, &request, &n).unwrap().applied);
    }

    #[test]
    fn actions_that_do_not_touch_state_write_no_backup() {
        let (_dir, cat) = manage_cat("manage-state-backup-skip");
        let dst = TmpDir::new("manage-state-backup-skip-dst");
        let mut conn = test_conn();
        bag_state(&mut conn, BACKUP_JSON);
        let input = ManageInput {
            action: "materials.set".into(),
            id: "launcher-0009-0003".into(),
            account: 1,
            character: 10,
            template: 3033,
            amount: 50,
            expected_amount: Some(0),
            ..Default::default()
        };
        let (n, g, request) = manage_req(&input);
        let art = ManageArtifacts { state_backup: Some(&dst.0), ..Default::default() };
        let out = manage_inner(&mut conn, &cat, 0, &art, &g, &request, &n).unwrap();
        assert!(out.applied);
        assert!(backup_names(&dst.0).is_empty());
        assert!(out.receipt.get("state_backup").is_none());

        // 排除式谓词：只有这两族明确不碰 `characters.state`，其余一律先备份。
        assert!(!writes_character_state("materials.set"));
        assert!(!writes_character_state("quests.complete"));
        assert!(!writes_character_state("quests.mainline"));
        for action in ["sp", "tp", "equipment.unlock", "inventory.amount", "inventory.delete", "equipment.durability", "level", "awakening"] {
            assert!(writes_character_state(action), "{action} 会覆盖那一列");
        }
    }

    #[test]
    fn grant_backs_up_the_state_and_refuses_when_the_backup_cannot_be_written() {
        let dir = TmpDir::new("grant-state-backup");
        let dst = TmpDir::new("grant-state-backup-dst");
        let r = rules();
        let mut conn = test_conn();
        bag_state(&mut conn, BACKUP_JSON);
        let before = raw_state(&conn);
        let cl = fake_classify(grant_table());

        let g = req("launcher-0010-0001", 1, 10, 100, 0, vec![]);
        let request = serde_json::to_vec(&g).unwrap();
        let out = grant_inner(&mut conn, &dir.0, &r, &g, &request, &cl, Some(&dst.0)).unwrap();
        assert!(out.applied);
        assert_eq!(char_state(&conn)["inventory"]["gold"], 200);
        let names = backup_names(&dst.0);
        assert_eq!(names.len(), 1, "{names:?}");
        assert!(names[0].starts_with("role-1-10-"), "{}", names[0]);
        assert_eq!(std::fs::read(dst.0.join(&names[0])).unwrap(), before);
        assert!(out.receipt["state_backup"].as_str().unwrap().ends_with(&names[0]));

        // 备份落点写不出来 ⇒ 这一笔整笔拒：金币没进背包、流水号也不留。
        std::fs::write(dir.0.join("blocker"), b"x").unwrap();
        let blocked = dir.0.join("blocker").join("state");
        let g2 = req("launcher-0010-0002", 1, 10, 50, 0, vec![]);
        let request2 = serde_json::to_vec(&g2).unwrap();
        let err = grant_inner(&mut conn, &dir.0, &r, &g2, &request2, &cl, Some(&blocked)).unwrap_err();
        assert!(err.contains("备份"), "{err}");
        assert_eq!(char_state(&conn)["inventory"]["gold"], 200, "第二笔一个字节都没写进去");
        assert_eq!(grant_rows(&conn), 1, "只剩第一笔的流水");
    }

    // ---- 邮件发放（#26）：只写 character_mail ----

    /// 三种各一枚：可堆叠（上限 1000）、时装（容器 1）、普通装备（这一轮不发）。
    /// 时装给两种形态——`kind` 直接标 avatar 的那种，和**现网那种**：`kind` 是 equipment、
    /// 由旁表 `equipment.slots.json` 把部位覆盖成 `[hat avatar]`（⇒ slot「时装」）。
    fn mail_cat(tag: &str) -> (TmpDir, gmbuild::gmindex::Catalog) {
        let dir = TmpDir::new(tag);
        dir.write(
            "items.index.json",
            r#"{"items":[
{"id":10327301,"kind":"stackable","stack_type":"[material]","stack_limit":1000,"name":"合成材料"},
{"id":10327302,"kind":"stackable","stack_type":"[material]","name":"没写上限的那一枚"},
{"id":100200301,"kind":"avatar","name":"帽子时装"},
{"id":100200399,"kind":"equipment","name":"现网形态的帽子时装"},
{"id":101040829,"kind":"equipment","name":"一把武器"}
]}"#,
        );
        dir.write(
            "equipment.slots.json",
            r#"{"slots":{
"100200399":{"cell":"[hat avatar]","level":0},
"101040829":{"cell":"[sword]","level":5}
}}"#,
        );
        let cat = gmbuild::gmindex::load(&dir.0).expect("fixture catalog");
        (dir, cat)
    }

    fn mail_input(id: &str, gold: u32, template: u32, amount: u32) -> ManageInput {
        ManageInput {
            action: "mail.send".into(),
            id: id.into(),
            account: 1,
            character: 10,
            gold,
            template,
            amount,
            body: "维护补偿".into(),
            ..Default::default()
        }
    }

    /// 整表读回来，逐字段可断言（`assets` 是 BLOB 里的 JSON 数组，解成值再比）。
    fn mails(conn: &rusqlite::Connection) -> Vec<Value> {
        let mut stmt = conn
            .prepare(
                "SELECT id,sender_id,sender_name,body,status,assets,created_at,expires_at \
                 FROM character_mail ORDER BY id",
            )
            .unwrap();
        stmt.query_map([], |r| {
            let assets: Vec<u8> = r.get(5)?;
            Ok(serde_json::json!({
                "id": r.get::<_, i64>(0)?,
                "sender_id": r.get::<_, Option<i64>>(1)?,
                "sender_name": r.get::<_, String>(2)?,
                "body": r.get::<_, String>(3)?,
                "status": r.get::<_, i64>(4)?,
                "assets": serde_json::from_slice::<Value>(&assets).unwrap(),
                "created_at": r.get::<_, i64>(6)?,
                "expires_at": r.get::<_, i64>(7)?,
            }))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    }

    /// 附件 JSON 的字节形状：服务端把它们 `json.Unmarshal` 进 `mail.Asset` /
    /// `inventory.MailItem`，键名大小写与缺省省略都由不得自创。
    /// 那三条串逐字取自真库 `character_mail.assets`（未领取的那一封去掉 `claimed`）。
    #[test]
    fn mail_attachment_json_bytes_match_the_server_rows() {
        let (_dir, cat) = mail_cat("mail-json");
        let encode = |items: Vec<crate::gmmail::MailItem>| {
            let assets: Vec<crate::gmmail::MailAsset> = items
                .into_iter()
                .enumerate()
                .map(|(i, item)| crate::gmmail::MailAsset {
                    id: i as i64 + 3,
                    gold: 0,
                    item: Some(item),
                })
                .collect();
            serde_json::to_string(&assets).unwrap()
        };
        assert_eq!(
            encode(crate::gmmail::build_items(&cat, 0, 10327301, 1).unwrap()),
            r#"[{"id":3,"item":{"stack":{"slot":0,"Template":10327301,"Amount":1,"expire_time":2147483647}}}]"#,
            "堆叠件：Template/Amount 大写、slot 为 0、期限是 MaxInt32 而不是 0"
        );
        assert_eq!(
            encode(crate::gmmail::build_items(&cat, 0, 100200301, 2).unwrap()),
            concat!(
                r#"[{"id":3,"item":{"equipment":{"slot":0,"template":100200301,"durability":0,"period":2147483647},"space":1}},"#,
                r#"{"id":4,"item":{"equipment":{"slot":0,"template":100200301,"durability":0,"period":2147483647},"space":1}}]"#
            ),
            "时装：按件拆成两个附件，容器字节 1 在 equipment 之后"
        );
        let gold = serde_json::to_string(&[crate::gmmail::MailAsset { id: 5, gold: 123_456, item: None }]).unwrap();
        assert_eq!(gold, r#"[{"id":5,"gold":123456}]"#, "金币附件没有 item 键");

        // 现网形态的那一枚：`kind` 是 equipment、只有部位旁表能认出它是时装。
        // 真产物 676,442 行里 `kind` 只有 stackable/equipment 两种值，走的就是这一支，不是上面那支。
        assert_eq!(
            encode(crate::gmmail::build_items(&cat, 0, 100200399, 1).unwrap()),
            r#"[{"id":3,"item":{"equipment":{"slot":0,"template":100200399,"durability":0,"period":2147483647},"space":1}}]"#,
            "按部位格认的时装与按 kind 标的时装必须是同一形状"
        );
        assert_eq!(
            crate::gmmail::build_items(&cat, 0, 100200399, 10).unwrap_err(),
            "装备每批最多9件"
        );
        // 旁表覆盖成普通部位的那一枚仍然拒（slot 是「其它装备」，不是时装格）
        assert_eq!(
            crate::gmmail::build_items(&cat, 0, 101040829, 1).unwrap_err(),
            "装备实例发放未支持（见 #27）：物品 101040829 不是可堆叠材料或时装"
        );
        assert_eq!(
            cat.get(101040829).unwrap().slot,
            "其它装备",
            "旁表没覆盖成功，上面那条拒得就没有意义"
        );

        // 门本身
        assert_eq!(
            crate::gmmail::build_items(&cat, 0, 10327301, 1001).unwrap_err(),
            "邮件数量超过堆叠上限1000"
        );
        // 产物没带上限的那一枚才用兜底值（Go：`if limit == 0 { limit = Rules.MissingStackLimit }`）
        assert_eq!(
            crate::gmmail::build_items(&cat, 200, 10327302, 201).unwrap_err(),
            "邮件数量超过堆叠上限200"
        );
        assert_eq!(
            crate::gmmail::build_items(&cat, 200, 10327301, 1001).unwrap_err(),
            "邮件数量超过堆叠上限1000",
            "自带 1000 的那一枚轮不到兜底值"
        );
        assert_eq!(
            crate::gmmail::build_items(&cat, 0, 100200301, 10).unwrap_err(),
            "装备每批最多9件"
        );
        assert_eq!(
            crate::gmmail::build_items(&cat, 0, 999999999, 1).unwrap_err(),
            "物品 999999999 不在物品库里"
        );
        assert_eq!(
            crate::gmmail::build_items(&cat, 0, 10327301, 0).unwrap_err(),
            "邮件附件缺少真实物品或数量"
        );
    }

    /// 一封邮件 = `character_mail` 的一行：署名 GM、`sender_id` NULL、状态走 DDL 默认 1（未读）、
    /// 期限是那个整数微秒域的 +15 天；且**附件号在消息号之前发**，两者共用一个号段。
    #[test]
    fn mail_send_inserts_one_row_and_keeps_the_shared_id_space() {
        let (_dir, cat) = mail_cat("mail-send");
        let mut conn = test_conn();
        let before = char_state(&conn);
        let input = mail_input("launcher-0026-0001", 123_456, 10327301, 7);
        let (n, g, request) = manage_req(&input);
        let out = manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap();
        assert!(out.applied);

        let rows = mails(&conn);
        assert_eq!(rows.len(), 1, "{rows:?}");
        let row = &rows[0];
        assert_eq!(row["sender_name"], "GM");
        assert_eq!(row["sender_id"], Value::Null, "系统邮件没有发件角色");
        assert_eq!(row["status"], 1, "未读是 DDL 默认，我们不写这一列");
        assert_eq!(row["body"], "维护补偿");
        assert_eq!(
            row["expires_at"].as_i64().unwrap() - row["created_at"].as_i64().unwrap(),
            1_296_000_000_000,
            "15 天必须落在同一个整数微秒域里"
        );
        assert_eq!(
            row["assets"],
            serde_json::json!([
                { "id": 1, "gold": 123_456 },
                { "id": 2, "item": { "stack": { "slot": 0, "Template": 10327301, "Amount": 7, "expire_time": 2147483647 } } }
            ]),
        );
        // 号段：附件占 1、2，消息 AUTOINCREMENT 接着发 ⇒ 消息号一定大于所有附件号。
        assert_eq!(row["id"].as_i64().unwrap(), 3);
        let seq: i64 = conn
            .query_row("SELECT seq FROM sqlite_sequence WHERE name='character_mail'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(seq, 3, "两个附件号 + 一封消息，高水位停在 3");

        // 存档一个字节都没动 ⇒ 这一档不该留改前备份（见 `writes_character_state`）。
        assert_eq!(char_state(&conn), before);
        assert!(!writes_character_state("mail.send"));
        assert!(writes_character_state("sp.recalculate"), "排除式谓词别把别的档一起放掉");

        // 回执与审计：mail_id 就是那一封的 id。
        assert_eq!(out.receipt["mail_id"], 3);
        assert_eq!(out.receipt["capacity_before"]["messages"], 0);
        assert_eq!(out.receipt["capacity_before"]["unclaimed_assets"], 0);
        assert!(out.message.contains("邮件已写入邮箱"), "{}", out.message);
    }

    #[test]
    fn mail_replay_does_not_send_twice() {
        let (_dir, cat) = mail_cat("mail-replay");
        let mut conn = test_conn();
        let input = mail_input("launcher-0026-0002", 1_000, 0, 0);
        let (n, g, request) = manage_req(&input);
        let first = manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap();
        assert!(first.applied);
        let again = manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap();
        assert!(!again.applied);
        assert_eq!(again.message, "该笔操作已存在，未重复执行。");
        assert_eq!(again.receipt["mail_id"], first.receipt["mail_id"], "回放的是原回执");
        assert_eq!(mails(&conn).len(), 1);
        // 同流水号换个金额 ⇒ 拒，且不新增邮件。
        let mut other = input.clone();
        other.gold = 2_000;
        let (n2, g2, request2) = manage_req(&other);
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g2, &request2, &n2).unwrap_err(),
            "流水号已用于不同操作"
        );
        assert_eq!(mails(&conn).len(), 1);
    }

    /// 被拒的邮件一个字节都不留：邮件行、流水号都没有（被拒的流水号可以再用，见 §2.3）。
    #[test]
    fn mail_refusals_rollback_the_whole_flow() {
        let (_dir, cat) = mail_cat("mail-refuse");
        let mut conn = test_conn();
        for (tag, input) in [
            ("上限", mail_input("launcher-0026-0003", 0, 10327301, 5_000)),
            ("批量", mail_input("launcher-0026-0004", 0, 100200301, 99)),
            ("装备", mail_input("launcher-0026-0005", 0, 101040829, 1)),
            ("未知", mail_input("launcher-0026-0006", 0, 777, 1)),
        ] {
            let (n, g, request) = manage_req(&input);
            let err =
                manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap_err();
            assert!(err.contains("上限") || err.contains("每批") || err.contains("#27") || err.contains("物品库"), "{tag}：{err}");
            assert_eq!(mails(&conn).len(), 0, "{tag}：一封都没发");
            assert_eq!(grant_rows(&conn), 0, "{tag}：流水号也没留");
            let seq = conn
                .query_row("SELECT count(*) FROM sqlite_sequence WHERE name='character_mail'", [], |r| r.get::<_, i64>(0))
                .unwrap();
            assert_eq!(seq, 0, "{tag}：附件号也不能留下尾巴");
        }
        // 同一支流水号在拒绝之后仍可再用。
        let input = mail_input("launcher-0026-0003", 500, 0, 0);
        let (n, g, request) = manage_req(&input);
        assert!(manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap().applied);
    }

    /// Go 的两道容量门：活着的消息 ≥255，或未领取的附件 + 本封附件 >255。
    /// 造数据时那一格 `expires_at` 用的是 2000000000000000：这一列是「自 Unix 起的整数微秒」，
    /// 999999999999999 看着大得离谱其实落在 2001 年，会被读侧当过期而算不进容量。
    #[test]
    fn mail_capacity_gate_refuses_a_full_mailbox() {
        let (_dir, cat) = mail_cat("mail-capacity");
        let mut conn = test_conn();
        let fill = |conn: &mut rusqlite::Connection, n: i64, assets: &str| {
            for i in 0..n {
                conn.execute(
                    "INSERT INTO character_mail(recipient_id,sender_name,body,assets,expires_at) \
                     VALUES(10,'GM',?1,CAST(?2 AS BLOB),2000000000000000)",
                    params![format!("m{i}"), assets],
                )
                .unwrap();
            }
        };

        // 消息数：255 封（附件全已领，所以另一道门不碰它）。
        fill(&mut conn, 255, r#"[{"id":1,"claimed":true}]"#);
        let (n, g, request) = manage_req(&mail_input("launcher-0026-0007", 1, 0, 0));
        assert_eq!(
            manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap_err(),
            "收件箱已满"
        );
        assert_eq!(mails(&conn).len(), 255);

        // 未领取附件数：一封带 254 个未领附件，再来 2 个就破 255。
        let mut conn2 = test_conn();
        let many: Vec<String> = (0..254).map(|i| format!(r#"{{"id":{i}}}"#)).collect();
        conn2
            .execute(
                "INSERT INTO character_mail(recipient_id,sender_name,body,assets,expires_at) \
                 VALUES(10,'GM','a',CAST(?1 AS BLOB),2000000000000000)",
                params![format!("[{}]", many.join(","))],
            )
            .unwrap();
        let input = mail_input("launcher-0026-0008", 1, 10327301, 1);
        let (n, g, request) = manage_req(&input);
        assert_eq!(
            manage_inner(&mut conn2, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap_err(),
            "收件箱已满"
        );
        assert_eq!(mails(&conn2).len(), 1, "被拒的那一封没进箱");

        // 同一份箱底：只带 1 个附件（金币）就还在门内 ⇒ 254+1 ≤ 255。
        let single = mail_input("launcher-0026-0009", 1, 0, 0);
        let (n, g, request) = manage_req(&single);
        assert!(manage_inner(&mut conn2, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap().applied);

        // 已过期但没被算进容量的那一封（status=3 免疫过期）仍占位：口径与读侧一致。
        let mut conn3 = test_conn();
        conn3
            .execute(
                "INSERT INTO character_mail(recipient_id,sender_name,body,assets,status,expires_at) \
                 VALUES(10,'GM','old',CAST('[{\"id\":1,\"gold\":5}]' AS BLOB),3,1)",
                [],
            )
            .unwrap();
        let (n, g, request) = manage_req(&mail_input("launcher-0026-0010", 1, 0, 0));
        let out = manage_inner(&mut conn3, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap();
        assert_eq!(out.receipt["capacity_before"]["messages"], 1);
        assert_eq!(out.receipt["capacity_before"]["unclaimed_assets"], 1);

        // 过期且没存进背包的那一封不占位（读侧也看不见它）。
        let mut conn4 = test_conn();
        conn4
            .execute(
                "INSERT INTO character_mail(recipient_id,sender_name,body,assets,status,expires_at) \
                 VALUES(10,'GM','gone',CAST('[{\"id\":1}]' AS BLOB),1,1)",
                [],
            )
            .unwrap();
        let (n, g, request) = manage_req(&mail_input("launcher-0026-0011", 1, 0, 0));
        let out = manage_inner(&mut conn4, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap();
        assert_eq!(out.receipt["capacity_before"]["messages"], 0);
    }

    /// 20261004 升级包把同一个号段放在 `mailbox_id_sequence` 里；带着那张表的存档
    /// 必须一起推进，否则两边各自发出对方用过的号。只往前推，绝不往后拉。
    #[test]
    fn mail_sequence_alignment_never_moves_the_other_counter_back() {
        let (_dir, cat) = mail_cat("mail-align");
        let mut conn = test_conn();
        conn.execute_batch(
            "CREATE TABLE mailbox_id_sequence(name TEXT PRIMARY KEY, value BIGINT NOT NULL);
             INSERT INTO mailbox_id_sequence(name,value) VALUES('mailbox_id_seq',100);",
        )
        .unwrap();
        let (n, g, request) = manage_req(&mail_input("launcher-0026-0012", 1, 10327301, 1));
        let out = manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap();
        // 对方已经 ahead：我们的附件号 1、2 与消息号 3 都不许把它拉回来。
        let value: i64 = conn
            .query_row("SELECT value FROM mailbox_id_sequence WHERE name='mailbox_id_seq'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(value, 100, "已经比它高时一步都不退");
        assert_eq!(out.receipt["mail_id"], 3);
        assert_eq!(mails(&conn)[0]["assets"][0]["id"], 1);

        // 反过来：那张表落后就必须追上（下一次发号才撞不上）。
        conn.execute("UPDATE mailbox_id_sequence SET value=1 WHERE name='mailbox_id_seq'", [])
            .unwrap();
        let (n, g, request) = manage_req(&mail_input("launcher-0026-0013", 1, 0, 0));
        let out = manage_inner(&mut conn, &cat, 0, &ManageArtifacts::default(), &g, &request, &n).unwrap();
        let value: i64 = conn
            .query_row("SELECT value FROM mailbox_id_sequence WHERE name='mailbox_id_seq'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(out.receipt["mail_id"], 5, "附件 4，消息 5");
        assert_eq!(value, 5, "对齐到我们自己号段的下一格");
    }

    /// 两条纯参数门开在事务之前：被拒的连流水号都不该留。
    /// 以及白名单真的收下了 `mail.send`（数值档那条「必须大于 0」的门不能套到它头上 ——
    /// 只发金币不带物品的话，`amount` 就是 0）。
    #[test]
    fn mail_gates_run_before_the_transaction() {
        let base = mail_input("launcher-0026-0014", 1_000, 0, 0);
        assert!(normalize_manage(&base).is_ok(), "只发金币：amount 为 0 也合法");

        let mut long_body = base.clone();
        long_body.body = "补".repeat(200); // 3 字节 × 200 = 600 > 512
        assert_eq!(normalize_manage(&long_body).unwrap_err(), "邮件正文过长");
        let mut edge = base.clone();
        edge.body = "a".repeat(crate::gmmail::MAX_BODY_BYTES);
        assert!(normalize_manage(&edge).is_ok(), "512 字节正好在 CHECK 之内");

        let mut empty = base.clone();
        empty.gold = 0;
        assert_eq!(normalize_manage(&empty).unwrap_err(), "邮件至少要带金币或物品");

        let unknown = ManageInput { action: "mail.delete".into(), gold: 1, ..base.clone() };
        assert_eq!(normalize_manage(&unknown).unwrap_err(), "不支持的管理操作");
    }

    /// 只在设了 `DFO_REHEARSAL_ROOT` 时执行：拿真产物点名一枚可堆叠与几枚时装，
    /// 确认离线判据真的把它们分对了，并且**把真产物 `kind` 只有两种值这件事钉在账上**
    /// （时装靠部位旁表的 `slot` 认，不靠 `kind`）。全程不碰库。
    #[test]
    #[ignore]
    fn live_mail_shapes_on_real_artifacts() {
        let Ok(root_dir) = std::env::var("DFO_REHEARSAL_ROOT") else {
            eprintln!("跳过：未设 DFO_REHEARSAL_ROOT");
            return;
        };
        let dir = gmbuild::gmindex::find_configs(&PathBuf::from(root_dir)).expect("真产物目录");
        let cat = gmbuild::gmindex::load(&dir).expect("真物品库");
        // 真背包里就在用的那枚可堆叠（与彩排发放同一件）。
        let stackable = cat.get(10327301).expect("真产物里没有 10327301");
        assert_eq!(stackable.kind, gmbuild::gmindex::Kind::Stackable);
        assert_eq!(crate::gmmail::build_items(&cat, 1000, 10327301, 1).unwrap().len(), 1);

        let mut kind_avatar = 0usize;
        let mut slot_avatar = 0usize;
        let mut avatars: Vec<u32> = Vec::new();
        for it in cat.iter() {
            if it.kind == gmbuild::gmindex::Kind::Avatar {
                kind_avatar += 1;
            }
            if it.avatar() {
                slot_avatar += 1;
                if avatars.len() < 3 {
                    avatars.push(it.id);
                }
            }
        }
        println!(
            "真产物：kind 标 avatar 的 {kind_avatar} 行 / 判据认出的时装 {slot_avatar} 行 / 总行数 {}",
            cat.iter().count()
        );
        assert!(slot_avatar > 0, "真产物里一件时装都没有？");
        for id in avatars {
            let one = crate::gmmail::build_items(&cat, 1000, id, 1).unwrap();
            assert_eq!(one.len(), 1);
            assert_eq!(one[0].space, 1, "时装必须落容器 1");
        }
    }
}
