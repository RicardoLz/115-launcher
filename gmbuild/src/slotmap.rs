// 与 Go gmtool buildSlotMapFiles（icon.go）+ 旁表信封（names.go）对齐的
// equipment.slots.json / equipment.jobs.json / item.rarity.json 构建器。
// 段落提取一律作用于 ReadText 解码后的脚本文本，规则逐字复刻：
// sectionLines 只看第一个标签、往后最多 5 行；sectionToken(s) 取反引号成对 token。

use crate::archive::Archive;
use crate::gojson::escape_string;
use crate::itemindex::Entry;
use std::collections::HashMap;
use std::io::Write;

fn first_int(s: &str) -> Option<i64> {
    for f in s.split_whitespace() {
        if let Ok(n) = f.parse::<i64>() {
            return Some(n);
        }
    }
    None
}

/// Go sectionLines：第一个 trim 后恰等于 section 的行之后、最多 5 行内的非空值行。
fn section_lines(text: &str, section: &str) -> Vec<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    for i in 0..lines.len() {
        if lines[i].trim() != section {
            continue;
        }
        let mut out = Vec::new();
        let mut j = i + 1;
        while j < lines.len() && j <= i + 5 {
            let trimmed = lines[j].trim();
            j += 1;
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                break;
            }
            out.push(trimmed.to_string());
        }
        return out;
    }
    Vec::new()
}

/// Go sectionTokens：段内全部反引号成对 token。
fn section_tokens(text: &str, section: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in section_lines(text, section) {
        let mut line = line;
        loop {
            let a = match line.find('`') {
                Some(a) => a,
                None => break,
            };
            let b = match line[a + 1..].find('`') {
                Some(b) => b,
                None => break,
            };
            out.push(line[a + 1..a + 1 + b].trim().to_string());
            line = line[a + 1 + b + 1..].to_string();
        }
    }
    out
}

fn section_token(text: &str, section: &str) -> String {
    section_tokens(text, section)
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// Go stripBackticks：成对删除反引号内容，用单个空格回填；落单则截断。
fn strip_backticks(line: &str) -> String {
    let mut line = line.to_string();
    loop {
        let a = match line.find('`') {
            Some(a) => a,
            None => return line,
        };
        let b = match line[a + 1..].find('`') {
            Some(b) => b,
            None => return line[..a].to_string(),
        };
        line = format!("{} {}", &line[..a], &line[a + 1 + b + 1..]);
    }
}

/// Go sectionIntOK：段内第一个反引号外整数；返回是否取到（品质 0 合法）。
fn section_int_ok(text: &str, section: &str) -> Option<i64> {
    for line in section_lines(text, section) {
        if let Some(n) = first_int(&strip_backticks(&line)) {
            return Some(n);
        }
    }
    None
}

fn section_int(text: &str, section: &str) -> i64 {
    section_int_ok(text, section).unwrap_or(0)
}

#[derive(Clone, Debug, PartialEq)]
pub struct SlotEntry {
    pub cell: String,
    pub level: i32,
}

/// Go loadSlotMap：文件缺失/非法 JSON 都回落空表。
pub fn load_slot_map(path: &std::path::Path) -> HashMap<String, SlotEntry> {
    let mut out = HashMap::new();
    let Ok(b) = std::fs::read(path) else { return out };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&String::from_utf8_lossy(&b)) else {
        eprintln!("警告：装备部位表 {} 不是合法 JSON，本次只用装备目录", path.display());
        return out;
    };
    if let Some(map) = v.get("slots").and_then(|s| s.as_object()) {
        for (k, e) in map {
            let cell = e.get("cell").and_then(|c| c.as_str()).unwrap_or_default().to_string();
            let level = e.get("level").and_then(|l| l.as_i64()).unwrap_or(0) as i32;
            out.insert(k.clone(), SlotEntry { cell, level });
        }
    }
    out
}

pub struct SlotStats {
    pub equipment: usize,
    pub read: usize,
    pub no_cell: usize,
    pub kept: usize,
    pub limited: usize,
    pub unrestricted: usize,
    pub rarity_ok: usize,
    pub no_rarity: usize,
    pub read_failed: usize,
}

