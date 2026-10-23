//! 服务端那两件带代际戳的产物在 Rust 侧重导 —— 不叫 Go、不链 PVF 内核之外的东西。
//!
//! - `equipment.current37.json`：逐字段照 Go
//!   `internal/toolcmd/equipmentfull/main.go` 的 `Run()`，对归档里全部 `equipment/**.equ`
//!   直接枚举。**刻意不是** `ImportEquipmentSelection` 那 3,174 行的「掉落白名单」——
//!   启动器发放侧按 ID 查这张表，查不到就硬报「装备 X 不在装备目录里」
//!   （`src-tauri/src/gmgrant.rs:413`），现网这份是 67,874 行的全量枚举，
//!   导出成白名单等于把可发的装备面缩掉 21 倍。
//!   Go 侧那份的 `source.checksum` 是 2026-09-11 那次解包的内层归档，归档本身早被换掉，
//!   所以**逐字节复现旧文件不可能**，能复现的是「同一套枚举规则 + 当代归档」。
//! - `inventory.current37.json`：整件 358 字节里只有 `source` 那 64 个十六进制字符是代际标记，
//!   其余（`model` / `slots` / `missing_stack_limit` / `equipment_slots` / `quick_slots`）全是
//!   人工策展的策略。Go 运行期也是拿实时 checksum 覆盖这个历史标记
//!   （`internal/inventory/bag.go:70-72`），所以这里**只换戳、不重算策略**，缺文件就整条拒。
//!
//! 两件必须一起动：启动器发放侧的门是「装备目录的 checksum == 背包规则的 source」
//! （`src-tauri/src/gmgrant.rs:388`，与 Go `LoadEquipmentCatalog` 同一条闸），
//! 只换一件会让发装备从「黄字提示」变成「硬拒」。
//!
//! `experience.compat90.json` 不在本模块范围内：它顶层没有 `source`，只有一个
//! `reference_sha256`（指向别的东西），其余全是等级倍率策略 —— 没有任何「代际」可换。

use crate::archive::Archive;
use crate::itemindex::{self, Token};
use crate::path::join_archive_path;
use crate::sha256_hex;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::Path;

/// Go `equipmentfull.wanted`，十条，一字不差。
/// 比 `ImportEquipmentSelection` 的九条多了 `[creation rate]` 与 `[item category]`，
/// 少了从不出现的 `[usable grow type]`；现网那份产物里实测恰好就是这十条键。
const WANTED: [&str; 10] = [
    "[name]",
    "[grade]",
    "[rarity]",
    "[creation rate]",
    "[minimum level]",
    "[equipment type]",
    "[durability]",
    "[attach type]",
    "[usable job]",
    "[item category]",
];

/// Go `equipmentfull` 的 `prefix` 默认值。
const EQUIPMENT_PREFIX: &str = "equipment/";

/// Go `normPath`：小写、`\`→`/`、去首尾空白。
fn norm_path(p: &str) -> String {
    p.trim().replace('\\', "/").to_lowercase()
}

// ---------------------------------------------------------------------------
// equipment.current37.json
// ---------------------------------------------------------------------------

/// Go `pvf.Token` 的 JSON 形状：`text` / `reference` / `number` 都是 omitempty，
/// `number` 只在 Type2 出现且为 `Float32frombits(value)`（为 0 时 Go 也会省掉）。
#[derive(Serialize)]
struct TokenJson {
    #[serde(rename = "type")]
    ttype: u8,
    value: i32,
    #[serde(skip_serializing_if = "String::is_empty")]
    text: String,
    #[serde(rename = "reference", skip_serializing_if = "String::is_empty")]
    _reference: String,
    #[serde(skip_serializing_if = "is_zero")]
    number: f32,
}

fn is_zero(n: &f32) -> bool {
    *n == 0.0
}

fn token_json(t: &Token) -> TokenJson {
    TokenJson {
        ttype: t.ttype,
        value: t.value,
        text: t.text.clone(),
        _reference: t.reference.clone(),
        number: if t.ttype == 2 { f32::from_bits(t.value as u32) } else { 0.0 },
    }
}

/// Go `equipmentfull.row`，但键序按现网产物落成 ID / Path / SHA256 / Fields
/// （Go 那边是 `map[string]any`，序列化出来恰好也是这个字母序）。
#[derive(Serialize)]
struct RowJson {
    #[serde(rename = "ID")]
    id: u32,
    #[serde(rename = "Path")]
    path: String,
    #[serde(rename = "SHA256")]
    sha: String,
    #[serde(rename = "Fields")]
    fields: BTreeMap<String, Vec<TokenJson>>,
}

