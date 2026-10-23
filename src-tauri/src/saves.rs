//! 存档页：定位 SQLite 档、备份、替换、列出已备份的档。
//!
//! 规矩照旧版那条链，一步都不能省：候选先体检（文件头 + `integrity_check` + 必备表）；
//! 替换前必把当前档快照进 `backup/` 并验这份快照；改名落地后复检。启动器不写
//! `runtime/storage/local.json`、不碰客户端目录。删除面只有两块：当前档自己的空 sidecar，
//! 和 `backup/` 底下玩家点名要删的那一份 `.sqlite3`；非 0 字节的 `-wal` 永不动
//! （里面可能有已提交的数据），当前档也永远删不掉。

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use rusqlite::Connection;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::config::{self, StorageRoute};
use crate::paths::Root;

/// SQLite 库的固定文件头，认它比认扩展名可靠。
const MAGIC: &[u8; 16] = b"SQLite format 3\0";
/// 服务端建库时必然存在这三张表；缺任何一张就不是这套服务的存档。
const REQUIRED_TABLES: [&str; 3] = ["accounts", "characters", "storage_migrations"];
const JOURNAL_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-journal"];

#[derive(Serialize)]
pub struct SaveInfo {
    pub db_path: String,
    pub storage_dir: String,
    pub backup_dir: String,
    pub exists: bool,
    pub size_text: String,
    pub modified: String,
    pub accounts: usize,
    pub characters: usize,
    pub detail: String,
    pub sidecars: Vec<String>,
}

#[derive(Serialize)]
pub struct BackupItem {
    pub name: String,
    pub path: String,
    pub size_text: String,
    pub modified: String,
    pub usable: bool,
    pub note: String,
    pub accounts: usize,
    pub characters: usize,
    /// 这一份就是当前正在用的库。界面据此把「删除」按住：坏掉的备份能删，正在用的不能。
    pub is_current: bool,
}

#[derive(Serialize)]
pub struct SavesState {
    pub save: SaveInfo,
    pub backups: Vec<BackupItem>,
}

/// 一次体检的结论：里面有多少账号与角色。
#[derive(Clone, Copy, Default)]
struct Audit {
    accounts: usize,
    characters: usize,
}

pub fn storage_dir(root: &Root) -> PathBuf {
    root.runtime_dir().join("storage")
}

pub fn backup_dir(root: &Root) -> PathBuf {
    storage_dir(root).join("backup")
}

/// 当前库的路径：`local.json` 写了 `sqlite_path` 就以它为准。
/// `None` 表示这份包还没定下 SQLite 档（没初始化，或写的是别的驱动），此时页面只读不给操作。
pub(crate) fn current_db(root: &Root) -> Option<PathBuf> {
    match config::storage_route_state(root) {
        StorageRoute::Ready { path } => Some(path),
        StorageRoute::NeedsInit | StorageRoute::OtherDriver(_) => None,
    }
}

fn stamp() -> String {
    Local::now().format("%Y%m%d-%H%M%S").to_string()
}

fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    if bytes < 1024 * 1024 {
        return format!("{} KB", bytes / 1024);
    }
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

fn size_text(path: &Path) -> String {
    std::fs::metadata(path).map(|m| human_size(m.len())).unwrap_or_default()
}

fn modified_text(path: &Path) -> String {
    let Ok(at) = path.metadata().and_then(|m| m.modified()) else {
        return "时间未知".to_string();
    };
    DateTime::<Local>::from(at).format("%Y-%m-%d %H:%M").to_string()
}

fn open_ro(path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|err| format!("打不开这份存档（{}）：{err}", path.display()))
}

fn count(conn: &Connection, table: &str) -> Result<usize, String> {
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
        row.get::<_, i64>(0)
    })
    .map(|n| n.max(0) as usize)
    .map_err(|err| format!("数 {table} 表失败：{err}"))
}

fn table_present(conn: &Connection, name: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |row| row.get(0),
    )
    .map_err(|err| format!("查表 {name} 失败：{err}"))
}

