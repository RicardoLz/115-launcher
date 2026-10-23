// 与 Go cmd/gmtool/names.go 的 readClientNames + buildClientNames 对齐。

use crate::archive::Archive;
use crate::gojson::escape_string;
use std::collections::HashMap;
use std::io::Write;

/// 客户端文本表按优先级排列：先出现的表优先，避免同名 key 被别的域覆盖。
const CLIENT_NAME_TABLES: [&str; 11] = [
    "string/equipment.uv.str",
    "string/stackable.uv.str",
    "string/setequipment.uv.str",
    "string/creature.uv.str",
    "string/pet.uv.str",
    "string/skin.uv.str",
    "string/cos.uv.str",
    "string/itemshop.uv.str",
    "string/etc.uv.str",
    "string/ui.uv.str",
    "string/character.uv.str",
];

const WANTED_PREFIXES: [&str; 3] = ["name_", "growtype_name_", "common_rarity_"];

fn wanted(key: &str) -> bool {
    WANTED_PREFIXES.iter().any(|p| key.starts_with(p))
}

pub fn read_client_names(a: &Archive) -> HashMap<String, String> {
    let mut names: HashMap<String, String> = HashMap::with_capacity(512 * 1024);
    let mut read = 0usize;
    for table in CLIENT_NAME_TABLES {
        let text = match a.read_text(table) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("提示：客户端文本表 {table} 不可读（{e}），跳过");
                continue;
            }
        };
        read += 1;
        let n = collect_table_lines(&text, &mut names);
        eprintln!("  客户端文本表 {table:<28} 新增 {n} 条");
    }
    if read == 0 {
        return HashMap::new();
    }
    names
}

/// 一张文本表并入 names（先出现的键优先），返回新增条数。
fn collect_table_lines(text: &str, names: &mut HashMap<String, String>) -> usize {
    let mut n = 0usize;
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let i = match line.find('>') {
            Some(i) if i > 0 => i,
            _ => continue,
        };
        let key = line[..i].trim();
        let val = &line[i + 1..];
        if key.is_empty() || val.is_empty() || !wanted(key) {
            continue;
        }
        if names.contains_key(key) {
            continue; // 先出现的表优先
        }
        names.insert(key.to_string(), val.to_string());
        n += 1;
    }
    n
}

/// 输出格式与 Go buildClientNames 完全一致：扁平 map、key 升序、无缩进、无末尾换行。
pub fn write_client_names(names: &HashMap<String, String>, out: &mut dyn Write) -> Result<usize, String> {
    let mut keys: Vec<&String> = names.keys().collect();
    keys.sort();
    let mut buf: Vec<u8> = Vec::with_capacity(1 << 20);
    buf.push(b'{');
    for (i, k) in keys.iter().enumerate() {
        if i > 0 {
            buf.push(b',');
        }
        escape_string(&mut buf, k);
        buf.push(b':');
        escape_string(&mut buf, &names[*k]);
        if buf.len() >= 1 << 20 {
            out.write_all(&buf).map_err(|e| e.to_string())?;
            buf.clear();
        }
    }
    buf.push(b'}');
    out.write_all(&buf).map_err(|e| e.to_string())?;
    Ok(keys.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(bytes: &[u8]) -> String {
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn prefix_filter_matches_go() {
        assert!(wanted("name_101001153"));
        assert!(wanted("growtype_name_0"));
        assert!(wanted("common_rarity_4"));
        assert!(!wanted("skill_1"));
        assert!(!wanted("message_2"));
    }

    #[test]
    fn line_parsing_rules() {
        // key 前段 TrimSpace、值原样保留（不 TrimRight 空格）；行尾 CR 被去掉
        let text = s(&[0x20, 0x6e, 0x61, 0x6d, 0x65, 0x5f, 0x31, 0x20, 0x3e, 0x41, 0x20, 0x20, 0x42, 0x0a, 0x2f, 0x2f, 0x63, 0x6f, 0x6d, 0x6d, 0x65, 0x6e, 0x74, 0x0a, 0x0a, 0x6e, 0x61, 0x6d, 0x65, 0x5f, 0x32, 0x3e, 0x78, 0x20, 0x20, 0x0a, 0x6e, 0x61, 0x6d, 0x65, 0x5f, 0x33, 0x3e, 0x61, 0x0d, 0x62]);
        let mut names = HashMap::new();
        let n = collect_table_lines(&text, &mut names);
        assert_eq!(n, 3);
        assert_eq!(*names.get("name_1").unwrap(), s(&[0x41, 0x20, 0x20, 0x42]));
        assert_eq!(*names.get("name_2").unwrap(), s(&[0x78, 0x20, 0x20]));
        assert_eq!(*names.get("name_3").unwrap(), s(&[97,13,98]));
        assert!(!names.contains_key("comment"));
    }

    #[test]
    fn first_table_wins() {
        let mut names = HashMap::new();
        collect_table_lines("name_1>first", &mut names);
        let second = s(&[0x6e, 0x61, 0x6d, 0x65, 0x5f, 0x31, 0x3e, 0x73, 0x65, 0x63, 0x6f, 0x6e, 0x64, 0x0a, 0x6e, 0x61, 0x6d, 0x65, 0x5f, 0x32, 0x3e, 0x6f, 0x74, 0x68, 0x65, 0x72]);
        let n = collect_table_lines(&second, &mut names);
        assert_eq!(n, 1);
        assert_eq!(names.get("name_1").unwrap(), "first");
    }

    #[test]
    fn writer_sorts_keys_and_escapes_html() {
        let mut names = HashMap::new();
        names.insert("name_2".to_string(), "b>c".to_string());
        names.insert("name_1".to_string(), "a&b".to_string());
        let mut out = Vec::new();
        let n = write_client_names(&names, &mut out).unwrap();
        assert_eq!(n, 2);
        assert_eq!(s(&out), s(&[0x7b, 0x22, 0x6e, 0x61, 0x6d, 0x65, 0x5f, 0x31, 0x22, 0x3a, 0x22, 0x61, 0x5c, 0x75, 0x30, 0x30, 0x32, 0x36, 0x62, 0x22, 0x2c, 0x22, 0x6e, 0x61, 0x6d, 0x65, 0x5f, 0x32, 0x22, 0x3a, 0x22, 0x62, 0x5c, 0x75, 0x30, 0x30, 0x33, 0x65, 0x63, 0x22, 0x7d]));
    }
}
