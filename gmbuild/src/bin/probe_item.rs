// 一次性探针：打印指定物品脚本里 [stack limit] 等段落实况（只查两清单，不建全索引）。
use gmbuild::itemindex;

fn main() {
    let a = gmbuild::open_gm_archive(std::path::Path::new(r"D:\115us\客户端\DFO\Script.pvf")).unwrap();
    let mut want: Vec<u32> = std::env::args().skip(1).map(|s| s.parse().unwrap()).collect();
    want.sort_unstable();
    let mut found = 0usize;
    for kind in ["equipment", "stackable"] {
        let (_, cells) = itemindex::resolve_script(&a, &format!("list/{kind}.lst")).unwrap();
        for (id, p) in itemindex::parse_index(&cells).unwrap() {
            if want.binary_search(&id).is_err() {
                continue;
            }
            found += 1;
            let mut path = p;
            if !path.starts_with(&format!("{kind}/")) {
                path = gmbuild::path::join_archive_path(kind, &path);
            }
            let (sp, sc) = itemindex::resolve_script(&a, &path).unwrap();
            println!("{id} kind={kind} path={sp}");
            for name in ["[stack limit]", "[stackable type]", "[durability]"] {
                println!("  {name}: {}", section_dump(&sc, name));
            }
        }
    }
    eprintln!("命中 {found}/{}", want.len());
}

fn section_dump(cells: &[itemindex::Token], name: &str) -> String {
    let mut out = Vec::new();
    let mut active = false;
    for c in cells {
        if c.ttype == 3 {
            active = c.text == name;
            continue;
        }
        if active {
            out.push(format!(
                "t{} v{} \"{}\"",
                c.ttype,
                c.value,
                if c.text.is_empty() { &c.reference } else { &c.text }
            ));
        }
    }
    out.join(" | ")
}
