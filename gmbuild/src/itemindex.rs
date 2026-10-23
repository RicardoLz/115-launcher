// 与 Go 服务端树 internal/catalog/{item_index,script,item_display,pvf/nameref}.go 对齐的
// items.index.json 构建器。字段推导取被删的 cmd/itemindeximport 的最近亲本
// catalog.VisitItemDisplay + gmtool/pvf.go 的 avatar→equipment 折叠。

use crate::archive::Archive;
use crate::gojson::escape_string;
use crate::path::{join_archive_path, normalize_path};
use std::collections::HashMap;
use std::io::Write;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub ttype: u8,
    pub value: i32,
    pub text: String,
    pub reference: String,
}

pub fn tokens_from_raw(a: &Archive, raw: &[u8]) -> Result<Vec<Token>, String> {
    if raw.len() % 5 != 0 {
        return Err("incomplete script cell".into());
    }
    let mut out = Vec::with_capacity(raw.len() / 5);
    for cell in raw.chunks_exact(5) {
        let ttype = cell[0];
        let value = i32::from_le_bytes(cell[1..5].try_into().unwrap());
        let mut t = Token { ttype, value, text: String::new(), reference: String::new() };
        match ttype {
            3 | 6 => t.text = a.resolve_string(value as isize),
            8 => t.reference = a.resolve_string(value as isize),
            _ => {}
        }
        out.push(t);
    }
    Ok(out)
}

fn section_cells(cells: &[Token], name: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut active = false;
    for (i, c) in cells.iter().enumerate() {
        if c.ttype == 3 {
            active = c.text == name;
            continue;
        }
        if active {
            out.push(i);
        }
    }
    out
}

/// Go lootInt：段落必须恰好一个数值单元。
fn loot_int(cells: &[Token], name: &str) -> Option<i32> {
    let a = section_cells(cells, name);
    if a.len() == 1 && cells[a[0]].ttype == 0 {
        Some(cells[a[0]].value)
    } else {
        None
    }
}

/// Go itemDisplayScalar：第一个同名 Type3 标签，下一单元必须是 Type0，否则放弃。
fn item_display_scalar(cells: &[Token], name: &str) -> Option<i32> {
    for (i, c) in cells.iter().enumerate() {
        if c.ttype == 3 && c.text == name {
            if i + 1 < cells.len() && cells[i + 1].ttype == 0 {
                return Some(cells[i + 1].value);
            }
            return None;
        }
    }
    None
}

/// Go ResolveScriptPath：原样命中即用，ANI/MOB/OBJ 不回落，其余前置 (r)。
pub fn resolve_script_path(a: &Archive, name: &str) -> String {
    let name = name.replace('\\', "/").to_lowercase();
    if a.find_file_index(&name).is_some() {
        return name;
    }
    for ext in [".ani", ".mob", ".obj"] {
        if name.contains(ext) {
            return name;
        }
    }
    let dir = match name.rfind('/') {
        Some(i) => &name[..i],
        None => "",
    };
    let base = match name.rfind('/') {
        Some(i) => &name[i + 1..],
        None => name.as_str(),
    };
    join_archive_path(dir, &format!("(r){base}"))
}

/// Go ReadScript：返回 (落定路径, 单元表)，dataType 必须为 1。
pub fn read_script(a: &Archive, name: &str) -> Result<(String, Vec<Token>), String> {
    let name = name.replace('\\', "/").to_lowercase();
    let idx = a
        .find_file_index(&name)
        .ok_or_else(|| format!("pvf file not found: {name}"))?;
    if a.file_at(idx).data_type != 1 {
        return Err(format!("entry {name} is not a script"));
    }
    let raw = a.read_raw_idx(idx)?;
    let cells = tokens_from_raw(a, &raw)?;
    Ok((name, cells))
}

pub fn resolve_script(a: &Archive, name: &str) -> Result<(String, Vec<Token>), String> {
    let resolved = resolve_script_path(a, name);
    read_script(a, &resolved)
}

/// Go ParseIndex：数值/文本成对，id 非负、不重复，路径小写、`\`→`/`。
pub fn parse_index(cells: &[Token]) -> Result<Vec<(u32, String)>, String> {
    if cells.len() % 2 != 0 {
        return Err("unpaired source index".into());
    }
    let mut rows = Vec::with_capacity(cells.len() / 2);
    let mut seen: HashMap<u32, ()> = HashMap::with_capacity(cells.len() / 2);
    for pair in cells.chunks_exact(2) {
        if pair[0].ttype != 0 || pair[0].value < 0 || pair[1].ttype != 6 || pair[1].text.is_empty() {
            return Err(format!("invalid source index at cell {}", rows.len() * 2));
        }
        let id = pair[0].value as u32;
        if seen.insert(id, ()).is_some() {
            return Err(format!("duplicate source index {id}"));
        }
        rows.push((id, pair[1].text.replace('\\', "/").to_lowercase()));
    }
    Ok(rows)
}

#[derive(Clone)]
pub struct Entry {
    pub id: u32,
    pub path: String,
    pub kind: String,
    pub stackable_type: String,
    pub stack_limit: u32,
}