/// 体检：文件头 → 只读打开 → `integrity_check` → 三张必备表 → 计数。
/// 任一步不过就把原因带回来，由调用方决定是拒绝替换还是在列表上标注。
fn audit(path: &Path) -> Result<Audit, String> {
    let mut head = [0_u8; 16];
    let mut file = File::open(path).map_err(|err| format!("读不到文件：{err}"))?;
    let got = file.read(&mut head).map_err(|err| format!("读文件头失败：{err}"))?;
    if got < MAGIC.len() || head != *MAGIC {
        return Err("文件头不是 SQLite 库，认不出这是存档".to_string());
    }
    drop(file);

    let conn = open_ro(path)?;
    let verdict: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|err| format!("完整性检查没能跑起来：{err}"))?;
    if verdict.trim() != "ok" {
        return Err(format!("完整性检查报：{}", verdict.trim()));
    }
    for name in REQUIRED_TABLES {
        if !table_present(&conn, name)? {
            return Err(format!("这份库里没有 {name} 表，不是本服务的存档"));
        }
    }
    Ok(Audit {
        accounts: count(&conn, "accounts")?,
        characters: count(&conn, "characters")?,
    })
}

fn sidecars(path: &Path) -> Vec<String> {
    let base = path.as_os_str().to_string_lossy().to_string();
    JOURNAL_SUFFIXES
        .iter()
        .filter(|suffix| Path::new(&format!("{base}{suffix}")).is_file())
        .map(|suffix| (*suffix).to_string())
        .collect()
}

fn sha256_of(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|err| format!("读不到 {}：{err}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let got = file.read(&mut buffer).map_err(|err| format!("读取失败：{err}"))?;
        if got == 0 {
            break;
        }
        digest.update(&buffer[..got]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>())
}

fn copy_candidate(from: &Path, to: &Path) -> Result<(), String> {
    let mut src = File::open(from).map_err(|err| format!("读不到候选：{err}"))?;
    let mut dst = File::options()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(|err| format!("建临时件失败（{}）：{err}", to.display()))?;
    std::io::copy(&mut src, &mut dst).map_err(|err| format!("拷贝失败：{err}"))?;
    dst.sync_all().map_err(|err| format!("落盘失败：{err}"))?;
    drop(dst);
    Ok(())
}

/// 旧库的 sidecar 必须跟着旧库走：留着，新库开起来会去套旧 WAL 里的帧。
fn remove_sidecars(path: &Path) -> Vec<String> {
    let base = path.as_os_str().to_string_lossy().to_string();
    JOURNAL_SUFFIXES
        .iter()
        .filter_map(|suffix| {
            let name = format!("{base}{suffix}");
            let side = Path::new(&name);
            match side.is_file() {
                true => std::fs::remove_file(side).ok().map(|_| (*suffix).to_string()),
                false => None,
            }
        })
        .collect()
}

/// 只清这轮自己弄出来的空 sidecar；非 0 字节的 `-wal` 里有已提交数据，绝不动。
fn sweep_empty_sidecars(path: &Path) {
    let base = path.as_os_str().to_string_lossy().to_string();
    for suffix in JOURNAL_SUFFIXES {
        let name = format!("{base}{suffix}");
        let side = Path::new(&name);
        let empty = side.is_file() && std::fs::metadata(side).map(|m| m.len() == 0).unwrap_or(false);
        if empty {
            let _ = std::fs::remove_file(side);
        }
    }
}

/// 同一个文件的两种写法（大小写、`..`、短名）不能算成两份不同的档。
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// 备份文件名里的库名主干，`dfolan.sqlite3` → `dfolan`。
fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "save".to_string())
}

/// 撞名就补 `-2`、`-3`，绝不覆盖已有的备份。
/// `ext` 交给调用方是因为这同一条规则也要给角色 state 的改前备份用（那是 `.json`，不是整库档）。
pub(crate) fn unique_backup_path(dir: &Path, base: &str, mark: &str, ext: &str) -> PathBuf {
    for index in 0..100 {
        let tail = if index == 0 {
            String::new()
        } else {
            format!("-{}", index + 1)
        };
        let path = dir.join(format!("{base}{mark}-{}{tail}.{ext}", stamp()));
        if !path.exists() {
            return path;
        }
    }
    dir.join(format!("{base}{mark}-{}.{ext}", stamp()))
}