/// Go `pvf.ArchiveSnapshot` 的九键。只有 `checksum` 是判据，其余八键是诊断
/// （Go 侧 `NewEquipmentCatalog` 只比 `Source.Checksum`）；`cached_chunks` /
/// `cached_texts` 在 Go 里是导出瞬间的缓存遥测，本可变量，Rust 侧固定写 0。
#[derive(Serialize)]
struct SourceJson {
    format: String,
    path: String,
    size: i64,
    checksum: String,
    loaded_at: String,
    file_count: usize,
    group_count: usize,
    cached_chunks: u32,
    cached_texts: u32,
}

/// 顶层只有 `source` 与 `rows` 两个键，且 `source` 在前 —— 与 Go 产物的头几字节一致，
/// 启动器 `gmrebuild.rs::load_artifact` 就是只读头 8 KB 取快照。
#[derive(Serialize)]
struct ExportFile {
    source: SourceJson,
    rows: Vec<RowJson>,
}

/// 落定路径 + 原文指纹 + 单元表；`data_type` 必须是 1，与 `itemindex::read_script` 同门。
fn read_script_with_sha(a: &Archive, name: &str) -> Result<(String, String, Vec<Token>), String> {
    let name = name.replace('\\', "/").to_lowercase();
    let idx = a
        .find_file_index(&name)
        .ok_or_else(|| format!("pvf file not found: {name}"))?;
    if a.file_at(idx).data_type != 1 {
        return Err(format!("entry {name} is not a script"));
    }
    let raw = a.read_raw_idx(idx)?;
    let sha = sha256_hex(&raw);
    let cells = itemindex::tokens_from_raw(a, &raw)?;
    Ok((name, sha, cells))
}

/// Go `equipmentIDByPath`：`list/equipment.lst` 是客户端自己的装备索引，ID 的唯一权威来源。
/// 之所以不能从文件名解析：早期装备的文件名不是纯数字（`vest_owool.equ`、`robe_cfiber.equ`…），
/// Go 作者侧实测那样会整条丢掉 43,349 条。
fn equipment_id_by_path(a: &Archive) -> Result<HashMap<String, u32>, String> {
    let (_, cells) = itemindex::resolve_script(a, "list/equipment.lst")?;
    let rows = itemindex::parse_index(&cells)?;
    let mut out = HashMap::with_capacity(rows.len());
    for (id, path) in rows {
        out.insert(norm_path(&path), id);
    }
    Ok(out)
}