/// Go ImportItemIndex（服务端树版）：两表撞 id 直接报错，堆叠物就地取类型与上限。
pub fn import_item_index(a: &Archive) -> Result<HashMap<u32, Entry>, String> {
    let mut items: HashMap<u32, Entry> = HashMap::with_capacity(512 * 1024);
    for kind in ["equipment", "stackable"] {
        let (_, cells) = resolve_script(a, &format!("list/{kind}.lst"))?;
        let rows = parse_index(&cells)?;
        for (i, (id, row_path)) in rows.iter().enumerate() {
            if items.contains_key(id) {
                return Err(format!("item ID {id} occurs in both source lists"));
            }
            let mut path = row_path.clone();
            if !path.starts_with(&format!("{kind}/")) {
                path = join_archive_path(kind, &path);
            }
            let mut k = kind.to_string();
            if kind == "equipment" && (path.contains("/avatar/") || path.contains("/at_avatar/")) {
                k = "avatar".into();
            }
            let mut entry = Entry { id: *id, path, kind: k, stackable_type: String::new(), stack_limit: 0 };
            if kind == "stackable" {
                let (_, sc) = resolve_script(a, &entry.path)
                    .map_err(|e| format!("item {}: {e}", entry.id))?;
                let types = section_cells(&sc, "[stackable type]");
                if !types.is_empty() {
                    entry.stackable_type = sc[types[0]].text.clone();
                }
                if let Some(n) = loot_int(&sc, "[stack limit]") {
                    if n > 0 {
                        entry.stack_limit = n as u32;
                    }
                }
            }
            items.insert(entry.id, entry);
            if i % 4096 == 4095 {
                a.release_read_caches();
            }
        }
    }
    if items.is_empty() {
        return Err("当前 PVF 物品索引为空".into());
    }
    Ok(items)
}

/// Go StringTables：list/n_string.lst 的 序号→表路径。
fn string_tables(a: &Archive) -> Result<HashMap<i32, String>, String> {
    let (_, cells) = read_script(a, "list/n_string.lst")?;
    let mut out = HashMap::new();
    let mut pending: i32 = -1;
    for t in &cells {
        if !t.text.is_empty() {
            if pending >= 0 {
                out.insert(pending, normalize_path(&t.text));
                pending = -1;
            }
            continue;
        }
        pending = t.value;
    }
    Ok(out)
}

/// Go parseDisplayTable：行 `key>value`，key 去空白、先出现者优先、值原样不裁剪。
fn parse_display_table(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::with_capacity(64 * 1024);
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.starts_with("//") {
            continue;
        }
        let i = match line.find('>') {
            Some(i) => i,
            None => continue,
        };
        let key = line[..i].trim();
        if key.is_empty() {
            continue;
        }
        out.entry(key.to_string()).or_insert_with(|| line[i + 1..].to_string());
    }
    out
}

/// Go ParseLocalizedRef：`<N::key>`。
fn parse_localized_ref(s: &str) -> Option<(i32, String)> {
    let t = s.trim();
    if t.len() < 5 || !t.starts_with('<') || !t.ends_with('>') {
        return None;
    }
    let body = &t[1..t.len() - 1];
    let i = body.find("::")?;
    if i <= 0 {
        return None;
    }
    let n: i32 = body[..i].trim().parse().ok()?;
    let key = body[i + 2..].trim();
    if key.is_empty() {
        return None;
    }
    Some((n, key.to_string()))
}

/// Go ItemNameRef 的 raw 取值：文本→引用→字符串池。
fn name_raw(a: &Archive, cells: &[Token]) -> Option<String> {
    for i in 0..cells.len().saturating_sub(1) {
        if cells[i].text != "[name]" {
            continue;
        }
        let v = &cells[i + 1];
        let mut raw = v.text.clone();
        if raw.is_empty() {
            raw = v.reference.clone();
        }
        if raw.is_empty() && v.ttype != 0 && v.ttype != 2 {
            raw = a.resolve_string(v.value as isize);
        }
        return Some(raw);
    }
    None
}

pub struct ItemRow {
    pub id: u32,
    pub name: String,
    pub kind: String,
    pub grade: i32,
    pub rarity: i32,
    pub stack_type: String,
    pub stack_limit: u32,
    pub durability: u32,
}

fn write_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(v.to_string().as_bytes());
}
fn write_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(v.to_string().as_bytes());
}

/// 与 Go json.Marshal(IndexItem) 逐字节一致：固定 key 序 + omitempty 三件套。
fn write_item(out: &mut Vec<u8>, it: &ItemRow) {
    out.push(b'{');
    out.extend_from_slice(b"\"id\":");
    write_u32(out, it.id);
    out.extend_from_slice(b",\"name\":");
    escape_string(out, &it.name);
    out.extend_from_slice(b",\"kind\":");
    escape_string(out, &it.kind);
    out.extend_from_slice(b",\"grade\":");
    write_i32(out, it.grade);
    out.extend_from_slice(b",\"rarity\":");
    write_i32(out, it.rarity);
    if !it.stack_type.is_empty() {
        out.extend_from_slice(b",\"stack_type\":");
        escape_string(out, &it.stack_type);
    }
    if it.stack_limit != 0 {
        out.extend_from_slice(b",\"stack_limit\":");
        write_u32(out, it.stack_limit);
    }
    if it.durability != 0 {
        out.extend_from_slice(b",\"durability\":");
        write_u32(out, it.durability);
    }
    out.push(b'}');
}

