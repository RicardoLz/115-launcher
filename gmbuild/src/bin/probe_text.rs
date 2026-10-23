// 一次性探针：打印物品脚本 ReadText 里 [usable job]/[equipment type] 邻域原文。
use gmbuild::itemindex;

fn main() {
    let a = gmbuild::open_gm_archive(std::path::Path::new(r"D:\115us\客户端\DFO\Script.pvf")).unwrap();
    let mut lists: Vec<(String, Vec<(u32, String)>)> = Vec::new();
    for kind in ["equipment", "stackable"] {
        let (_, cells) = itemindex::resolve_script(&a, &format!("list/{kind}.lst")).unwrap();
        lists.push((kind.to_string(), itemindex::parse_index(&cells).unwrap()));
    }
    for arg in std::env::args().skip(1) {
        let id: u32 = arg.parse().unwrap();
        let mut hit = false;
        for (kind, rows) in &lists {
            if let Some((_, p)) = rows.iter().find(|(i, _)| *i == id) {
                let path = if p.starts_with(&format!("{kind}/")) { p.clone() } else { format!("{kind}/{p}") };
                let resolved = itemindex::resolve_script_path(&a, &path);
                let text = a.read_text(&resolved).unwrap();
                println!("=== {id} {resolved} ({} 字节) ===", text.len());
                let lines: Vec<&str> = text.split('\n').collect();
                for (j, ln) in lines.iter().enumerate() {
                    let t = ln.trim();
                    if t == "[usable job]" || t == "[equipment type]" || t == "[minimum level]" || t == "[rarity]" {
                        for k in j..(j + 6).min(lines.len()) {
                            println!("  [{k}] {:?}", lines[k]);
                        }
                        println!("  ---");
                    }
                }
                hit = true;
                break;
            }
        }
        if !hit {
            println!("{id}: 不在两清单");
        }
    }
}
