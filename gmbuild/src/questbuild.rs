//! 任务产物构建器：离线直读 PVF，落一个 `gm-quests.json`。
//!
//! 覆盖的 Go 口径（逐条对齐，来源标注在函数上）：
//! - `catalog.ImportQuests` + `catalog.LoadQuests` 的重投影（前置段、互斥段、答案段、主目标 kind）
//! - `quest.BuildIndex` 里 GM 侧真正读的 `GrowUsable / RewardUsable / GrowTypes`
//! - `quest.InitialProgress` 的 8 个进度模型（含 `[look cinematic]`、`[monster kill checkpoint]`、
//!   `[legion content clear]` 三个「客户端把关、进度 0」形态）
//! - `cmd/gmtool/bridge_manage.go` 的 `gmQuestCells` / `gmQuestTargetAllowed`
//! - `cmd/gmtool/bridge_quests.go` 的 `buildQuestText` + `buildQuestNPCNames` + `gmQuestText`
//!   ——中文与 NPC 名在构建期折完，运行期不再需要 `quest.text.client.json` 与目录哈希绑定。

use crate::archive::Archive;
use crate::growth::SourceSnapshot;
use crate::itemindex::{parse_index, resolve_script_path, tokens_from_raw, Token};
use crate::quest::{QuestAnswer, QuestEntry, QuestFile, TargetChar};
use crate::slotmap::now_rfc3339;
use sha2::Digest;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// 一条 `.qst`（或 `.lst` 索引）的导入结果，对应 Go `catalog.ScriptRecord`。
struct Imported {
    path: String,
    sha: String,
    cells: Vec<Token>,
    /// Go `QuestDefinition.Pending`：源里这一段读不进去的原因，非空即禁止自动补完。
    pending: Vec<String>,
}

/// Go `catalog.ReadScript`：原样路径取原文与单元表，并留下原文指纹。
/// 刻意不额外断言 `data_type`——Go 的 `ReadRaw` 不判，多一道断言就会把源里本来
/// 能读的段落判成待核实。
fn import_script_at(a: &Archive, path: &str) -> Result<Imported, String> {
    let raw = a.read_raw(path)?;
    let cells = tokens_from_raw(a, &raw)?;
    Ok(Imported { path: path.to_string(), sha: sha256_hex(&raw), cells, pending: Vec::new() })
}

/// Go `catalog.ResolveScript`：先原样，未命中且不是 ANI/MOB/OBJ 时前置 `(r)` 再取。
fn import_script(a: &Archive, name: &str) -> Result<Imported, String> {
    import_script_at(a, &resolve_script_path(a, name))
}

/// Go `catalog.sectionCells` / `gmQuestCells`：段落边界由任意 Type3 标签重置，
/// 匹配用的是原文（不 lowercase），与任务解析各处一致。
fn cells_in(cells: &[Token], name: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut active = false;
    for c in cells {
        if c.ttype == 3 {
            active = c.text == name;
            continue;
        }
        if active {
            out.push(c.clone());
        }
    }
    out
}