pub struct Maps {
    pub slots: HashMap<String, SlotEntry>,
    pub jobs: HashMap<String, Vec<String>>,
    pub rarity: HashMap<String, i32>,
    pub stats: SlotStats,
}

/// Go buildSlotMapFiles 的遍历主体：全量物品扫 [rarity]，装备再扫 [usable job]/[equipment type]。
/// 装备口径与产物一致：avatar 计入 equipment。
pub fn build_slot_maps(a: &Archive, index: &HashMap<u32, Entry>) -> Result<Maps, String> {
    let mut ids: Vec<u32> = index.keys().copied().collect();
    ids.sort_unstable();
    let mut slots: HashMap<String, SlotEntry> = HashMap::with_capacity(512 * 1024);
    let mut jobs: HashMap<String, Vec<String>> = HashMap::new();
    let mut rarity: HashMap<String, i32> = HashMap::with_capacity(512 * 1024);
    let mut st = SlotStats {
        equipment: 0, read: 0, no_cell: 0, kept: 0,
        limited: 0, unrestricted: 0, rarity_ok: 0, no_rarity: 0, read_failed: 0,
    };
    for (n, id) in ids.iter().enumerate() {
        let entry = &index[id];
        let resolved = crate::itemindex::resolve_script_path(a, &entry.path);
        let text = match a.read_text(&resolved) {
            Ok(t) => t,
            Err(_) => {
                st.read_failed += 1;
                continue;
            }
        };
        if let Some(v) = section_int_ok(&text, "[rarity]") {
            rarity.insert(id.to_string(), v as i32);
            st.rarity_ok += 1;
        } else {
            st.no_rarity += 1;
        }
        if entry.kind == "stackable" {
            continue;
        }
        st.equipment += 1;
        st.read += 1;
        let usable = section_tokens(&text, "[usable job]");
        let generic = usable.is_empty() || usable.iter().any(|u| u == "[all]");
        if generic {
            st.unrestricted += 1;
        } else {
            jobs.insert(id.to_string(), usable);
            st.limited += 1;
        }
        let cell = section_token(&text, "[equipment type]");
        if cell.is_empty() {
            st.no_cell += 1;
            continue;
        }
        let level = section_int(&text, "[minimum level]") as i32;
        slots.insert(id.to_string(), SlotEntry { cell, level });
        if n % 2000 == 1999 {
            a.release_read_caches();
        }
        if n % 50_000 == 49_999 {
            eprintln!("  旁表进度 {}/{}/read={}", n + 1, ids.len(), st.read);
        }
    }
    Ok(Maps { slots, jobs, rarity, stats: st })
}

fn sorted_keys(keys: Vec<String>) -> Vec<String> {
    let mut keys = keys;
    keys.sort_by(|a, b| match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        _ => a.cmp(b),
    });
    keys
}

/// 现产物的键序是数值升序（不是 Go map 的字节序），照此写出。
pub fn write_slots_file(
    slots: &HashMap<String, SlotEntry>,
    source: &str,
    built_at: &str,
    out: &mut dyn Write,
) -> Result<usize, String> {
    let keys = sorted_keys(slots.keys().cloned().collect());
    let mut buf: Vec<u8> = Vec::with_capacity(1 << 20);
    buf.extend_from_slice(b"{\"source\":");
    escape_string(&mut buf, source);
    buf.extend_from_slice(b",\"built_at\":");
    escape_string(&mut buf, built_at);
    buf.extend_from_slice(b",\"slots\":{");
    for (i, k) in keys.iter().enumerate() {
        if i > 0 {
            buf.push(b',');
        }
        escape_string(&mut buf, k);
        buf.extend_from_slice(b":{\"cell\":");
        escape_string(&mut buf, &slots[k].cell);
        buf.extend_from_slice(b",\"level\":");
        buf.extend_from_slice(slots[k].level.to_string().as_bytes());
        buf.push(b'}');
        if buf.len() >= 1 << 20 {
            out.write_all(&buf).map_err(|e| e.to_string())?;
            buf.clear();
        }
    }
    buf.extend_from_slice(b"}}");
    out.write_all(&buf).map_err(|e| e.to_string())?;
    Ok(keys.len())
}

