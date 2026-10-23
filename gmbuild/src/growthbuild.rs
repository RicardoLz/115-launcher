//! 养成产物构建器：离线直读 PVF，落一个 `gm-growth.json`。
//!
//! 数据口径逐条抄自 Go 服务端树，产物只回答「服务端会把这个角色算成什么样」：
//!
//! | 产物字段 | Go 来源 |
//! | --- | --- |
//! | `thresholds` | `catalog.ExperienceThresholds(character/exptable.tbl)` |
//! | `skill_points` | `catalog.ParseProgression` 里 `sectionCells(etc/sptable.etc, "[sp table]")` 的 `parsePairs` |
//! | `professions[*].raw_sha256` / `job` / `base_growth` | `catalog.ImportCharacters` 的初始段与 `[growtype 1]` 段 |
//! | `professions[*].advancement_growth` | `catalog.ProfessionGrowth` 对 `[growtype N+1]` 的投影 |
//! | `professions[*].awakening_skills` | `catalog.AwakeningSkillGrants` |
//! | `professions[*].awakening_levels` | `character.ImportLearningCatalog` + `LearningDefinition.Ints("[required level]")` |
//!
//! 这里**不**投影 `advancement_skills` / `initial_skill_slots` / `create_equipment`：
//! 等级、转职、觉醒三条写链路都不读它们（转职技能在学习侧按 growtype 现取，
//! 快捷栏与出生装备属于建角路径）。少一块就少一块会漂移的产物。

use crate::archive::Archive;
use crate::growth::{GrowthFile, GrowthProfession, SourceSnapshot};
use crate::itemindex::{parse_index, read_script, resolve_script, tokens_from_raw, Token};
use crate::slotmap::now_rfc3339;
use sha2::Digest;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

/// Go `fmt.Sscanf(section, "[growtype %d]", &grow)`：读一段十进制整数，
/// 读不出来就保持原值不变（前缀命中但内容不是数字，正是这种情形）。
fn scanned_number(section: &str, prefix: &str) -> Option<i32> {
    let body = section.strip_prefix(prefix)?;
    let bytes = body.as_bytes();
    let mut end = 0usize;
    if end < bytes.len() && (bytes[end] == b'-' || bytes[end] == b'+') {
        end += 1;
    }
    let start = end;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    if end == start {
        return None;
    }
    body[start..end].parse::<i32>().ok()
}

fn numeric(t: &Token) -> Option<f32> {
    match t.ttype {
        0 => Some(t.value as f32),
        2 => Some(f32::from_bits(t.value as u32)),
        _ => None,
    }
}

fn is_bad_number(v: f32) -> bool {
    v.is_nan() || v.is_infinite() || v < 0.0
}

/// Go `ImportCharacters` 的初始段扫描：职业名 + `[initial value]` 之下的属性表。
/// 返回 (job, initial_attributes, initial_skill_cells)。
fn initial_pass(cells: &[Token]) -> Result<(String, HashMap<String, f32>, Vec<i32>), String> {
    let mut job = String::new();
    let mut sections: HashMap<String, Vec<Token>> = HashMap::new();
    let mut initial_skills: Vec<i32> = Vec::new();
    let mut initial = false;
    let mut section = String::new();
    for t in cells {
        if t.ttype == 3 {
            section = t.text.to_lowercase();
            if section == "[initial value]" {
                initial = true;
            }
            if section.starts_with("[growtype ") {
                initial = false;
            }
            continue;
        }
        if section == "[job]" && t.ttype == 6 {
            job = t.text.clone();
        }
        if !initial {
            continue;
        }
        if section == "[skill]" {
            if t.ttype != 0 {
                return Err("unsupported initial skill cell".into());
            }
            initial_skills.push(t.value);
            continue;
        }
        if section.starts_with("[/") {
            continue;
        }
        sections.entry(section.clone()).or_default().push(t.clone());
    }
    let mut attrs: HashMap<String, f32> = HashMap::new();
    for (key, list) in &sections {
        if list.len() != 1 {
            continue;
        }
        if let Some(v) = numeric(&list[0]) {
            attrs.insert(key.clone(), v);
        }
    }
    Ok((job, attrs, initial_skills))
}