fn has_tag(cells: &[Token], name: &str) -> bool {
    cells.iter().any(|c| c.ttype == 3 && c.text == name)
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = sha2::Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Go `parsePrerequisiteGroups`：每个重复 `[pre required quest]` 段是一组必须全部
/// 完成的前置；非数值或非正数直接判整条待核实。
fn prerequisite_groups(cells: &[Token]) -> Result<(Vec<Vec<u32>>, Vec<u32>), String> {
    let mut groups: Vec<Vec<u32>> = Vec::new();
    let mut flat: Vec<u32> = Vec::new();
    let mut group: Vec<u32> = Vec::new();
    let mut active = false;
    for c in cells {
        if c.ttype == 3 {
            if active && !group.is_empty() {
                groups.push(std::mem::take(&mut group));
            }
            active = c.text == "[pre required quest]";
            continue;
        }
        if !active {
            continue;
        }
        if c.ttype != 0 || c.value <= 0 {
            return Err("unsupported prerequisite condition".into());
        }
        let id = c.value as u32;
        group.push(id);
        flat.push(id);
    }
    if active && !group.is_empty() {
        groups.push(group);
    }
    Ok((groups, flat))
}

/// Go `parseCollisionQuests`：空段就是无约束，非数值单元直接跳过。
fn collision_quests(cells: &[Token]) -> Vec<u32> {
    let mut ids = Vec::new();
    let mut on = false;
    for c in cells {
        if c.ttype == 3 {
            on = c.text == "[collision quest]";
            continue;
        }
        if on && c.ttype == 0 && c.value > 0 {
            ids.push(c.value as u32);
        }
    }
    ids
}

/// Go `parsePrerequisiteAnswers` + `answerFromCells`：段形必须是恰好两个数值单元、
/// 问题 id 为正；不符整段丢弃，绝不半个半个地读。
fn prerequisite_answers(cells: &[Token]) -> Vec<QuestAnswer> {
    let mut out = Vec::new();
    let mut on = false;
    let mut pending: Vec<i32> = Vec::new();
    for c in cells {
        if c.ttype == 3 {
            if on {
                if let Some(a) = answer_from(&pending) {
                    out.push(a);
                }
                pending.clear();
            }
            on = c.text == "[pre required quest answer]";
            continue;
        }
        if on && c.ttype == 0 && c.value >= 0 {
            pending.push(c.value);
        }
    }
    if on {
        if let Some(a) = answer_from(&pending) {
            out.push(a);
        }
    }
    out
}

fn answer_from(v: &[i32]) -> Option<QuestAnswer> {
    if v.len() != 2 || v[0] <= 0 {
        return None;
    }
    Some(QuestAnswer { question: v[0] as u32, index: v[1] as u32 })
}

/// Go `quest.slotExpansion`：`[reward type]` 恰好一个 `[slot expansion]`，
/// `[reward int data]` 恰好一个 0..=3 的槽位序号。
fn slot_expansion(cells: &[Token]) -> Option<u8> {
    let t = cells_in(cells, "[reward type]");
    if t.len() != 1 || t[0].ttype != 6 || t[0].text != "[slot expansion]" {
        return None;
    }
    let ids = cells_in(cells, "[reward int data]");
    if ids.len() != 1 || ids[0].ttype != 0 || !(0..=3).contains(&ids[0].value) {
        return None;
    }
    Some(ids[0].value as u8)
}

/// Go `quest.rewardUsable`。
fn reward_usable(cells: &[Token]) -> bool {
    let t = cells_in(cells, "[reward type]");
    if !t.is_empty() {
        if t.len() != 1 || t[0].ttype != 6 {
            return false;
        }
        match t[0].text.as_str() {
            "[item]" | "[none]" => {}
            "[slot expansion]" => {
                if slot_expansion(cells).is_none() {
                    return false;
                }
            }
            _ => return false,
        }
    }
    cells_in(cells, "[reward select int data]").is_empty() && cells_in(cells, "[reward selection int data]").is_empty()
}

/// Go `quest.ReachRange`：六个非负数值单元，城镇号非零、宽高为正。
fn reach_range_ok(kind: &str, objective: &[Token]) -> bool {
    if kind != "[reach the range]" || objective.len() != 6 {
        return false;
    }
    if objective.iter().any(|t| t.ttype != 0 || t.value < 0) {
        return false;
    }
    objective[0].value != 0 && objective[4].value > 0 && objective[5].value > 0
}

/// Go `quest.AlflyraReachTarget`：只认 3252 这一条 `[303,1000,1000]`，
/// 且 `[complete npc index]` 必须与目标 NPC 同号。
fn alflyra_reach_ok(id: u32, kind: &str, cells: &[Token], objective: &[Token]) -> bool {
    if id != 3252 || kind != "[reach the range]" || objective.len() != 3 {
        return false;
    }
    let want = [(303i32, 0usize), (1000, 1), (1000, 2)];
    for (v, i) in want {
        if objective[i].ttype != 0 || objective[i].value != v {
            return false;
        }
    }
    let complete = cells_in(cells, "[complete npc index]");
    complete.len() == 1 && complete[0].ttype == 0 && complete[0].value == objective[0].value
}

/// Go `quest.SeekMeet`：奇数长度、全为正数值单元，末尾是 NPC，其余成对是（物品, 数量）。
fn seek_meet_ok(kind: &str, objective: &[Token]) -> bool {
    if kind != "[seek n meet npc]" || objective.len() < 3 || objective.len() % 2 == 0 {
        return false;
    }
    objective.iter().all(|t| t.ttype == 0 && t.value > 0)
}

/// Go `quest.MonsterKillCheckpointShape`：三元组、键为正且递增、末尾 (-1,-1)。
fn monster_kill_checkpoint_shape(kind: &str, objective: &[Token]) -> bool {
    if kind != "[monster kill checkpoint]" {
        return false;
    }
    if objective.len() < 3 || objective.len() % 3 != 0 {
        return false;
    }
    for (i, t) in objective.iter().enumerate() {
        if t.ttype != 0 {
            return false;
        }
        if i % 3 == 0 && t.value <= 0 {
            return false;
        }
    }
    if objective[objective.len() - 2].value != -1 || objective[objective.len() - 1].value != -1 {
        return false;
    }
    for i in (3..objective.len()).step_by(3) {
        if objective[i].value <= objective[i - 3].value {
            return false;
        }
    }
    true
}

/// Go `quest.LegionContentClearShape`：只校形态（长度与首项非负），不解释尾数。
fn legion_content_clear_shape(kind: &str, objective: &[Token]) -> bool {
    // 不带难度允许 2 或 3 项（内容号 [+ 次数]）；带难度必须恰好 4 项。
    let len_ok = match kind {
        "[legion content clear]" => matches!(objective.len(), 2 | 3),
        "[legion content clear with difficulty]" => objective.len() == 4,
        _ => return false,
    };
    if !len_ok {
        return false;
    }
    for (i, t) in objective.iter().enumerate() {
        if t.ttype != 0 || (i == 0 && t.value < 0) {
            return false;
        }
    }
    true
}

/// Go `quest.InitialProgress`：返回模型名，`None` = 本构建不认识该目标形态。
/// `pending` 非空时除「整段不认识」外一律按不认识处理（与 Go 各分支的前置守卫同口径）。
fn initial_model(id: u32, kind: &str, cells: &[Token], objective: &[Token], pending: bool) -> Option<&'static str> {
    if pending {
        return None;
    }
    let single_positive = objective.len() == 1 && objective[0].ttype == 0 && objective[0].value > 0;
    if kind == "[meet npc]" && single_positive {
        return Some("single-meet-npc-remaining-v1");
    }
    if kind == "[clear map]" && single_positive {
        return Some("single-clear-map-remaining-v1");
    }
    if reach_range_ok(kind, objective) {
        return Some("single-reach-range-remaining-v1");
    }
    if alflyra_reach_ok(id, kind, cells, objective) {
        return Some("alflyra-3252-reach-npc-remaining-v1");
    }
    if seek_meet_ok(kind, objective) {
        return Some("seek-items-and-meet-npc-remaining-v1");
    }
    if kind == "[look cinematic]" && single_positive {
        return Some("look-cinematic-client-gated-v1");
    }
    if monster_kill_checkpoint_shape(kind, objective) {
        return Some("monster-kill-checkpoint-client-gated-v1");
    }
    if legion_content_clear_shape(kind, objective) {
        return Some("legion-content-clear-client-gated-v1");
    }
    None
}