/// 直读 PVF 重导装备目录，产出可被 Go `LoadEquipmentCatalog(path, checksum)` 与启动器
/// `EquipCatalog::load` 同时接受的那件产物。返回 (行数, 各类计数)。
///
/// 计数与 Go 的 `counts` 同一套口径，只是不落进产物（现网那份没有 `counts` 键），
/// 而是打到 stderr 供进度与事后核对。
pub fn build_equipment_export(
    a: &Archive,
    pvf: &Path,
    format_label: &str,
    out: &mut dyn Write,
) -> Result<(usize, [(&'static str, usize); 6]), String> {
    let by_path = equipment_id_by_path(a)?;
    let files = a.all_files();
    let total = files.len();

    let mut candidate = 0usize;
    let mut unreadable = 0usize;
    let mut bad_name = 0usize;
    let mut id_from_index = 0usize;
    let mut id_from_filename = 0usize;
    let mut dup_id = 0usize;

    let mut selected: Vec<RowJson> = Vec::with_capacity(64 * 1024);
    // Go 的 `byID`：同 ID 多条 `.equ` 时记下该留哪一条（索引认可者优先，其次路径字典序小者）。
    let mut winner: HashMap<u32, (String, bool)> = HashMap::with_capacity(64 * 1024);

    for (n, f) in files.iter().enumerate() {
        let p = f.archive_path.as_str();
        if !p.ends_with(".equ") {
            continue;
        }
        // 时装判定用「路径含 avatar」而不是 "/avatar/"：还有 `at_avatar` 这种变体，
        // 只匹配前者会漏 10 万条 —— 现网这份产物里 avatar 路径为 0，就是这个 skip 的效果。
        if p.to_lowercase().contains("avatar") {
            continue;
        }
        candidate += 1;

        // Go 的 `tries`：Files() 的完整路径可能不带 `equipment/` 前缀，两种写法都试。
        let mut prefixed = String::new();
        if !p.starts_with(EQUIPMENT_PREFIX) {
            prefixed = join_archive_path("equipment", p);
        }
        let tries: [&str; 2] = [p, prefixed.as_str()];
        let n_tries = if prefixed.is_empty() { 1 } else { 2 };

        let mut resolved: Option<(String, String, Vec<Token>)> = None;
        for t in tries.iter().take(n_tries) {
            let name = itemindex::resolve_script_path(a, t);
            if let Ok(got) = read_script_with_sha(a, &name) {
                resolved = Some(got);
                break;
            }
        }
        let Some((path, sha, cells)) = resolved else {
            unreadable += 1;
            continue;
        };

        let mut fields: BTreeMap<String, Vec<TokenJson>> = BTreeMap::new();
        let mut tag = String::new();
        for t in &cells {
            if t.ttype == 3 {
                tag = t.text.clone();
                continue;
            }
            if WANTED.contains(&tag.as_str()) {
                fields.entry(tag.clone()).or_default().push(token_json(t));
            }
        }

        // ID 优先取自 `list/equipment.lst`（权威）；文件名只在索引查不到时兜底。
        let mut from_index = true;
        let mut id = None;
        for t in tries.iter().take(n_tries) {
            if let Some(v) = by_path.get(&norm_path(t)) {
                id = Some(*v);
                break;
            }
        }
        if id.is_none() {
            from_index = false;
            let base = p.rsplit('/').next().unwrap_or(p).trim_end_matches(".equ");
            id = base.parse::<u32>().ok().filter(|v| *v != 0);
            if id.is_some() {
                id_from_filename += 1;
            } else {
                bad_name += 1;
                continue;
            }
        } else {
            id_from_index += 1;
        }
        let id = id.unwrap();

        if let Some((prev_path, prev_from_index)) = winner.get(&id) {
            dup_id += 1;
            let keep_new = (from_index && !*prev_from_index)
                || (from_index == *prev_from_index && path.as_str() < prev_path.as_str());
            if !keep_new {
                continue;
            }
        }
        winner.insert(id, (path.clone(), from_index));
        selected.push(RowJson { id, path, sha, fields });

        if n % 20_000 == 0 {
            // 与 Go 的 MaxBytes 无关：整份归档常驻，这里释放的是解压块，避免边扫边攒。
            a.release_read_caches();
            eprintln!("  equipment.current37 进度 {}/{total} 候选 {candidate}", n + 1);
        }
    }
    a.release_read_caches();

    // Go 的二次去重：上面的 byID 只能「跳过后来的」，早收进去的落选行仍在 selected 里；
    // `seen` 再挡住「同 ID 同路径出现两次」这种归档内重复条目。
    let rows_hint = selected.len();
    let mut seen: HashSet<u32> = HashSet::with_capacity(rows_hint);
    let rows: Vec<RowJson> = selected
        .into_iter()
        .filter(|r| winner.get(&r.id).is_some_and(|(p, _)| *p == r.path) && seen.insert(r.id))
        .collect();
    let count = rows.len();

    let source = SourceJson {
        format: format_label.to_string(),
        path: pvf.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        size: std::fs::metadata(pvf).map(|m| m.len() as i64).unwrap_or(0),
        checksum: a.checksum.clone(),
        loaded_at: crate::slotmap::now_rfc3339(),
        file_count: files.len(),
        group_count: a.group_count(),
        cached_chunks: 0,
        cached_texts: 0,
    };
    // 紧凑写出（`json.Marshal` 同形）：现网这份 67,874 行的产物就是单行 42 MB，
    // 缩进版会翻到近 100 MB，而两边的 reader 都不依赖缩进。
    let mut w = std::io::BufWriter::with_capacity(1 << 20, out);
    serde_json::to_writer(&mut w, &ExportFile { source, rows })
        .map_err(|e| format!("写装备目录失败：{e}"))?;
    w.write_all(b"\n").map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())?;

    Ok((
        count,
        [
            ("candidate", candidate),
            ("unreadable", unreadable),
            ("bad_name", bad_name),
            ("id_from_index", id_from_index),
            ("id_from_filename", id_from_filename),
            ("dup_id", dup_id),
        ],
    ))
}

// ---------------------------------------------------------------------------
// inventory.current37.json —— 只换代际戳
// ---------------------------------------------------------------------------

/// 把背包规则里的 `source` 换成当次归档的校验值，**其余字节原样保留**。
///
/// 刻意不走「反序列化再序列化」：那会把人工策展的策略字段（部位区间、`missing_stack_limit`…）
/// 重新排键、重新格式化，等于悄悄改配置。这里先按 Go 的口径校验语义，再只替换那 64 个字符，
/// 最后把两份都重新解析、逐键比对，证明除 `source` 之外没有任何变化。
pub fn refresh_bag_rules(raw: &str, checksum: &str) -> Result<String, String> {
    if checksum.len() != 64 || !checksum.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("代际校验值不是 64 位十六进制：{checksum}"));
    }
    let before: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("解析背包规则失败：{e}"))?;
    let obj = before.as_object().ok_or("背包规则顶层不是对象")?;
    if obj.get("model").and_then(|v| v.as_str()) != Some("reference90-bag-v1") {
        return Err("背包规则的 model 不是 reference90-bag-v1，不敢动".to_string());
    }
    match obj.get("missing_stack_limit").and_then(|v| v.as_u64()) {
        Some(n) if n != 0 => {}
        other => return Err(format!("背包规则的 missing_stack_limit 不合法（{other:?}）")),
    }
    let old_source = obj
        .get("source")
        .and_then(|v| v.as_str())
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("背包规则里没有可替换的 64 位 source 标记")?;

    // 只替换那一段十六进制：先定位键，再定位其后第一个字符串字面量。
    let key_at = raw.find("\"source\"").ok_or("背包规则里找不到 \"source\"")?;
    let colon = raw[key_at..].find(':').map(|i| key_at + i).ok_or("source 键没有冒号")?;
    let open = raw[colon + 1..]
        .find('"')
        .map(|i| colon + 1 + i)
        .ok_or("source 的值不是字符串")?;
    let close = raw[open + 1..]
        .find('"')
        .map(|i| open + 1 + i)
        .ok_or("source 的字符串没有收尾")?;
    if &raw[open + 1..close] != old_source {
        return Err("背包规则里的 source 字面量与解析结果不一致，不敢动".to_string());
    }
    let mut after_text = String::with_capacity(raw.len());
    after_text.push_str(&raw[..open + 1]);
    after_text.push_str(checksum);
    after_text.push_str(&raw[close..]);

    let after: serde_json::Value =
        serde_json::from_str(&after_text).map_err(|e| format!("换戳后的背包规则解析不了：{e}"))?;
    let after_obj = after.as_object().expect("刚判过顶层是对象");
    if after_obj.get("source").and_then(|v| v.as_str()) != Some(checksum) {
        return Err("换戳后 source 没落到该落的位置".to_string());
    }
    if after_obj.len() != obj.len() {
        return Err("换戳后键数变了".to_string());
    }
    for (k, v) in obj {
        if k == "source" {
            continue;
        }
        if after_obj.get(k) != Some(v) {
            return Err(format!("换戳意外改动了策略字段 {k}，整条拒"));
        }
    }
    Ok(after_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(ttype: u8, value: i32, text: &str) -> Token {
        Token { ttype, value, text: text.to_string(), reference: String::new() }
    }

    /// 键集合就是 Go 的十条；`[import script]` 不在其中 —— 现网产物里也确实是 0 条，
    /// 启动器的继承链（`gmgrant.rs:417`）在这张表上是空转，换代际不会让它突然生效。
    #[test]
    fn wanted_is_exactly_the_go_ten() {
        assert_eq!(WANTED.len(), 10);
        for k in ["[creation rate]", "[item category]", "[name]", "[durability]"] {
            assert!(WANTED.contains(&k), "{k} 必须在");
        }
        assert!(!WANTED.contains(&"[usable grow type]"), "equipmentfull 没有这条");
        assert!(!WANTED.contains(&"[import script]"), "现网产物没有这条，别凭空加键");
    }

    /// Go `normPath`：小写 + 反斜杠转正斜杠 + 去首尾空白。
    #[test]
    fn norm_path_matches_go() {
        assert_eq!(norm_path(r" Equipment\Character\A.EQU "), "equipment/character/a.equ");
    }

    /// Type8 落 `reference`、Type2 落 `number`、Type0 三个可选键全省掉，键序与 Go 一致。
    #[test]
    fn token_json_matches_go_omitempty_shape() {
        let numeric = serde_json::to_string(&token_json(&token(0, 5, ""))).unwrap();
        assert_eq!(numeric, r#"{"type":0,"value":5}"#);
        let text = serde_json::to_string(&token_json(&token(6, 157680695, "[free]"))).unwrap();
        assert_eq!(text, r#"{"type":6,"value":157680695,"text":"[free]"}"#);
        let mut name = token(8, 12, "");
        name.reference = "<8::abc>".to_string();
        let ref_json = serde_json::to_string(&token_json(&name)).unwrap();
        assert_eq!(ref_json, r#"{"type":8,"value":12,"reference":"<8::abc>"}"#);
        let float = serde_json::to_string(&token_json(&token(2, 1048576000, ""))).unwrap();
        assert!(float.contains("\"number\":0.25"), "{float}");
        let zero_float = serde_json::to_string(&token_json(&token(2, 0, ""))).unwrap();
        assert_eq!(zero_float, r#"{"type":2,"value":0}"#, "Go 的 omitempty 对 0.0 同样省略 number");
    }

    /// 行键序 ID/Path/SHA256/Fields，Fields 的键按字典序（现网产物就是这个序）。
    #[test]
    fn row_json_key_order_and_field_sorting() {
        let mut fields = BTreeMap::new();
        fields.insert("[rarity]".to_string(), vec![token_json(&token(0, 3, ""))]);
        fields.insert("[attach type]".to_string(), vec![token_json(&token(6, 1, "[free]"))]);
        let row = RowJson { id: 10000, path: "equipment/x.equ".into(), sha: "a".repeat(64), fields };
        let text = serde_json::to_string(&row).unwrap();
        assert!(text.starts_with(r#"{"ID":10000,"Path":"equipment/x.equ","SHA256":"#), "{text}");
        assert!(text.contains(r#""Fields":{"[attach type]":"#), "{text}");
        assert!(
            text.find("[attach type]").unwrap() < text.find("[rarity]").unwrap(),
            "Fields 必须按字典序，否则与现网产物形状不一致"
        );
    }

    fn bag(source: &str) -> String {
        format!(
            "{{\n  \"model\": \"reference90-bag-v1\",\n  \"source\": \"{source}\",\n  \
             \"slots\": {{\"[throw]\": [65, 120]}},\n  \"missing_stack_limit\": 1000,\n  \
             \"equipment_slots\": [9],\n  \"quick_slots\": [0]\n}}\n"
        )
    }

    /// 换戳只动那一段：策略字段逐键不变，且差异连成一段、段长恰好等于一个校验值。
    #[test]
    fn bag_refresh_touches_only_the_stamp() {
        let old = "7ef2db59331f7e5b18b2f250b8b907526bf2c94b17a7312036cf599644d88e80";
        let new = "32dd76978ff079529fcb0356a33ef986f9aed66ec855a908160e3c048e0dae61";
        let raw = bag(old);
        let after = refresh_bag_rules(&raw, new).unwrap();
        assert_eq!(after.len(), raw.len(), "换戳不该改变长度");
        let diff: Vec<usize> = raw
            .chars()
            .zip(after.chars())
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect();
        // 两个校验值里有几个字符恰好相同，所以「不同字符数」不是 64；能证明的是
        // 差异连成一段、且段长恰好等于一个校验值。
        assert_eq!(
            diff.first().map(|s| diff.last().unwrap() - s + 1),
            Some(64),
            "差异必须落在连续 64 个字符的窗口里"
        );
        assert!(after.contains(new) && !after.contains(old));
        assert_eq!(after.replace(new, "<S>"), raw.replace(old, "<S>"), "除戳以外必须逐字节相同");
    }

    /// 缺 model / 缺戳 / 新戳不是 64 位十六进制 —— 都整条拒，不猜。
    #[test]
    fn bag_refresh_refuses_anything_it_cannot_prove_safe() {
        let old = "7ef2db59331f7e5b18b2f250b8b907526bf2c94b17a7312036cf599644d88e80";
        let new = "32dd76978ff079529fcb0356a33ef986f9aed66ec855a908160e3c048e0dae61";
        assert!(refresh_bag_rules(&bag(old).replace("reference90-bag-v1", "other"), new).is_err());
        assert!(refresh_bag_rules(&bag(old), &new[..63]).is_err());
        assert!(refresh_bag_rules(&bag(old), &"z".repeat(64)).is_err());
        assert!(refresh_bag_rules(&bag(old), "").is_err());
        let no_stamp = bag(old).replace(&format!("\"source\": \"{old}\",\n"), "");
        assert!(refresh_bag_rules(&no_stamp, new).is_err());
    }

    /// `missing_stack_limit` 为 0 是 Go 的 `invalid bag policy`，我们同样不敢动。
    #[test]
    fn bag_refresh_refuses_zero_stack_limit() {
        let raw = bag("7ef2db59331f7e5b18b2f250b8b907526bf2c94b17a7312036cf599644d88e80")
            .replace("\"missing_stack_limit\": 1000", "\"missing_stack_limit\": 0");
        assert!(refresh_bag_rules(&raw, "32dd76978ff079529fcb0356a33ef986f9aed66ec855a908160e3c048e0dae61").is_err());
    }
}