/// 角色 state 的改前备份落点：整库备份目录下的 `state/` 子目录。
/// 不跟整库档平铺在同一处，是因为 `list_backups` 按 `.sqlite3` 认「能替换回去的档」——
/// 一份单角色 JSON 混进那张列表里，看起来就像一份可用的整库备份，那是误导。
pub fn role_state_backup_dir(root: &Root) -> PathBuf {
    backup_dir(root).join("state")
}

/// 只读连接做 `VACUUM INTO`，所以服务端正开着也能拿到已提交的那一面。
fn vacuum_into(db: &Path, target: &Path) -> Result<(), String> {
    let conn = open_ro(db)?;
    conn.execute("VACUUM INTO ?1", [target.to_string_lossy().as_ref()])
        .map_err(|err| format!("备份失败：{err}"))?;
    drop(conn);
    audit(target).map(|_| ())
}

fn list_backups(root: &Root, db: Option<&Path>) -> Vec<BackupItem> {
    let Ok(entries) = std::fs::read_dir(backup_dir(root)) else {
        return Vec::new();
    };
    let mut items: Vec<BackupItem> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let meta = entry.metadata().ok()?;
            if !meta.is_file() || path.extension().and_then(|e| e.to_str()) != Some("sqlite3") {
                return None;
            }
            let is_current = db.map(|seen| same_file(seen, &path)).unwrap_or(false);
            let (usable, note, accounts, characters) = match audit(&path) {
                Ok(ok) => (
                    true,
                    format!("账号 {} · 角色 {}", ok.accounts, ok.characters),
                    ok.accounts,
                    ok.characters,
                ),
                Err(err) => (false, err, 0, 0),
            };
            Some(BackupItem {
                name: path.file_name()?.to_string_lossy().to_string(),
                path: path.display().to_string(),
                size_text: human_size(meta.len()),
                modified: modified_text(&path),
                // 当前档自己也躺在 backup 里时，选它替换等于没换，所以按「不可用」标出。
                usable: usable && !is_current,
                note: if is_current {
                    "和当前档是同一个文件".to_string()
                } else {
                    note
                },
                accounts,
                characters,
                is_current,
            })
        })
        .collect();
    items.sort_by(|a, b| b.modified.cmp(&a.modified));
    items
}

fn empty_save(dir: PathBuf, detail: String) -> SaveInfo {
    SaveInfo {
        db_path: String::new(),
        storage_dir: dir.display().to_string(),
        backup_dir: dir.join("backup").display().to_string(),
        exists: false,
        size_text: String::new(),
        modified: String::new(),
        accounts: 0,
        characters: 0,
        detail,
        sidecars: Vec::new(),
    }
}

pub fn state(root: &Root) -> SavesState {
    let dir = storage_dir(root);
    let Some(path) = current_db(root) else {
        let detail = match config::storage_route_state(root) {
            StorageRoute::NeedsInit => "存档配置里还没有 sqlite_path：先跑一次游戏，服务端会自己建库".to_string(),
            StorageRoute::OtherDriver(driver) => format!(
                "存档配置写的是 driver={driver}，启动器不替你改成 SQLite，这里没有可备份的库"
            ),
            StorageRoute::Ready { .. } => String::new(),
        };
        return SavesState {
            save: empty_save(dir, detail),
            backups: list_backups(root, None),
        };
    };

    if !path.is_file() {
        return SavesState {
            save: SaveInfo {
                db_path: path.display().to_string(),
                detail: format!("{} 还不存在：跑一次游戏后由服务端建库", path.display()),
                ..empty_save(dir, String::new())
            },
            backups: list_backups(root, Some(&path)),
        };
    }

    let verdict = audit(&path);
    let (accounts, characters, detail) = match verdict {
        Ok(ok) => (
            ok.accounts,
            ok.characters,
            format!(
                "完整：{}（账号 {} · 角色 {}）",
                size_text(&path),
                ok.accounts,
                ok.characters
            ),
        ),
        Err(err) => (0, 0, err),
    };

    SavesState {
        save: SaveInfo {
            db_path: path.display().to_string(),
            storage_dir: dir.display().to_string(),
            backup_dir: backup_dir(root).display().to_string(),
            exists: true,
            size_text: size_text(&path),
            modified: modified_text(&path),
            accounts,
            characters,
            detail,
            sidecars: sidecars(&path),
        },
        backups: list_backups(root, Some(&path)),
    }
}