/// Go `gmQuestTargetAllowed` 的静态那半：段是否在场、形态认不认。
/// 认不出时整段丢弃（`target_triples` 留空），运行期一律 (拒, 不可用)。
fn target_character(cells: &[Token]) -> (bool, bool, Vec<TargetChar>) {
    if !has_tag(cells, "[target character]") {
        return (false, true, Vec::new());
    }
    let flat = cells_in(cells, "[target character]");
    if flat.is_empty() || flat.len() % 3 != 0 {
        return (true, false, Vec::new());
    }
    let mut out = Vec::with_capacity(flat.len() / 3);
    for t in flat.chunks_exact(3) {
        if t[0].ttype != 6 || t[0].text.is_empty() || t[1].ttype != 0 || t[2].ttype != 0 {
            return (true, false, Vec::new());
        }
        if !(t[1].value >= -1 && t[1].value <= 15 && t[2].value >= -1 && t[2].value <= 3) {
            return (true, false, Vec::new());
        }
        out.push(TargetChar { job: t[0].text.clone(), advancement: t[1].value, awakening: t[2].value });
    }
    (true, true, out)
}

/// Go `avatarStringTable`：`key>value`，先 TrimSpace 再去 BOM，`//` 行跳过，
/// 后面的行覆盖前面的（与 GM 名字表的「先出现优先」刻意不同，这里跟任务文本表本身一致）。
fn kv_table(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::with_capacity(64 * 1024);
    for line in text.split('\n') {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if let Some(i) = line.find('>') {
            if i > 0 {
                out.insert(line[..i].trim().to_string(), line[i + 1..].trim().to_string());
            }
        }
    }
    out
}