/// Go `ImportCharacters` 里 `[growtype 1]` 那一遍：未转职时每级成长。
/// 注意与 `ProfessionGrowth` 的差别——这一遍只被 `[growtype ` 标签开关，
/// 不被 `[awakening*` 关闭。
fn base_growth_pass(cells: &[Token], attrs: &HashMap<String, f32>) -> Result<BTreeMap<String, f32>, String> {
    let mut out: BTreeMap<String, f32> = BTreeMap::new();
    let mut growth = false;
    let mut section = String::new();
    for t in cells {
        if t.ttype == 3 {
            section = t.text.to_lowercase();
            if section.starts_with("[growtype ") {
                growth = section == "[growtype 1]";
            }
            continue;
        }
        if !growth || !attrs.contains_key(&section) {
            continue;
        }
        if out.contains_key(&section) {
            return Err(format!("duplicate base growth {section}"));
        }
        let v = numeric(t).ok_or_else(|| format!("unsupported base growth cell in {section}"))?;
        out.insert(section.clone(), v);
    }
    Ok(out)
}

/// Go `ProfessionGrowth(tokens, attributes, advancement)`：`[growtype adv+1]` 段内、
/// 键必须在初始属性里、任何 `[awakening` 标签都会关掉本段。
/// 与 Go 一样，不合格返回 `Err`，调用方据此**省略**该分支（分支不存在）。
fn profession_growth(cells: &[Token], attrs: &HashMap<String, f32>, advancement: u8) -> Result<BTreeMap<String, f32>, String> {
    let mut out: BTreeMap<String, f32> = BTreeMap::new();
    let want = format!("[growtype {}]", advancement as u32 + 1);
    let mut active = false;
    let mut section = String::new();
    for t in cells {
        if t.ttype == 3 {
            section = t.text.to_lowercase();
            if section.starts_with("[growtype ") {
                active = section == want;
            }
            if section.starts_with("[awakening") {
                active = false;
            }
            continue;
        }
        if !active || !attrs.contains_key(&section) {
            continue;
        }
        if out.contains_key(&section) {
            return Err(format!("duplicate growth {section}"));
        }
        let v = numeric(t).ok_or_else(|| format!("invalid growth cell in {section}"))?;
        if is_bad_number(v) {
            return Err(format!("invalid growth value in {section}"));
        }
        out.insert(section.clone(), v);
    }
    if out.get("[hp max]").copied().unwrap_or(0.0) <= 0.0 || out.get("[mp max]").copied().unwrap_or(0.0) <= 0.0 {
        return Err("missing source growth".into());
    }
    Ok(out)
}

/// Go `AwakeningSkillGrants`：`[growtype N]` 决定分支（N-1），`[awakening S]` 决定阶段，
/// 只有 `[awakening skill]` 段内的数值单元才算授予。
fn awakening_grants(cells: &[Token]) -> BTreeMap<u8, BTreeMap<u8, Vec<i32>>> {
    let mut result: BTreeMap<u8, BTreeMap<u8, Vec<i32>>> = BTreeMap::new();
    let mut grow = 0i32;
    let mut stage = 0i32;
    let mut section = String::new();
    for t in cells {
        if t.ttype == 3 {
            section = t.text.to_lowercase();
            if section.starts_with("[growtype ") {
                if let Some(n) = scanned_number(&section, "[growtype ") {
                    grow = n;
                }
                stage = 0;
            }
            if section.starts_with("[awakening ") {
                if let Some(n) = scanned_number(&section, "[awakening ") {
                    stage = n;
                }
            }
            continue;
        }
        if section != "[awakening skill]" || !(1..=16).contains(&grow) || !(1..=3).contains(&stage) || t.ttype != 0 {
            continue;
        }
        result
            .entry((grow - 1) as u8)
            .or_default()
            .entry(stage as u8)
            .or_default()
            .push(t.value);
    }
    result
}

/// Go `sectionCells` 口径的段落取值（本构建器只需要 `[sp table]`）。
fn section_cells(cells: &[Token], name: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut active = false;
    for t in cells {
        if t.ttype == 3 {
            active = t.text == name;
            continue;
        }
        if active {
            out.push(t.clone());
        }
    }
    out
}

