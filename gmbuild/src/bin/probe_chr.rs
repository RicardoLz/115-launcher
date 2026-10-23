// 一次性探针：用当前客户端 PVF 重放每个职业 .chr 的原文 SHA-256，
// 与 characters.next25.json 的 raw_sha256 逐条对拍，确认成长产物能否对上真库存档。
use gmbuild::itemindex;
use sha2::Digest;

fn sha256_hex(data: &[u8]) -> String {
    let mut h = sha2::Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn main() {
    let pvf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| String::from(r"D:\115us\客户端\DFO\Script.pvf"));
    let artifact = std::env::args()
        .nth(2)
        .unwrap_or_else(|| String::from(r"D:\115us\源码\115US当前源码\gm-tool\configs\characters.next25.json"));
    let a = gmbuild::open_gm_archive(std::path::Path::new(&pvf)).unwrap();
    println!("pvf checksum={} files={}", a.checksum, a.all_files().len());
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&artifact).unwrap()).unwrap();
    let profs = raw["professions"].as_object().unwrap();
    let (_, cells) = itemindex::resolve_script(&a, "list/character.lst").unwrap();
    let rows = itemindex::parse_index(&cells).unwrap();
    let mut same = 0usize;
    let mut diff = 0usize;
    for (id, path) in &rows {
        // Go ImportCharacters 用的是索引里的原路径，不做 (r) 回落。
        let resolved = path.clone();
        let Some(idx) = a.find_file_index(&resolved) else {
            diff += 1;
            println!("MISS {id:>3} {resolved}");
            continue;
        };
        let bytes = a.read_raw_idx(idx).unwrap();
        let hex = sha256_hex(&bytes);
        let expect = profs.get(&id.to_string()).and_then(|p| p["raw_sha256"].as_str()).unwrap_or("");
        let verdict = if hex == expect {
            same += 1;
            "SAME"
        } else {
            diff += 1;
            "DIFF"
        };
        println!("{verdict} {id:>3} {resolved} {} n={expect}", hex);
    }
    println!("一致 {same}，不一致 {diff}，索引 {}/产物 {}", rows.len(), profs.len());
}
