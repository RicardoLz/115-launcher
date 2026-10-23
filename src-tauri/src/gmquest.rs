//! 任务读侧与写侧（route C）。
//!
//! 判据全部来自离线产物 `gm-quests.json`（`gmbuild build-quests` 直读 PVF 烘出来
//! 的那份投影），运行期既不碰 PVF 也不跑 Go。落库口径与 Go 的 `gmQuestList` /
//! `mainlinePlan` + `resolveMainlineQuests` / `questPrerequisitePlan` /
//! `bridge_manage.go` 的 `quests.*` 逐条对齐，拒绝话也逐字一致。
//!
//! 与 Go 的三处刻意差异，都在这里说明：
//!
//! - **身份门只认存档契约常量**。Go 比较的是 `data.Source.SaveIdentity()`
//!   （= `savecontract.Identity()`，与 PVF 无关），所以任务这一侧没有
//!   「归档校验值当旧身份」那条分支 —— 养成侧有（`gmstat::identity_ok`），这里刻意不收。
//! - **中文文本在构建期折叠**。Go 运行期还要读 `quest.text.client.json` 并校验
//!   目录哈希，产物里 `name/depend/condition/solve` 已经是折好的中文，
//!   因此本模块没有 Go 那句「任务中文文本未就绪」的 warning —— 它没有对应的失败面。
//! - **`[grade]` 段只留非空文本单元**（构建期筛掉），Go 的运行期判据是
//!   「段内恰好一个单元且等于 `[epic]`」。已部署的产物里 2844 条任务的
//!   `grade_texts` 长度全是 1，唯一残余风险是「一个 `[epic]` 文本单元 + 若干数值单元」
//!   这种段形：Go 会整条排除、本侧会当成主线候选。方向是多标几条已完成，
//!   而计划要先经预览与签名确认才落库。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use gmbuild::quest::{QuestEntry, QuestFile};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::gmstat::{self, Context};
use crate::paths::Root;

// ---------------------------------------------------------------------------
// 产物与角色行
// ---------------------------------------------------------------------------

/// 读 `gm-quests.json`。缺失即整条链路拒绝：宁可不做任务，也不要在没有目录的情况下
/// 把任务标成完成（`gmbuild::quest::load_quests` 自己也是这个口径）。
pub(crate) fn load(root: &Root) -> Result<QuestFile, String> {
    let dir = gmbuild::gmindex::find_configs(&root.path)
        .ok_or_else(|| "未找到 GM 产物目录（gm-tool/configs）".to_string())?;
    load_from(&dir)
}

fn load_from(dir: &Path) -> Result<QuestFile, String> {
    gmbuild::quest::load_quests(dir)
}

/// Go `storage.Character` 里任务这一批用得上的字段。`state` 留原始字节：
/// 参与 revision / 计划签名时必须按库里的字节算，二次序列化会改键序与空白。
#[derive(Clone, Debug)]
pub(crate) struct Role {
    pub id: i64,
    pub account_id: i64,
    pub profession: i64,
    pub config_version: String,
    pub state: Vec<u8>,
}

/// 与 Go 的 `ApplyManagedGrant` 同：账号 + 角色双重定位，软删过的角色一律取不到。
pub(crate) fn read_role(conn: &Connection, account: i64, character: i64) -> Result<Role, String> {
    conn.query_row(
        "SELECT id,account_id,profession,config_version,state FROM characters \
         WHERE account_id=?1 AND id=?2 AND deleted_at IS NULL",
        params![account, character],
        |r| {
            Ok(Role {
                id: r.get(0)?,
                account_id: r.get(1)?,
                profession: r.get(2)?,
                config_version: r.get::<_, String>(3).unwrap_or_default(),
                state: r.get::<_, Vec<u8>>(4)?,
            })
        },
    )
    .map_err(|e| e.to_string())
}

/// Go `storage.QuestState`（json 键序也照它排，revision 要用）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestState {
    pub id: u16,
    pub status: String,
    pub progress: u32,
    pub config_version: String,
    pub progress_model: String,
}

fn state_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<QuestState> {
    Ok(QuestState {
        id: row.get::<_, i64>(0)? as u16,
        status: row.get(1)?,
        progress: row.get::<_, i64>(2)? as u32,
        config_version: row.get(3)?,
        progress_model: row.get(4)?,
    })
}

const SQL_QUESTS: &str = "SELECT quest_id,status,progress,config_version,progress_model \
 FROM character_quests WHERE character_id=?1 ORDER BY quest_id";

pub(crate) fn read_states(conn: &Connection, character: i64) -> Result<Vec<QuestState>, String> {
    let mut stmt = conn.prepare(SQL_QUESTS).map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![character], state_of).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// 指纹（Go 的 gmQuestRevision 与计划签名）
// ---------------------------------------------------------------------------

fn sha256_hex(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 库里的 state 原样嵌入；空档按 Go 的 `json.RawMessage(nil)` 落成 `null`。
fn raw_json(raw: &[u8]) -> Result<String, String> {
    if raw.is_empty() {
        return Ok("null".into());
    }
    let text = std::str::from_utf8(raw).map_err(|e| format!("角色存档不是 UTF-8：{e}"))?;
    // 先解一次再原样使用：签名要的是「这一份 JSON」，而写入路径本来就要求它可解。
    let _: Value = serde_json::from_str(text).map_err(|e| format!("角色存档不是合法 JSON：{e}"))?;
    Ok(text.to_string())
}

/// Go `gmQuestRevision`：账号、角色、职业、存档身份、state 原文、任务状态列表。
/// 只与本模块自己产出的值比对，所以关键是「同一次输入永远算出同一个值」。
pub(crate) fn quest_revision(role: &Role, states: &[QuestState]) -> Result<String, String> {
    let mut list = String::new();
    for (i, q) in states.iter().enumerate() {
        if i > 0 {
            list.push(',');
        }
        list.push_str(&format!(
            "{{\"id\":{},\"status\":\"{}\",\"progress\":{},\"config_version\":\"{}\",\"progress_model\":\"{}\"}}",
            q.id, q.status, q.progress, q.config_version, q.progress_model
        ));
    }
    Ok(sha256_hex(&format!(
        "{{\"account\":{},\"character\":{},\"profession\":{},\"source\":\"{}\",\"state\":{},\"quests\":[{}]}}",
        role.account_id,
        role.id,
        role.profession,
        role.config_version,
        raw_json(&role.state)?,
        list
    )))
}

/// 角色存档里任务判据要用的那三个数（Go 的 `character.State` 那几项）。
#[derive(Clone, Copy, Debug, Default)]
struct CharacterView {
    level: u32,
    advancement: u8,
    awakening: u8,
}

fn view_of(state: &Value) -> Result<CharacterView, String> {
    Ok(CharacterView {
        level: gmstat::read_uint(state, "level", u32::MAX as u64)? as u32,
        advancement: gmstat::read_uint(state, "advancement", u8::MAX as u64)? as u8,
        awakening: gmstat::read_uint(state, "awakening", u8::MAX as u64)? as u8,
    })
}

// ---------------------------------------------------------------------------
// 计划（预览与执行共用这一份算法）
// ---------------------------------------------------------------------------

/// Go `mainlinePreview`。字段声明顺序就是 JSON 键序，签名要按这个字节算，
/// 所以**调整字段顺序等于改签名**（`signature` 在算签名时必须是空串）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct GmQuestPlan {
    pub quests: Vec<u16>,
    pub blocked: Vec<u16>,
    pub before: Vec<QuestState>,
    pub level: u8,
    pub signature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer_picks: Option<BTreeMap<u16, u16>>,
}