pub fn write_jobs_file(
    jobs: &HashMap<String, Vec<String>>,
    all: usize,
    source: &str,
    built_at: &str,
    out: &mut dyn Write,
) -> Result<usize, String> {
    let keys = sorted_keys(jobs.keys().cloned().collect());
    let mut buf: Vec<u8> = Vec::with_capacity(1 << 20);
    buf.extend_from_slice(b"{\"source\":");
    escape_string(&mut buf, source);
    buf.extend_from_slice(b",\"built_at\":");
    escape_string(&mut buf, built_at);
    buf.extend_from_slice(format!(",\"all\":{all},\"jobs\":{{").as_bytes());
    for (i, k) in keys.iter().enumerate() {
        if i > 0 {
            buf.push(b',');
        }
        escape_string(&mut buf, k);
        buf.extend_from_slice(b":[");
        for (j, token) in jobs[k].iter().enumerate() {
            if j > 0 {
                buf.push(b',');
            }
            escape_string(&mut buf, token);
        }
        buf.push(b']');
    }
    buf.extend_from_slice(b"}}");
    out.write_all(&buf).map_err(|e| e.to_string())?;
    Ok(keys.len())
}

pub fn write_rarity_file(
    rarity: &HashMap<String, i32>,
    source: &str,
    built_at: &str,
    out: &mut dyn Write,
) -> Result<usize, String> {
    let keys = sorted_keys(rarity.keys().cloned().collect());
    let mut buf: Vec<u8> = Vec::with_capacity(1 << 20);
    buf.extend_from_slice(b"{\"source\":");
    escape_string(&mut buf, source);
    buf.extend_from_slice(b",\"built_at\":");
    escape_string(&mut buf, built_at);
    buf.extend_from_slice(b",\"rarity\":{");
    for (i, k) in keys.iter().enumerate() {
        if i > 0 {
            buf.push(b',');
        }
        escape_string(&mut buf, k);
        buf.push(b':');
        buf.extend_from_slice(rarity[k].to_string().as_bytes());
        if buf.len() >= 1 << 20 {
            out.write_all(&buf).map_err(|e| e.to_string())?;
            buf.clear();
        }
    }
    buf.extend_from_slice(b"}}");
    out.write_all(&buf).map_err(|e| e.to_string())?;
    Ok(keys.len())
}

pub fn now_rfc3339() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_lines_five_line_cap() {
        // 第 6 行之后的值不属于本段（Go j<=i+5 上限）
        let text = "[a]\n1\n2\n3\n4\n5\n[b]\n9";
        assert_eq!(section_lines(text, "[a]"), vec!["1", "2", "3", "4", "5"]);
    }

    #[test]
    fn section_tokens_pairs() {
        let text = "[usable job]\n`[swordman]` `[fighter]`\n[/usable job]";
        assert_eq!(section_tokens(text, "[usable job]"), vec!["[swordman]", "[fighter]"]);
    }

    #[test]
    fn int_ok_skips_backticks() {
        let text = "[rarity]\n`[epic]` 4\n";
        assert_eq!(section_int_ok(text, "[rarity]"), Some(4));
        let zero = "[rarity]\n0\n";
        assert_eq!(section_int_ok(zero, "[rarity]"), Some(0));
        assert_eq!(section_int_ok("[rarity]\n`[epic]`\n", "[rarity]"), None);
        assert_eq!(section_int_ok("[none]\n5\n", "[rarity]"), None);
    }

    #[test]
    fn strip_backticks_shapes() {
        assert_eq!(strip_backticks("`x` 5"), "  5");
        assert_eq!(strip_backticks("a`b"), "a");
        assert_eq!(strip_backticks("`a`b`c`"), " b ");
    }

    #[test]
    fn first_int_parses_signed() {
        assert_eq!(first_int("ab -12 cd"), Some(-12));
        assert_eq!(first_int("x y"), None);
    }
}