/// 手工备份一份，名字带秒级时间戳，落在 `<存档目录>/backup/`。
pub fn backup(root: &Root) -> Result<String, String> {
    let db = current_db(root).ok_or_else(|| "这份包还没定下 SQLite 存档，没有可备份的东西".to_string())?;
    if !db.is_file() {
        return Err(format!("{} 还不存在，先跑一次游戏再备份", db.display()));
    }
    let dir = backup_dir(root);
    std::fs::create_dir_all(&dir).map_err(|err| format!("创建 {} 失败：{err}", dir.display()))?;
    let target = unique_backup_path(&dir, &stem(&db), "", "sqlite3");
    let ok = match vacuum_into(&db, &target) {
        Ok(()) => audit(&target),
        Err(err) => {
            let _ = std::fs::remove_file(&target);
            return Err(err);
        }
    }?;
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(format!(
        "已备份为 {name}（账号 {} · 角色 {}，{}）",
        ok.accounts,
        ok.characters,
        size_text(&target)
    ))
}

/// 用选中的备份替换当前档。任何一步不对就停下；失败时原档保持原样，快照留在 backup 里。
pub fn restore(root: &Root, candidate_raw: &str) -> Result<String, String> {
    let db = current_db(root).ok_or_else(|| "这份包还没定下 SQLite 存档，无法替换".to_string())?;
    let raw = PathBuf::from(candidate_raw.trim());
    if !raw.is_absolute() {
        return Err("只能选整合包内的存档文件（要绝对路径）".to_string());
    }
    let candidate = crate::paths::squash(&raw);
    let storage = storage_dir(root);
    if !candidate.starts_with(&storage) {
        return Err(format!(
            "只允许从 {} 底下选存档，免得误指到别的东西",
            storage.display()
        ));
    }
    if !candidate.is_file() {
        return Err(format!("{} 不是一个文件", candidate.display()));
    }
    if std::fs::metadata(&candidate).map(|m| m.len() == 0).unwrap_or(true) {
        return Err("这份存档是 0 字节，不能拿它替换".to_string());
    }
    if same_file(&candidate, &db) {
        return Err("这份就是当前正在用的存档，替换它等于没换".to_string());
    }
    let incoming = audit(&candidate)?;

    // 当前档先在 backup/ 里留一张快照；快照本身验不过就一个字节都不动。
    let snapshot: Option<PathBuf> = if db.is_file() {
        audit(&db).map_err(|err| format!("当前存档体检没过，不敢替换：{err}"))?;
        let dir = backup_dir(root);
        std::fs::create_dir_all(&dir).map_err(|err| format!("创建 {} 失败：{err}", dir.display()))?;
        let target = unique_backup_path(&dir, &stem(&db), "-replaced", "sqlite3");
        match vacuum_into(&db, &target) {
            Ok(()) => Some(target),
            Err(err) => {
                let _ = std::fs::remove_file(&target);
                return Err(format!("替换前的快照没做成，已放弃替换：{err}"));
            }
        }
    } else {
        None
    };

    let summary = format!(
        "{}（账号 {} · 角色 {}）",
        incoming.accounts,
        incoming.characters,
        size_text(&candidate)
    );
    // 失败发生在 rename 之前还是之后，话不能说反：之后就是新档已经落地，原档早就不在了。
    let mut renamed = false;
    let outcome = (|| -> Result<(), String> {
        let tmp = db.with_extension(format!("sqlite3.partial-{}", std::process::id()));
        copy_candidate(&candidate, &tmp)?;
        let staged = match (std::fs::metadata(&tmp), std::fs::metadata(&candidate)) {
            (Ok(a), Ok(b)) if a.len() == b.len() => Ok(()),
            _ => Err("临时件大小与候选不符".to_string()),
        };
        staged?;
        if sha256_of(&tmp)? != sha256_of(&candidate)? {
            let _ = std::fs::remove_file(&tmp);
            return Err("临时件与候选逐字节不符".to_string());
        }
        audit(&tmp).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        std::fs::rename(&tmp, &db).map_err(|err| format!("替换失败：{err}"))?;
        renamed = true;
        remove_sidecars(&db);
        audit(&db).map(|_| ()).map_err(|err| format!("替换后复检失败：{err}"))
    })();

    match outcome {
        Ok(()) => {
            sweep_empty_sidecars(&db);
            let kept = match &snapshot {
                Some(snap) => format!(
                    "；替换前的档存成了 {}",
                    snap.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                ),
                None => "；原来没有存档，故替换前没有快照".to_string(),
            };
            let name = candidate
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            Ok(format!("已用「{name}」替换存档：{summary}{kept}"))
        }
        Err(err) => {
            let tail = match (&snapshot, renamed) {
                (Some(snap), true) => format!(
                    "；新档已经落地但复检没过，当前档就是选中的那份。替换前的快照留在 {}，要退回去就选它再替换一次",
                    snap.display()
                ),
                (Some(snap), false) => format!(
                    "；原档未动，替换前的快照留在 {}（需要时可再选它换回去）",
                    snap.display()
                ),
                (None, true) => "；新档已经落地但复检没过，原来没有存档故无快照可退".to_string(),
                (None, false) => "；原来没有存档，故未做任何改动".to_string(),
            };
            Err(format!("{err}{tail}"))
        }
    }
}