impl GmQuestPlan {
    fn empty(level: u8) -> GmQuestPlan {
        GmQuestPlan {
            quests: Vec::new(),
            blocked: Vec::new(),
            before: Vec::new(),
            level,
            signature: String::new(),
            answer_picks: None,
        }
    }
}

fn sign_mainline(role: &Role, plan: &GmQuestPlan) -> Result<String, String> {
    let body = serde_json::to_string(plan).map_err(|e| e.to_string())?;
    Ok(sha256_hex(&format!(
        "{{\"account\":{},\"character\":{},\"source\":\"{}\",\"state\":{},\"plan\":{}}}",
        role.account_id,
        role.id,
        role.config_version,
        raw_json(&role.state)?,
        body
    )))
}

fn sign_prerequisite(role: &Role, states: &[QuestState], target: u16, plan: &GmQuestPlan) -> Result<String, String> {
    let body = serde_json::to_string(plan).map_err(|e| e.to_string())?;
    Ok(sha256_hex(&format!(
        "{{\"revision\":\"{}\",\"target\":{},\"plan\":{}}}",
        quest_revision(role, states)?,
        target,
        body
    )))
}

/// Go `gmQuestTargetAllowed` 的运行期那半：段不在 = 不限；形态不认 = 两值都拒；
/// 否则多组三元组任选其一命中即可。
fn target_allowed(entry: &QuestEntry, job: &str, view: CharacterView) -> (bool, bool) {
    if !entry.target_present {
        return (true, true);
    }
    if !entry.target_valid {
        return (false, false);
    }
    let allowed = entry.target_triples.iter().any(|t| {
        (t.job == "[all]" || t.job == job)
            && (t.advancement == -1 || t.advancement == i32::from(view.advancement))
            && (t.awakening == -1 || t.awakening == i32::from(view.awakening))
    });
    (allowed, true)
}

/// Go 的职业门：`[job]` 段缺席代表通用任务，不能漏掉；`[all]` 与本命职业都算命中。
fn job_allowed(entry: &QuestEntry, job: &str, view: CharacterView) -> bool {
    let mut allowed = entry.jobs.is_empty();
    for j in &entry.jobs {
        allowed = allowed || j == "[all]" || j == job;
    }
    for g in &entry.grow_types {
        if *g >= 0 && *g != i32::from(view.advancement) {
            allowed = false;
        }
    }
    allowed
}

fn collisions_blocked(collisions: &[u32], taken: &HashSet<u32>) -> bool {
    collisions.iter().any(|c| taken.contains(c))
}

/// 去重并保持升序（调用方已排过序）——Go 的 `dedupeUint16`，Blocked 的两个来源
/// （资格筛选、结果兜底）会同时命中同一条任务。
fn dedupe_ascending(v: &mut Vec<u16>) {
    v.dedup();
}

/// 存档身份之外的第一道门：任务目录只服务当前契约身份。
fn identity_gate(role: &Role) -> Result<(), String> {
    if role.config_version != gmstat::save_identity() {
        return Err("任务目录与角色版本不匹配".into());
    }
    Ok(())
}

