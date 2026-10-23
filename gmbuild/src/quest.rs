//! 任务产物的模式与读取侧（route C）。
//!
//! 由 `gmbuild build-quests` 离线直读 PVF 生成，把 Go 侧「每次加载目录都要重新投影」
//! 的那一层（`catalog.ImportQuests` + `catalog.LoadQuests` 的重投影 +
//! `quest.BuildIndex` + `quest.InitialProgress`）一次性烘成数据，加上
//! `gmQuestText`/`buildQuestNPCNames` 的中文折叠。运行期（启动器）只读 JSON：
//!
//! - 判据仍是代码：`model` 只说明「这条任务的进度模型服务端能不能自己结算」，
//!   能不能强制改、能不能进主线，由 `src-tauri/src/gmquest.rs` 按 Go 的规则算。
//! - `pending` 非空 = 源里这一段读不进去（等级/职业/前置格式不认识）。Go 用它
//!   挡住自动补完，本路线同口径。
//! - 文本在构建期已经解析完（`<11::…>` → `string/quest.uv.str`，`<npc::N>` →
//!   `list/npc.lst` + `string/npc.uv.str`），所以运行期不再需要
//!   `quest.text.client.json` 与目录哈希的绑定。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// 一条 `[pre required quest answer]`：问答任务 `question` 的第 `index` 个答案解锁本任务。
/// 段内恒为两个数值单元，答案索引允许 0（第一个答案）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestAnswer {
    pub question: u32,
    pub index: u32,
}

