//! 方案 C 的启动器侧：GM 物品库只读桥。
//!
//! 读取的是离线构建器（gmbuild）产出的五份 JSON 产物，运行期不碰 PVF。
//! `Catalog` 的条目字符串是 `Arc`，本身就 `Send + Sync`，所以启动器直接共享
//! 这一份目录，不再另存一份字段自有的副本；序列化给界面的行只按当页命中的
//! 那几十条来构造。搜索谓词只有一份实现：gmindex 的 `search_indices` 吃借用
//! 视图 `GmView`，与旧版 Go 工具的对拍钉在 gmbuild 的单测里，这里不复刻判据。
//! 写库的发放链路在隔壁 `gmgrant`，它复用这里加载好的同一份目录。

use std::sync::{Arc, Mutex};

use serde::Serialize;

use gmbuild::gmindex::{self, Catalog, GmItem, Query};

use crate::paths::Root;

const LOADING: &str = "加载中";
const READY: &str = "就绪";
const FAILED: &str = "失败";

/// 命令返回给界面的一行。只在命中集分页后构造（一页 ≤500 行，默认 60 行），
/// 字段与键名保持旧快照的形状，前端不用跟着改。
#[derive(Clone, Serialize)]
pub struct GmSearchRow {
    pub id: u32,
    pub name: String,
    pub name_en: String,
    pub kind: &'static str,
    pub grade: i32,
    pub rarity: i32,
    pub rarity_label: &'static str,
    pub level: i32,
    pub slot: &'static str,
    pub group: &'static str,
    pub type_key: &'static str,
    pub type_label: &'static str,
    pub type_text: Option<String>,
    pub stack_limit: u32,
    pub durability: u32,
    /// 通用为 None（Go 的 [all] 约定）；有则为受限职业 token 串。
    pub jobs: Option<Vec<String>>,
    pub grantable: bool,
    /// 时装（`special_equipment` 容器 1 那一族）。判据只住在 `GmItem::avatar()`，
    /// 界面与邮件发放都读这一格，不在前端复制「slot 是不是时装」那条规则。
    pub avatar: bool,
}

fn row_of(it: &GmItem) -> GmSearchRow {
    GmSearchRow {
        id: it.id,
        name: it.name.to_string(),
        name_en: it.name_en.to_string(),
        kind: it.kind.as_str(),
        grade: it.grade,
        rarity: it.rarity,
        rarity_label: gmindex::rarity_label(it.rarity),
        level: it.level,
        slot: it.slot,
        group: it.group,
        type_key: it.type_key,
        type_label: gmindex::type_label(it.type_key),
        type_text: it.type_text.as_ref().map(|t| t.to_string()),
        stack_limit: it.stack_limit,
        durability: it.durability,
        jobs: it.jobs.as_ref().map(|j| j.to_vec()),
        grantable: it.grantable(),
        avatar: it.avatar(),
    }
}

#[derive(Clone)]
pub struct GmShared(Arc<Mutex<Inner>>);

struct Inner {
    stage: String,
    error: String,
    catalog: Option<Arc<Catalog>>,
}

impl GmShared {
    pub fn new() -> GmShared {
        GmShared(Arc::new(Mutex::new(Inner {
            stage: "未加载".into(),
            error: String::new(),
            catalog: None,
        })))
    }

