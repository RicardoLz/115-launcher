//! 邮件发放（#26）：只写玩家侧的 `character_mail`，与 Go 服务端已上线的 `mail_send`
//! 同一条落库口径 —— 署名固定 `GM`、15 天期限、附件与消息共用一个号段。
//!
//! 为什么不是 `gm_mail`：那张表是旧管理台自己的待发放队列，现役服务端里一条读写
//! 它的 SQL 都没有（Go 链接期把没人调用的函数连同 SQL 字面量一起剥掉了），玩家永远
//! 看不到它。`character_mail` 才是 CMD96 读取、NOTI99 提醒的那张表。
//!
//! 附件的 JSON 形状全部按 Go 的结构体逐字段复刻（键名大小写、缺省省略、键序）：
//! 服务端 `json.Unmarshal` 进 `mail.Asset` / `inventory.MailItem`，任何一侧自创的键
//! 都会让它读不出实例。可发的只有金币、可堆叠材料与时装（容器字节 1）；
//! 普通装备实例要真实例字节，属于 #27，这一轮在参数门就拒。

use rusqlite::{params, Transaction};
use serde::Serialize;

use crate::gmgrant::{is_zero_u8, is_zero_u32, ManageInput};

/// Go `mail.MaxMessages` / `MaxUnclaimedAssets` / `MaxAttachments`。
const MAX_MESSAGES: i64 = 255;
const MAX_UNCLAIMED_ASSETS: i64 = 255;
const MAX_ATTACHMENTS: usize = 11;
/// Go `grantGMItem` 里非堆叠那一条的门：装备每批最多 9 件，一件一个附件。
const MAX_EQUIPMENT_BATCH: u32 = 9;
/// `character_mail.body` 的 CHECK 上限；Go `GMSystemMail` 在写库前同样按字节拒。
/// 拒在 `normalize_manage`（开事务之前），被拒的流水号才不会占用号段。
pub(crate) const MAX_BODY_BYTES: usize = 512;
/// Go `inventory.GrantExpireTime` = `protocol.MaxItemPeriod` = `math.MaxInt32`。
/// 写 0 会让带期限的物品一进背包就显示「剩余期限已过」。
const GRANT_EXPIRE_TIME: u32 = 2_147_483_647;
/// Go `GMSystemMail` 写死的署名（`sender_name` 的 CHECK 上限 29 字节，这里用不到）。
const SENDER: &str = "GM";

/// Go `MailboxCapacity`：活着的消息数 + 未领取的附件数（`$.claimed` 为假才算未领）。
/// 过期但 `status=3`（已存入）的那一封仍算在内，与读侧同一条 WHERE。
const SQL_MAILBOX_CAPACITY: &str = "SELECT count(*), CAST(coalesce(sum((SELECT count(*) \
    FROM json_each(m.assets) a WHERE NOT coalesce(json_extract(a.value,'$.claimed'),0))),0) AS INTEGER) \
    FROM character_mail m WHERE recipient_id=?1 AND deleted_at IS NULL \
    AND (expires_at>(CAST((julianday('now') - 2440587.5) * 86400000000 AS INTEGER)) OR status=3)";
/// Go `InsertSystemMail`，逐字照抄：不写 `sender_id`（NULL = 系统邮件）、不写 `status`
/// （DDL 默认 1 = 未读）。SQLite 没有 interval 类型，15 天期限只能按这个整数微秒域算，
/// 所以末尾那个加数就是 `mail.RetentionDays`(15) 换成微秒：1296000000000。
const SQL_INSERT_MAIL: &str = "INSERT INTO character_mail(recipient_id,sender_name,body,assets,expires_at) \
    VALUES(?1,?2,?3,?4,(CAST((julianday('now') - 2440587.5) * 86400000000 AS INTEGER) + 1296000000000)) RETURNING id";
const SQL_SEQUENCE_BUMP: &str = "UPDATE sqlite_sequence SET seq = seq + 1 WHERE name = 'character_mail'";
const SQL_SEQUENCE_OPEN: &str = "INSERT INTO sqlite_sequence(name, seq) VALUES('character_mail', 1)";
const SQL_SEQUENCE_READ: &str = "SELECT seq FROM sqlite_sequence WHERE name = 'character_mail'";
const SQL_SEQUENCE_TABLE_PRESENT: &str =
    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='mailbox_id_sequence'";
const SQL_SEQUENCE_TABLE_ALIGN: &str =
    "UPDATE mailbox_id_sequence SET value = ?1 WHERE name = 'mailbox_id_seq' AND value < ?2";

