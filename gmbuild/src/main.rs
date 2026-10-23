use std::io::{BufWriter, Write};
use std::path::PathBuf;

const DEFAULT_PVF: &str = r"D:\115us\客户端\DFO\Script.pvf";

fn format_label(f: gmbuild::archive::Format) -> &'static str {
    match f {
        gmbuild::archive::Format::Nkpi => "nkpi",
        gmbuild::archive::Format::ProtectedNkpi => "protected_nkpi",
        gmbuild::archive::Format::Dfo20260901 => "dfo_20260901",
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| String::from("smoke"));
    let pvf = args.next().unwrap_or_else(|| DEFAULT_PVF.to_string());
    let out = args.next().unwrap_or_else(|| String::from("out"));
    // 只有 refresh-bag 用得上：旧背包规则所在的目录（与 out 不同盘时也只能是那一份）。
    let src_dir = args.next();
    let pvf = PathBuf::from(pvf);
    let a = gmbuild::open_gm_archive(&pvf).expect("打开归档失败");
    eprintln!(
        "format={} files={} checksum={}",
        format_label(a.format()),
        a.all_files().len(),
        a.checksum
    );
    match cmd.as_str() {
        "smoke" => {
            let t = a.read_text("string/ui.uv.str").expect("读 ui.str 失败");
            println!(
                "ui.uv.str 文本 {} 字节，首行: {}",
                t.len(),
                t.lines().next().unwrap_or("")
            );
            if let Some(f) = a.find_file_index("list/equipment.lst") {
                let raw = a.read_raw_idx(f).unwrap();
                println!("list/equipment.lst {} 字节", raw.len());
            } else {
                println!("缺 list/equipment.lst");
            }
        }
        "build-names" => {
            let names = gmbuild::names::read_client_names(&a);
            let path = PathBuf::from(out).join("names.client.json");
            let file = std::fs::File::create(&path).expect("创建输出失败");
            let mut w = BufWriter::with_capacity(1 << 20, file);
            let n = gmbuild::names::write_client_names(&names, &mut w).expect("写出失败");
            w.flush().unwrap();
            println!("names.client.json 写出 {n} 条 -> {}", path.display());
        }
        "build-items" => {
            let path = PathBuf::from(out).join("items.index.json");
            let file = std::fs::File::create(&path).expect("创建输出失败");
            let mut w = BufWriter::with_capacity(1 << 20, file);
            let n = gmbuild::itemindex::build_items(&a, &mut w).expect("构建物品索引失败");
            w.flush().unwrap();
            println!("items.index.json 写出 {n} 件 -> {}", path.display());
        }
        "build-slots" => {
            let outdir = PathBuf::from(&out);
            std::fs::create_dir_all(&outdir).expect("创建输出目录失败");
            let index = gmbuild::itemindex::import_item_index(&a).expect("导入物品索引失败");
            let slots_path = outdir.join("equipment.slots.json");
            let existing = gmbuild::slotmap::load_slot_map(&slots_path);
            let existing_n = existing.len();
            let maps = gmbuild::slotmap::build_slot_maps(&a, &index).expect("扫描旁表失败");
            let mut slots = maps.slots;
            let mut kept = 0usize;
            for (k, v) in existing {
                if !slots.contains_key(&k) {
                    slots.insert(k, v);
                    kept += 1;
                }
            }
            let base = pvf.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let built_at = gmbuild::slotmap::now_rfc3339();
            let mut w = BufWriter::with_capacity(1 << 20, std::fs::File::create(&slots_path).unwrap());
            let n = gmbuild::slotmap::write_slots_file(
                &slots,
                &format!(
                    "{base} [equipment type] 全量导出（装备 {} 件，读到 {}，无部位 {}；保留原目录条目 {kept}）",
                    maps.stats.equipment, maps.stats.read, maps.stats.no_cell
                ),
                &built_at,
                &mut w,
            )
            .unwrap();
            w.flush().unwrap();
            println!(
                "equipment.slots.json 写出 {n} 条（现有 {} + 保留 {kept}，原表 {existing_n}）；无 [rarity] {}、读取失败 {}",
                slots.len() - kept,
                maps.stats.no_rarity,
                maps.stats.read_failed
            );
            let jobs_path = outdir.join("equipment.jobs.json");
            let mut w = BufWriter::with_capacity(1 << 20, std::fs::File::create(&jobs_path).unwrap());
            let n = gmbuild::slotmap::write_jobs_file(
                &maps.jobs,
                maps.stats.unrestricted,
                &format!(
                    "{base} [usable job] 全量导出（装备 {} 件；有职业限制 {}，通用 {}）",
                    maps.stats.equipment, maps.stats.limited, maps.stats.unrestricted
                ),
                &built_at,
                &mut w,
            )
            .unwrap();
            w.flush().unwrap();
            println!("equipment.jobs.json 写出 {n} 条（限制 {}，通用 {}）", maps.stats.limited, maps.stats.unrestricted);
            let rarity_path = outdir.join("item.rarity.json");
            let mut w = BufWriter::with_capacity(1 << 20, std::fs::File::create(&rarity_path).unwrap());
            let n = gmbuild::slotmap::write_rarity_file(
                &maps.rarity,
                &format!(
                    "{base} [rarity] 全量导出（物品 {} 件，取到品质 {}，无该段 {}）",
                    index.len(),
                    maps.stats.rarity_ok,
                    maps.stats.no_rarity
                ),
                &built_at,
                &mut w,
            )
            .unwrap();
            w.flush().unwrap();
            println!("item.rarity.json 写出 {n} 条（取到 {}，无该段 {}）", maps.stats.rarity_ok, maps.stats.no_rarity);
        }
        "build-growth" => {
            let outdir = PathBuf::from(&out);
            std::fs::create_dir_all(&outdir).expect("创建输出目录失败");
            let label = format_label(a.format()).to_string();
            let file = gmbuild::growthbuild::build_growth(&a, &pvf, &label).expect("构建养成产物失败");
            let path = outdir.join("gm-growth.json");
            std::fs::write(&path, serde_json::to_vec(&file).expect("序列化养成产物失败")).expect("写出失败");
            let branches: usize = file.professions.values().map(|p| p.advancement_growth.len()).sum();
            let grants: usize = file
                .professions
                .values()
                .flat_map(|p| p.awakening_skills.values())
                .flat_map(|st| st.values())
                .map(|r| r.len() / 2)
                .sum();
            println!(
                "gm-growth.json 写出：职业 {}，门槛 {}，SP 行 {}，转职成长 {} 项，觉醒授予 {} 条 -> {}",
                file.professions.len(),
                file.thresholds.len(),
                file.skill_points.len(),
                branches,
                grants,
                path.display()
            );
        }
        "build-quests" => {
            let outdir = PathBuf::from(&out);
            std::fs::create_dir_all(&outdir).expect("创建输出目录失败");
            let label = format_label(a.format()).to_string();
            let file = gmbuild::questbuild::build_quests(&a, &pvf, &label).expect("构建任务产物失败");
            let path = outdir.join("gm-quests.json");
            std::fs::write(&path, serde_json::to_vec(&file).expect("序列化任务产物失败")).expect("写出失败");
            let named = file.quests.values().filter(|q| !q.name.is_empty()).count();
            let models = file.quests.values().filter(|q| !q.model.is_empty()).count();
            let epics = file.quests.values().filter(|q| q.grade_texts.iter().any(|g| g == "[epic]")).count();
            println!(
                "gm-quests.json 写出：任务 {}（有中文名 {}，进度模型已认 {}，主线 [epic] {}）-> {}",
                file.quests.len(),
                named,
                models,
                epics,
                path.display()
            );
        }
        "build-equipment" => {
            let outdir = PathBuf::from(&out);
            std::fs::create_dir_all(&outdir).expect("创建输出目录失败");
            let path = outdir.join("equipment.current37.json");
            let label = format_label(a.format()).to_string();
            let file = std::fs::File::create(&path).expect("创建装备目录失败");
            let mut w = BufWriter::with_capacity(1 << 20, file);
            let (rows, counts) =
                gmbuild::equipmentexport::build_equipment_export(&a, &pvf, &label, &mut w)
                    .expect("导出装备目录失败");
            w.flush().unwrap();
            let detail = counts
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" ");
            println!("equipment.current37.json 写出 {rows} 行（{detail}）-> {}", path.display());
        }
        "refresh-bag" => {
            let outdir = PathBuf::from(&out);
            std::fs::create_dir_all(&outdir).expect("创建输出目录失败");
            let src_dir = src_dir.expect("refresh-bag 需要第 4 个参数：旧背包规则所在目录");
            let src = PathBuf::from(src_dir).join("inventory.current37.json");
            let raw = std::fs::read_to_string(&src)
                .unwrap_or_else(|e| panic!("读取旧背包规则 {}：{e}", src.display()));
            let after = gmbuild::equipmentexport::refresh_bag_rules(&raw, &a.checksum)
                .expect("换代际戳被拒");
            let path = outdir.join("inventory.current37.json");
            std::fs::write(&path, after).expect("写出背包规则失败");
            println!("inventory.current37.json 换戳完成（策略字段逐键未动）-> {}", path.display());
        }
        other => panic!("未知子命令 {other}"),
    }
}