    /// 就绪则直接共享目录；缺失/失败/未加载则投递后台加载并回报当前阶段。
    /// 加载线程只写回 `Ready`/`Failed`，不会覆盖后来者手动触发的 `Loading`。
    pub(crate) fn ensure(&self, root: &Root) -> Result<Arc<Catalog>, String> {
        let (start, located, note) = {
            let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
            match &inner.catalog {
                Some(cat) => return Ok(cat.clone()),
                None => {
                    let already = inner.stage == LOADING;
                    if !already {
                        inner.stage = LOADING.into();
                        inner.error = String::new();
                    }
                    let found = gmindex::find_configs(&root.path);
                    let note = found
                        .as_ref()
                        .map(|d| format!("产物目录：{}", d.display()))
                        .unwrap_or_else(|| {
                            format!(
                                "未找到 GM 产物目录（{} 与 {}/*/gm-tool/configs 都没有 items.index.json）；请先离线运行 gmbuild 构建",
                                root.path.join("gm-tool").join("configs").display(),
                                root.path.join("源码").display()
                            )
                        });
                    let has_dir = found.is_some();
                    (
                        !already && has_dir,
                        found,
                        (!already && !has_dir).then_some(note),
                    )
                }
            }
        };
        if let Some(note) = note {
            let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if inner.stage == LOADING {
                inner.stage = FAILED.into();
                inner.error = note.clone();
            }
            return Err(note);
        }
        if start {
            let shared = self.clone();
            let begin = located.expect("start 只在找到目录时置真");
            std::thread::spawn(move || {
                match gmindex::load(&begin) {
                    Ok(cat) => {
                        let mut inner = shared.0.lock().unwrap_or_else(|e| e.into_inner());
                        inner.catalog = Some(Arc::new(cat));
                        inner.stage = READY.into();
                        inner.error = String::new();
                    }
                    Err(err) => {
                        let mut inner = shared.0.lock().unwrap_or_else(|e| e.into_inner());
                        if inner.stage == LOADING {
                            inner.stage = FAILED.into();
                            inner.error = err;
                        }
                    }
                }
            });
        }
        Err(self.stage())
    }

    /// 当前阶段（供状态视图用）。
    fn stage(&self) -> String {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).stage.clone()
    }

    /// 重新读一遍产物（离线重建后点「重新加载」），成功前保留旧目录可查询。
    pub(crate) fn reload(&self, root: &Root) -> Result<(), String> {
        let dir = gmindex::find_configs(&root.path)
            .ok_or_else(|| "未找到 GM 产物目录，请先离线运行 gmbuild 构建".to_string())?;
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        inner.stage = LOADING.into();
        let shared = self.clone();
        std::thread::spawn(move || {
            match gmindex::load(&dir) {
                Ok(cat) => {
                    let mut inner = shared.0.lock().unwrap_or_else(|e| e.into_inner());
                    inner.catalog = Some(Arc::new(cat));
                    inner.stage = READY.into();
                    inner.error = String::new();
                }
                Err(err) => {
                    let mut inner = shared.0.lock().unwrap_or_else(|e| e.into_inner());
                    if inner.stage == LOADING {
                        inner.stage = FAILED.into();
                        inner.error = err;
                    }
                }
            }
        });
        Ok(())
    }
}

#[derive(Serialize)]
pub struct GmStatusView {
    pub stage: String,
    pub error: String,
    pub dir: String,
    pub catalog: Option<gmindex::CatalogStatus>,
}

#[derive(Serialize)]
pub struct GmFacetsView {
    pub types: Vec<gmindex::Facet>,
    pub slots: Vec<gmindex::Facet>,
    pub rarities: Vec<gmindex::Facet>,
    pub level_bands: Vec<gmindex::Facet>,
}

#[derive(Serialize)]
pub struct GmSearchResult {
    pub total: usize,
    pub items: Vec<GmSearchRow>,
}