/// Go `inventory.BagItem`：`Template`/`Amount` 没有标签，序列化出来就是大写键；
/// `slot` 与 `expire_time` 有标签。键序即字段声明序，与 Go 的 `json.Marshal` 一致。
#[derive(Clone, Debug, Serialize)]
pub(crate) struct MailStack {
    pub slot: u16,
    #[serde(rename = "Template")]
    pub template: u32,
    #[serde(rename = "Amount")]
    pub amount: u32,
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub expire_time: u32,
}

/// Go `inventory.BagEquipment` 里非 omitempty 的那三格 + `period`。
/// 时装邮件只带模板与期限，实例字节（`record`）留空 —— 服务端 `ValidateRecord` 认空。
#[derive(Clone, Debug, Serialize)]
pub(crate) struct MailEquipment {
    pub slot: u16,
    pub template: u32,
    pub durability: u16,
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub period: u32,
}

/// Go `inventory.MailItem`：`stack`/`equipment` 二选一，`space` 只有 0（普通背包）与 1（时装栏）。
#[derive(Clone, Debug, Serialize)]
pub(crate) struct MailItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<MailStack>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipment: Option<MailEquipment>,
    #[serde(skip_serializing_if = "is_zero_u8")]
    pub space: u8,
}

/// Go `mail.Asset`：`id` 无 omitempty（恒写），`gold`/`item` 缺省省略，
/// `claimed` 省略 = 未领取。领取与否都不由这一档写。
#[derive(Clone, Debug, Serialize)]
pub(crate) struct MailAsset {
    pub id: i64,
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub gold: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<MailItem>,
}

/// 物品 → 附件实例，口径逐条对 `cmd/gmtool/edition_operations.go` 的 `grantGMItem`（`Mail:true` 那半）：
/// 堆叠一件一个附件、期限打 `GrantExpireTime`、上限按定义（缺上限按规则里的兜底值）；
/// 时装按件拆成 N 个附件，容器字节 1；其余装备这一轮拒（要真实例，见 #27）。
/// 时装判据是 `GmItem::avatar()`：真产物 676,442 行的 `kind` 只有 stackable/equipment 两种值，
/// 时装完全靠部位旁表把 `slot` 覆盖成「时装」来认，所以这里不能只看 `kind`。
///
/// Go 那一条最后还要过 `MailItem.Row()`；这里不必复述它的门，因为形状是由分支结构
/// 本身保证的：堆叠支只带 `stack` 且容器留 0，时装支只带 `equipment` 且容器是 1，
/// 模板与件数的下限就是函数开头那一道。
pub(crate) fn build_items(
    cat: &gmbuild::gmindex::Catalog,
    missing_stack_limit: u32,
    template: u32,
    amount: u32,
) -> Result<Vec<MailItem>, String> {
    if template == 0 {
        // 只发金币那一支；调用方保证这时 amount 也是 0。
        return Ok(Vec::new());
    }
    if template < 2 || amount == 0 {
        return Err("邮件附件缺少真实物品或数量".into());
    }
    let item = cat
        .get(template)
        .ok_or_else(|| format!("物品 {template} 不在物品库里"))?;
    if item.kind == gmbuild::gmindex::Kind::Stackable {
        // Go：`limit := def.StackLimit; if limit == 0 { limit = a.Rules.MissingStackLimit }`。
        // 这里刻意不套背包发放那条「类型串含 unlimited 就放开到 i32::MAX」的例外：
        // 邮寄那一档在服务端走的是这道更紧的门，不是 `Bag.Add`。
        let limit = if item.stack_limit > 0 { item.stack_limit } else { missing_stack_limit };
        if amount > limit {
            return Err(format!("邮件数量超过堆叠上限{limit}"));
        }
        return Ok(vec![MailItem {
            stack: Some(MailStack {
                slot: 0,
                template,
                amount,
                expire_time: GRANT_EXPIRE_TIME,
            }),
            equipment: None,
            space: 0,
        }]);
    }
    if item.avatar() {
        if amount > MAX_EQUIPMENT_BATCH {
            return Err(format!("装备每批最多{MAX_EQUIPMENT_BATCH}件"));
        }
        let one = MailItem {
            stack: None,
            equipment: Some(MailEquipment {
                slot: 0,
                template,
                durability: 0,
                period: GRANT_EXPIRE_TIME,
            }),
            space: 1,
        };
        return Ok((0..amount).map(|_| one.clone()).collect());
    }
    Err(format!("装备实例发放未支持（见 #27）：物品 {template} 不是可堆叠材料或时装"))
}