/// Go `resolveMainlineQuests`：逐层闭合前置链，互斥门与答案门都在「准备标记完成」
/// 之前判。`completed` 会被就地扩充（选中的也算完成，作为下一轮闭合的依据）。
///
/// 答案门不能靠 Collisions 兜底：目录里那批答案分支任务的互斥边是残缺的
/// （Go 侧为此硬编码了 factionBranchGroups），只有答案段本身是完整判据。
/// 口径：候选按 id 升序，所以每个问答任务第一次出现的成员就是选中的答案索引，
/// 其余成员进 blocked —— 结果因此确定、可复现。
fn resolve_mainline(
    file: &QuestFile,
    eligible: &[u32],
    completed: &mut HashSet<u32>,
) -> (Vec<u16>, Vec<u16>, BTreeMap<u16, u16>) {
    let mut picks: HashMap<u32, u32> = HashMap::new();
    for id in eligible {
        let Some(entry) = file.quests.get(id) else { continue };
        if let Some(a) = entry.answers.first() {
            picks.entry(a.question).or_insert(a.index);
        }
    }
    let mut selected: Vec<u16> = Vec::new();
    loop {
        let mut changed = false;
        for id in eligible {
            if completed.contains(id) {
                continue;
            }
            let Some(entry) = file.quests.get(id) else { continue };
            if collisions_blocked(&entry.collisions, completed) {
                continue;
            }
            if let Some(a) = entry.answers.first() {
                if picks.get(&a.question).copied().unwrap_or(a.index) != a.index {
                    continue;
                }
            }
            let groups = file.prerequisite_groups_of(*id);
            let mut ready = groups.is_empty();
            for group in &groups {
                if group.is_empty() {
                    continue;
                }
                if group.iter().all(|pre| completed.contains(pre)) {
                    ready = true;
                    break;
                }
            }
            if ready {
                completed.insert(*id);
                selected.push(*id as u16);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let blocked: Vec<u16> = eligible.iter().copied().filter(|id| !completed.contains(id)).map(|id| id as u16).collect();
    let mut answer_picks: BTreeMap<u16, u16> = BTreeMap::new();
    for (q, idx) in picks {
        answer_picks.insert(q as u16, idx as u16);
    }
    (selected, blocked, answer_picks)
}

/// Go `mainlinePlan`：资格筛选 → 闭合链 → 排序去重 → 签名。
/// 只改当前目录的主线完成记录，保留日常、活动与其他职业任务；不模拟任务奖励。
pub(crate) fn mainline_plan(
    file: &QuestFile,
    ctx: &Context,
    role: &Role,
    states: &[QuestState],
    level: u8,
) -> Result<GmQuestPlan, String> {
    let mut out = GmQuestPlan::empty(level);
    let state: Value = serde_json::from_slice(&role.state).map_err(|e| format!("角色存档不是合法 JSON：{e}"))?;
    let view = view_of(&state)?;
    if level == 0 || u32::from(level) > view.level {
        return Err("主线目标等级必须在 1 至角色当前等级之间".into());
    }
    identity_gate(role)?;
    let job = ctx
        .profession(&role.config_version, role.profession, &state)?
        .job
        .clone();
    if job.is_empty() {
        return Err("角色职业配置缺失".into());
    }

    let mut completed: HashSet<u32> = HashSet::new();
    let mut prior: HashMap<u32, QuestState> = HashMap::new();
    for q in states {
        prior.insert(u32::from(q.id), q.clone());
        if q.config_version == role.config_version && q.status == "completed" {
            completed.insert(u32::from(q.id));
        }
    }

    // `file.quests` 是 BTreeMap，迭代即 id 升序 = Go 的 `index.Ordered`。
    let mut eligible: Vec<u32> = Vec::new();
    for (id, entry) in &file.quests {
        let id = *id;
        let epic = entry.grade_texts.len() == 1 && entry.grade_texts[0] == "[epic]";
        if id == 0 || id >= 65535 || !epic || entry.minimum_level > u32::from(level) || completed.contains(&id) {
            continue;
        }
        if !job_allowed(entry, &job, view) {
            continue;
        }
        let (target_allowed, target_usable) = target_allowed(entry, &job, view);
        if target_usable && !target_allowed {
            continue;
        }
        let prior_mismatch = prior
            .get(&id)
            .map(|q| q.config_version != role.config_version)
            .unwrap_or(false);
        if !target_usable || !entry.grow_usable || !entry.pending.is_empty() || prior_mismatch {
            out.blocked.push(id as u16);
            continue;
        }
        eligible.push(id);
    }

    let (selected, blocked_eligible, answer_picks) = resolve_mainline(file, &eligible, &mut completed);
    out.quests = selected;
    out.blocked.extend(blocked_eligible);
    out.quests.sort_unstable();
    out.blocked.sort_unstable();
    dedupe_ascending(&mut out.blocked);
    for id in &out.quests {
        if let Some(q) = prior.get(&u32::from(*id)) {
            out.before.push(q.clone());
        }
    }
    // 把「哪个问答任务选了哪个答案」写进回执：被答案门挡掉的分支是玩家可见的行为
    // 变化，不能只体现在 quests 少了几条上。
    if !answer_picks.is_empty() {
        out.answer_picks = Some(answer_picks);
    }
    out.signature = sign_mainline(role, &out)?;
    Ok(out)
}

/// Go `questPrerequisitePlan` 的递归那半：只追溯所选任务的前置，不碰目标任务本身。
struct Trace<'a> {
    file: &'a QuestFile,
    job: String,
    view: CharacterView,
    identity: String,
    prior: HashMap<u32, QuestState>,
    /// 库里已占用的任务：accepted 与 completed 都算（服务端 available.go 同口径）。
    /// 版本不匹配的旧目录行不算 —— 那不是当前分支的选择。
    taken: HashSet<u32>,
    visiting: HashSet<u32>,
    done: HashSet<u32>,
    out: Vec<u16>,
}

impl<'a> Trace<'a> {
    fn visit(&mut self, id: u32, root: bool) -> Result<(), String> {
        let old = self.prior.get(&id).cloned();
        if let Some(q) = &old {
            if q.config_version != self.identity {
                return Err(format!("前置任务 {id} 版本不匹配"));
            }
        }
        if self.done.contains(&id) || old.as_ref().map(|q| q.status == "completed").unwrap_or(false) {
            return Ok(());
        }
        if self.visiting.contains(&id) {
            return Err(format!("任务 {id} 存在循环前置，未作修改"));
        }
        let Some(entry) = self.file.quests.get(&id) else {
            return Err(format!("任务 {id} 的前置配置待核实"));
        };
        // Go 用 `index.Entries[id]` 是否为 nil、Pending 是否为空、GrowUsable 三件事
        // 合成同一句话；产物里每条任务都有条目，所以剩下的判据就是后两条。
        if id == 0 || id >= 65535 || !entry.pending.is_empty() || !entry.grow_usable {
            return Err(format!("任务 {id} 的前置配置待核实"));
        }
        let allowed = job_allowed(entry, &self.job, self.view);
        let (target_allowed, target_usable) = target_allowed(entry, &self.job, self.view);
        if !target_usable {
            return Err(format!("任务 {id} 的目标角色条件待核实"));
        }
        if !allowed || !target_allowed || entry.minimum_level > self.view.level {
            return Err(format!("任务 {id} 不满足当前角色职业或等级条件"));
        }
        self.visiting.insert(id);
        // 互斥与答案门与「一键完成主线」同一套判据，否则两条入口对同一角色会给出
        // 互相矛盾的结论。这里只认库里已经落地的占用，不认本次访问链自己勾选的行。
        if collisions_blocked(&entry.collisions, &self.taken) {
            return Err(format!("任务 {id} 与已接受或已完成的分支互斥，未作修改"));
        }
        if let Some(a) = entry.answers.first() {
            for peer in &self.taken {
                let Some(pe) = self.file.quests.get(peer) else { continue };
                let Some(pa) = pe.answers.first() else { continue };
                if pa.question == a.question && pa.index != a.index {
                    return Err(format!(
                        "任务 {id} 与已选分支（问答 {} 的第 {} 个答案）互斥，未作修改",
                        a.question, pa.index
                    ));
                }
            }
        }
        let groups = self.file.prerequisite_groups_of(id);
        let mut satisfied = groups.is_empty();
        for group in &groups {
            let complete = group.iter().all(|pre| {
                self.done.contains(pre)
                    || self
                        .prior
                        .get(pre)
                        .map(|q| q.status == "completed" && q.config_version == self.identity)
                        .unwrap_or(false)
            });
            if complete {
                satisfied = true;
                break;
            }
        }
        if !satisfied {
            if groups.len() != 1 {
                return Err(format!("任务 {id} 有多条可选前置链，请先在游戏中确定分支"));
            }
            for pre in &groups[0] {
                self.visit(*pre, false)?;
            }
        }
        self.visiting.remove(&id);
        self.done.insert(id);
        if !root {
            self.out.push(id as u16);
        }
        if self.out.len() > 100 {
            return Err("前置链超过 100 个任务，请分阶段处理".into());
        }
        Ok(())
    }
}

/// 只追溯所选任务的前置，不完成目标任务；多条未满足的替代链不擅自选择。
pub(crate) fn prerequisite_plan(
    file: &QuestFile,
    ctx: &Context,
    role: &Role,
    states: &[QuestState],
    target: u16,
) -> Result<GmQuestPlan, String> {
    let mut out = GmQuestPlan::empty(0);
    out.before = states.to_vec();
    identity_gate(role)?;
    let state: Value = serde_json::from_slice(&role.state).map_err(|e| format!("角色存档不是合法 JSON：{e}"))?;
    let view = view_of(&state)?;
    let prior: HashMap<u32, QuestState> = states.iter().map(|q| (u32::from(q.id), q.clone())).collect();
    let q = prior.get(&u32::from(target));
    if q.map(|x| x.status != "accepted" || x.config_version != role.config_version).unwrap_or(true) {
        return Err("请选择当前角色进行中的任务".into());
    }
    let job = ctx
        .profession(&role.config_version, role.profession, &state)?
        .job
        .clone();
    if job.is_empty() {
        return Err("角色职业配置缺失".into());
    }
    let mut taken: HashSet<u32> = HashSet::new();
    for q in states {
        if q.config_version == role.config_version && (q.status == "accepted" || q.status == "completed") {
            taken.insert(u32::from(q.id));
        }
    }
    let mut trace = Trace {
        file,
        job,
        view,
        identity: role.config_version.clone(),
        prior,
        taken,
        visiting: HashSet::new(),
        done: HashSet::new(),
        out: Vec::new(),
    };
    trace.visit(u32::from(target), true)?;
    out.quests = trace.out;
    out.quests.sort_unstable();
    out.blocked.sort_unstable();
    dedupe_ascending(&mut out.blocked);
    out.signature = sign_prerequisite(role, states, target, &out)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// 列表读侧（Go gmQuestList）
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
pub struct GmQuestPre {
    pub id: u32,
    pub name: String,
    pub completed: bool,
}

#[derive(Clone, Serialize)]
pub struct GmQuestRow {
    pub id: u16,
    pub name: String,
    /// `[grade]` 段原文（`[epic]` 即主线）。
    pub grade: String,
    /// 目录里的最低等级要求。
    pub level: u32,
    /// `accepted` / `ready`（进度已够、只差提交）/ `completed`。
    pub status: String,
    pub progress: u32,
    pub description: String,
    pub condition: String,
    pub solve: String,
    /// 每个内层数组是一组「必须全部完成」的前置；组与组之间是可选分支。
    pub prerequisites: Vec<Vec<GmQuestPre>>,
    pub can_abandon: bool,
    pub can_modify: bool,
    pub can_trace: bool,
    /// 非空即这一行不可改，原文就是拒绝理由。
    pub blocked: String,
}

#[derive(Serialize)]
pub struct GmQuestView {
    pub quests: Vec<GmQuestRow>,
    /// 逐任务改写的乐观锁：界面带着它提交，库里变了就整笔拒。
    pub revision: String,
    /// 角色当前等级：一键完成主线的目标等级上限。
    pub level: u8,
    /// 任务这一侧能不能开工（存档身份不是当前契约时为 false，并给出理由）。
    pub usable: bool,
    pub unusable_reason: String,
    /// 产物里的归档指纹，界面上用它说明「这份目录是哪一次构建」。
    pub source_checksum: String,
}

/// 空名 = 构建期折叠时这一条确实没有可显示文本；照 Go 的兜底话显示 id。
fn display_name(file: &QuestFile, id: u32) -> String {
    match file.quests.get(&id).filter(|q| !q.name.is_empty()) {
        Some(q) => q.name.clone(),
        None => format!("未解析名称 #{id}"),
    }
}

pub(crate) fn build_view(file: &QuestFile, role: &Role, states: &[QuestState]) -> Result<GmQuestView, String> {
    let identity = gmstat::save_identity();
    let state: Value = serde_json::from_slice(&role.state).unwrap_or(Value::Null);
    let level = if state.is_null() {
        0
    } else {
        view_of(&state)?.level.min(u8::MAX as u32) as u8
    };
    let mut rows: Vec<GmQuestRow> = Vec::with_capacity(states.len());
    let prior: HashMap<u32, QuestState> = states.iter().map(|q| (u32::from(q.id), q.clone())).collect();
    for q in states {
        let entry = file.quests.get(&u32::from(q.id));
        let model = entry.map(|e| e.model.clone()).unwrap_or_default();
        let exists = entry.is_some();
        let editable = exists
            && q.status == "accepted"
            && q.config_version == identity
            && role.config_version == identity;
        let mut blocked = String::new();
        if !exists {
            blocked = "当前目录没有此任务".into();
        } else if q.config_version != identity || role.config_version != identity {
            blocked = "任务存档与目录版本不匹配".into();
        } else if model.is_empty() || q.progress_model != model {
            blocked = "此任务进度暂不支持强制修改".into();
        }
        let mut status = q.status.clone();
        if q.status == "accepted" && q.progress == 0 && blocked.is_empty() {
            status = "ready".into();
        }
        let groups = file.prerequisite_groups_of(u32::from(q.id));
        let prerequisites = groups
            .iter()
            .map(|group| {
                group
                    .iter()
                    .map(|pre| GmQuestPre {
                        id: *pre,
                        name: display_name(file, *pre),
                        completed: prior
                            .get(pre)
                            .map(|x| x.status == "completed" && x.config_version == identity)
                            .unwrap_or(false),
                    })
                    .collect()
            })
            .collect();
        let e = entry.cloned().unwrap_or_default();
        rows.push(GmQuestRow {
            id: q.id,
            name: display_name(file, u32::from(q.id)),
            grade: e.grade_texts.join("\n"),
            level: e.minimum_level,
            status,
            progress: q.progress,
            description: e.depend,
            condition: e.condition,
            solve: e.solve,
            prerequisites,
            can_abandon: editable,
            can_modify: editable && blocked.is_empty(),
            can_trace: editable && !groups.is_empty(),
            blocked,
        });
    }
    let usable = role.config_version == identity;
    Ok(GmQuestView {
        quests: rows,
        revision: quest_revision(role, states)?,
        level,
        usable,
        unusable_reason: if usable {
            String::new()
        } else {
            "任务目录与角色版本不匹配".into()
        },
        source_checksum: file.source.checksum.clone(),
    })
}

// ---------------------------------------------------------------------------
// 命令层
// ---------------------------------------------------------------------------

fn view(root: &Root, account: i64, character: i64) -> Result<GmQuestView, String> {
    if account <= 0 || character <= 0 {
        return Err("请选择账号和角色".into());
    }
    let file = load(root)?;
    let db = crate::saves::current_db(root)
        .ok_or_else(|| "存档还没有 SQLite 档：先启动一次游戏".to_string())?;
    let conn = crate::gmchar::open_ro(&db)?;
    let role = read_role(&conn, account, character)?;
    let states = read_states(&conn, role.id)?;
    build_view(&file, &role, &states)
}

pub(crate) fn preview_mainline(root: &Root, account: i64, character: i64, level: u8) -> Result<GmQuestPlan, String> {
    if account <= 0 || character <= 0 {
        return Err("请选择账号和角色".into());
    }
    let file = load(root)?;
    let ctx = Context::load(root)?;
    let db = crate::saves::current_db(root)
        .ok_or_else(|| "存档还没有 SQLite 档：先启动一次游戏".to_string())?;
    let conn = crate::gmchar::open_ro(&db)?;
    let role = read_role(&conn, account, character)?;
    let states = read_states(&conn, role.id)?;
    mainline_plan(&file, &ctx, &role, &states, level)
}

pub(crate) fn preview_prerequisites(root: &Root, account: i64, character: i64, target: u16) -> Result<GmQuestPlan, String> {
    if account <= 0 || character <= 0 {
        return Err("请选择账号和角色".into());
    }
    let file = load(root)?;
    let ctx = Context::load(root)?;
    let db = crate::saves::current_db(root)
        .ok_or_else(|| "存档还没有 SQLite 档：先启动一次游戏".to_string())?;
    let conn = crate::gmchar::open_ro(&db)?;
    let role = read_role(&conn, account, character)?;
    let states = read_states(&conn, role.id)?;
    prerequisite_plan(&file, &ctx, &role, &states, target)
}

// ---------------------------------------------------------------------------
// 写侧：跑在 `gmgrant` 的管理事务里（租约 / 流水号 / 审计都由那边负责）
// ---------------------------------------------------------------------------

/// Go `manage` 的动作白名单里任务这五个。
pub(crate) const QUEST_ACTIONS: [&str; 5] = [
    "quests.ready",
    "quests.complete",
    "quests.abandon",
    "quests.mainline",
    "quests.prerequisites",
];

pub(crate) fn is_quest_action(action: &str) -> bool {
    QUEST_ACTIONS.contains(&action)
}

const SQL_QUEST_ONE: &str = "SELECT quest_id,status,progress,config_version,progress_model \
 FROM character_quests WHERE character_id=?1 AND quest_id=?2";
const SQL_ABANDON: &str = "DELETE FROM character_quests WHERE character_id=?1 AND quest_id=?2";
const SQL_READY: &str = "UPDATE character_quests SET progress=0 WHERE character_id=?1 AND quest_id=?2";
/// 与 Go 的 `mainlineSkipSQL` 同一份语义，但写成逐条 VALUES 形态。Go 那边用
/// `INSERT ... SELECT json_each ... ON CONFLICT`，必须给 SELECT 补一个 `WHERE 1`，
/// 否则 SQLite 的语法分析器把 `ON` 读成连表的 `JOIN ... ON`（实测在 DO 处报
/// `near "DO": syntax error`）。VALUES 形态没有这个歧义，落库结果逐条一致 ——
/// 包括 `DO UPDATE ... WHERE` 不成立时 SQLite 报 0 行这一条。
/// 那条 0 行口径由 `gmgrant` 的整链路用例直接打在内存库上钉住。
pub(crate) const SQL_MAINLINE_SKIP: &str = "INSERT INTO character_quests(character_id,quest_id,status,progress,config_version,progress_model,completed_at) \
 VALUES(?1,?2,'completed',0,?3,'gm-mainline-skip-v1',(CAST((julianday('now') - 2440587.5) * 86400000000 AS INTEGER))) \
 ON CONFLICT(character_id,quest_id) DO UPDATE SET status='completed',progress=0,\
 completed_at=(CAST((julianday('now') - 2440587.5) * 86400000000 AS INTEGER)) \
 WHERE character_quests.config_version=EXCLUDED.config_version AND character_quests.status='accepted'";
const SQL_COMPLETE_ONE: &str = "UPDATE character_quests SET status='completed',progress=0,\
 completed_at=(CAST((julianday('now') - 2440587.5) * 86400000000 AS INTEGER)) WHERE character_id=?1 AND quest_id=?2";

/// `quests.*` 五个动作的本体。`role` 与 `states` 都由调用方在同一事务里取好，
/// 免得「按 A 版本校验、按 B 版本写入」。
pub(crate) fn apply(
    conn: &Connection,
    file: &QuestFile,
    ctx: Option<&Context>,
    role: &Role,
    input: &crate::gmgrant::ManageInput,
) -> Result<(Value, String, String), String> {
    let states = read_states(conn, role.id)?;
    if input.action == "quests.mainline" || input.action == "quests.prerequisites" {
        return apply_plan(conn, file, ctx, role, &states, input);
    }
    if !input.revision.is_empty() && quest_revision(role, &states)? != input.revision {
        return Err("角色或任务已变化，请刷新任务后再操作".into());
    }
    identity_gate(role)?;
    let mut before: Vec<QuestState> = Vec::new();
    let mut seen: HashSet<u16> = HashSet::new();
    for id in &input.quests {
        if !seen.insert(*id) {
            continue;
        }
        if file.quests.get(&u32::from(*id)).is_none() {
            return Err(format!("任务 {id} 不在当前目录"));
        }
        let q = conn
            .query_row(SQL_QUEST_ONE, params![role.id, id], state_of)
            .map_err(|_| format!("任务 {id} 尚未接取"))?;
        if q.status != "accepted" || q.config_version != gmstat::save_identity() {
            return Err(format!("任务 {id} 不处于可修改的进行中状态"));
        }
        before.push(q.clone());
        if input.action == "quests.abandon" {
            conn.execute(SQL_ABANDON, params![role.id, id]).map_err(|e| e.to_string())?;
            continue;
        }
        let model = file.quests[&u32::from(*id)].model.clone();
        if model.is_empty() || q.progress_model != model {
            return Err(format!("任务 {id} 的进度模型尚未闭环，拒绝强制修改"));
        }
        if input.action == "quests.ready" {
            conn.execute(SQL_READY, params![role.id, id]).map_err(|e| e.to_string())?;
        } else {
            conn.execute(SQL_COMPLETE_ONE, params![role.id, id]).map_err(|e| e.to_string())?;
        }
    }
    let receipt = serde_json::json!({
        "action": input.action,
        "before": before,
        "quests": input.quests,
        "rewards": false,
    });
    Ok((receipt, "修改已保存并记录审计".into(), String::new()))
}

/// 计划类动作：在同一事务里重算计划，与界面带来的签名逐字比对，才允许落库。
fn apply_plan(
    conn: &Connection,
    file: &QuestFile,
    ctx: Option<&Context>,
    role: &Role,
    states: &[QuestState],
    input: &crate::gmgrant::ManageInput,
) -> Result<(Value, String, String), String> {
    let ctx = ctx.ok_or_else(|| "养成产物未就绪：gm-growth.json".to_string())?;
    let prereq = input.action == "quests.prerequisites";
    let plan = if prereq {
        if input.quests.len() != 1 {
            return Err("追溯前置需要选择一个任务".into());
        }
        prerequisite_plan(file, ctx, role, states, input.quests[0])?
    } else {
        mainline_plan(file, ctx, role, states, input.level)?
    };
    if input.plan.is_empty() || input.plan != plan.signature {
        return Err("角色或任务已变化，请重新预览任务范围".into());
    }
    if plan.quests.is_empty() {
        return Err("没有符合条件的未完成任务".into());
    }
    let mut changed = 0usize;
    for id in &plan.quests {
        // DO UPDATE 的 WHERE 不成立（这一行属于别的存档身份，或早就不是 accepted）
        // 时 SQLite 报 0 行，与 Go 的 RowsAffected 口径一致。
        changed += conn
            .execute(SQL_MAINLINE_SKIP, params![role.id, id, role.config_version])
            .map_err(|e| e.to_string())?;
    }
    if changed != plan.quests.len() {
        return Err("任务状态发生变化，整组操作已撤销".into());
    }
    let receipt = serde_json::json!({ "action": input.action, "plan": plan, "rewards": false });
    let message = if prereq {
        "前置任务已按预览完成，未发放奖励，目标任务保留；重新选择角色后生效"
    } else {
        "主线任务已按预览完成，未发放奖励；重新选择角色后生效"
    };
    Ok((receipt, message.into(), String::new()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn gm_quests(
    state: tauri::State<'_, crate::commands::AppState>,
    account: i64,
    character: i64,
) -> Result<GmQuestView, String> {
    let root = state.root.clone();
    crate::commands::join(move || view(&root, account, character)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn gm_quest_preview_mainline(
    state: tauri::State<'_, crate::commands::AppState>,
    account: i64,
    character: i64,
    level: u8,
) -> Result<GmQuestPlan, String> {
    let root = state.root.clone();
    crate::commands::join(move || preview_mainline(&root, account, character, level)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn gm_quest_preview_prerequisites(
    state: tauri::State<'_, crate::commands::AppState>,
    account: i64,
    character: i64,
    target: u16,
) -> Result<GmQuestPlan, String> {
    let root = state.root.clone();
    crate::commands::join(move || preview_prerequisites(&root, account, character, target)).await
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) fn test_quest_file() -> QuestFile {
    use gmbuild::growth::SourceSnapshot;
    let mut quests: BTreeMap<u32, QuestEntry> = BTreeMap::new();
    for (id, e) in [
        (100u32, epic(100)),
        (101, { let mut e = epic(101); e.prerequisite_groups = vec![vec![100]]; e }),
        (102, { let mut e = epic(102); e.prerequisite_groups = vec![vec![101]]; e }),
        (103, { let mut e = epic(103); e.prerequisite_groups = vec![vec![100], vec![101]]; e }),
        (104, { let mut e = epic(104); e.prerequisite_groups = vec![vec![404]]; e }),
        (105, { let mut e = epic(105); e.collisions = vec![300]; e }),
        (106, { let mut e = epic(106); e.prerequisite_groups = vec![vec![105]]; e }),
        (108, { let mut e = epic(108); e.prerequisite_groups = vec![vec![201]]; e }),
        (109, { let mut e = epic(109); e.minimum_level = 60; e }),
        (110, { let mut e = epic(110); e.jobs = vec!["[gunner]".into()]; e }),
        (150, { let mut e = epic(150); e.grade_texts = vec!["[normal]".into()]; e }),
        (200, { let mut e = epic(200); e.answers = vec![gmbuild::quest::QuestAnswer { question: 150, index: 0 }]; e }),
        (201, { let mut e = epic(201); e.answers = vec![gmbuild::quest::QuestAnswer { question: 150, index: 1 }]; e }),
        (300, { let mut e = epic(300); e.collisions = vec![400]; e }),
        (400, { let mut e = epic(400); e.grade_texts = vec!["[normal]".into()]; e }),
        (500, { let mut e = epic(500); e.pending = vec!["[reward] 读不进去".into()]; e }),
        (600, { let mut e = epic(600); e.grow_usable = false; e.prerequisite_groups = vec![vec![601]]; e }),
        (601, { let mut e = epic(601); e.prerequisite_groups = vec![vec![600]]; e }),
        // 纯循环对（两道可用性门都过），只用来钉「循环前置不擅自解开」这一条。
        (610, { let mut e = epic(610); e.prerequisite_groups = vec![vec![611]]; e }),
        (611, { let mut e = epic(611); e.prerequisite_groups = vec![vec![610]]; e }),
        (700, {
            let mut e = epic(700);
            e.target_present = true;
            e.target_valid = false;
            e
        }),
        (800, { let mut e = epic(800); e.minimum_level = 60; e }),
        (900, { let mut e = epic(900); e.grade_texts = vec!["[legend]".into()]; e }),
    ] {
        quests.insert(id, e);
    }
    QuestFile {
        source: SourceSnapshot {
            format: "dfo_20260901".into(),
            path: "Script.pvf".into(),
            size: 1,
            checksum: crate::gmstat::TEST_HASH.into(),
            built_at: "2026-10-21T00:00:00Z".into(),
        },
        index_sha256: "b".repeat(64),
        quests,
    }
}

/// 夹具的默认形态：主线 `[epic]`、无职业限制、等级要求 1、进度模型已认。
/// 各用例只改自己要看的那一个判据。
#[cfg(test)]
fn epic(id: u32) -> QuestEntry {
    QuestEntry {
        path: format!("n_quest/fixture_{id}.qst"),
        sha256: crate::gmstat::TEST_HASH.into(),
        minimum_level: 1,
        jobs: vec!["[all]".into()],
        grade_texts: vec!["[epic]".into()],
        grow_usable: true,
        reward_usable: true,
        model: "single-meet-npc-remaining-v1".into(),
        target_valid: true,
        name: format!("任务 {id}"),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gmbuild::quest::{QuestAnswer, TargetChar};

    fn identity() -> String {
        gmstat::save_identity()
    }

    /// 与 `epic()` 的 model 同一条口径，用例里只写 status。
    fn st(id: u16, status: &str) -> QuestState {
        QuestState {
            id,
            status: status.into(),
            progress: if status == "accepted" { 3 } else { 0 },
            config_version: identity(),
            progress_model: "single-meet-npc-remaining-v1".into(),
        }
    }

    fn role(level: u32, advancement: u8, awakening: u8, config: &str) -> Role {
        let state = format!(
            "{{\"level\":{level},\"advancement\":{advancement},\"awakening\":{awakening},\"source_sha256\":\"{}\"}}",
            crate::gmstat::TEST_HASH
        );
        Role {
            id: 10,
            account_id: 1,
            profession: 0,
            config_version: config.into(),
            state: state.into_bytes(),
        }
    }

    fn ctx() -> Context {
        crate::gmstat::test_context()
    }

    #[test]
    fn mainline_closes_prerequisite_chain_and_blocks_the_other_answer() {
        let file = test_quest_file();
        let states = vec![st(400, "completed"), st(101, "accepted")];
        let plan = mainline_plan(&file, &ctx(), &role(50, 0, 0, &identity()), &states, 50).unwrap();
        // 逐层闭合：100 → 101 → 102；103 的替代段有任一段满足即可；105/106 的互斥同伴
        // 300 本身被挡，所以不会占用它；200 是问答 150 里 id 最小的成员，选中它。
        assert_eq!(plan.quests, vec![100, 101, 102, 103, 105, 106, 200]);
        // 201 被答案门挡掉；300 的互斥同伴 400 已完成；104/108/601/610/611 的前置链
        // 闭不上；500 有 pending、600 的 [grow type] 形态不认识、700 的目标角色段
        // 读不进 —— 全部如实报出。
        assert_eq!(
            plan.blocked,
            vec![104, 108, 201, 300, 500, 600, 601, 610, 611, 700]
        );
        // 800/900/109/110 连候选都不是（等级不够、不是 epic、职业不符），既不选也不进 blocked。
        let mut picks = BTreeMap::new();
        picks.insert(150u16, 0u16);
        assert_eq!(plan.answer_picks, Some(picks));
        assert_eq!(plan.before, vec![st(101, "accepted")]);
        assert_eq!(plan.level, 50);
        assert_eq!(plan.signature.len(), 64);
    }

    #[test]
    fn mainline_signature_is_reproducible_and_input_sensitive() {
        let file = test_quest_file();
        let states = vec![st(400, "completed")];
        let r = role(50, 0, 0, &identity());
        let a = mainline_plan(&file, &ctx(), &r, &states, 50).unwrap();
        let b = mainline_plan(&file, &ctx(), &r, &states, 50).unwrap();
        assert_eq!(a.signature, b.signature);
        // 换目标等级：候选集变了，签名必须跟着变（预览与执行比对的就是它）。
        let c = mainline_plan(&file, &ctx(), &r, &states, 49).unwrap();
        assert_ne!(a.signature, c.signature);
        // 任务状态变了也算换了计划。
        let d = mainline_plan(&file, &ctx(), &r, &[st(100, "completed")], 50).unwrap();
        assert_ne!(a.signature, d.signature);
        // 转职后 `[grow type]` 的判定域变了：同一批任务，签名不同。
        let advanced = role(50, 1, 0, &identity());
        let e = mainline_plan(&file, &ctx(), &advanced, &states, 50).unwrap();
        assert_ne!(a.signature, e.signature);
    }

    #[test]
    fn mainline_refusals_match_go() {
        let file = test_quest_file();
        let states = vec![st(400, "completed")];
        let good = role(50, 0, 0, &identity());
        // 目标等级为 0 或高于角色当前等级。
        for level in [0u8, 51, 200] {
            assert_eq!(
                mainline_plan(&file, &ctx(), &good, &states, level).unwrap_err(),
                "主线目标等级必须在 1 至角色当前等级之间",
                "level={level}"
            );
        }
        // 存档身份不是当前契约：任务这一侧没有「归档校验值」那条旧身份分支。
        assert_eq!(
            mainline_plan(&file, &ctx(), &role(50, 0, 0, crate::gmstat::TEST_HASH), &states, 50).unwrap_err(),
            "任务目录与角色版本不匹配"
        );
        // 职业配置缺失（Go 的 `job == ""`）。
        let mut bare = ctx();
        bare.growth.professions.get_mut(&0).unwrap().job = String::new();
        assert_eq!(
            mainline_plan(&file, &bare, &good, &states, 50).unwrap_err(),
            "角色职业配置缺失"
        );
        // 职业目录与存档不匹配：`state.source_sha256` 与 `.chr` 原文指纹不符。
        let mut tampered = good;
        tampered.state = br#"{"level":50,"advancement":0,"awakening":0,"source_sha256":"deadbeef"}"#.to_vec();
        assert_eq!(
            mainline_plan(&file, &ctx(), &tampered, &states, 50).unwrap_err(),
            "当前 PVF 职业目录与角色存档不匹配，已拒绝修改"
        );
    }

    #[test]
    fn prerequisite_plan_traces_only_the_unmet_chain() {
        let file = test_quest_file();
        let states = vec![st(100, "completed"), st(101, "accepted"), st(102, "accepted")];
        let plan = prerequisite_plan(&file, &ctx(), &role(50, 0, 0, &identity()), &states, 102).unwrap();
        // 101 是 102 唯一未闭合的前置；100 已完成，102 自己（目标任务）保留不动。
        assert_eq!(plan.quests, vec![101]);
        assert!(plan.blocked.is_empty());
        // Go 的追溯把整份现状塞进 before（回执要能整组撤销）。
        assert_eq!(plan.before, states);
        assert_eq!(plan.level, 0);
        assert_eq!(plan.signature.len(), 64);
    }

    #[test]
    fn prerequisite_refusals_match_go() {
        let file = test_quest_file();
        let r = role(50, 0, 0, &identity());
        let err = |states: Vec<QuestState>, target: u16| {
            prerequisite_plan(&file, &ctx(), &r, &states, target).unwrap_err()
        };
        // 目标任务不处于进行中的当前存档身份。
        assert_eq!(err(vec![st(102, "completed")], 102), "请选择当前角色进行中的任务");
        assert_eq!(
            err(vec![{ let mut q = st(102, "accepted"); q.config_version = "other".into(); q }], 102),
            "请选择当前角色进行中的任务"
        );
        // 多条未满足的替代链：不擅自选边。
        assert_eq!(
            err(vec![st(103, "accepted")], 103),
            "任务 103 有多条可选前置链，请先在游戏中确定分支"
        );
        // 循环前置。
        assert_eq!(
            err(vec![st(610, "accepted"), st(611, "accepted")], 610),
            "任务 610 存在循环前置，未作修改"
        );
        // 前置行属于别的存档身份。
        assert_eq!(
            err(vec![st(104, "accepted"), { let mut q = st(404, "completed"); q.config_version = "other".into(); q }], 104),
            "前置任务 404 版本不匹配"
        );
        // 前置的互斥同伴已被库里占用（accepted 也算占用）。
        assert_eq!(
            err(vec![st(106, "accepted"), st(105, "accepted"), st(300, "accepted")], 106),
            "任务 105 与已接受或已完成的分支互斥，未作修改"
        );
        // 前置与库里已选的另一答案分支互斥。
        assert_eq!(
            err(vec![st(108, "accepted"), st(201, "accepted"), st(200, "accepted")], 108),
            "任务 201 与已选分支（问答 150 的第 0 个答案）互斥，未作修改"
        );
        // 已经闭合的链不重复补：前置完成 → 本次没有要做的行。
        let closed = prerequisite_plan(&file, &ctx(), &r, &[st(106, "accepted"), st(105, "completed")], 106).unwrap();
        assert!(closed.quests.is_empty());
        // 等级、职业两道资格门，以及读不进去的段落。
        assert_eq!(err(vec![st(109, "accepted")], 109), "任务 109 不满足当前角色职业或等级条件");
        assert_eq!(err(vec![st(110, "accepted")], 110), "任务 110 不满足当前角色职业或等级条件");
        assert_eq!(err(vec![st(700, "accepted")], 700), "任务 700 的目标角色条件待核实");
        assert_eq!(err(vec![st(600, "accepted")], 600), "任务 600 的前置配置待核实");
    }

    #[test]
    fn target_character_triples_are_or_groups_and_honour_awakening() {
        let file = {
            let mut f = test_quest_file();
            let mut e = epic(120);
            e.target_present = true;
            e.target_triples = vec![
                TargetChar { job: "[swordman]".into(), advancement: 1, awakening: -1 },
                TargetChar { job: "[all]".into(), advancement: -1, awakening: 2 },
            ];
            f.quests.insert(120, e);
            f
        };
        let states = vec![st(400, "completed")];
        // 两组判据是「任选其一」：本命职业 + 分支 1；或通用 + 任意分支但必须二觉。
        let plan = mainline_plan(&file, &ctx(), &role(50, 1, 0, &identity()), &states, 50).unwrap();
        assert!(plan.quests.contains(&120));
        let plan = mainline_plan(&file, &ctx(), &role(50, 3, 2, &identity()), &states, 50).unwrap();
        assert!(plan.quests.contains(&120));
        let plan = mainline_plan(&file, &ctx(), &role(50, 2, 0, &identity()), &states, 50).unwrap();
        assert!(!plan.quests.contains(&120));
        assert!(!plan.blocked.contains(&120), "目标角色不匹配是排除，不是待核实");
    }

    #[test]
    fn revision_changes_only_with_the_inputs_go_hashes() {
        let r = role(50, 0, 0, &identity());
        let one = st(100, "accepted");
        let a = quest_revision(&r, &[one.clone()]).unwrap();
        assert_eq!(a, quest_revision(&r, &[one.clone()]).unwrap());
        // 进度、状态、任务集合、职业、存档身份、state 原文任一变化都要换指纹。
        let mut moved = one.clone();
        moved.progress = 4;
        assert_ne!(a, quest_revision(&r, &[moved.clone()]).unwrap());
        assert_ne!(a, quest_revision(&r, &[st(100, "completed")]).unwrap());
        assert_ne!(a, quest_revision(&r, &[one.clone(), moved]).unwrap());
        assert_ne!(a, quest_revision(&role(50, 1, 0, &identity()), &[one.clone()]).unwrap());
        assert_ne!(a, quest_revision(&role(50, 0, 0, "other"), &[one.clone()]).unwrap());
        assert_ne!(a, quest_revision(&role(51, 0, 0, &identity()), &[one]).unwrap());
    }

    #[test]
    fn view_flags_match_go() {
        let file = test_quest_file();
        let r = role(50, 0, 0, &identity());
        let states = vec![
            st(100, "accepted"),
            {
                let mut q = st(101, "accepted");
                q.progress = 0;
                q
            },
            {
                // 模型与产物不符：进度不可强制修改，但仍可放弃。
                let mut q = st(102, "accepted");
                q.progress_model = "legacy-zero".into();
                q
            },
            {
                // 目录里没有这一条。
                let mut q = st(999, "accepted");
                q.progress = 0;
                q
            },
            {
                let mut q = st(200, "accepted");
                q.config_version = "other-pack".into();
                q
            },
        ];
        let rows = build_view(&file, &r, &states).unwrap();
        let row = |id: u16| rows.quests.iter().find(|x| x.id == id).unwrap().clone();
        let a = row(100);
        assert_eq!(a.status, "accepted");
        assert_eq!(a.progress, 3);
        assert!(a.can_abandon && a.can_modify && !a.can_trace);
        assert_eq!(a.name, "任务 100");
        assert_eq!(a.grade, "[epic]");
        let b = row(101);
        assert_eq!(b.status, "ready", "进度归零且可改时 Go 报 ready");
        assert!(b.can_trace, "101 有前置组");
        assert_eq!(b.prerequisites[0][0].id, 100);
        assert!(!b.prerequisites[0][0].completed);
        let c = row(102);
        assert_eq!(c.blocked, "此任务进度暂不支持强制修改");
        assert!(c.can_abandon && !c.can_modify);
        let d = row(999);
        assert_eq!(d.blocked, "当前目录没有此任务");
        assert_eq!(d.name, "未解析名称 #999");
        assert!(!d.can_abandon && !d.can_modify);
        let e = row(200);
        assert_eq!(e.blocked, "任务存档与目录版本不匹配");
        assert!(rows.usable && rows.unusable_reason.is_empty());
        assert_eq!(rows.level, 50);
        assert_eq!(rows.source_checksum, crate::gmstat::TEST_HASH);
        // 存档身份不符时整侧不可用，每一行也都被挡住。
        let stale = build_view(&file, &role(50, 0, 0, "other"), &[st(100, "accepted")]).unwrap();
        assert!(!stale.usable);
        assert_eq!(stale.quests[0].blocked, "任务存档与目录版本不匹配");
    }

    #[test]
    fn answer_gate_picks_the_lowest_id_member() {
        // 同一问答的两个分支：只有 id 小的那个会被选中，口径是「按 id 升序取第一次
        // 出现的答案索引」，所以结果不随遍历顺序漂移。
        let mut f = test_quest_file();
        let mut low = epic(210);
        low.answers = vec![QuestAnswer { question: 151, index: 2 }];
        let mut high = epic(211);
        high.answers = vec![QuestAnswer { question: 151, index: 0 }];
        f.quests.insert(210, low);
        f.quests.insert(211, high);
        let states = vec![st(400, "completed")];
        let plan = mainline_plan(&f, &ctx(), &role(50, 0, 0, &identity()), &states, 50).unwrap();
        assert!(plan.quests.contains(&210));
        assert!(plan.blocked.contains(&211));
        assert_eq!(plan.answer_picks.unwrap().get(&151), Some(&2));
    }

    #[test]
    fn empty_plan_when_everything_is_done() {
        // 全部候选都已完成时计划是空的：执行侧据此整笔拒
        // （Go 的「没有符合条件的未完成任务」），界面不会看到一次「成功但什么都没做」。
        let mut f = test_quest_file();
        f.quests.retain(|id, q| q.grade_texts == ["[epic]".to_string()] && *id <= 102);
        let states = vec![st(100, "completed"), st(101, "completed"), st(102, "completed")];
        let plan = mainline_plan(&f, &ctx(), &role(50, 0, 0, &identity()), &states, 50).unwrap();
        assert!(plan.quests.is_empty());
        assert!(plan.blocked.is_empty());
        assert!(plan.before.is_empty());
        assert_eq!(plan.signature.len(), 64);
    }
}