/// Go `catalog.ExperienceThresholds`：type0 非负；type9 成对拼 64 位（高/低）；
/// 必须非零且严格递增。
fn experience_thresholds(cells: &[Token]) -> Result<Vec<u64>, String> {
    let mut values: Vec<u64> = Vec::new();
    let mut i = 0usize;
    while i < cells.len() {
        let c = &cells[i];
        let v: u64 = match c.ttype {
            0 => {
                if c.value < 0 {
                    return Err("negative32-bit experience threshold".into());
                }
                c.value as u64
            }
            9 => {
                if i + 1 >= cells.len() || cells[i + 1].ttype != 9 {
                    return Err("unpaired64-bit experience threshold".into());
                }
                ((c.value as u32) as u64) << 32 | (cells[i + 1].value as u32) as u64
            }
            t => return Err(format!("unsupported experience token {t}")),
        };
        if v == 0 || values.last().is_some_and(|p| v <= *p) {
            return Err("experience thresholds are not strictly increasing".into());
        }
        values.push(v);
        i += if c.ttype == 9 { 2 } else { 1 };
    }
    if values.is_empty() {
        return Err("empty experience table".into());
    }
    Ok(values)
}

/// Go `parsePairs`（`ParseProgression` 内）：成对、键 1..=65535、值非负、不重复。
fn skill_points(cells: &[Token]) -> Result<BTreeMap<u16, u16>, String> {
    if cells.is_empty() || cells.len() % 2 != 0 {
        return Err("unpaired progression rows".into());
    }
    let mut out: BTreeMap<u16, u16> = BTreeMap::new();
    for pair in cells.chunks_exact(2) {
        if pair[0].ttype != 0 || pair[0].value <= 0 || pair[0].value > 65535 || pair[1].ttype != 0 || pair[1].value < 0 {
            return Err("invalid progression row".into());
        }
        if pair[1].value > 65535 {
            return Err("SP overflow".into());
        }
        if out.insert(pair[0].value as u16, pair[1].value as u16).is_some() {
            return Err("duplicate progression level".into());
        }
    }
    Ok(out)
}