/// Go `advanceMailSequence`：附件号在消息号**之前**发，两者共用一个号段。
/// AUTOINCREMENT 只在 INSERT 时分配，所以这里直接推进 `sqlite_sequence` 里那条高水位；
/// 事务内跑（本函数只在 `manage_inner` 的 `BEGIN IMMEDIATE` 里被调），读改写是原子的。
fn next_mail_id(tx: &Transaction<'_>) -> Result<i64, String> {
    let bumped = tx
        .execute(SQL_SEQUENCE_BUMP, [])
        .map_err(|e| format!("分配邮件编号失败：{e}"))?;
    if bumped == 0 {
        // 一张都没写过：号段还没开，第一条给 1。
        tx.execute(SQL_SEQUENCE_OPEN, [])
            .map_err(|e| format!("分配邮件编号失败：{e}"))?;
    }
    let id: i64 = tx
        .query_row(SQL_SEQUENCE_READ, [], |row| row.get(0))
        .map_err(|e| format!("读取邮件编号失败：{e}"))?;
    align_mail_sequence_table(tx, id)?;
    Ok(id)
}

/// 20261004 升级包把同一个号段存在 `mailbox_id_sequence` 里。这份存档带那张表时
/// 必须一起推进，否则两边的计数器会各自发出对方已经用过的号；不带就是空操作。
/// 与 Go 同：只把它往前推，**绝不往后拉**（它可能已经在我们前面）。
fn align_mail_sequence_table(tx: &Transaction<'_>, id: i64) -> Result<(), String> {
    let present: i64 = tx
        .query_row(SQL_SEQUENCE_TABLE_PRESENT, [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if present == 0 {
        return Ok(());
    }
    tx.execute(SQL_SEQUENCE_TABLE_ALIGN, params![id + 1, id + 1])
        .map_err(|e| format!("对齐邮件编号计数器失败：{e}"))?;
    Ok(())
}

/// Go `MailboxCapacity`：活着的消息数 + 未领取的附件数（`$.claimed` 为假才算未领）。
/// 过期但 `status=3`（已存入）的那一封仍算在内，与读侧同一条 WHERE。
fn mailbox_capacity(tx: &Transaction<'_>, recipient: i64) -> Result<(i64, i64), String> {
    tx.query_row(SQL_MAILBOX_CAPACITY, params![recipient], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })
    .map_err(|e| format!("读取收件箱容量失败：{e}"))
}

/// 邮件发放本体：容量门 → 逐个附件取号 → 序列化 → 插一封（`sender_id` 留 NULL = 系统邮件）。
/// 返回（回执, 给界面的话），落库与审计由 `manage_inner` 那一笔事务带着。
/// 纯参数门（正文长度、至少带一样）在 `normalize_manage`，开事务之前就拒掉了。
pub(crate) fn send(
    tx: &Transaction<'_>,
    cat: &gmbuild::gmindex::Catalog,
    missing_stack_limit: u32,
    recipient: i64,
    input: &ManageInput,
) -> Result<(serde_json::Value, String), String> {
    let mut assets: Vec<MailAsset> = Vec::new();
    if input.gold > 0 {
        assets.push(MailAsset { id: 0, gold: input.gold, item: None });
    }
    let items = build_items(cat, missing_stack_limit, input.template, input.amount)?;
    assets.extend(items.into_iter().map(|item| MailAsset { id: 0, gold: 0, item: Some(item) }));
    if assets.len() > MAX_ATTACHMENTS {
        // Go 的 `insertSystemMailTx` 第一道门就在这，容量检查之前。
        return Err("系统邮件附件过多".into());
    }

    let (messages, unclaimed) = mailbox_capacity(tx, recipient)?;
    if messages >= MAX_MESSAGES || unclaimed + assets.len() as i64 > MAX_UNCLAIMED_ASSETS {
        return Err("收件箱已满".into());
    }
    for asset in &mut assets {
        asset.id = next_mail_id(tx)?;
    }
    let encoded = serde_json::to_vec(&assets).map_err(|e| e.to_string())?;
    let mail_id: i64 = tx
        .query_row(SQL_INSERT_MAIL, params![recipient, SENDER, input.body, encoded], |row| {
            row.get(0)
        })
        .map_err(|e| format!("写入邮件失败：{e}"))?;

    let receipt = serde_json::json!({
        "action": input.action,
        "mail_id": mail_id,
        "recipient": recipient,
        "sender": SENDER,
        "body": input.body,
        "gold": input.gold,
        "template": input.template,
        "amount": input.amount,
        "assets": serde_json::from_slice::<serde_json::Value>(&encoded).unwrap_or(serde_json::Value::Null),
        "capacity_before": { "messages": messages, "unclaimed_assets": unclaimed },
    });
    Ok((receipt, "邮件已写入邮箱；停服发放，玩家重新进入游戏后可见".into()))
}
