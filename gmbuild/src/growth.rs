//! 养成产物的模式与读取侧（route C）。
//!
//! 产物由 `gmbuild build-growth` 离线直读 PVF 生成，回答的是「服务端会把这个角色的
//! 等级/转职/觉醒算成什么样」：
//!
//! - `thresholds` / `skill_points`：`character/exptable.tbl` 的累计经验门槛与
//!   `etc/sptable.etc` 的每级技能点，即 Go `catalog.ImportProgression` 里
//!   `AddExperience` 真正读的两张表。
//! - `professions[*].advancement_growth`：Go `ProfessionGrowth` 对 `[growtype N+1]`
//!   段的投影，决定某个转职分支是否**存在**（`ApplyAdvancement` 以「该分支成长表为空」
//!   拒绝）。
//! - `professions[*].awakening_skills` / `awakening_levels`：`.chr` 的
//!   `[awakening skill]` 授予表，以及每个被授技能在 `.skl` 里的 `[required level]`
//!   原文（Go 用学习目录做成员判定与等级门，两者缺一即拒）。
//!
//! 身份门禁依赖 `raw_sha256`：角色存档的 `source_sha256` 必须等于本职业 `.chr`
//! 原文的 SHA-256，这与服务端 `ApplyAdvancement`/`ApplyAwakening` 完全同口径。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// 生成产物的那一份 PVF 快照。两份产物（成长与任务）的 `checksum` 必须相等，
/// 这是 Go `ApplyGain` 里 `Professions.Source.Checksum == Catalog.Source.Checksum`
/// 那条内部一致性检查在本路线下的对应物。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSnapshot {
    pub format: String,
    pub path: String,
    pub size: i64,
    pub checksum: String,
    pub built_at: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GrowthProfession {
    pub path: String,
    pub raw_sha256: String,
    pub job: String,
    /// `[growtype 1]` 段：未转职时每级成长。键只可能是初始属性里出现过的段落名。
    pub base_growth: BTreeMap<String, f32>,
    /// 0 基转职分支号 1..=15 → 该分支每级成长。缺席即该分支不存在。
    pub advancement_growth: BTreeMap<u8, BTreeMap<String, f32>>,
    /// 分支号 → 觉醒阶段 1..=3 → `(技能 id, 品级)` 扁平二元组。
    pub awakening_skills: BTreeMap<u8, BTreeMap<u8, Vec<i32>>>,
    /// 觉醒授予技能 id → `.skl` 里第一段 `[required level]` 的数值单元原文。
    /// 键缺席代表该 id 不在本职业技能索引里；值长度不为 1 时 Go 判
    /// 「awakening skill level missing」，本路线同样拒绝。
    pub awakening_levels: BTreeMap<u16, Vec<i32>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GrowthFile {
    pub source: SourceSnapshot,
    /// 累计经验门槛；升到 L 级所需的总量取自 `thresholds[L-2]`（L 从 1 起）。
    pub thresholds: Vec<u64>,
    /// 跨过的每一级补发的技能点。
    pub skill_points: BTreeMap<u16, u16>,
    pub professions: BTreeMap<u8, GrowthProfession>,
}

impl GrowthFile {
    /// 产物自检：门槛严格递增且非零、SP 表非空、每个职业都有 raw_sha256 与
    /// 未转职成长。这一层拒绝的是「产物本身残缺」，不是「这个角色不能改」。
    pub fn validate(&self) -> Result<(), String> {
        if self.source.checksum.len() != 64 {
            return Err("成长产物的归档校验不是 64 位十六进制".into());
        }
        if self.thresholds.is_empty() {
            return Err("成长产物缺少经验门槛表".into());
        }
        for w in self.thresholds.windows(2) {
            if w[0] >= w[1] {
                return Err("经验门槛不是严格递增".into());
            }
        }
        if self.thresholds[0] == 0 {
            return Err("经验门槛出现 0".into());
        }
        if self.skill_points.is_empty() {
            return Err("成长产物缺少每级技能点表".into());
        }
        if self.professions.is_empty() {
            return Err("成长产物缺少职业".into());
        }
        for (id, p) in &self.professions {
            if p.raw_sha256.len() != 64 || p.job.is_empty() {
                return Err(format!("职业 {id} 的成长产物缺少来源指纹或职业名"));
            }
            if p.base_growth.is_empty() {
                return Err(format!("职业 {id} 缺少未转职成长"));
            }
            for (_, grants) in &p.awakening_skills {
                for (_, row) in grants {
                    if row.len() % 2 != 0 {
                        return Err(format!("职业 {id} 的觉醒授予不是二元组"));
                    }
                }
            }
        }
        Ok(())
    }
}

/// 读取 `<configs>/gm-growth.json`。缺失即返回错误：养成写链路宁可拒绝，
/// 也不要在没有成长表的情况下硬写等级。
pub fn load_growth(dir: &Path) -> Result<GrowthFile, String> {
    let path = dir.join("gm-growth.json");
    let b = std::fs::read(&path).map_err(|e| format!("读取 {}：{}", path.display(), e))?;
    let file: GrowthFile = serde_json::from_slice(&b).map_err(|e| format!("解析 {}：{}", path.display(), e))?;
    file.validate()?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> GrowthFile {
        let mut base = BTreeMap::new();
        base.insert("[hp max]".to_string(), 12.5f32);
        let mut adv = BTreeMap::new();
        adv.insert(1u8, base.clone());
        let mut grants = BTreeMap::new();
        grants.insert(1u8, vec![100i32, 1]);
        let mut stages = BTreeMap::new();
        stages.insert(1u8, grants);
        let mut levels = BTreeMap::new();
        levels.insert(100u16, vec![50i32]);
        GrowthFile {
            source: SourceSnapshot {
                format: "dfo_20260901".into(),
                path: "Script.pvf".into(),
                size: 1,
                checksum: "a".repeat(64),
                built_at: "2026-10-21T00:00:00Z".into(),
            },
            thresholds: vec![100, 250, 500],
            skill_points: BTreeMap::from([(2u16, 3u16), (3u16, 4u16)]),
            professions: BTreeMap::from([(0u8, GrowthProfession {
                path: "character/swordman/swordman.chr".into(),
                raw_sha256: "b".repeat(64),
                job: "[swordman]".into(),
                base_growth: base,
                advancement_growth: adv,
                awakening_skills: stages,
                awakening_levels: levels,
            })]),
        }
    }

    #[test]
    fn round_trips_and_validates() {
        let text = serde_json::to_vec(&sample()).unwrap();
        let back: GrowthFile = serde_json::from_slice(&text).unwrap();
        back.validate().unwrap();
        // 整数键在 JSON 里落成字符串，读回仍按数值域索引。
        assert_eq!(back.professions[&0].advancement_growth[&1]["[hp max]"], 12.5);
        assert_eq!(back.professions[&0].awakening_levels[&100][0], 50);
    }

    #[test]
    fn refuses_incomplete_artifacts() {
        let mut f = sample();
        f.thresholds[1] = f.thresholds[0];
        assert!(f.validate().unwrap_err().contains("严格递增"));
        let mut f = sample();
        f.professions.get_mut(&0).unwrap().base_growth.clear();
        assert!(f.validate().unwrap_err().contains("未转职成长"));
        let mut f = sample();
        f.professions.get_mut(&0).unwrap().raw_sha256.truncate(63);
        assert!(f.validate().unwrap_err().contains("来源指纹"));
    }
}