/// Go `nameKey`：`<11::key>` → `key`。
fn name_key(ref_text: &str) -> String {
    let t = ref_text.trim().trim_start_matches('<').trim_end_matches('>');
    match t.find("::") {
        Some(i) => t[i + 2..].to_string(),
        None => t.to_string(),
    }
}

/// 从一段以 `<` 开头的文本头取一个 `<npc::数字>` 引用，返回引用原文、数字与长度。
/// 与 Go 的 `<npc::([0-9]+)>` 同口径：数字后必须紧跟 `>`，中间混了别的就整段不认。
fn npc_ref_at(s: &str) -> Option<(&str, &str, usize)> {
    let end = s.find('>')?;
    let body = s[1..end].strip_prefix("npc::")?;
    if body.is_empty() || !body.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((&s[..end + 1], body, end + 1))
}

/// Go `questNPCReference.FindAllString`：把文本里出现过的 `<npc::N>` 全部收集。
fn npc_refs(s: &str, want: &mut HashSet<String>) {
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        rest = &rest[i..];
        match npc_ref_at(rest) {
            Some((text, _, len)) => {
                want.insert(text.to_string());
                rest = &rest[len..];
            }
            None => rest = &rest[1..],
        }
    }
}

/// Go `questNPCReference.ReplaceAllStringFunc` + `gmQuestText` 的落定形态。
/// 按 `&str` 切片推进而不是按字节 `as char`，否则任何中文都会被拆成单字节乱码。
fn fold_text(parts: &[String], refs: &HashMap<String, String>) -> String {
    let joined = parts.join("\n").replace("\\n", "\n");
    let mut out = String::with_capacity(joined.len());
    let mut rest = joined.as_str();
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        match npc_ref_at(rest) {
            Some((text, digits, len)) => {
                out.push('【');
                match refs.get(text) {
                    Some(name) if !name.is_empty() => out.push_str(name),
                    _ => {
                        out.push_str("NPC ");
                        out.push_str(digits);
                        out.push_str("（名称缺失）");
                    }
                }
                out.push('】');
                rest = &rest[len..];
            }
            None => {
                out.push('<');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Go `gmQuestText`：文本单元取 Text，引用单元取折叠表里的值，空值跳过。
fn text_parts(cells: &[Token], tag: &str, refs: &HashMap<String, String>) -> Vec<String> {
    let mut parts = Vec::new();
    for t in cells_in(cells, tag) {
        let value = if !t.reference.is_empty() { refs.get(&t.reference).cloned().unwrap_or_default() } else { t.text.clone() };
        if !value.is_empty() {
            parts.push(value);
        }
    }
    parts
}

/// Go `buildQuestNPCNames`：按 `list/npc.lst` + 各 `.npc` 的 `[name]` +
/// `string/npc.uv.str` 解析 NPC 显示名，写回引用表。
fn npc_names(a: &Archive, wanted: &HashSet<String>) -> Result<HashMap<String, String>, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    if wanted.is_empty() {
        return Ok(out);
    }
    let index = import_script_at(a, "list/npc.lst")?;
    let rows = parse_index(&index.cells)?;
    let names = kv_table(&a.read_text("string/npc.uv.str").map_err(|e| format!("读取 NPC 文本表：{e}"))?);
    for (id, path) in rows {
        let ref_text = format!("<npc::{id}>");
        if !wanted.contains(&ref_text) {
            continue;
        }
        let def = import_script(a, &path).map_err(|e| format!("读取 NPC {id} 定义：{e}"))?;
        for token in cells_in(&def.cells, "[name]") {
            let mut name = token.text.clone();
            if !token.reference.is_empty() {
                if !token.reference.starts_with("<8::") {
                    continue;
                }
                name = names.get(&name_key(&token.reference)).cloned().unwrap_or_default();
            }
            let name = name.trim().to_string();
            if !name.is_empty() {
                out.insert(ref_text, name);
                break;
            }
        }
    }
    Ok(out)
}

/// 直读 PVF 生成任务产物。
pub fn build_quests(a: &Archive, pvf: &Path, format_label: &str) -> Result<QuestFile, String> {
    let mut file = QuestFile::default();
    file.source = SourceSnapshot {
        format: format_label.to_string(),
        path: pvf.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        size: std::fs::metadata(pvf).map(|m| m.len() as i64).unwrap_or(0),
        checksum: a.checksum.clone(),
        built_at: now_rfc3339(),
    };
    let index = import_script_at(a, "list/quest.lst")?;
    file.index_sha256 = index.sha;
    let rows = parse_index(&index.cells)?;

    // 第一遍：导入目录并收集需要翻译的引用（Go buildQuestText 的同一次遍历）。
    let quest_texts = kv_table(&a.read_text("string/quest.uv.str").map_err(|e| format!("读取任务文本表：{e}"))?);
    let mut refs: HashMap<String, String> = HashMap::with_capacity(8192);
    let mut loaded: Vec<(u32, Imported)> = Vec::with_capacity(rows.len());
    for (id, row_path) in &rows {
        let imported = import_script(a, row_path).unwrap_or_else(|e| Imported { path: String::new(), sha: String::new(), cells: Vec::new(), pending: vec![e] });
        if imported.pending.is_empty() {
            for tag in ["[name]", "[depend message]", "[condition message]", "[solve message]"] {
                for t in cells_in(&imported.cells, tag) {
                    if t.reference.starts_with("<11::") {
                        if let Some(v) = quest_texts.get(&name_key(&t.reference)) {
                            if !v.is_empty() {
                                refs.insert(t.reference.clone(), v.clone());
                            }
                        }
                    }
                }
            }
        }
        loaded.push((*id, imported));
        // 逐条释放解压块：单元格已整份落在 `loaded` 里，块留着只会把常驻内存抬高。
        if loaded.len() % 400 == 0 {
            a.release_read_caches();
        }
    }
    // NPC 名藏在已翻译文本的 <npc::N> 引用里，必须等上一步做完才能收集。
    let mut wanted: HashSet<String> = HashSet::new();
    for value in refs.values() {
        npc_refs(value, &mut wanted);
    }
    for (name, value) in npc_names(a, &wanted)? {
        refs.insert(name, value);
    }
    a.release_read_caches();

    // 第二遍：投影 + 折文本。
    for (id, mut imported) in loaded {
        let cells = std::mem::take(&mut imported.cells);
        let mut pending = std::mem::take(&mut imported.pending);
        let mut entry = QuestEntry::default();
        if pending.is_empty() {
            entry.path = imported.path;
            entry.sha256 = imported.sha;
            let level = cells_in(&cells, "[level]");
            if level.len() == 2 && level[0].ttype == 0 && level[1].ttype == 0 && level[0].value >= 0 && level[1].value >= level[0].value {
                entry.minimum_level = level[0].value as u32;
            } else {
                pending.push("unsupported level condition".into());
            }
            for c in cells_in(&cells, "[job]") {
                if c.ttype == 6 {
                    entry.jobs.push(c.text.clone());
                } else {
                    pending.push("unsupported job condition".into());
                }
            }
            match prerequisite_groups(&cells) {
                Ok((groups, flat)) => {
                    entry.prerequisite_groups = groups;
                    entry.prerequisites = flat;
                }
                Err(e) => pending.push(e),
            }
            entry.collisions = collision_quests(&cells);
            entry.answers = prerequisite_answers(&cells);
            entry.kind = cells_in(&cells, "[type]").iter().find(|t| t.ttype == 6).map(|t| t.text.clone()).unwrap_or_default();
            let objective = cells_in(&cells, "[int data]");
            entry.grow_usable = true;
            for g in cells_in(&cells, "[grow type]") {
                if g.ttype != 0 {
                    entry.grow_usable = false;
                    break;
                }
                entry.grow_types.push(g.value);
            }
            entry.reward_usable = reward_usable(&cells);
            entry.model = initial_model(id, &entry.kind, &cells, &objective, false).unwrap_or_default().to_string();
            let (present, valid, triples) = target_character(&cells);
            entry.target_present = present;
            entry.target_valid = valid;
            entry.target_triples = triples;
            entry.grade_texts = cells_in(&cells, "[grade]").iter().map(|t| t.text.clone()).filter(|s| !s.is_empty()).collect();
            entry.name = fold_text(&text_parts(&cells, "[name]", &refs), &refs);
            entry.depend = fold_text(&text_parts(&cells, "[depend message]", &refs), &refs);
            entry.condition = fold_text(&text_parts(&cells, "[condition message]", &refs), &refs);
            entry.solve = fold_text(&text_parts(&cells, "[solve message]", &refs), &refs);
        }
        entry.pending = pending;
        file.quests.insert(id, entry);
    }
    if file.quests.is_empty() {
        return Err("当前 PVF 任务索引为空".into());
    }
    file.validate()?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok(ttype: u8, value: i32, text: &str, reference: &str) -> Token {
        Token { ttype, value, text: text.to_string(), reference: reference.to_string() }
    }
    fn num(v: i32) -> Token {
        tok(0, v, "", "")
    }
    fn tag(t: &str) -> Token {
        tok(3, 0, t, "")
    }
    fn txt(t: &str) -> Token {
        tok(6, 0, t, "")
    }
    fn section(name: &str, cells: &[Token]) -> Vec<Token> {
        let mut out = vec![tag(name)];
        out.extend_from_slice(cells);
        out
    }
    fn objective(v: &[i32]) -> Vec<Token> {
        v.iter().copied().map(num).collect()
    }

    #[test]
    fn prerequisite_sections_are_alternative_groups() {
        // Go parsePrerequisiteGroups：每个重复段是一组，段间是可选分支。
        let mut cells = section("[pre required quest]", &[num(10), num(11)]);
        cells.extend(section("[pre required quest]", &[num(20)]));
        let (groups, flat) = prerequisite_groups(&cells).unwrap();
        assert_eq!(groups, vec![vec![10u32, 11], vec![20]]);
        assert_eq!(flat, vec![10u32, 11, 20]);
        // 空段就是无前置；非数值单元整条待核实。
        let empty = section("[pre required quest]", &[]);
        assert!(prerequisite_groups(&empty).unwrap().0.is_empty());
        let bad = section("[pre required quest]", &[txt("10")]);
        assert!(prerequisite_groups(&bad).is_err());
    }

    #[test]
    fn collision_and_answer_sections_never_leak() {
        let mut cells = section("[collision quest]", &[num(101), num(102)]);
        cells.extend(section("[pre required quest answer]", &[num(90), num(0)]));
        cells.extend(section("[pre required quest]", &[num(7)]));
        assert_eq!(collision_quests(&cells), vec![101u32, 102]);
        // 答案索引 0 合法（第一个答案）；问题 id 必须为正，段形必须恰好两格。
        assert_eq!(prerequisite_answers(&cells), vec![QuestAnswer { question: 90, index: 0 }]);
        assert!(prerequisite_answers(&section("[pre required quest answer]", &[num(0), num(1)])).is_empty());
        assert!(prerequisite_answers(&section("[pre required quest answer]", &[num(90), num(1), num(2)])).is_empty());
        // 段落边界由任意 type-3 标签重置：前置段不会吃到互斥段的号。
        assert_eq!(prerequisite_groups(&cells).unwrap().1, vec![7u32]);
    }

    #[test]
    fn reward_usable_follows_go() {
        assert!(reward_usable(&section("[reward type]", &[txt("[item]")])));
        assert!(reward_usable(&section("[reward type]", &[txt("[none]")])));
        assert!(!reward_usable(&section("[reward type]", &[txt("[exp]")])));
        // [slot expansion] 必须带一个 0..=3 的槽位序号，否则奖励无法结算。
        let mut ok = section("[reward type]", &[txt("[slot expansion]")]);
        ok.extend(section("[reward int data]", &[num(2)]));
        assert!(reward_usable(&ok));
        let mut bad = section("[reward type]", &[txt("[slot expansion]")]);
        bad.extend(section("[reward int data]", &[num(9)]));
        assert!(!reward_usable(&bad));
        // 可选奖励（选择池）本构建不结算。
        let mut select = section("[reward type]", &[txt("[item]")]);
        select.extend(section("[reward select int data]", &[num(1)]));
        assert!(!reward_usable(&select));
        // 没有 [reward type] 段 = 无奖励，可用。
        assert!(reward_usable(&[]));
    }

    #[test]
    fn eight_progress_models_match_go() {
        let cases: Vec<(u32, &str, Vec<Token>, Vec<Token>, &str)> = vec![
            (3160, "[meet npc]", Vec::new(), objective(&[29]), "single-meet-npc-remaining-v1"),
            (3161, "[clear map]", Vec::new(), objective(&[5]), "single-clear-map-remaining-v1"),
            (3156, "[reach the range]", Vec::new(), objective(&[38, 3, 445, 115, 400, 300]), "single-reach-range-remaining-v1"),
            (3252, "[reach the range]", section("[complete npc index]", &[num(303)]), objective(&[303, 1000, 1000]), "alflyra-3252-reach-npc-remaining-v1"),
            (3173, "[seek n meet npc]", Vec::new(), objective(&[10164787, 1, 10164789, 1, 29]), "seek-items-and-meet-npc-remaining-v1"),
            (21029, "[look cinematic]", Vec::new(), objective(&[7]), "look-cinematic-client-gated-v1"),
            (
                23053,
                "[monster kill checkpoint]",
                Vec::new(),
                objective(&[109019626, 1, 8, 109019627, 3, 5, 109019628, 1, 3, 109019629, 2, 1, 109019630, -1, -1]),
                "monster-kill-checkpoint-client-gated-v1",
            ),
            (23100, "[legion content clear]", Vec::new(), objective(&[5, 1]), "legion-content-clear-client-gated-v1"),
            (23101, "[legion content clear with difficulty]", Vec::new(), objective(&[6, 1, 2, 1]), "legion-content-clear-client-gated-v1"),
        ];
        for (id, kind, cells, obj, want) in cases {
            assert_eq!(initial_model(id, kind, &cells, &obj, false), Some(want), "id {id}");
        }
    }

    #[test]
    fn unknown_shapes_stay_unmodelled() {
        // 待核实的任务永远不认：Go 各分支都先判 len(Pending)==0。
        assert_eq!(initial_model(3160, "[meet npc]", &[], &objective(&[29]), true), None);
        // 陌生目标类型。
        assert_eq!(initial_model(1, "[arrive in town]", &[], &objective(&[38]), false), None);
        // 矩形形态不符：城镇号为 0、宽高非正。
        assert_eq!(initial_model(1, "[reach the range]", &[], &objective(&[0, 3, 445, 115, 400, 300]), false), None);
        assert_eq!(initial_model(1, "[reach the range]", &[], &objective(&[38, 3, 445, 115, 0, 300]), false), None);
        // 3252 之外的三格 reach 不套用那条特例。
        assert_eq!(initial_model(3253, "[reach the range]", &[], &objective(&[303, 1000, 1000]), false), None);
        // 检查点缺 (-1,-1) 收尾。
        assert_eq!(
            initial_model(1, "[monster kill checkpoint]", &[], &objective(&[109019626, 1, 8, 109019627, 3, 5, 109019628, 1, 3]), false),
            None
        );
        // 军团内容：内容号 0 合法（stolenlandispins），负数不合法。
        assert_eq!(initial_model(1, "[legion content clear]", &[], &objective(&[0, 1]), false), Some("legion-content-clear-client-gated-v1"));
        assert_eq!(initial_model(1, "[legion content clear]", &[], &objective(&[-1, 1]), false), None);
    }

    #[test]
    fn target_character_shape_matches_go() {
        // 段缺席 = 不限角色（allowed, usable = true, true）。
        assert_eq!(target_character(&[]), (false, true, Vec::new()));
        let one = section("[target character]", &[txt("[all]"), num(-1), num(-1)]);
        let (present, valid, triples) = target_character(&one);
        assert!((present, valid) == (true, true));
        assert_eq!(triples[0], TargetChar { job: "[all]".into(), advancement: -1, awakening: -1 });
        // 长度不整除 3、类型不符、数值越界：整段丢弃，不留半截解码结果。
        assert_eq!(target_character(&section("[target character]", &[txt("[all]"), num(0)])), (true, false, Vec::new()));
        assert_eq!(target_character(&section("[target character]", &[txt("[all]"), num(16), num(0)])), (true, false, Vec::new()));
        assert_eq!(target_character(&section("[target character]", &[txt("[all]"), num(0), num(4)])), (true, false, Vec::new()));
        assert_eq!(target_character(&section("[target character]", &[num(1), num(0), num(0)])), (true, false, Vec::new()));
        // 段存在但为空。
        assert_eq!(target_character(&section("[target character]", &[])), (true, false, Vec::new()));
    }

    #[test]
    fn string_table_follows_go() {
        let t = kv_table("  100>首行  \n//200>注释\n300>尾行\n300>覆盖\n\u{feff}400>BOM\n500\n");
        assert_eq!(t["100"], "首行");
        assert_eq!(t["300"], "覆盖");
        assert_eq!(t["400"], "BOM");
        assert!(!t.contains_key("200"));
        assert!(!t.contains_key("500"));
    }

    #[test]
    fn npc_refs_found_anywhere() {
        let mut want = HashSet::new();
        npc_refs("去见<npc::29>，路上小心<npc::1000>。", &mut want);
        // 数字后必须紧跟 `>`，所以这条不是引用；`<npc::abc>` 同样不认。
        npc_refs("残缺 <npc::12abc> 与 <npc::>", &mut want);
        assert!(want.contains("<npc::29>"));
        assert!(want.contains("<npc::1000>"));
        assert!(!want.contains("<npc::12abc>"));
        assert_eq!(want.len(), 2);
    }

    #[test]
    fn fold_text_keeps_chinese() {
        let mut refs = HashMap::new();
        refs.insert("<npc::29>".to_string(), "赛丽亚".to_string());
        let parts = vec!["去见<npc::29>".to_string(), "然后回<npc::77>".to_string()];
        // 逐字节 `as char` 会把每个 UTF-8 字节当成一个字符，中文全废——这里必须整字保留。
        assert_eq!(fold_text(&parts, &refs), "去见【赛丽亚】\n然后回【NPC 77（名称缺失）】");
        // Go 只把字面量 `\n` 换成真换行，两侧原有空格照留——不做 trim。
        assert_eq!(fold_text(&[String::from("字面 \\n 换行")], &refs), "字面 \n 换行");
        assert_eq!(fold_text(&[String::from("<npc::12abc> 不是引用")], &refs), "<npc::12abc> 不是引用");
        assert_eq!(fold_text(&[], &refs), "");
    }

    #[test]
    fn text_parts_reads_reference_or_text() {
        let mut refs = HashMap::new();
        refs.insert("<11::9001>".to_string(), "新的开始".to_string());
        let cells = section("[name]", &[tok(8, 0, "", "<11::9001>"), txt("直写名"), tok(8, 0, "", "<11::9999>")]);
        // 引用查不到 = 空值，与 Go 一样跳过，不占一个换行。
        assert_eq!(text_parts(&cells, "[name]", &refs), vec!["新的开始".to_string(), "直写名".to_string()]);
    }

    #[test]
    fn section_cells_match_go_semantics() {
        let mut cells = section("[level]", &[num(20), num(60)]);
        cells.extend(section("[grade]", &[txt("[epic]")]));
        assert_eq!(cells_in(&cells, "[level]"), vec![num(20), num(60)]);
        // 任意 type-3 标签都会关闭上一段。
        assert_eq!(cells_in(&cells, "[grade]"), vec![txt("[epic]")]);
        assert!(cells_in(&cells, "[int data]").is_empty());
        assert!(has_tag(&cells, "[grade]"));
        assert!(!has_tag(&cells, "[target character]"));
    }
}