/// 删掉备份目录里的一份备份。口径比替换更窄：只准落在 `backup/` 底下、必须是自己认得的
/// `.sqlite3`、不能是当前正在用的档；旁边压着非 0 字节的日志文件就拒绝，因为那份里
/// 可能还有没合进主库的已提交数据，删主文件等于替玩家做决定。
pub fn delete_backup(root: &Root, candidate_raw: &str) -> Result<String, String> {
    let raw = PathBuf::from(candidate_raw.trim());
    if !raw.is_absolute() {
        return Err("只能删备份目录里的文件（要绝对路径）".to_string());
    }
    let candidate = crate::paths::squash(&raw);
    let dir = backup_dir(root);
    if !candidate.starts_with(&dir) {
        return Err(format!(
            "只允许删 {} 底下的备份，免得误指到当前存档或别的东西",
            dir.display()
        ));
    }
    if candidate.extension().and_then(|e| e.to_str()) != Some("sqlite3") {
        return Err(format!("{} 不是 .sqlite3 备份，启动器不删它", candidate.display()));
    }
    if !candidate.is_file() {
        return Err(format!("{} 不是一个文件", candidate.display()));
    }
    if let Some(db) = current_db(root) {
        if same_file(&candidate, &db) {
            return Err(
                "这份就是当前正在用的存档，删除按钮够不到它；要清当前档请在服务端那边处理".to_string(),
            );
        }
    }
    let base = candidate.as_os_str().to_string_lossy().to_string();
    let holding: Vec<String> = JOURNAL_SUFFIXES
        .iter()
        .filter_map(|suffix| {
            let side = PathBuf::from(format!("{base}{suffix}"));
            let non_empty =
                side.is_file() && std::fs::metadata(&side).map(|m| m.len() > 0).unwrap_or(false);
            non_empty.then(|| side.display().to_string())
        })
        .collect();
    if !holding.is_empty() {
        return Err(format!(
            "{} 旁边还压着非 0 字节的日志文件（{}），这份可能带还没合进主库的数据，先别删",
            candidate.display(),
            holding.join("、")
        ));
    }

    let name = candidate
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let size = size_text(&candidate);
    std::fs::remove_file(&candidate).map_err(|err| format!("删除失败：{err}"))?;
    // 空 sidecar 是这条备份自己留下的壳，跟着主文件一起走；非空的上面已经拦下了。
    sweep_empty_sidecars(&candidate);
    Ok(format!("已删除备份「{name}」（{size}）。当前存档一个字节都没动。"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 备份与替换前快照全建立在「只读连接也能 `VACUUM INTO`」这一条上：
    /// 这条不成立就等于服务端在跑的时候备不了份。用临时目录里自造的库验，不碰整合包。
    #[test]
    fn vacuum_into_works_on_a_read_only_database() {
        let dir = std::env::temp_dir().join(format!("dfo-saves-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("临时目录");
        let db = dir.join("t.sqlite3");
        let out = dir.join("out.sqlite3");
        {
            let conn = Connection::open(&db).expect("建临时库");
            conn.execute_batch(
                "CREATE TABLE accounts(id);
                 CREATE TABLE characters(id);
                 CREATE TABLE storage_migrations(id);
                 INSERT INTO accounts VALUES (1), (2);
                 INSERT INTO characters VALUES (7);",
            )
            .expect("造表");
        }
        let verdict = vacuum_into(&db, &out).and_then(|()| audit(&out));
        let _ = std::fs::remove_dir_all(&dir);
        let ok = match verdict {
            Ok(ok) => ok,
            Err(err) => panic!("只读连接应当能 VACUUM INTO，实际报：{err}"),
        };
        assert_eq!(ok.accounts, 2);
        assert_eq!(ok.characters, 1);
    }

    /// 真正要成立的是这一条：服务端正开着库、`-wal` 里还压着没落盘的一坨时，
    /// 只读连接能不能 `VACUUM INTO`。整合包里实测过 4 MB 的非空 `-wal`，所以不是假想。
    #[test]
    fn vacuum_into_copies_committed_rows_while_another_writer_holds_the_wal() {
        let dir = std::env::temp_dir().join(format!("dfo-saves-wal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("临时目录");
        let db = dir.join("t.sqlite3");
        let out = dir.join("out.sqlite3");

        // 留着这个连接不撤：它就是「服务端还在跑」那一方，WAL 与 shm 都归它管。
        let writer = Connection::open(&db).expect("建临时库");
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE accounts(id);
                 CREATE TABLE characters(id);
                 CREATE TABLE storage_migrations(id);
                 INSERT INTO accounts VALUES (1), (2), (3);
                 INSERT INTO characters VALUES (7);",
            )
            .expect("造表");
        let wal = dir.join("t.sqlite3-wal");
        let wal_live = wal.is_file() && std::fs::metadata(&wal).map(|m| m.len() > 0).unwrap_or(false);

        let verdict = vacuum_into(&db, &out).and_then(|()| audit(&out));
        drop(writer);
        let _ = std::fs::remove_dir_all(&dir);

        if !wal_live {
            panic!("这场没留下非空 -wal，用例没能覆盖到「服务端在跑」的条件");
        }
        let ok = match verdict {
            Ok(ok) => ok,
            Err(err) => panic!("有活的写入方时只读备份应当照样做成，实际报：{err}"),
        };
        assert_eq!(ok.accounts, 3);
        assert_eq!(ok.characters, 1);
    }

    /// 备份名撞车时必须另起一枚，绝不把已有的那份覆盖掉。
    #[test]
    fn unique_path_never_reuses_an_existing_backup() {
        let dir = std::env::temp_dir().join(format!("dfo-saves-uniq-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("临时目录");
        let first = unique_backup_path(&dir, "dfolan", "", "sqlite3");
        std::fs::write(&first, b"x").expect("占位");
        let second = unique_backup_path(&dir, "dfolan", "", "sqlite3");
        let _ = std::fs::remove_dir_all(&dir);
        assert_ne!(first, second);
    }

    /// 落点必须是整合包那个 `backup/` 底下的 `state/`：整库档已经实证写得进去（存档页那一枚），
    /// 这一层只是它旁边的一个子目录，不另起炉灶。
    #[test]
    fn role_state_backups_land_under_the_backup_dir() {
        let root = Root {
            path: PathBuf::from("pack"),
            source: "test",
        };
        let dir = role_state_backup_dir(&root);
        assert_eq!(dir, backup_dir(&root).join("state"));
        assert_eq!(
            dir,
            PathBuf::from("pack/server/work/dfo-lan/runtime/storage/backup/state")
        );
    }

    /// `state/` 里的改前原文不许冒充「能替换回去的整库档」：列表按 `.sqlite3` 认，
    /// 删除按钮的白名单也按 `.sqlite3` 认，所以那两份 JSON 两边都够不到。
    #[test]
    fn state_backups_are_invisible_to_the_database_backup_list() {
        let (root, dir) = temp_root("state-vs-list");
        let state = role_state_backup_dir(&root);
        std::fs::create_dir_all(&state).expect("state 目录");
        let sample = state.join("role-1-10-20261022-191500.json");
        std::fs::write(&sample, b"{\"level\":1}").expect("一份改前原文");

        let listed = list_backups(&root, None);
        assert_eq!(listed.len(), 0, "整库备份列表里出现了 state 里的 JSON");
        assert!(delete_backup(&root, &sample.display().to_string()).is_err(), "删除按钮够不到 JSON 才对");
        assert!(sample.is_file(), "被拒的删除不许把改前原文带走");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 删除只建立在「`backup/` 底下那一层」这条路径契约上，用临时包验，不碰整合包。
    fn temp_root(tag: &str) -> (Root, PathBuf) {
        let dir = std::env::temp_dir().join(format!("dfo-saves-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = Root {
            path: dir.clone(),
            source: "test",
        };
        std::fs::create_dir_all(backup_dir(&root)).expect("临时备份目录");
        (root, dir)
    }

    /// 删除面必须比替换面更窄：存档目录里、备份目录外的东西一律碰不到。
    #[test]
    fn delete_backup_only_reaches_files_under_the_backup_dir() {
        let (root, dir) = temp_root("del-scope");
        let current = storage_dir(&root).join("dfolan.sqlite3");
        std::fs::write(&current, b"x").expect("造当前档");
        let err = delete_backup(&root, &current.to_string_lossy()).unwrap_err();
        assert!(err.contains("只允许删"), "{err}");
        assert!(current.is_file(), "越界的删除必须一个字节都不动");

        let relative = delete_backup(&root, "backup/dfolan.sqlite3").unwrap_err();
        assert!(relative.contains("绝对路径"), "{relative}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 当前档自己躺在 backup 里时（替换快照的常见情形），删除按钮不能把它删掉。
    #[test]
    fn delete_backup_refuses_the_database_in_use() {
        let (root, dir) = temp_root("del-current");
        let backup = backup_dir(&root).join("now-in-use.sqlite3");
        std::fs::write(&backup, b"x").expect("造备份");
        std::fs::create_dir_all(storage_dir(&root)).expect("存档目录");
        let quoted = backup.display().to_string().replace('\\', "\\\\");
        std::fs::write(
            root.storage_local_json(),
            format!("{{\"sqlite_path\": \"{quoted}\"}}"),
        )
        .expect("写 sqlite_path 指向备份里这一份");

        let err = delete_backup(&root, &backup.to_string_lossy()).unwrap_err();
        assert!(err.contains("当前正在用的存档"), "{err}");
        assert!(backup.is_file(), "被拒时不能已经把文件删了");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 非 0 字节的 `-wal` 里可能是已提交、还没合进主库的数据，这种份先别删。
    #[test]
    fn delete_backup_refuses_a_non_empty_journal() {
        let (root, dir) = temp_root("del-wal");
        let backup = backup_dir(&root).join("with-wal.sqlite3");
        std::fs::write(&backup, b"x").expect("造备份");
        std::fs::write(PathBuf::from(format!("{}-wal", backup.display())), b"data").expect("造非空 wal");

        let err = delete_backup(&root, &backup.to_string_lossy()).unwrap_err();
        assert!(err.contains("非 0 字节的日志文件"), "{err}");
        assert!(backup.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 正常那份连带它的空 sidecar 一起清掉，别在备份目录里留孤零零的空壳。
    #[test]
    fn delete_backup_removes_the_file_and_its_empty_sidecars() {
        let (root, dir) = temp_root("del-ok");
        let backup = backup_dir(&root).join("good.sqlite3");
        std::fs::write(&backup, b"xx").expect("造备份");
        let shm = PathBuf::from(format!("{}-shm", backup.display()));
        std::fs::write(&shm, b"").expect("造空 shm");

        let message = delete_backup(&root, &backup.to_string_lossy()).expect("该删得动");
        assert!(message.contains("已删除备份「good.sqlite3」"), "{message}");
        assert!(!backup.is_file());
        assert!(!shm.is_file(), "空 sidecar 要跟着主文件一起走");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