/// `[target character]` 的一三元组：职业、转职、觉醒；-1 代表不限。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetChar {
    pub job: String,
    pub advancement: i32,
    pub awakening: i32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QuestEntry {
    /// `.qst` 在归档里的落定路径与原文指纹（只做 provenance，不参与判定）。
    pub path: String,
    pub sha256: String,
    pub minimum_level: u32,
    /// `[job]` 段的文本单元；空 = 源没有职业限制，适用于所有职业。
    pub jobs: Vec<String>,
    /// `[pre required quest]` 展平结果（重复段的并集），Go 的 Prerequisites。
    pub prerequisites: Vec<u32>,
    /// 每个重复段是一组「必须全部完成」的前置；段与段之间是可选分支。
    pub prerequisite_groups: Vec<Vec<u32>>,
    /// `[collision quest]` 点名的互斥同伴。
    pub collisions: Vec<u32>,
    pub answers: Vec<QuestAnswer>,
    /// `[type]` 段第一个文本单元 = 主目标；后面的单元是子条件。
    pub kind: String,
    /// 源里读不进去的部分。非空即禁止自动补完（Go 同口径）。
    pub pending: Vec<String>,
    /// `[grow type]` 段；`grow_usable=false` 代表段里混了非数值单元。
    pub grow_types: Vec<i32>,
    pub grow_usable: bool,
    /// `[reward type]` / `[reward select int data]` 能否被服务端结算。
    pub reward_usable: bool,
    /// 进度模型名；空 = 该目标形态本构建不认识（`InitialProgress` 报错）。
    pub model: String,
    /// `[grade]` 段的文本单元原文，主线判据是「恰好一个且等于 `[epic]`」。
    pub grade_texts: Vec<String>,
    /// `[target character]`：缺席 = 不限角色；`valid=false` = 形态不认识（两值都拒）。
    pub target_present: bool,
    pub target_valid: bool,
    pub target_triples: Vec<TargetChar>,
    /// 构建期折好的中文显示文本。空串 = 源里确实没有可显示文本。
    pub name: String,
    pub depend: String,
    pub condition: String,
    pub solve: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QuestFile {
    pub source: super::growth::SourceSnapshot,
    /// `list/quest.lst` 的原文指纹（provenance）。
    pub index_sha256: String,
    pub quests: BTreeMap<u32, QuestEntry>,
}

impl QuestFile {
    pub fn validate(&self) -> Result<(), String> {
        if self.source.checksum.len() != 64 {
            return Err("任务产物的归档校验不是 64 位十六进制".into());
        }
        if self.index_sha256.len() != 64 {
            return Err("任务产物缺少任务索引指纹".into());
        }
        if self.quests.is_empty() {
            return Err("任务产物没有任何任务".into());
        }
        for (id, q) in &self.quests {
            if *id == 0 {
                return Err("任务产物包含 0 号任务".into());
            }
            if q.pending.is_empty() && q.sha256.len() != 64 {
                return Err(format!("任务 {id} 缺少来源指纹"));
            }
            if q.target_valid {
                for t in &q.target_triples {
                    if t.job.is_empty() || !(t.advancement == -1 || (0..=15).contains(&t.advancement)) || !(t.awakening == -1 || (0..=3).contains(&t.awakening)) {
                        return Err(format!("任务 {id} 的目标角色条件越界"));
                    }
                }
            } else if !q.target_triples.is_empty() {
                // Go 对形态不认识的段落整段丢弃：留半截会让「目标角色条件」被部分解码。
                return Err(format!("任务 {id} 的目标角色段形态不认识却带着解码结果"));
            }
        }
        Ok(())
    }

    /// Go `BuildIndex` 的防御性重投影：没有重复段但有展平前置时，
    /// 把整条展平列表当作唯一一段。
    pub fn prerequisite_groups_of(&self, id: u32) -> Vec<Vec<u32>> {
        let Some(q) = self.quests.get(&id) else {
            return Vec::new();
        };
        if q.prerequisite_groups.is_empty() && !q.prerequisites.is_empty() {
            return vec![q.prerequisites.clone()];
        }
        q.prerequisite_groups.clone()
    }
}

/// 读取 `<configs>/gm-quests.json`。缺失即返回错误：任务写链路宁可拒绝，
/// 也不要在没有目录的情况下把任务标成完成。
pub fn load_quests(dir: &Path) -> Result<QuestFile, String> {
    let path = dir.join("gm-quests.json");
    let b = std::fs::read(&path).map_err(|e| format!("读取 {}：{}", path.display(), e))?;
    let file: QuestFile = serde_json::from_slice(&b).map_err(|e| format!("解析 {}：{}", path.display(), e))?;
    file.validate()?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> QuestFile {
        let mut quests = BTreeMap::new();
        quests.insert(
            100u32,
            QuestEntry {
                path: "n_quest/a.qst".into(),
                sha256: "c".repeat(64),
                minimum_level: 1,
                jobs: vec!["[all]".into()],
                prerequisites: vec![90],
                prerequisite_groups: Vec::new(),
                collisions: vec![101],
                answers: vec![QuestAnswer { question: 90, index: 0 }],
                kind: "[meet npc]".into(),
                pending: Vec::new(),
                grow_types: vec![0],
                grow_usable: true,
                reward_usable: true,
                model: "single-meet-npc-remaining-v1".into(),
                grade_texts: vec!["[epic]".into()],
                target_present: false,
                target_valid: true,
                target_triples: Vec::new(),
                name: "启程".into(),
                depend: String::new(),
                condition: String::new(),
                solve: String::new(),
            },
        );
        QuestFile {
            source: super::super::growth::SourceSnapshot {
                format: "dfo_20260901".into(),
                path: "Script.pvf".into(),
                size: 1,
                checksum: "a".repeat(64),
                built_at: "2026-10-21T00:00:00Z".into(),
            },
            index_sha256: "b".repeat(64),
            quests,
        }
    }

    #[test]
    fn round_trips_and_validates() {
        let text = serde_json::to_vec(&sample()).unwrap();
        let back: QuestFile = serde_json::from_slice(&text).unwrap();
        back.validate().unwrap();
        assert_eq!(back.quests[&100].name, "启程");
        assert_eq!(back.quests[&100].answers[0].question, 90);
    }

    #[test]
    fn flat_prerequisites_become_one_group() {
        let f = sample();
        assert_eq!(f.prerequisite_groups_of(100), vec![vec![90u32]]);
        assert_eq!(f.prerequisite_groups_of(999), Vec::<Vec<u32>>::new());
    }

    #[test]
    fn refuses_incomplete_artifacts() {
        let mut f = sample();
        f.index_sha256.truncate(63);
        assert!(f.validate().unwrap_err().contains("任务索引指纹"));
        let mut f = sample();
        f.quests.clear();
        assert!(f.validate().unwrap_err().contains("没有任何任务"));
        let mut f = sample();
        f.quests.get_mut(&100).unwrap().sha256.clear();
        assert!(f.validate().unwrap_err().contains("来源指纹"));
        // 目标角色段：数值越界、空职业都要拒
        let mut f = sample();
        let q = f.quests.get_mut(&100).unwrap();
        q.target_present = true;
        q.target_triples = vec![TargetChar { job: "[all]".into(), advancement: 99, awakening: 0 }];
        assert!(f.validate().unwrap_err().contains("目标角色条件越界"));
        // 形态不认识的段落必须整段丢弃，不留半截解码结果
        let mut f = sample();
        let q = f.quests.get_mut(&100).unwrap();
        q.target_present = true;
        q.target_valid = false;
        q.target_triples = vec![TargetChar::default()];
        assert!(f.validate().unwrap_err().contains("形态不认识"));
    }
}