/// Go VisitItemDisplay + 落盘：count 在前、items 流式、source（PVF sha256）在最后。
pub fn build_items(a: &Archive, out: &mut dyn Write) -> Result<usize, String> {
    let index = import_item_index(a)?;
    let mut ids: Vec<u32> = index.keys().copied().collect();
    ids.sort_unstable();
    let tables = string_tables(a)?;
    let mut text_tables: HashMap<i32, HashMap<String, String>> = HashMap::new();

    let head = format!("{{\"count\":{},\"items\":[", ids.len());
    out.write_all(head.as_bytes()).map_err(|e| e.to_string())?;

    let mut buf: Vec<u8> = Vec::with_capacity(1 << 16);
    for (n, id) in ids.iter().enumerate() {
        let entry = &index[id];
        let (script_path, cells) = resolve_script(a, &entry.path)
            .map_err(|e| format!("item display {id}: {e}"))?;
        let grade = item_display_scalar(&cells, "[grade]").unwrap_or(0);
        let rarity = item_display_scalar(&cells, "[rarity]").unwrap_or(0);
        let durability = item_display_scalar(&cells, "[durability]").unwrap_or(0) as u32;
        let raw = name_raw(a, &cells);
        let mut name = String::new();
        if let Some(raw) = raw {
            name = raw.clone();
            if let Some((table, key)) = parse_localized_ref(&raw) {
                if !text_tables.contains_key(&table) {
                    let path = tables
                        .get(&table)
                        .ok_or_else(|| format!("item {id} references unknown string table {table}"))?;
                    let text = a
                        .read_text(path)
                        .map_err(|e| format!("item {id} 文本表 {path}：{e}"))?;
                    text_tables.insert(table, parse_display_table(&text));
                }
                if let Some(v) = text_tables[&table].get(&key) {
                    name = v.clone();
                }
            }
        }
        let kind = if entry.kind == "avatar" { "equipment".to_string() } else { entry.kind.clone() };
        let row = ItemRow {
            id: *id,
            name,
            kind,
            grade,
            rarity,
            stack_type: entry.stackable_type.clone(),
            stack_limit: entry.stack_limit,
            durability,
        };
        if n > 0 {
            buf.push(b',');
        }
        write_item(&mut buf, &row);
        if buf.len() >= 1 << 20 {
            out.write_all(&buf).map_err(|e| e.to_string())?;
            buf.clear();
        }
        if n % 512 == 511 {
            a.release_read_caches();
        }
        if n % 50_000 == 49_999 {
            eprintln!("  items.index 进度 {}/{}/{script_path}", n + 1, ids.len());
        }
    }
    let tail = format!("],\"source\":\"{}\"}}", a.checksum);
    buf.extend_from_slice(tail.as_bytes());
    out.write_all(&buf).map_err(|e| e.to_string())?;
    Ok(ids.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tk(ttype: u8, value: i32, text: &str) -> Token {
        Token { ttype, value, text: text.to_string(), reference: String::new() }
    }

    #[test]
    fn scalar_first_label() {
        let cells = vec![
            tk(3, 0, "[grade]"),
            tk(3, 0, "[/grade]"),
            tk(3, 0, "[grade]"),
            tk(0, 7, ""),
        ];
        // 第一个 [grade] 后面不是数值 ⇒ 放弃，绝不落到第二个
        assert_eq!(item_display_scalar(&cells, "[grade]"), None);
    }

    #[test]
    fn localized_refs() {
        assert_eq!(parse_localized_ref("<13::name_10362480>"), Some((13, "name_10362480".into())));
        assert_eq!(parse_localized_ref("<::x>"), None);
        assert_eq!(parse_localized_ref("<3 :: name_1>"), Some((3, "name_1".into())));
        assert_eq!(parse_localized_ref("abc"), None);
    }

    #[test]
    fn display_table_first_wins() {
        let t = parse_display_table("a>x\n//c\n a>y\nb>z");
        assert_eq!(t["a"], "x");
        assert_eq!(t["b"], "z");
        assert!(!t.contains_key("c"));
    }

    #[test]
    fn loot_int_requires_single_numeric() {
        let cells = vec![tk(3, 0, "[stack limit]"), tk(0, 400, ""), tk(3, 0, "[/stack limit]")];
        assert_eq!(loot_int(&cells, "[stack limit]"), Some(400));
        let two = vec![tk(3, 0, "[stack limit]"), tk(0, 1, ""), tk(0, 2, "")];
        assert_eq!(loot_int(&two, "[stack limit]"), None);
    }
}