/// Go Search 的 limit 口径：<=0 或 >500 一律回落 60。
fn clamp_limit(limit: i64) -> usize {
    if limit <= 0 || limit > 500 {
        60
    } else {
        limit as usize
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn gm_status(state: tauri::State<'_, crate::commands::AppState>) -> Result<GmStatusView, String> {
    let shared = state.gm.clone();
    let root = state.root.clone();
    // 不留 move 闭包：ensure 现在只借 self，句柄克隆进闭包参数即可，外面还要读状态。
    let shared2 = shared.clone();
    let stage = crate::commands::block(move || match shared2.ensure(&root) {
        Ok(_) => READY.to_string(),
        Err(_) => shared2.stage(),
    })
    .await?;
    let inner = shared.0.lock().unwrap_or_else(|e| e.into_inner());
    Ok(GmStatusView {
        stage,
        error: inner.error.clone(),
        dir: inner.catalog.as_ref().map(|c| c.status.dir.clone()).unwrap_or_default(),
        catalog: inner.catalog.as_ref().map(|c| c.status.clone()),
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn gm_reload(state: tauri::State<'_, crate::commands::AppState>) -> Result<(), String> {
    let shared = state.gm.clone();
    let root = state.root.clone();
    crate::commands::block(move || shared.clone().reload(&root)).await?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn gm_search(
    query: String,
    type_: Option<String>,
    slot: Option<String>,
    level_min: Option<i32>,
    level_max: Option<i32>,
    rarity: Option<i32>,
    limit: Option<i64>,
    offset: Option<i64>,
    state: tauri::State<'_, crate::commands::AppState>,
) -> Result<GmSearchResult, String> {
    let shared = state.gm.clone();
    let root = state.root.clone();
    let raw = Query {
        q: query,
        type_: type_.unwrap_or_default(),
        slot: slot.unwrap_or_default(),
        level_min: level_min.unwrap_or(0),
        level_max: level_max.unwrap_or(0),
        rarity,
    };
    let limit = clamp_limit(limit.unwrap_or(0));
    let offset = offset.unwrap_or(0).max(0) as usize;
    crate::commands::join(move || {
        let cat = shared.ensure(&root)?;
        let hits = cat.search(raw);
        let total = hits.len();
        let items = hits.into_iter().skip(offset).take(limit).map(row_of).collect();
        Ok(GmSearchResult { total, items })
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn gm_item(
    id: u32,
    state: tauri::State<'_, crate::commands::AppState>,
) -> Result<GmSearchRow, String> {
    let shared = state.gm.clone();
    let root = state.root.clone();
    crate::commands::join(move || {
        let cat = shared.ensure(&root)?;
        cat.get(id).map(row_of).ok_or_else(|| format!("物品 {id} 不在物品库里"))
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn gm_facets(
    state: tauri::State<'_, crate::commands::AppState>,
) -> Result<GmFacetsView, String> {
    let shared = state.gm.clone();
    let root = state.root.clone();
    crate::commands::join(move || {
        let cat = shared.ensure(&root)?;
        let f = cat.facets();
        Ok(GmFacetsView {
            types: f.types,
            slots: f.slots,
            rarities: f.rarities,
            level_bands: f.level_bands,
        })
    })
    .await
}

#[cfg(test)]
mod probes {
    use super::*;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    /// 只在设了 `DFO_PROBE_CONFIGS` 时执行：那是 GM 产物目录（有 items.index.json 的那层）。
    /// 装载后停 `DFO_PROBE_HOLD` 秒（默认 20）让外面的轮询取到常驻工作集，
    /// 再做两次查询看它们各自的耗时与瞬时占用。
    /// 跑法：`DFO_PROBE_CONFIGS=<产物目录> cargo test --lib --release catalog_memory_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn catalog_memory_probe() {
        let Ok(dir) = std::env::var("DFO_PROBE_CONFIGS") else {
            eprintln!("跳过：未设 DFO_PROBE_CONFIGS");
            return;
        };
        let hold = Duration::from_secs(
            std::env::var("DFO_PROBE_HOLD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),
        );
        println!("PHASE=0 PID={} 等待加载", std::process::id());
        let begin = Instant::now();
        let cat = gmindex::load(&PathBuf::from(dir)).expect("产物加载失败");
        println!(
            "PHASE=1 载入完成 {:.1}s 件数={} —— 目录就是产品运行期的常驻形状",
            begin.elapsed().as_secs_f64(),
            cat.iter().count()
        );
        println!("指纹：{}", serde_json::to_string(&cat.status).unwrap());
        std::thread::sleep(hold);

        let t = Instant::now();
        let all = cat.search(Query::default());
        println!("PHASE=2 空条件 {:.3}s 命中={}", t.elapsed().as_secs_f64(), all.len());
        let t = Instant::now();
        let kw = cat.search(Query { q: "巨剑".into(), ..Default::default() });
        println!("PHASE=3 关键字 {:.3}s 命中={}", t.elapsed().as_secs_f64(), kw.len());
        let row = row_of(kw.first().expect("关键字无命中"));
        println!("行样例：{} {} / {}", row.id, row.name, row.type_label);
        std::thread::sleep(hold);
    }
}