/// Go `LearningFieldsFromTokens` 里 `[required level]` 那一条：`[variation point]`
/// 整块吞掉；`[vp explain]`/`[explain group]`/`[preset info]` 之内不采集；
/// 同名标签只取**第一段**；`Ints` 要求全部是数值单元，否则当作没有。
fn required_levels(cells: &[Token]) -> Vec<i32> {
    let mut out: Vec<i32> = Vec::new();
    let mut open: HashSet<String> = HashSet::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut variation = false;
    let mut enabled = false;
    let mut bad = false;
    let mut tag = String::new();
    for t in cells {
        if t.ttype == 3 && t.text == "[variation point]" {
            variation = true;
            enabled = false;
            continue;
        }
        if variation {
            if t.ttype == 3 && t.text == "[/variation point]" {
                variation = false;
            }
            continue;
        }
        if t.ttype == 3 {
            if t.text.starts_with("[/") {
                open.remove(&format!("[{}", &t.text[2..]));
                enabled = false;
                continue;
            }
            tag = t.text.clone();
            enabled = open.is_empty() && !seen.contains(&tag) && tag != "[type]";
            if enabled {
                seen.insert(tag.clone());
            }
            if tag == "[vp explain]" || tag == "[explain group]" || tag == "[preset info]" {
                open.insert(tag.clone());
                enabled = false;
            }
            continue;
        }
        if !enabled {
            continue;
        }
        if tag != "[required level]" {
            continue;
        }
        if t.ttype != 0 {
            bad = true;
        } else {
            out.push(t.value);
        }
    }
    if bad {
        return Vec::new();
    }
    out
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = sha2::Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Go `ImportCharacters` 认职业索引的口径：原样路径，不做 `(r)` 回落。
fn profession_entries(a: &Archive) -> Result<Vec<(u8, String)>, String> {
    let idx = a
        .find_file_index("list/character.lst")
        .ok_or_else(|| "pvf file not found: list/character.lst".to_string())?;
    let raw = a.read_raw_idx(idx)?;
    let cells = tokens_from_raw(a, &raw)?;
    if cells.len() % 2 != 0 {
        return Err("invalid profession index".into());
    }
    let mut out: Vec<(u8, String)> = Vec::with_capacity(cells.len() / 2);
    let mut seen: HashSet<u8> = HashSet::new();
    for pair in cells.chunks_exact(2) {
        if pair[0].ttype != 0 || !(0..=255).contains(&pair[0].value) || pair[1].ttype != 6 {
            return Err("invalid profession pair".into());
        }
        let id = pair[0].value as u8;
        let path = pair[1].text.replace('\\', "/").to_lowercase();
        if !seen.insert(id) {
            return Err(format!("duplicate profession {id}"));
        }
        out.push((id, path));
    }
    Ok(out)
}

/// 本职业技能索引：`skill/<stem>skill.lst`，缺席才用 `skill/<stem>.lst`（Go 同次序）。
/// 返回 `None` 表示两个都不存在——Go 在这一步直接判产物无效。
fn skill_index(a: &Archive, chr_path: &str) -> Result<Option<HashMap<u32, String>>, String> {
    let stem = chr_path
        .rsplit('/')
        .next()
        .unwrap_or(chr_path)
        .strip_suffix(".chr")
        .unwrap_or_else(|| chr_path.rsplit('/').next().unwrap_or(chr_path));
    let mut chosen: Option<String> = None;
    for cand in [format!("skill/{stem}skill.lst"), format!("skill/{stem}.lst")] {
        if a.find_file_index(&cand).is_some() {
            chosen = Some(cand);
            break;
        }
    }
    let Some(cand) = chosen else {
        return Ok(None);
    };
    let (_, cells) = read_script(a, &cand)?;
    let rows = parse_index(&cells)?;
    let mut map: HashMap<u32, String> = HashMap::with_capacity(rows.len());
    for (id, path) in rows {
        if id == 0 || id > 65535 {
            return Err(format!("技能编号越界：{id}"));
        }
        map.insert(id, path);
    }
    Ok(Some(map))
}

/// 直读 PVF 生成养成产物。任何一处硬性缺失都返回 `Err`：宁可不出产物，
/// 也不要把「成长表残缺」伪装成「这个角色不能改」。
pub fn build_growth(a: &Archive, pvf: &Path, format_label: &str) -> Result<GrowthFile, String> {
    let mut file = GrowthFile::default();
    let size = std::fs::metadata(pvf).map(|m| m.len() as i64).unwrap_or(0);
    file.source = SourceSnapshot {
        format: format_label.to_string(),
        path: pvf.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        size,
        checksum: a.checksum.clone(),
        built_at: now_rfc3339(),
    };

    let (_, cells) = resolve_script(a, "character/exptable.tbl")?;
    file.thresholds = experience_thresholds(&cells)?;
    let (_, cells) = resolve_script(a, "etc/sptable.etc")?;
    file.skill_points = skill_points(&section_cells(&cells, "[sp table]"))?;

    let mut skipped: Vec<String> = Vec::new();
    for (id, path) in profession_entries(a)? {
        let idx = a
            .find_file_index(&path)
            .ok_or_else(|| format!("pvf file not found: {path}"))?;
        let raw = (*a.read_raw_idx(idx)?).clone();
        let cells = tokens_from_raw(a, &raw)?;
        let (job, attrs, initial_skills) = initial_pass(&cells)?;
        let hp = attrs.get("[hp max]").copied().unwrap_or(0.0);
        let mp = attrs.get("[mp max]").copied().unwrap_or(0.0);
        if job.is_empty() || hp <= 0.0 || mp <= 0.0 || initial_skills.len() % 3 != 0 {
            return Err(format!("incomplete initial configuration for {id}"));
        }
        let base_growth = base_growth_pass(&cells, &attrs)?;
        if base_growth.is_empty() {
            return Err(format!("职业 {id} 缺少 [growtype 1] 成长"));
        }
        let mut advancement_growth: BTreeMap<u8, BTreeMap<String, f32>> = BTreeMap::new();
        for adv in 1u8..16 {
            if let Ok(g) = profession_growth(&cells, &attrs, adv) {
                advancement_growth.insert(adv, g);
            }
        }
        let awakening_skills = awakening_grants(&cells);
        let mut p = GrowthProfession {
            path: path.clone(),
            raw_sha256: sha256_hex(&raw),
            job,
            base_growth,
            advancement_growth,
            awakening_skills,
            awakening_levels: BTreeMap::new(),
        };
        // 觉醒授予的等级门来自各 `.skl` 原文；成员判定来自本职业技能索引。
        let mut granted: Vec<i32> = Vec::new();
        for stages in p.awakening_skills.values() {
            for row in stages.values() {
                granted.extend(row.iter().step_by(2).copied());
            }
        }
        if !granted.is_empty() {
            let index = match skill_index(a, &path)? {
                Some(m) => m,
                None => return Err(format!("no source skill index for profession {id}")),
            };
            let mut ids: Vec<i32> = granted;
            ids.sort_unstable();
            ids.dedup();
            for sid in ids {
                if sid <= 0 || sid > 65535 {
                    return Err(format!("职业 {id} 的觉醒技能编号越界：{sid}"));
                }
                let Some(name) = index.get(&(sid as u32)) else { continue };
                let full = if name.starts_with("skill/") { name.clone() } else { format!("skill/{name}") };
                let Ok((_, skl)) = resolve_script(a, &full) else { continue };
                p.awakening_levels.insert(sid as u16, required_levels(&skl));
            }
        }
        if p.awakening_skills.is_empty() {
            skipped.push(format!("{id}"));
        }
        file.professions.insert(id, p);
    }
    if file.professions.is_empty() {
        return Err("当前 PVF 职业索引为空".into());
    }
    file.validate()?;
    if !skipped.is_empty() {
        eprintln!("提示：{} 个职业在本源里没有 [awakening skill] 授予表（未转职/无觉醒分支）", skipped.len());
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tk(ttype: u8, value: i32, text: &str) -> Token {
        Token { ttype, value, text: text.to_string(), reference: String::new() }
    }
    fn tag(text: &str) -> Token {
        tk(3, 0, text)
    }
    fn f32cell(v: f32) -> Token {
        tk(2, v.to_bits() as i32, "")
    }

    #[test]
    fn sscan_like_number_parse() {
        assert_eq!(scanned_number("[growtype 2]", "[growtype "), Some(2));
        assert_eq!(scanned_number("[growtype 10]", "[growtype "), Some(10));
        // Go 的 Sscanf 解不出数字时保持原值：这里返回 None，由调用方保留旧值
        assert_eq!(scanned_number("[growtype first]", "[growtype "), None);
        assert_eq!(scanned_number("[awakening skill]", "[awakening "), None);
        assert_eq!(scanned_number("[awakening 3]", "[awakening "), Some(3));
        assert_eq!(scanned_number("[growtype 1]", "[awakening "), None);
    }

    #[test]
    fn thresholds_follow_go_token_rules() {
        // type9 成对：高位在前，拼成 64 位
        let c = vec![tk(0, 100, ""), tk(9, 1, ""), tk(9, 2, "")];
        assert_eq!(experience_thresholds(&c).unwrap(), vec![100u64, 4294967298]);
        assert!(experience_thresholds(&[tk(0, -1, "")]).is_err());
        assert!(experience_thresholds(&[tk(9, 5, "")]).is_err());
        assert!(experience_thresholds(&[tk(0, 5, ""), tk(0, 5, "")]).is_err());
        assert!(experience_thresholds(&[tk(0, 0, "")]).is_err());
        assert!(experience_thresholds(&[tag("[x]")]).is_err());
        assert!(experience_thresholds(&[]).is_err());
    }

    #[test]
    fn skill_point_pairs_follow_go() {
        let c = vec![tk(0, 1, ""), tk(0, 3, ""), tk(0, 2, ""), tk(0, 4, "")];
        assert_eq!(skill_points(&c).unwrap()[&2], 4);
        assert!(skill_points(&[]).is_err());
        assert!(skill_points(&c[..3]).is_err());
        // 键必须是 1..=65535，值必须非负且不超 u16
        assert!(skill_points(&[tk(0, 0, ""), tk(0, 1, "")]).is_err());
        assert!(skill_points(&[tk(0, 3, ""), tk(0, -1, "")]).is_err());
        assert!(skill_points(&[tk(0, 3, ""), tk(0, 70000, "")]).is_err());
        assert!(skill_points(&[tk(0, 3, ""), tk(0, 1, ""), tk(0, 3, ""), tk(0, 2, "")]).is_err());
    }

    #[test]
    fn initial_pass_mirrors_go_sections() {
        let c = vec![
            tag("[job]"),
            tk(6, 0, "[swordman]"),
            tag("[initial value]"),
            tag("[hp max]"),
            tk(0, 200, ""),
            tag("[mp max]"),
            f32cell(120.0),
            tag("[str]"),
            tk(0, 30, ""),
            tag("[dual]"),
            tk(0, 1, ""),
            tk(0, 2, ""),
            tag("[skill]"),
            tk(0, 7, ""),
            tk(0, 1, ""),
            tk(0, 1, ""),
            tag("[growtype 1]"),
            tag("[hp max]"),
            tk(0, 10, ""),
        ];
        let (job, attrs, skills) = initial_pass(&c).unwrap();
        assert_eq!(job, "[swordman]");
        assert_eq!(skills, vec![7, 1, 1]);
        assert_eq!(attrs["[hp max]"], 200.0);
        assert_eq!(attrs["[mp max]"], 120.0);
        assert_eq!(attrs["[str]"], 30.0);
        // 两个单元的段落不算属性（Go 只在 len(cells)==1 时取值）
        assert!(!attrs.contains_key("[dual]"));
        // [growtype 1] 关掉初始段采集，其下的 hp 不会污染属性表；
        // 未转职成长恰好相反，它只认这一段的值。
        assert_eq!(base_growth_pass(&c, &attrs).unwrap()["[hp max]"], 10.0);
    }

    #[test]
    fn non_numeric_initial_skill_cell_refuses() {
        let c = vec![tag("[initial value]"), tag("[skill]"), tk(6, 0, "x")];
        assert!(initial_pass(&c).unwrap_err().contains("initial skill"));
    }

    #[test]
    fn advancement_growth_follows_growtype_and_awakening_switch() {
        let mut attrs: HashMap<String, f32> = HashMap::new();
        attrs.insert("[hp max]".to_string(), 200.0);
        attrs.insert("[mp max]".to_string(), 100.0);
        attrs.insert("[str]".to_string(), 30.0);
        let c = vec![
            tag("[growtype 2]"),
            tag("[hp max]"),
            tk(0, 9, ""),
            tag("[mp max]"),
            tk(0, 5, ""),
            tag("[awakening skill]"),
            tk(0, 100, ""),
            tag("[hp max]"),
            tk(0, 777, ""),
            tag("[growtype 3]"),
            tag("[hp max]"),
            tk(0, 7, ""),
            tag("[mp max]"),
            tk(0, 4, ""),
            tag("[str]"),
            tk(0, 2, ""),
        ];
        // 分支 1 = [growtype 2]：任何 [awakening 标签一露面就关掉本段
        let g = profession_growth(&c, &attrs, 1).unwrap();
        assert_eq!(g["[hp max]"], 9.0);
        assert_eq!(g["[mp max]"], 5.0);
        assert!(!g.contains_key("[str]"));
        // 分支 2 = [growtype 3]
        let g2 = profession_growth(&c, &attrs, 2).unwrap();
        assert_eq!((g2["[hp max]"], g2["[mp max]"], g2["[str]"]), (7.0, 4.0, 2.0));
        // 未出现的 growtype 一律拒绝（= 该分支不存在）
        assert!(profession_growth(&c, &attrs, 5).is_err());
        // 负数成长、非数值单元、重复键都拒绝
        let neg = vec![tag("[growtype 2]"), tag("[hp max]"), tk(0, -1, ""), tag("[mp max]"), tk(0, 1, "")];
        assert!(profession_growth(&neg, &attrs, 1).is_err());
        let txt = vec![tag("[growtype 2]"), tag("[hp max]"), tk(6, 0, "x"), tag("[mp max]"), tk(0, 1, "")];
        assert!(profession_growth(&txt, &attrs, 1).is_err());
        let dup = vec![tag("[growtype 2]"), tag("[hp max]"), tk(0, 1, ""), tag("[hp max]"), tk(0, 2, ""), tag("[mp max]"), tk(0, 1, "")];
        assert!(profession_growth(&dup, &attrs, 1).is_err());
        // 属性表里没有的段落不进成长表（Go 只投影初始属性认识的键）
        let unknown = vec![tag("[growtype 2]"), tag("[hp max]"), tk(0, 1, ""), tag("[mp max]"), tk(0, 1, ""), tag("[spirit]"), tk(0, 9, "")];
        assert!(!profession_growth(&unknown, &attrs, 1).unwrap().contains_key("[spirit]"));
    }

    #[test]
    fn awakening_grants_need_growtype_then_stage() {
        let c = vec![
            tag("[awakening 1]"),
            tag("[awakening skill]"),
            tk(0, 10, ""),
            tag("[growtype 1]"),
            tag("[awakening 1]"),
            tag("[awakening skill]"),
            tk(0, 11, ""),
            tk(0, 1, ""),
            tag("[awakening 2]"),
            tag("[awakening skill]"),
            tk(0, 12, ""),
            tk(0, 2, ""),
            tag("[/awakening skill]"),
            tag("[growtype 3]"),
            tag("[awakening 1]"),
            tag("[awakening skill]"),
            tk(0, 13, ""),
            tk(0, 1, ""),
            tag("[awakening 4]"),
            tag("[awakening skill]"),
            tk(0, 14, ""),
            tk(0, 1, ""),
        ];
        let g = awakening_grants(&c);
        // [growtype 1] 之前的授予（grow=0）与第 4 阶段都被拒
        assert_eq!(g.len(), 2);
        assert_eq!(g[&0][&1], vec![11, 1]);
        assert_eq!(g[&0][&2], vec![12, 2]);
        assert_eq!(g[&2][&1], vec![13, 1]);
        let all: Vec<i32> = g.values().flat_map(|s| s.values()).flatten().copied().collect();
        assert!(!all.contains(&10) && !all.contains(&14));
    }

    #[test]
    fn required_levels_only_first_section_and_numeric() {
        let c = vec![
            tag("[type]"),
            tk(6, 0, "[active]"),
            tag("[required level]"),
            tk(0, 48, ""),
            tag("[required level]"),
            tk(0, 60, ""),
        ];
        assert_eq!(required_levels(&c), vec![48]);
        // 说明块内的等级不采集（open 非空 ⇒ enabled 关掉）
        let nested = vec![
            tag("[vp explain]"),
            tag("[required level]"),
            tk(0, 48, ""),
            tag("[/vp explain]"),
            tag("[required level]"),
            tk(0, 50, ""),
        ];
        assert_eq!(required_levels(&nested), vec![50]);
        // [variation point] 整块吞掉，包括它自己的 required level
        let varied = vec![
            tag("[variation point]"),
            tag("[required level]"),
            tk(0, 40, ""),
            tag("[/variation point]"),
            tag("[required level]"),
            tk(0, 45, ""),
        ];
        assert_eq!(required_levels(&varied), vec![45]);
        // 段落里混进非数值单元 ⇒ Go 的 Ints 返回 nil ⇒ 这里返回空
        let bad = vec![tag("[required level]"), tk(0, 48, ""), tk(6, 0, "x")];
        assert!(required_levels(&bad).is_empty());
    }

    #[test]
    fn sp_table_section_is_name_exact() {
        let c = vec![tag("[sp table]"), tk(0, 1, ""), tk(0, 3, ""), tag("[/sp table]"), tk(0, 9, ""), tk(0, 9, "")];
        assert_eq!(section_cells(&c, "[sp table]").len(), 2);
        assert_eq!(skill_points(&section_cells(&c, "[sp table]")).unwrap()[&1], 3);
    }
}
