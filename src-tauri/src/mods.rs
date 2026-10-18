use std::fs;
use std::io::Read as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config;
use crate::paths::Root;

/// 网关日志只扫到这个字节数：mod 的装载与自检都发生在 boot 附近，
/// 后面滚起来的战斗日志跟这一页无关。
const GATEWAY_SCAN_LIMIT: u64 = 2_000_000;

/// 动过这些文件的 mod 不能单独回滚：半截恢复会把包变成开不了的档
/// （`Script.pvf` 与 `sk.dat` 是一对，`dinput8.dll` 与 `plugins.json` 是一对）。
const BRICK_FILES: [&str; 6] = [
    "Script.pvf",
    "sk.dat",
    "DFO.exe",
    "dinput8.dll",
    "ChineseLocalization.dll",
    "plugins.json",
];

#[derive(Serialize)]
pub struct ModsState {
    pub server: ServerMods,
    pub scripts: ScriptMods,
    pub client: ClientMods,
    pub modkit: ModkitMods,
}

// ------------------------------------------------------------------ 服务端 mod

#[derive(Serialize)]
pub struct ServerMods {
    pub mods_dir: String,
    pub enabled_present: bool,
    /// 网关自己报的那句启用清单结论，原样搬过来，不在界面里重述一遍。
    pub gateway_note: String,
    pub broken_note: String,
    /// 名单写在盘上、但上一场网关报「没找到」：这个位置它不读，开关是空转的。
    pub ignored_note: String,
    /// 装载证据来自哪一场的日志；空字符串表示还没有跑过。
    pub evidence_run: String,
    pub mods: Vec<ServerMod>,
}

#[derive(Serialize)]
pub struct ServerMod {
    pub id: String,
    pub purpose: String,
    pub enabled: bool,
    /// 这一场的日志里真的装载了它（被禁用的 mod 只会出现在清单里，不会出现在日志里）。
    pub seen: bool,
}

#[derive(Serialize)]
pub struct ScriptMods {
    pub dir: String,
    pub exists: bool,
    pub scripts: Vec<ScriptFile>,
    /// 网关报「已接收」的、由 mod 自带的脚本（`<mod id>:<文件名>`）。
    /// 它们不在盘上，只列目录会漏掉这一份 —— 玩家看到的「脚本 1 份」就是这么来的。
    pub provided: Vec<String>,
}

#[derive(Serialize)]
pub struct ScriptFile {
    pub name: String,
    pub size: u64,
}

// ---------------------------------------------------------------- 客户端 mod 插件

#[derive(Serialize)]
pub struct ClientMods {
    pub dir: String,
    pub present: bool,
    pub manifest: ManifestState,
    pub auto_discover: bool,
    /// `mods/mods.json`（启动器自己那份开关清单）的形态。
    pub switches: ManifestState,
    pub switch_dir: String,
    /// 开关能不能用：只有走 mod-loader 这一层才是「改了算数」的。
    pub switchable: bool,
    /// 宿主清单没缩成缩减版 / 登记被 modkit 换回去时的报警，空串表示正常。
    pub bypass_note: String,
    /// mod-loader 自己落的自证状态：它到底把哪几条装上了。
    pub loader: LoaderStatus,
    pub plugins: Vec<ClientPlugin>,
    pub host_log: String,
    pub host_tail: Vec<String>,
}

/// `plugins.json` 自己在不在、认不认。宿主把「解析不出形状的清单」当成没有清单，
/// 所以这两种情况下盘上所有 DLL 都会被加载 —— 页面上不能都写成「只加载清单里列的」。
#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ManifestState {
    Present,
    Missing,
    Corrupt,
}

#[derive(Serialize)]
pub struct ClientPlugin {
    pub id: String,
    pub dll: String,
    /// 清单里有没有这一条：没有但文件在场的，是靠自动发现被加载的。
    pub in_manifest: bool,
    pub enabled: bool,
    pub deps: Vec<String>,
    /// 内置中文功能提示；表里没有就是空串，界面自己决定怎么兜底。
    pub description: String,
    /// mod-loader 报的实装结果：`None` = 这一层还没跑过（没迁移，或客户端没启动）。
    pub loaded: Option<bool>,
    pub load_note: String,
    /// 统一入口调 `ModStart` 拿到的返回值。0 = 入口自己认账；非 0 表示 DLL 是装进进程了，
    /// 但它拒绝启动（版本守卫不过、依赖没就绪……），所以「装上了」不等于「在干活」。
    pub start_return: u32,
    /// 插件自己写的状态文件里的 `ready`；没写过就是 `None`。
    pub ready: Option<bool>,
    pub reject_reason: String,
}

/// `mods/mod-loader.status.json` —— 统一入口写给自己的一页账。
#[derive(Serialize, Default)]
pub struct LoaderStatus {
    pub seen: bool,
    pub ready: Option<bool>,
    pub reject_reason: String,
    pub declared: usize,
    pub started: usize,
    pub failed: usize,
    /// 登记名（小写）→ (装上了没, 失败原因, 入口返回值)。界面只看每条插件自己的 `loaded`
    /// 与 `start_return`，这页原始账不外传。
    #[serde(skip)]
    pub rows: Vec<(String, bool, String, u32)>,
}

// -------------------------------------------------------------------- modkit 安装层

#[derive(Serialize)]
pub struct ModkitMods {
    pub present: bool,
    pub registry: String,
    pub total: usize,
    pub entries: usize,
    pub risky: Vec<ModkitRisk>,
}

#[derive(Serialize)]
pub struct ModkitRisk {
    pub id: String,
    pub target: String,
    pub kind: String,
}

#[derive(Deserialize)]
struct EnabledFile {
    #[serde(default)]
    disabled: Vec<String>,
}

#[derive(Deserialize)]
struct PluginFile {
    #[serde(rename = "autoDiscover", default = "is_true")]
    auto_discover: bool,
    #[serde(default = "is_true")]
    recurse: bool,
    #[serde(default)]
    plugins: Vec<PluginEntry>,
}

#[derive(Deserialize)]
struct PluginEntry {
    #[serde(default)]
    name: String,
    #[serde(default)]
    path: String,
    #[serde(default = "is_true")]
    enabled: bool,
    #[serde(default)]
    description: String,
    #[serde(default)]
    deps: Vec<String>,
}

#[derive(Deserialize)]
struct PluginStatus {
    #[serde(default)]
    ready: Option<bool>,
    #[serde(rename = "rejectReason", default)]
    reject_reason: String,
}

/// `mods/mod-loader.status.json` 的形状（字段名按统一入口写出的那份）。
#[derive(Deserialize)]
struct LoaderStatusFile {
    #[serde(default)]
    ready: Option<bool>,
    #[serde(rename = "rejectReason", default)]
    reject_reason: String,
    #[serde(default)]
    declared: usize,
    #[serde(default)]
    started: usize,
    #[serde(default)]
    failed: usize,
    #[serde(default)]
    mods: Vec<LoaderRow>,
}

#[derive(Deserialize)]
struct LoaderRow {
    #[serde(default)]
    id: String,
    #[serde(default)]
    loaded: bool,
    #[serde(default)]
    returned: u32,
    #[serde(default)]
    fail: String,
}

#[derive(Deserialize)]
struct Registry {
    #[serde(default)]
    mods: Vec<RegistryMod>,
}

#[derive(Deserialize)]
struct RegistryMod {
    #[serde(default)]
    id: String,
    #[serde(default)]
    entries: Option<Vec<RegistryEntry>>,
}

#[derive(Deserialize)]
struct RegistryEntry {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    target: String,
}

fn is_true() -> bool {
    true
}

pub fn state(root: &Root) -> ModsState {
    let scan = scan_gateway_log(root);
    ModsState {
        server: server_mods(root, &scan),
        scripts: script_mods(root, &scan),
        client: client_mods(root),
        modkit: modkit_mods(root),
    }
}

fn read_enabled_file(path: &Path) -> Result<EnabledFile, String> {
    let text = fs::read_to_string(path).map_err(|err| format!("enabled.json 读不动：{err}"))?;
    serde_json::from_str(&text).map_err(|err| format!("enabled.json 解析不了：{err}"))
}

fn purpose_of(purposes: &[(String, String)], id: &str) -> String {
    purposes
        .iter()
        .find(|(seen, _)| seen == id)
        .map(|(_, purpose)| purpose.clone())
        .unwrap_or_default()
}

/// 最近一场网关 stderr 里跟 mod 有关的全部事实。
///
/// 为什么从日志取而不是从目录扫：服务端 mod 是编译进网关二进制的，
/// 盘上有没有 `mods/` 都跟它装没装载无关；只有日志说得出「这一场到底装载了哪几个」。
struct GatewayScan {
    /// 这一场真的装载了的 mod id（`已装载 …` 那一句报的名单）。
    loaded: Vec<String>,
    /// 每个 mod 自己写的用途文案（`[mod <id>] 已启用：<用途>`）。
    purposes: Vec<(String, String)>,
    /// `servermod: 启用清单：…` 后面网关自己的结论，原样搬过来。
    note: String,
    /// 装载证据来自哪一场；空字符串 = 还没跑过。
    run: String,
    /// 网关接收下来的规则脚本（`奖励规则脚本 N 份：<mod>:<文件>`）。
    scripts: Vec<String>,
}

/// 网关会去哪一个 `mods/` 找 `enabled.json`。
///
/// 它开的是**相对路径** `mods/enabled.json`，基准是网关自己的工作目录；编排程序在
/// `internal/launcher/serverrun.go` 里把 cwd 设成 `gatewayWorkingDirectory(module, 内层PVF)`：
/// 内层 PVF 是显式指定的才用模块根，否则用模块根往上两级（= `<包根>/server`）。
/// 而 `serverModuleDir()` 又要求那一层带 `module dfolan` 的 `go.mod` —— 纯二进制包没有，
/// 于是退回 `"."`，相对路径就落在 cwd 上。本包是拆壳包、没有 go.mod ⇒ `<包根>/server/mods`。
fn runtime_mods_dir(root: &Root) -> std::path::PathBuf {
    let module = root.work_dir();
    let declares_module = |dir: &Path| match fs::read_to_string(dir.join("go.mod")) {
        Ok(text) => text
            .lines()
            .map(str::trim)
            .any(|line| line == "module dfolan" || line.starts_with("module dfolan ")),
        Err(_) => false,
    };
    // 模块根带 `module dfolan` 时它自己就是答案；否则服务端退回相对路径，落在 cwd 上。
    if declares_module(&module) {
        module.join("mods")
    } else {
        root.path.join("server").join("mods")
    }
}

fn server_mods(root: &Root, scan: &GatewayScan) -> ServerMods {
    let mods_dir = runtime_mods_dir(root);
    let enabled_path = mods_dir.join("enabled.json");
    let mut disabled: Vec<String> = Vec::new();
    let mut broken_note = String::new();
    let enabled_present = enabled_path.is_file();
    if enabled_present {
        match read_enabled_file(&enabled_path) {
            Ok(file) => disabled = file.disabled,
            Err(err) => broken_note = err,
        }
    }

    let mut mods: Vec<ServerMod> = scan
        .loaded
        .iter()
        .map(|id| ServerMod {
            purpose: purpose_of(&scan.purposes, id),
            id: id.clone(),
            enabled: !disabled.iter().any(|seen| seen == id),
            seen: true,
        })
        .collect();
    for id in disabled.iter().filter(|id| !scan.loaded.contains(id)) {
        mods.push(ServerMod {
            purpose: purpose_of(&scan.purposes, id),
            id: id.clone(),
            enabled: false,
            seen: false,
        });
    }
    mods.sort_by(|a, b| a.id.cmp(&b.id));

    // 名单在场、可上一场网关报的是「没找到」，而且文件比那场日志还老 ⇒ 它读的不是这个位置，
    // 开关按下去只是写了一份没人看的文件。这条只能由实机日志说话，界面上必须显出来。
    let mut ignored_note = String::new();
    if enabled_present && scan.note.contains("没有 enabled.json") {
        let written = fs::metadata(&enabled_path).and_then(|m| m.modified()).ok();
        let logged = root
            .newest_run_dir()
            .map(|dir| fs::metadata(dir.join("gateway.err")).and_then(|m| m.modified()).ok())
            .flatten();
        if let (Some(written), Some(logged)) = (written, logged) {
            if written <= logged {
                ignored_note = format!(
                    "名单写在 {}，可上一场网关报「{}」—— 它不读这个位置，这里的开关按了不生效",
                    enabled_path.display(),
                    scan.note
                );
            }
        }
    }

    ServerMods {
        mods_dir: mods_dir.display().to_string(),
        enabled_present,
        gateway_note: scan.note.clone(),
        broken_note,
        ignored_note,
        evidence_run: scan.run.clone(),
        mods,
    }
}

fn scan_gateway_log(root: &Root) -> GatewayScan {
    let empty = GatewayScan {
        loaded: Vec::new(),
        purposes: Vec::new(),
        note: String::new(),
        run: String::new(),
        scripts: Vec::new(),
    };
    let Some(dir) = root.newest_run_dir() else {
        return empty;
    };
    let path = dir.join("gateway.err");
    let Ok(mut file) = fs::File::open(&path) else {
        return empty;
    };
    let mut buf = Vec::new();
    if file
        .by_ref()
        .take(GATEWAY_SCAN_LIMIT)
        .read_to_end(&mut buf)
        .is_err()
    {
        return empty;
    }
    let text = String::from_utf8_lossy(&buf);

    let mut out = GatewayScan {
        loaded: Vec::new(),
        purposes: Vec::new(),
        note: String::new(),
        run: dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        scripts: Vec::new(),
    };
    for line in text.lines() {
        if let Some((id, purpose)) = enabled_line(line) {
            if !out.purposes.iter().any(|(seen, _)| seen == &id) {
                out.purposes.push((id, purpose));
            }
        }
        // 被禁用的 mod 走另一条文案，它才是「这一场它真的没装载」的第一证据。
        if let Some((id, words)) = disabled_line(line) {
            if !out.purposes.iter().any(|(seen, _)| seen == &id) {
                out.purposes.push((id, words));
            }
        }
        if let Some(ids) = summary_line(line) {
            for id in ids {
                if !out.loaded.contains(&id) {
                    out.loaded.push(id);
                }
            }
        }
        if let Some(pos) = line.find("启用清单：") {
            out.note = line[pos + "启用清单：".len()..].trim().to_string();
        }
        // 清单读成功时网关不打「启用清单：」那一句，只打这一句。
        if let Some(pos) = line.find("已禁用 ") {
            out.note = line[pos..].trim().to_string();
        }
        if let Some(pos) = line.find("规则脚本 ") {
            if let Some(rest) = line[pos..].split_once('：').map(|(_, tail)| tail) {
                for item in rest.split(',') {
                    let item = item.trim();
                    if !item.is_empty() && !out.scripts.contains(&item.to_string()) {
                        out.scripts.push(item.to_string());
                    }
                }
            }
        }
    }
    out
}

/// `[mod <id>] 已启用：<用途>` —— 服务端把这一条当「mod 真的注册上了」的第一证据，
/// 用途文案也是它自己写的，比启动器编一份名字表可信。
fn enabled_line(line: &str) -> Option<(String, String)> {
    let rest = line.split_once("[mod ")?.1;
    let (id, tail) = rest.split_once("] 已启用：")?;
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    Some((id.to_string(), tail.trim().to_string()))
}

/// `[mod <id>] 已被禁用（mods/enabled.json），本次不加载任何规则` —— 同一宿主函数
/// 在 `Enabled()` 为假时打的另一条；用途文案此时拿不到，只能引用网关自己的这句
/// （括号里是它给的落点，界面不该重复一遍）。
fn disabled_line(line: &str) -> Option<(String, String)> {
    let rest = line.split_once("[mod ")?.1;
    let (id, tail) = rest.split_once("] 已被禁用")?;
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    let words = match tail.rsplit_once('）') {
        Some((_, after)) => after.trim_start_matches(['，', ' ']).to_string(),
        None => tail.trim().to_string(),
    };
    Some((id.to_string(), words))
}

/// `servermod: 已装载 N 个服务端 mod：<id>、<id>；boot 钩子 …` —— 取 mod 名单那一段。
///
/// 这一场只有 1 个 mod，日志里看不出分隔符是 `,` 还是 `、`，所以按「mod id 的合法字符
/// 之外都算分隔」来切（`validModID` 只认小写字母、数字、点、短横线），两种写法都能解。
fn summary_line(line: &str) -> Option<Vec<String>> {
    if !line.contains("已装载") {
        return None;
    }
    let rest = line.split_once("个服务端 mod：")?.1;
    let head = rest.split('；').next().unwrap_or(rest);
    Some(
        head.split(|ch: char| ch == ',' || ch == '，' || ch == '、' || ch.is_whitespace())
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

fn script_mods(root: &Root, scan: &GatewayScan) -> ScriptMods {
    let dir = root.mods_dir();
    let exists = dir.is_dir();
    let mut scripts = Vec::new();
    if exists {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| !ext.eq_ignore_ascii_case("lua"))
                    .unwrap_or(true)
                {
                    continue;
                }
                let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                    continue;
                };
                let size = entry.metadata().map(|meta| meta.len()).unwrap_or_default();
                scripts.push(ScriptFile { name, size });
            }
        }
    }
    scripts.sort_by(|a, b| a.name.cmp(&b.name));
    ScriptMods {
        dir: dir.display().to_string(),
        exists,
        scripts,
        provided: scan.scripts.clone(),
    }
}

/// 客户端目录下的 mod 全景。
///
/// 两套清单要分清：`.115us-mods/plugins.json` 归 modkit 所有（它是
/// `qol.r7-runtime-repair` 的 file.replace 目标，启动器写进去的东西下一次装模就会被换回去），
/// `mods/mods.json` 才是统一入口与启动器共用的开关。只有后者在场，勾才是「改了算数」的。
fn client_mods(root: &Root) -> ClientMods {
    let client = config::client_dir(root);
    let dir = client.join(".115us-mods");
    let switch_dir = client.join("mods");
    let host = read_plugin_file(&dir.join("plugins.json"));
    let switches = read_plugin_file(&switch_dir.join("mods.json"));
    let loader = read_loader_status(&switch_dir);
    let present = dir.is_dir();
    let switchable = matches!(switches.0, ManifestState::Present);
    let host_reduced = is_reduced_host_manifest(&host.1);

    let mut bypass_note = String::new();
    if switchable && matches!(host.0, ManifestState::Present) && !host_reduced {
        bypass_note = "宿主清单不是缩减版（mod-loader 之外还有声明，或没关 autoDiscover/recurse）：\
                       这些插件仍会被宿主直接装上，开关会被绕过。"
            .to_string();
    } else if switchable && !matches!(host.0, ManifestState::Present) {
        bypass_note = "宿主清单 plugins.json 不在场或认不出形状：宿主回退旧行为，\
                       自己把 .115us-mods 下的每个 DLL 都装上，那些开关照样被绕过\
                       （mods 目录下的仍按开关走）。"
            .to_string();
    } else if !switchable && host_reduced {
        bypass_note = "宿主清单已缩减成只加载 mod-loader，但 mods/mods.json 不在场或认不出形状：\
                       现在客户端 mod 全都不装。先跑 analysis/tasks/mod-loader-unify-20261016 的迁移。"
            .to_string();
    }

    let file = if switchable { switches.1 } else { host.1 };
    let auto_discover = file.as_ref().is_some_and(|f| f.auto_discover);
    let entries: Vec<PluginEntry> = file.map(|f| f.plugins).unwrap_or_default();
    // 状态文件跟着 DLL 住，而两份清单的路径口径不同，先归一成一个口径。
    let base = client.clone();
    let declared_rel: Vec<String> = entries
        .iter()
        .map(|e| norm_rel(&entry_rel(switchable, &e.path)))
        .collect();

    let mut plugins: Vec<ClientPlugin> = Vec::new();
    for entry in entries {
        let rel = entry_rel(switchable, &entry.path);
        let status = read_status(
            &base.join(rel_dir(&rel)),
            &status_names(&entry.name, &base_name(&rel)),
        );
        let (loaded, load_note, start_return) = loader.row_of(&entry.name);
        plugins.push(ClientPlugin {
            dll: base_name(&rel),
            id: entry.name,
            in_manifest: true,
            enabled: entry.enabled,
            deps: entry.deps,
            description: pick_description(&rel, &entry.description),
            loaded,
            load_note,
            start_return,
            ready: status.0,
            reject_reason: status.1,
        });
    }

    // 文件在场但清单里没列的 DLL：统一入口只补 `mods\<子目录>\` 这一种，
    // `.115us-mods\` 下的清单外文件在缩减之后是**没人装卸**的（等于被静默关掉）。
    for rel in scan_plugin_dlls(&client, switchable) {
        if declared_rel.contains(&norm_rel(&rel)) {
            continue;
        }
        let status = read_status(&base.join(rel_dir(&rel)), &status_names("", &base_name(&rel)));
        let enabled = if switchable {
            // 缩减清单之后宿主不再扫这个根：只有 mods\ 的子目录会被统一入口补装。
            auto_discover && !rel_in_legacy_root(&rel)
        } else {
            !matches!(host.0, ManifestState::Present) || auto_discover
        };
        let (extra_loaded, extra_note, extra_return) =
            loader.row_of(&stem_of(&base_name(&rel)));
        plugins.push(ClientPlugin {
            dll: base_name(&rel),
            id: stem_of(&base_name(&rel)),
            in_manifest: false,
            enabled,
            deps: Vec::new(),
            description: pick_description(&rel, ""),
            loaded: extra_loaded,
            load_note: extra_note,
            start_return: extra_return,
            ready: status.0,
            reject_reason: status.1,
        });
    }
    plugins.sort_by(|a, b| a.dll.cmp(&b.dll));

    let host_log = client.join("client-host.log");
    ClientMods {
        dir: dir.display().to_string(),
        present,
        manifest: host.0,
        auto_discover,
        switches: switches.0,
        switch_dir: switch_dir.display().to_string(),
        switchable,
        bypass_note,
        loader,
        plugins,
        host_tail: tail_lines(&host_log, 6),
        host_log: host_log.display().to_string(),
    }
}
/// 内置中文功能提示。键是 `.115us-mods` 下 DLL 文件名的小写形。
///
/// 每一条取自插件自己日志里的首轮陈述，不是从文件名猜的。`mods/mods.json` 里的
/// `description` 是同一批文案，但界面以这张表为准 —— 迁移脚本没跑、或那份清单被
/// 人改过的时候，提示照样在。
const PLUGIN_DESCRIPTIONS: [(&str, &str); 13] = [
    (
        "apocalypsenativehard.dll",
        "末世录·原生整块硬补丁：等 waiting-panel 与 native-source 就绪后按段下钩，各段计数写在 apocalypse-native-hard.log",
    ),
    (
        "apocalypsewaitingpanel.dll",
        "末世录关底撤退面板：面板线程冻结不可用时它自己会禁用，panels 计数在 apocalypse-waiting-panel.log",
    ),
    (
        "apocalypsenativesourceprobe.dll",
        "末世录原生数据来源探针：给整块硬补丁供数据，它没就绪时硬补丁会明确拒绝下钩而不是半套生效",
    ),
    (
        "apocalypsethirdcard.dll",
        "末世录第三档机制候选：默认 enabled=false 且 entrypoint=false，只加载不启动",
    ),
    (
        "cnfontraster.dll",
        "宋体等宽光栅化 r3：family14 的 11~24 号由比例转等宽，原头顶插件、字体字节、加载标志、度量、图层与设置全部保留",
    ),
    (
        "chineseinputprobe.dll",
        "中文输入只观察探针：不碰输入法也不改键盘布局，只在聊天框打字时记录；日志很大，看完就可以删",
    ),
    (
        "elitecompanionpilot.dll",
        "精锐同伴·累计 R3 补丁 + r6 诊断观察者：图形失败与 C++ 异常按原路抛出；ModStart 实测约 10.3 秒，关掉它能省这段等待",
    ),
    (
        "headtextupscale.dll",
        "头顶与主菜单文字增强 r5：十处带守卫的调用，教程按键走原生层排除，只放大菜单字体",
    ),
    (
        "localdungeonexit.dll",
        "本地副本回城 r14：仅普通副本空闲时接管 action 22/30，结算权限不变",
    ),
    (
        "localfeatures.dll",
        "累计 R3 本地功能：NPC 菜单路由（角色 535/536/537，原生 538 保留）、道具使用路由、原生背包路由、Ctrl+F10 旧控制器",
    ),
    (
        "robotnativeai.dll",
        "女圣职者三觉背景绘制层 r34-render2 + 假人原生 AI 驱动：启动前逐字节核对，不符就整体停用而不是半套生效",
    ),
    (
        "robotobjectauthority.dll",
        "假人技能对象控制权 r30 effects1：type3/4 与经核对的假人父体 type17 权限适配器，原生门槛条件保留",
    ),
    (
        "weaponskinanimation.dll",
        "武器幻化动画 r1：只在原生动画/扩展作用域内改，作用域外的装备与属性调用不变；ModStart 实测约 3.0 秒",
    ),
];

/// 统一入口这个根之外，存量插件住的根。
const LEGACY_ROOT: &str = ".115us-mods";
/// 启动器与统一入口共用的家：新 mod 导到这里，一个 mod 一个目录。
const SWITCH_ROOT: &str = "mods";

fn read_plugin_file(path: &Path) -> (ManifestState, Option<PluginFile>) {
    let Ok(text) = fs::read_to_string(path) else {
        return (ManifestState::Missing, None);
    };
    // BOM 会要命：统一入口自己跳得掉，serde 不掉。
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    match serde_json::from_str::<PluginFile>(text) {
        Ok(file) => (ManifestState::Present, Some(file)),
        Err(_) => (ManifestState::Corrupt, None),
    }
}

/// 宿主清单是不是「只声明 mod-loader、并且关掉 recurse 与 autoDiscover」的缩减版。
/// 这三条少一条，存量插件就会被宿主绕过开关直接装上。
fn is_reduced_host_manifest(file: &Option<PluginFile>) -> bool {
    let Some(file) = file.as_ref() else {
        return false;
    };
    !file.auto_discover
        && !file.recurse
        && file.plugins.len() == 1
        && base_name(&file.plugins[0].path).eq_ignore_ascii_case("mod-loader.dll")
        && file.plugins[0].enabled
}

fn rel_in_legacy_root(rel: &str) -> bool {
    norm_rel(rel).starts_with(&format!("{LEGACY_ROOT}/"))
}

/// 清单项的 `path` 归一成「相对客户端目录、用 `/`」这一个口径。
///
/// 宿主那份相对 `.115us-mods`（host 直接拼 `%s\\%s\\%s`），统一入口那份相对客户端目录。
/// 不归一，状态文件就会跟着错的目录去找。
fn entry_rel(switchable: bool, path: &str) -> String {
    let unified = path.replace('\\', "/");
    if switchable {
        unified
    } else {
        format!("{LEGACY_ROOT}/{unified}")
    }
}

/// `mods/mods.json` 到底有没有人读 —— 也就是勾是不是改了算数。
///
/// 宿主扫不到 `mods\`，所以只有统一入口跑起来，这份清单才生效：DLL 要在场，
/// 且宿主没在自己的清单里显式禁用它。没清单、清单认不出形状、缩减版这三种都会加载它
/// （前两种走自动发现，最后一种走显式声明）。
fn loader_will_run(client: &Path) -> bool {
    let legacy = client.join(LEGACY_ROOT);
    if !legacy.join("mod-loader.dll").is_file() {
        return false;
    }
    let read = read_plugin_file(&legacy.join("plugins.json")).1;
    !read.is_some_and(|file| {
        file.plugins
            .iter()
            .any(|e| base_name(&e.path).eq_ignore_ascii_case("mod-loader.dll") && !e.enabled)
    })
}

/// `\` 与 `/` 同一个口径，大小写不敏感 —— 跟统一入口的 `same_rel_path` 一致，
/// 否则启动器写 `/` 会被当成「清单外文件」再报一条。
fn norm_rel(rel: &str) -> String {
    rel.replace('\\', "/").to_lowercase()
}

/// 相对路径里除去最后一段的那部分（`.115us-mods/A.dll` → `.115us-mods`）。
fn rel_dir(rel: &str) -> String {
    let normalized = rel.replace('\\', "/");
    match normalized.rsplit_once('/') {
        Some((head, _)) => head.to_string(),
        None => String::new(),
    }
}

fn pick_description(rel: &str, from_file: &str) -> String {
    let dll = base_name(rel).to_lowercase();
    PLUGIN_DESCRIPTIONS
        .iter()
        .find(|(name, _)| *name == dll)
        .map(|(_, text)| text.to_string())
        .unwrap_or_else(|| from_file.to_string())
}

/// 盘上实扫：这一层该被装卸的每一枚 DLL，返回相对客户端目录、用 `/` 的路径。
///
/// 缩减清单之后，`.115us-mods\` 下的清单外文件没人扫（宿主的 autoDiscover 被关），
/// 所以这里必须扫出来给界面报警，否则「盘上有、界面没」就等于静默关掉一台插件。
fn scan_plugin_dlls(client: &Path, switchable: bool) -> Vec<String> {
    let mut out = Vec::new();
    for dll in dlls_in(&client.join(LEGACY_ROOT)) {
        if dll.eq_ignore_ascii_case("mod-loader.dll") {
            continue;
        }
        out.push(format!("{LEGACY_ROOT}/{dll}"));
    }
    if !switchable {
        return out;
    }
    let Ok(entries) = fs::read_dir(client.join(SWITCH_ROOT)) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().map(|v| v.to_string_lossy().into_owned()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        for dll in dlls_in(&path) {
            out.push(format!("{SWITCH_ROOT}/{name}/{dll}"));
        }
    }
    out
}

fn read_loader_status(switch_dir: &Path) -> LoaderStatus {
    let path = switch_dir.join("mod-loader.status.json");
    let Some(text) = fs::read_to_string(&path).ok() else {
        return LoaderStatus::default();
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let Ok(file) = serde_json::from_str::<LoaderStatusFile>(text) else {
        return LoaderStatus::default();
    };
    LoaderStatus {
        seen: true,
        ready: file.ready,
        reject_reason: file.reject_reason,
        declared: file.declared,
        started: file.started,
        failed: file.failed,
        rows: file
            .mods
            .into_iter()
            .map(|row| {
                (
                    row.id.to_lowercase(),
                    row.loaded,
                    row.fail,
                    row.returned,
                )
            })
            .collect(),
    }
}

impl LoaderStatus {
    /// 按登记名取统一入口报的实装结果；没这一条就是 `None`（它没装卸过这个名）。
    fn row_of(&self, id: &str) -> (Option<bool>, String, u32) {
        let key = id.to_lowercase();
        match self.rows.iter().find(|(seen, _, _, _)| *seen == key) {
            Some((_, loaded, fail, returned)) => {
                (Some(*loaded), fail.clone(), *returned)
            }
            None => (None, String::new(), 0),
        }
    }
}

fn base_name(path: &str) -> String {
    Path::new(&path.replace('/', "\\"))
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

fn stem_of(file_name: &str) -> String {
    Path::new(file_name)
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| file_name.to_string())
}

/// 状态文件由插件自己挑名字，宿主与清单都不管。先认清单名，再认 DLL 文件名的短横线小写形
/// 式 —— 清单名与文件名对不上是盘上实况（`apocalypse-native-whole-hard` 写的是
/// `apocalypse-native-hard.status.json`），只认清单名会把那份自检结果整个读丢。
fn status_names(name: &str, path: &str) -> Vec<String> {
    let dll = base_name(path);
    let stem = stem_of(&dll);
    let mut names: Vec<String> = Vec::new();
    for candidate in [name.to_string(), kebab(&stem)] {
        if !candidate.is_empty() && !names.contains(&candidate) {
            names.push(candidate);
        }
    }
    names
}

/// `ApocalypseNativeHard` → `apocalypse-native-hard`、`CNFontRaster` → `cn-font-raster`。
/// 连续大写只在后面接小写时才断词，否则 `CNFont` 会被拆成 `c-n-font`。
fn kebab(stem: &str) -> String {
    let chars: Vec<char> = stem.chars().collect();
    let mut out = String::new();
    for (index, ch) in chars.iter().enumerate() {
        let break_before = index > 0
            && ch.is_uppercase()
            && (!chars[index - 1].is_uppercase()
                || chars
                    .get(index + 1)
                    .is_some_and(|next| next.is_lowercase()));
        if break_before {
            out.push('-');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

fn dlls_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            path.extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("dll"))
                .unwrap_or(false)
                .then(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))?
        })
        .collect()
}

fn read_status(dir: &Path, names: &[String]) -> (Option<bool>, String) {
    for name in names {
        let path = dir.join(format!("{name}.status.json"));
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(status) = serde_json::from_str::<PluginStatus>(&text) {
            return (status.ready, status.reject_reason);
        }
    }
    (None, String::new())
}

fn tail_lines(path: &Path, count: usize) -> Vec<String> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut lines: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    let keep = lines.len().saturating_sub(count);
    lines.drain(..keep);
    lines
}

/// modkit 自己的落点那一层：`registry.json` 与它放下的件都在这一层之下。
fn modkit_dir(root: &Root) -> std::path::PathBuf {
    config::client_dir(root)
        .join(".launcher-mods")
        .join("modkit")
}

fn modkit_mods(root: &Root) -> ModkitMods {
    let registry = modkit_dir(root).join("registry.json");
    let present = registry.is_file();
    let parsed: Option<Registry> = fs::read_to_string(&registry)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let Some(registry_file) = parsed else {
        return ModkitMods {
            present,
            registry: registry.display().to_string(),
            total: 0,
            entries: 0,
            risky: Vec::new(),
        };
    };

    let mut entries = 0usize;
    let mut risky = Vec::new();
    for module in &registry_file.mods {
        let list = module.entries.as_deref().unwrap_or_default();
        entries += list.len();
        for entry in list {
            let target = base_name(&entry.target);
            if BRICK_FILES.iter().any(|brick| *brick == target) {
                risky.push(ModkitRisk {
                    id: module.id.clone(),
                    target,
                    kind: entry.kind.clone(),
                });
            }
        }
    }
    ModkitMods {
        present,
        registry: registry.display().to_string(),
        total: registry_file.mods.len(),
        entries,
        risky,
    }
}

// ------------------------------------------------------------------------ 写回

/// 勾选/取消一个服务端 mod：写 `<整合包根>/mods/enabled.json` 的禁用名单。
///
/// 口径跟服务端 `servermod.SetModEnabled` 一致（同一份文件、同样是禁用名单、
/// 同样拒绝覆盖解析不了的旧文件），因为源码里明写了「唯一出路是把该 mod 写进
/// mods/enabled.json（启动器「MOD 工具」页可做）」——这一页就是那个页。
/// 落点由 `runtime_mods_dir` 按网关自己的解析算，不按「模块根」想当然。
pub fn set_mod_enabled(root: &Root, id: &str, enabled: bool) -> Result<String, String> {
    if !valid_mod_id(id) {
        return Err(format!(
            "mod id 不合法：{id}（只允许小写字母、数字、点与短横线，长度 1~64）"
        ));
    }
    let mods_dir = runtime_mods_dir(root);
    let path = mods_dir.join("enabled.json");

    let mut disabled: Vec<String> = Vec::new();
    if path.is_file() {
        let file = read_enabled_file(&path).map_err(|err| {
            format!("{err} —— 启动器拒绝覆盖，请先人工修好：{}", path.display())
        })?;
        disabled = file.disabled;
    }
    disabled.retain(|seen| seen.trim() != id);
    if !enabled {
        disabled.push(id.to_string());
    }
    disabled.sort();

    fs::create_dir_all(&mods_dir).map_err(|err| format!("建 mods 目录失败：{err}"))?;
    let body = serde_json::json!({
        "schema": 1,
        "disabled": disabled,
        "updatedAt": chrono::Local::now().to_rfc3339(),
        "updatedBy": "DFO-115US 启动器",
    });
    let tmp = mods_dir.join("enabled.json.tmp");
    let text = serde_json::to_string_pretty(&body).map_err(|err| err.to_string())?;
    fs::write(&tmp, format!("{text}\n")).map_err(|err| format!("写临时文件失败：{err}"))?;
    fs::rename(&tmp, &path).map_err(|err| format!("替换 enabled.json 失败：{err}"))?;

    Ok(format!(
        "已{}服务端 mod「{id}」（{}，下次启动服务端生效）",
        if enabled { "启用" } else { "停用" },
        path.display()
    ))
}

/// 与服务端 `validModID` 同一口径，免得写出一个服务端根本不认的名单。
fn valid_mod_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.chars().all(|ch| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '.' || ch == '-'
        })
}

/// 勾/撤一个客户端插件：写 `mods/mods.json` 里那一条的 `enabled`。
///
/// 为什么写这份而不是宿主的 `plugins.json`：那份归 modkit 所有（它是
/// `qol.r7-runtime-repair` 的 file.replace 目标），下一次装那个 mod 就连同开关一起被换回去。
/// 读改成 `Value` 上原地改一个字段再整体写回：清单里别人加的键不该被这一勾弄丢。
pub fn set_client_mod_enabled(root: &Root, id: &str, enabled: bool) -> Result<String, String> {
    let path = client_switch_file(root);
    let text = fs::read_to_string(&path)
        .map_err(|err| format!("mods.json 读不动（{}）：{err}", path.display()))?;
    let stripped = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut value: serde_json::Value = serde_json::from_str(stripped)
        .map_err(|err| format!("mods.json 解析不了，启动器拒绝覆盖：{err}"))?;
    let rows = value
        .get_mut("plugins")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "mods.json 里没有 plugins 数组，启动器拒绝改写".to_string())?;
    let row = rows
        .iter_mut()
        .find(|row| {
            row.get("name")
                .and_then(|v| v.as_str())
                .is_some_and(|name| name.eq_ignore_ascii_case(id))
        })
        .ok_or_else(|| {
            format!(
                "「{id}」不在 mods.json 里：清单外的文件是统一入口自动发现补上的，开关打不动它。\
                 要能关就得把它写进清单（重跑 analysis/tasks/mod-loader-unify-20261016）"
            )
        })?;
    let in_legacy = row
        .get("path")
        .and_then(|v| v.as_str())
        .is_some_and(rel_in_legacy_root);
    row["enabled"] = serde_json::Value::Bool(enabled);
    write_json_atomic(&path, &value)?;

    let client = config::client_dir(root);
    let mut message = format!(
        "已{}客户端 mod「{id}」（{}）",
        if enabled { "启用" } else { "停用" },
        path.display()
    );
    let host = read_plugin_file(&client.join(LEGACY_ROOT).join("plugins.json"));
    let reduced = matches!(host.0, ManifestState::Present) && is_reduced_host_manifest(&host.1);
    if !loader_will_run(&client) {
        message.push_str(
            " —— 注意：统一入口跑不起来（.115us-mods\\mod-loader.dll 不在场或被宿主清单禁用），\
             这份 mods.json 现在没人读。先跑 analysis/tasks/mod-loader-unify-20261016",
        );
    } else if in_legacy && !reduced {
        message.push_str(" —— 注意：宿主清单不是缩减版，这件存量 DLL 会被宿主自己装上，开关被绕过");
    }
    message.push_str("。开关下次启动客户端才生效：清单只在启动时读一次，装进来的模块被 PIN 到进程结束");
    Ok(message)
}

const IMPORT_SIZE_LIMIT: u64 = 64 * 1024 * 1024;

/// 导入一个 mod 目录：整份拷进 `<客户端>\mods\<mod-id>\` 并登记进 `mods.json`。
///
/// 只拷，不搬：源目录留在原处，导入失败不至于把别人的东西弄丢。
pub fn import_client_mod(root: &Root, source: &Path) -> Result<String, String> {
    let client = config::client_dir(root);
    let mods_dir = client.join(SWITCH_ROOT);
    let source = source
        .canonicalize()
        .map_err(|err| format!("源目录读不动：{err}"))?;
    if !source.is_dir() {
        return Err("导入要选的是一个 mod 目录，不是一个文件".to_string());
    }
    if let Ok(client_real) = client.canonicalize() {
        if source.starts_with(&client_real) {
            return Err("这个目录已经在客户端里了，不需要导入（它本来就在那两个根之下）".to_string());
        }
    }
    let name = source
        .file_name()
        .map(|v| v.to_string_lossy().into_owned())
        .ok_or_else(|| "源目录没有名字，取不到 mod id".to_string())?;
    let id = kebab(&name).to_lowercase();
    if !valid_mod_id(&id) {
        return Err(format!(
            "目录名「{name}」转不成合法 mod id（{id}）：只允许小写字母、数字、点与短横线；先把目录改个名再导入"
        ));
    }
    let target = mods_dir.join(&id);
    if target.exists() {
        return Err(format!(
            "已经导入过同名的 mod 目录：{}（要换版本请先把它删掉或改源目录名）",
            target.display()
        ));
    }
    let dlls = top_level_dlls(&source);
    if dlls.is_empty() {
        return Err(format!(
            "「{name}」这一层里没有 .dll：统一入口只认 mods\\<mod-id>\\ 之下的第一层 DLL"
        ));
    }
    let copied = copy_tree(&source, &target, IMPORT_SIZE_LIMIT)?;
    register_imported(&mods_dir, &id, &dlls, copied)
}

fn top_level_dlls(dir: &Path) -> Vec<String> {
    let mut names = dlls_in(dir);
    names.sort();
    names
}

/// 递归拷贝，累计字节数超上限就整份撤掉目标目录 —— 半套 mod 比没导更糟。
fn copy_tree(from: &Path, to: &Path, limit: u64) -> Result<u64, String> {
    fs::create_dir_all(to).map_err(|err| format!("建目标目录失败：{err}"))?;
    let mut total = 0u64;
    let mut stack = vec![from.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|err| format!("读 {} 失败：{err}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = path
                .strip_prefix(from)
                .map_err(|_| "拷贝路径跑出去了".to_string())?;
            let dst = to.join(rel);
            if path.is_dir() {
                fs::create_dir_all(&dst).map_err(|err| format!("建子目录失败：{err}"))?;
                stack.push(path);
                continue;
            }
            total += fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if total > limit {
                let _ = fs::remove_dir_all(to);
                return Err(format!("这个 mod 超过 {limit} 字节，不像一个 mod 目录（已撤销拷贝）"));
            }
            fs::copy(&path, &dst)
                .map_err(|err| format!("拷 {} 失败：{err}", dst.display()))?;
        }
    }
    Ok(total)
}

/// 把导入的 DLL 逐条登记进 `mods.json`；那份清单还不在场就新建一份，并在结论里说清楚
/// 「没缩宿主清单之前它不会被加载」，不然玩家以为导完就生效。
fn register_imported(mods_dir: &Path, id: &str, dlls: &[String], bytes: u64) -> Result<String, String> {
    let path = mods_dir.join("mods.json");
    let mut value: serde_json::Value = match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text))
            .map_err(|err| format!("mods.json 解析不了，启动器拒绝覆盖：{err}"))?,
        Err(_) => serde_json::json!({ "schema": 1, "autoDiscover": true, "plugins": [] }),
    };
    let root = value
        .as_object_mut()
        .ok_or_else(|| "mods.json 顶层不是对象，启动器拒绝改写".to_string())?;
    let rows = root
        .entry("plugins")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .ok_or_else(|| "mods.json 的 plugins 不是数组，启动器拒绝改写".to_string())?;
    let mut names = Vec::new();
    for dll in dlls {
        let row_id = if dlls.len() == 1 {
            id.to_string()
        } else {
            format!("{id}-{}", kebab(&stem_of(dll)))
        };
        if rows.iter().any(|row| {
            row.get("name")
                .and_then(|v| v.as_str())
                .is_some_and(|name| name.eq_ignore_ascii_case(&row_id))
        }) {
            return Err(format!("mods.json 里已有同名条目「{row_id}」，先人工改名"));
        }
        rows.push(serde_json::json!({
            "name": row_id,
            "path": format!("{SWITCH_ROOT}/{id}/{dll}"),
            "description": pick_description(dll, ""),
            "enabled": true,
            "entrypoint": true,
            "deps": [],
        }));
        names.push(row_id);
    }
    write_json_atomic(&path, &value)?;
    let mut message = format!(
        "已导入 mod「{id}」：{count} 枚 DLL、{bytes} 字节，落到 {dir}，登记为 {names}",
        count = dlls.len(),
        dir = mods_dir.join(id).display(),
        names = names.join("、"),
    );
    let client = mods_dir.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let host = read_plugin_file(&client.join(LEGACY_ROOT).join("plugins.json"));
    let reduced = matches!(host.0, ManifestState::Present) && is_reduced_host_manifest(&host.1);
    if !loader_will_run(&client) {
        message.push_str(
            " —— .115us-mods\\mod-loader.dll 不在场或被宿主清单禁用：这份 mods.json 没人读，\
             导入的这条要等 analysis/tasks/mod-loader-unify-20261016 跑过才会装载",
        );
    } else if !reduced {
        message.push_str(
            " —— 宿主清单不是缩减版：这条能装载，但 .115us-mods 里的存量件仍会被宿主绕过开关",
        );
    }
    message.push_str("；下次启动客户端生效");
    Ok(message)
}

fn write_json_atomic(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let tmp = path.with_extension("json.partial");
    let text = serde_json::to_string_pretty(value).map_err(|err| err.to_string())?;
    fs::write(&tmp, format!("{text}\n")).map_err(|err| format!("写临时文件失败：{err}"))?;
    fs::rename(&tmp, path).map_err(|err| format!("替换 mods.json 失败：{err}"))?;
    Ok(())
}

/// 启动器写的那份开关清单的落点。
fn client_switch_file(root: &Root) -> std::path::PathBuf {
    config::client_dir(root).join(SWITCH_ROOT).join("mods.json")
}

// ---------------------------------------------------------------- 服务端 mod 导入

/// 在资源管理器里打开 modkit 的落点那一层。
///
/// 路径由启动器自己算，界面只说「开哪一族」，不传路径 —— 免得把任意路径打开做成一条命令。
pub fn open_modkit_dir(root: &Root) -> Result<String, String> {
    let dir = modkit_dir(root);
    if !dir.is_dir() {
        return Err(format!(
            "这一层还不在场：{}（modkit 还没往这台客户端落过东西）",
            dir.display()
        ));
    }
    tauri_plugin_opener::open_path(&dir, None::<&str>)
        .map_err(|err| format!("打不开这个文件夹：{err}"))?;
    Ok(format!("已在资源管理器里打开：{}", dir.display()))
}

/// 服务端 mod 有两半，生效条件不一样，所以导入按内容分流：
///
/// - 规则脚本（`.lua`）平铺进 `<整合包根>/mods/`：网关每次启动自己扫这一层，不用重编译就生效；
/// - 整个 mod 目录另存一份到 `<整合包根>/mods/<mod-id>/`：Go 型那一半是编译进网关二进制的，
///   放对位置只是让它等着被重编译，不重编译不会装载。
///
/// 只拷不搬；落点先全部核对再动手，绝不覆盖玩家已有的脚本。
pub fn import_server_mod(root: &Root, source: &Path) -> Result<String, String> {
    let scripts_dir = root.mods_dir();
    let source = source
        .canonicalize()
        .map_err(|err| format!("源目录读不动：{err}"))?;
    if !source.is_dir() {
        return Err("导入要选的是一个 mod 目录，不是一个文件".to_string());
    }
    if let Ok(real) = scripts_dir.canonicalize() {
        if source.starts_with(&real) {
            return Err(
                "这份目录已经在服务端脚本目录里了，不需要导入（要挪位置请自己挪）".to_string(),
            );
        }
    }
    let name = source
        .file_name()
        .map(|v| v.to_string_lossy().into_owned())
        .ok_or_else(|| "源目录没有名字，取不到 mod id".to_string())?;
    let id = kebab(&name).to_lowercase();
    if !valid_mod_id(&id) {
        return Err(format!(
            "目录名「{name}」转不成合法 mod id（{id}）：只允许小写字母、数字、点与短横线；先把目录改个名再导入"
        ));
    }
    let target = scripts_dir.join(&id);
    if target.exists() {
        return Err(format!(
            "已经导入过同名的 mod 目录：{}（要换版本请先把它删掉或改源目录名）",
            target.display()
        ));
    }
    let scripts = rule_scripts(&source);
    if !scripts.is_empty() {
        // 这条 mod 若在停用名单里，平铺脚本会绕过名单：网关照读那一层，
        // 界面上刚按下去的「已停用」当场变成假话。名单读不动时不算（网关自己也读不动它）。
        let enabled_path = runtime_mods_dir(root).join("enabled.json");
        if let Ok(file) = read_enabled_file(&enabled_path) {
            if file.disabled.iter().any(|seen| *seen == id) {
                return Err(format!(
                    "「{id}」在服务端停用名单里（{}）：把它的规则脚本平铺到 {} 会让这条照样装载，\
                     开关就成了假话。请先在「服务端 mod」里把它打开，或先删掉这处落点再导入",
                    enabled_path.display(),
                    scripts_dir.display()
                ));
            }
        }
    }
    for file in &scripts {
        let stem = file
            .file_name()
            .ok_or_else(|| "脚本文件没有名字".to_string())?;
        let dst = scripts_dir.join(stem);
        if dst.exists() {
            return Err(format!(
                "{} 这一层已经有同名脚本「{}」，启动器不覆盖：要换版本请先把它删掉再导入",
                scripts_dir.display(),
                stem.to_string_lossy()
            ));
        }
    }
    fs::create_dir_all(&scripts_dir).map_err(|err| format!("建脚本目录失败：{err}"))?;
    let copied = copy_tree(&source, &target, IMPORT_SIZE_LIMIT)?;
    let mut placed: Vec<std::path::PathBuf> = Vec::new();
    for file in &scripts {
        let Some(stem) = file.file_name() else { continue };
        let dst = scripts_dir.join(stem);
        match fs::copy(file, &dst) {
            Ok(_) => placed.push(dst),
            Err(err) => {
                // 半套脚本比没导更糟：同名以先者为准，缺一份就会静默少一条规则。
                for done in &placed {
                    let _ = fs::remove_file(done);
                }
                let _ = fs::remove_dir_all(&target);
                return Err(format!("拷 {} 失败：{err}（已撤销本轮导入）", dst.display()));
            }
        }
    }
    let names: Vec<String> = placed
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    if names.is_empty() {
        return Ok(format!(
            "已把「{id}」整个目录放到 {}（{copied} 字节）：这一份里没取到 .lua 规则脚本，\
             它是 Go 型服务端 mod —— 服务端 mod 编译进网关二进制才会装载，启动器只把它放对位置",
            target.display()
        ));
    }
    Ok(format!(
        "已导入服务端 mod「{id}」：{count} 份规则脚本（{names}）平铺到 {}，整个目录另放到 \
         {}（{copied} 字节）。脚本下次「开始游戏」时由服务端自己读；Go 型那一半要重编译网关二进制\
         才会装载，不重编译不会出现在装载名单里",
        scripts_dir.display(),
        target.display(),
        count = names.len(),
        names = names.join("、"),
    ))
}

/// mod 目录里「该平铺进脚本目录」的那几份 `.lua`：先看 `rules/`（Go 型 mod 把规则放这一层），
/// 取不到才看 mod 目录第一层。只看这两层、不递归 —— 递归会把附带的样例当成规则发出去。
fn rule_scripts(source: &Path) -> Vec<std::path::PathBuf> {
    let in_rules = lua_files(&source.join("rules"));
    if !in_rules.is_empty() {
        return in_rules;
    }
    lua_files(source)
}

fn lua_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let is_lua = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("lua"))
                .unwrap_or(false);
            if is_lua {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 断词规则建立在盘上真实存在的 13 个文件名上：状态文件名是插件自己挑的，
    /// 拆错一个词，那一行的自检结果就永远读不到。
    #[test]
    fn kebab_matches_the_dll_names_on_disk() {
        for (stem, want) in [
            ("ApocalypseNativeHard", "apocalypse-native-hard"),
            ("ApocalypseNativeSourceProbe", "apocalypse-native-source-probe"),
            ("ApocalypseThirdCard", "apocalypse-third-card"),
            ("ApocalypseWaitingPanel", "apocalypse-waiting-panel"),
            ("CNFontRaster", "cn-font-raster"),
            ("ChineseInputProbe", "chinese-input-probe"),
            ("EliteCompanionPilot", "elite-companion-pilot"),
            ("HeadTextUpscale", "head-text-upscale"),
            ("LocalDungeonExit", "local-dungeon-exit"),
            ("LocalFeatures", "local-features"),
            ("RobotNativeAI", "robot-native-ai"),
            ("RobotObjectAuthority", "robot-object-authority"),
            ("WeaponSkinAnimation", "weapon-skin-animation"),
        ] {
            assert_eq!(kebab(stem), want, "{stem} 拆词不对");
        }
    }

    /// 盘上实况：清单名 `apocalypse-native-whole-hard`，插件写的是
    /// `apocalypse-native-hard.status.json`。只认清单名会把这份自检整个读丢。
    #[test]
    fn status_file_is_found_by_the_dll_name_when_the_manifest_name_differs() {
        let dir = std::env::temp_dir().join(format!("dfo-mods-status-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("临时目录");
        std::fs::write(
            dir.join("apocalypse-native-hard.status.json"),
            r#"{"ready":false,"rejectReason":"native-waiting-provider-not-ready"}"#,
        )
        .expect("造状态文件");
        let names = status_names("apocalypse-native-whole-hard", "ApocalypseNativeHard.dll");
        let found = read_status(&dir, &names);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(found.0, Some(false));
        assert_eq!(found.1, "native-waiting-provider-not-ready");
    }

    /// 清单名与文件名主干都能对上时不要重复查，也不要把空名字当成候选。
    #[test]
    fn status_candidates_are_deduped_and_never_empty() {
        let names = status_names("local-features", "LocalFeatures.dll");
        assert_eq!(names, vec!["local-features".to_string()]);
        assert!(!status_names("", "CNFontRaster.dll").contains(&String::new()));
    }

    /// 宿主把「没有清单」和「清单认不出形状」都当成没有清单，于是盘上每个 DLL 都被加载。
    /// 这时页面若因为读不到清单而一片空白，就是让玩家以为客户端一侧没装东西。
    /// 全程在临时目录里造整合包骨架，不碰真包。
    #[test]
    fn dlls_on_disk_are_listed_even_when_the_manifest_is_gone_or_corrupt() {
        let dir =
            std::env::temp_dir().join(format!("dfo-mods-manifest-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let plugins_dir = dir.join("client").join(".115us-mods");
        std::fs::create_dir_all(&plugins_dir).expect("临时插件目录");
        std::fs::create_dir_all(dir.join("server")).expect("临时 server 目录");
        for dll in ["CNFontRaster.dll", "FakePlugin.dll"] {
            std::fs::write(plugins_dir.join(dll), b"not a real dll").expect("造 DLL 占位");
        }
        let quoted = dir.join("client").display().to_string().replace('\\', "\\\\");
        std::fs::write(
            dir.join("server").join("launcher.local.json"),
            format!("{{\"client_dir\": \"{quoted}\"}}"),
        )
        .expect("造 launcher.local.json");
        let root = Root {
            path: dir.clone(),
            source: "test",
        };

        let bare = client_mods(&root);
        assert!(matches!(bare.manifest, ManifestState::Missing));
        assert_eq!(bare.plugins.len(), 2, "没有清单时盘上 DLL 都要列出来");
        assert!(bare
            .plugins
            .iter()
            .all(|p| !p.in_manifest && p.enabled && p.id == stem_of(&p.dll)));
        // 状态文件按文件名主干找得到，清单缺席也不能把自检丢掉。
        std::fs::write(
            plugins_dir.join("cn-font-raster.status.json"),
            r#"{"ready":false,"rejectReason":"face mismatch"}"#,
        )
        .expect("造状态文件");
        let with_status = client_mods(&root);
        let raster = with_status
            .plugins
            .iter()
            .find(|p| p.dll == "CNFontRaster.dll")
            .expect("CNFontRaster 在列");
        assert_eq!(raster.ready, Some(false));
        assert_eq!(raster.reject_reason, "face mismatch");

        std::fs::write(plugins_dir.join("plugins.json"), b"{ not json").expect("写坏清单");
        let corrupt = client_mods(&root);
        assert!(matches!(corrupt.manifest, ManifestState::Corrupt));
        assert_eq!(corrupt.plugins.len(), 2, "清单认不出时照样按盘上列");

        std::fs::write(
            plugins_dir.join("plugins.json"),
            r#"{"autoDiscover":false,"plugins":[{"name":"fake-plugin","path":"FakePlugin.dll","enabled":false}]}"#,
        )
        .expect("写真清单");
        let declared = client_mods(&root);
        assert!(matches!(declared.manifest, ManifestState::Present));
        assert_eq!(declared.plugins.len(), 2);
        let fake = declared
            .plugins
            .iter()
            .find(|p| p.dll == "FakePlugin.dll")
            .expect("FakePlugin 在列");
        assert!(fake.in_manifest && !fake.enabled, "清单里 enabled:false 要显示成没加载");
        let raster = declared
            .plugins
            .iter()
            .find(|p| p.dll == "CNFontRaster.dll")
            .expect("CNFontRaster 在列");
        assert!(
            !raster.in_manifest && !raster.enabled,
            "autoDiscover=false 时清单外不该显示成已加载"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 在临时目录里造一个整合包骨架：`server/launcher.local.json` 指过去 `client/`。
    fn temp_pack(tag: &str) -> (Root, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("dfo-mods-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("client").join(".115us-mods")).expect("临时插件目录");
        std::fs::create_dir_all(dir.join("server")).expect("临时 server 目录");
        let quoted = dir.join("client").display().to_string().replace('\\', "\\\\");
        std::fs::write(
            dir.join("server").join("launcher.local.json"),
            format!("{{\"client_dir\": \"{quoted}\"}}"),
        )
        .expect("造 launcher.local.json");
        (Root { path: dir.clone(), source: "test" }, dir)
    }

    const REDUCED_HOST: &str = r#"{"autoDiscover":false,"recurse":false,"plugins":[
        {"name":"mod-loader","path":"mod-loader.dll","enabled":true}]}"#;

    /// 迁移之后，开关的权威是 `mods/mods.json`；`.115us-mods\` 下没被声明的 DLL
    /// 一个都没人装卸 —— 界面必须这么说，不然玩家以为它还在跑。
    #[test]
    fn switches_come_from_mods_json_once_migrated() {
        let (root, dir) = temp_pack("switches");
        let plugins = dir.join("client").join(".115us-mods");
        let mods = dir.join("client").join("mods");
        std::fs::create_dir_all(mods.join("cool-mod")).expect("mods 子目录");
        for dll in [
            "CNFontRaster.dll",
            "EliteCompanionPilot.dll",
            "RobotNativeAI.dll",
            "mod-loader.dll",
        ] {
            std::fs::write(plugins.join(dll), b"not a real dll").expect("造存量 DLL");
        }
        std::fs::write(plugins.join("plugins.json"), REDUCED_HOST).expect("写缩减宿主清单");
        std::fs::write(mods.join("cool-mod").join("Cool.dll"), b"not a real dll").expect("造导入 DLL");
        std::fs::write(
            mods.join("mods.json"),
            r#"{"schema":1,"autoDiscover":true,"plugins":[
                {"name":"cn-font-raster","path":".115us-mods/CNFontRaster.dll","enabled":false},
                {"name":"robot-native-ai","path":".115us-mods/RobotNativeAI.dll","enabled":true},
                {"name":"cool-mod","path":"mods/cool-mod/Cool.dll","enabled":true,"origin":"手工导入"}]}"#,
        )
        .expect("写 mods.json");
        std::fs::write(
            mods.join("mod-loader.status.json"),
            r#"{"schema":1,"ready":true,"rejectReason":"","declared":3,"started":2,"failed":1,
                "mods":[{"id":"cn-font-raster","path":".115us-mods/CNFontRaster.dll","enabled":false,
                         "loaded":false,"returned":0,"fail":""},
                        {"id":"robot-native-ai","path":".115us-mods/RobotNativeAI.dll","enabled":true,
                         "loaded":true,"returned":3,"fail":"ModStart=3"},
                        {"id":"cool-mod","path":"mods/cool-mod/Cool.dll","enabled":true,
                         "loaded":false,"returned":0,"fail":"LoadLibrary 126"}]}"#,
        )
        .expect("写统一入口状态");

        let state = client_mods(&root);
        assert!(state.switchable, "mods.json 在场时开关该可用");
        assert!(state.bypass_note.is_empty(), "缩减版宿主清单不该报警：{}", state.bypass_note);
        assert!(state.loader.seen, "统一入口的自证状态要读到");

        let raster = state.plugins.iter().find(|p| p.dll == "CNFontRaster.dll").expect("在列");
        assert!(!raster.enabled, "mods.json 里的 enabled:false 要显示成关掉");
        assert!(raster.description.contains("宋体等宽光栅化"), "内置说明要在：{}", raster.description);

        let cool = state.plugins.iter().find(|p| p.dll == "Cool.dll").expect("导入的在列");
        assert!(cool.enabled && cool.in_manifest);
        assert_eq!(cool.loaded, Some(false), "统一入口报的是没装上");
        assert_eq!(cool.load_note, "LoadLibrary 126");

        // 装进进程 ≠ 在干活：入口调 `ModStart` 拿到非 0，说明 DLL 自己拒绝启动，
        // 这条必须带着返回码外传，否则界面只会显示「上次启动确实装上了」。
        let ai = state
            .plugins
            .iter()
            .find(|p| p.dll == "RobotNativeAI.dll")
            .expect("在列");
        assert_eq!(ai.loaded, Some(true), "入口确实把它 LoadLibrary 成功了");
        assert_eq!(ai.start_return, 3, "非 0 返回码不能被吞掉");
        assert_eq!(ai.load_note, "ModStart=3");

        let dark = state
            .plugins
            .iter()
            .find(|p| p.dll == "EliteCompanionPilot.dll")
            .expect("清单外文件也要列出来");
        assert!(!dark.in_manifest && !dark.enabled, "缩减之后没人扫这个根，不能显示成已加载");
        assert!(!dark.description.is_empty(), "清单外的件也该有内置说明");
        assert!(!state.plugins.iter().any(|p| p.dll == "mod-loader.dll"), "统一入口自己不是 mod");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 宿主清单已经缩减、但 mods.json 不在场：这时候客户端 mod 一个都不装，
    /// 页面必须直接说出来并指回迁移脚本，不能让玩家以为「跟以前一样」。
    #[test]
    fn reduced_host_manifest_without_mods_json_says_everything_is_dark() {
        let (root, dir) = temp_pack("dark");
        let plugins = dir.join("client").join(".115us-mods");
        std::fs::write(plugins.join("CNFontRaster.dll"), b"not a real dll").expect("造 DLL");
        std::fs::write(plugins.join("plugins.json"), REDUCED_HOST).expect("写缩减清单");
        let state = client_mods(&root);
        assert!(!state.switchable);
        assert!(state.bypass_note.contains("mod-loader-unify-20261016"), "{}", state.bypass_note);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// mods.json 在场、宿主清单却不是缩减版（比如那个 mod 被重装过）：开关会被绕过。
    #[test]
    fn full_host_manifest_beside_mods_json_warns_about_bypass() {
        let (root, dir) = temp_pack("bypass");
        let plugins = dir.join("client").join(".115us-mods");
        std::fs::create_dir_all(dir.join("client").join("mods")).expect("mods 目录");
        std::fs::write(
            plugins.join("plugins.json"),
            r#"{"autoDiscover":true,"recurse":true,"plugins":[
                {"name":"mod-loader","path":"mod-loader.dll","enabled":true},
                {"name":"other","path":"Other.dll","enabled":true}]}"#,
        )
        .expect("写全量宿主清单");
        std::fs::write(
            dir.join("client").join("mods").join("mods.json"),
            r#"{"schema":1,"plugins":[{"name":"other","path":".115us-mods/Other.dll","enabled":false}]}"#,
        )
        .expect("写 mods.json");
        let state = client_mods(&root);
        assert!(state.switchable);
        assert!(state.bypass_note.contains("绕过"), "{}", state.bypass_note);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 改一条开关只能动那一个字段：清单里别人写的键（导入来源、说明）不能在这一勾里丢掉。
    #[test]
    fn switch_write_keeps_unknown_keys_and_refuses_unknown_names() {
        let (root, dir) = temp_pack("write");
        let mods = dir.join("client").join("mods");
        let plugins = dir.join("client").join(".115us-mods");
        std::fs::create_dir_all(&mods).expect("mods 目录");
        // 迁移后的正常态：宿主只声明统一入口，统一入口自己也在场，这一勾才是「改了算数」。
        std::fs::write(plugins.join("plugins.json"), REDUCED_HOST).expect("写缩减清单");
        std::fs::write(plugins.join("mod-loader.dll"), b"not a real dll").expect("造统一入口");
        let path = mods.join("mods.json");
        std::fs::write(
            &path,
            r#"{"schema":1,"autoDiscover":true,"plugins":[
                {"name":"cn-font-raster","path":".115us-mods/CNFontRaster.dll","enabled":true,
                 "origin":"手工导入","description":"自定义文案"}]}"#,
        )
        .expect("写 mods.json");
        let message = set_client_mod_enabled(&root, "cn-font-raster", false).expect("改得动");
        assert!(
            message.contains("停用") && message.contains("下次启动客户端才生效"),
            "{message}"
        );
        assert!(!message.contains("注意"), "缩减清单 + 统一入口在场时不该报警：{message}");
        let text = std::fs::read_to_string(&path).expect("读回来");
        assert!(text.contains("\"origin\": \"手工导入\""), "未知键被弄丢了：{text}");
        assert!(text.contains("\"enabled\": false"), "开关没落盘：{text}");
        assert!(text.contains("自定义文案"), "说明被弄丢了：{text}");
        let err = set_client_mod_enabled(&root, "no-such-mod", true).unwrap_err();
        assert!(err.contains("不在 mods.json 里"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 导入 = 选一个 mod 目录 → 整份拷进 `mods/<mod-id>/` 并逐条登记。
    #[test]
    fn import_copies_the_folder_and_registers_each_dll() {
        let (root, dir) = temp_pack("import");
        let outer = std::env::temp_dir().join(format!("dfo-mods-src-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outer);
        let source = outer.join("MyMod");
        std::fs::create_dir_all(source.join("data")).expect("源子目录");
        std::fs::write(source.join("My.dll"), b"not a real dll").expect("造 DLL");
        std::fs::write(source.join("data").join("note.txt"), b"hello").expect("造附属文件");

        let message = import_client_mod(&root, &source).expect("导入成功");
        let target = dir.join("client").join("mods").join("my-mod");
        assert!(target.join("My.dll").is_file() && target.join("data").join("note.txt").is_file(),
                "附属文件要跟着拷：{}", target.display());
        let text = std::fs::read_to_string(dir.join("client").join("mods").join("mods.json"))
            .expect("登记写出来了");
        assert!(text.contains("mods/my-mod/My.dll"), "{text}");
        assert!(text.contains("\"autoDiscover\": true"), "新建清单要带自动发现：{text}");
        assert!(message.contains("mod-loader.dll 不在场"), "没迁移时要讲清楚不会装载：{message}");
        let err = import_client_mod(&root, &source).unwrap_err();
        assert!(err.contains("已经导入过同名"), "{err}");
        let _ = std::fs::remove_dir_all(outer);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 内置说明表的键必须钉在盘上真实文件名上：拼错一个字母，那一行就永远没提示。
    #[test]
    fn description_table_covers_every_client_dll() {
        let on_disk: Vec<String> = [
            "ApocalypseNativeHard",
            "ApocalypseNativeSourceProbe",
            "ApocalypseThirdCard",
            "ApocalypseWaitingPanel",
            "CNFontRaster",
            "ChineseInputProbe",
            "EliteCompanionPilot",
            "HeadTextUpscale",
            "LocalDungeonExit",
            "LocalFeatures",
            "RobotNativeAI",
            "RobotObjectAuthority",
            "WeaponSkinAnimation",
        ]
        .iter()
        .map(|stem| format!("{}.dll", stem.to_lowercase()))
        .collect();
        for name in &on_disk {
            assert!(
                PLUGIN_DESCRIPTIONS.iter().any(|(key, _)| key == name),
                "「{name}」没有内置说明"
            );
            assert!(!pick_description(name, "").is_empty(), "「{name}」的说明是空的");
        }
        assert_eq!(PLUGIN_DESCRIPTIONS.len(), on_disk.len(), "表里多了一条不存在的件");
    }

    /// 服务端 mod 有两半：规则脚本要平铺进 `<包根>/mods` 才不用重编译，整个目录另存一份等着重编译。
    #[test]
    fn import_server_mod_routes_scripts_and_keeps_the_folder() {
        let (root, dir) = temp_pack("srv-import");
        let outer = std::env::temp_dir().join(format!("dfo-mods-srvsrc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outer);
        let source = outer.join("giveaway.random-equipment");
        std::fs::create_dir_all(source.join("rules")).expect("rules 子目录");
        std::fs::write(source.join("mod.go"), b"package giveaway").expect("造 mod.go");
        std::fs::write(source.join("rules").join("giveaway.lua"), b"-- rule").expect("造规则脚本");
        std::fs::write(source.join("README.md"), b"readme").expect("造附带文件");

        let message = import_server_mod(&root, &source).expect("导入成功");
        let scripts = dir.join("mods");
        assert!(
            scripts.join("giveaway.lua").is_file(),
            "规则脚本要平铺进脚本目录：{}",
            scripts.display()
        );
        let laid = scripts.join("giveaway.random-equipment");
        assert!(
            laid.join("mod.go").is_file() && laid.join("rules").join("giveaway.lua").is_file(),
            "整个目录要另存一份：{}",
            laid.display()
        );
        assert!(message.contains("1 份规则脚本"), "要说清平铺了几份：{message}");
        assert!(message.contains("重编译"), "Go 型那一半要讲清不重编译不装载：{message}");

        let err = import_server_mod(&root, &source).unwrap_err();
        assert!(err.contains("已经导入过同名"), "{err}");
        let _ = std::fs::remove_dir_all(outer);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 同名脚本一律不覆盖，而且落点先全部核对：撞名就一个字节都不落。
    #[test]
    fn import_server_mod_refuses_to_overwrite_a_same_named_script() {
        let (root, dir) = temp_pack("srv-clash");
        let outer = std::env::temp_dir().join(format!("dfo-mods-srvclash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outer);
        let source = outer.join("boxed.loot");
        std::fs::create_dir_all(source.join("rules")).expect("rules 子目录");
        std::fs::write(source.join("rules").join("a.lua"), b"mine").expect("造脚本 a");
        std::fs::write(source.join("rules").join("b.lua"), b"ours").expect("造脚本 b");
        std::fs::create_dir_all(dir.join("mods")).expect("脚本目录");
        let keep = dir.join("mods").join("b.lua");
        std::fs::write(&keep, b"player-s own").expect("造同名脚本");

        let err = import_server_mod(&root, &source).unwrap_err();
        assert!(err.contains("已经有同名脚本"), "{err}");
        assert!(
            !dir.join("mods").join("a.lua").exists(),
            "撞名时不能先把另一份落下"
        );
        assert!(
            !dir.join("mods").join("boxed.loot").exists(),
            "撞名时整个目录也不落"
        );
        assert_eq!(std::fs::read(&keep).expect("读回同名脚本"), b"player-s own");
        let _ = std::fs::remove_dir_all(outer);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 名单里停着的那一条不能靠平铺脚本绕回去：那样界面上的「已停用」就成了假话。
    #[test]
    fn import_server_mod_refuses_to_route_scripts_of_a_disabled_mod() {
        let (root, dir) = temp_pack("srv-disabled");
        let outer = std::env::temp_dir().join(format!("dfo-mods-srvoff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outer);
        let source = outer.join("giveaway.random-equipment");
        std::fs::create_dir_all(source.join("rules")).expect("rules 子目录");
        std::fs::write(source.join("rules").join("giveaway.lua"), b"-- rule").expect("造规则脚本");
        std::fs::create_dir_all(dir.join("server").join("mods")).expect("名单目录");
        std::fs::write(
            dir.join("server").join("mods").join("enabled.json"),
            r#"{"disabled":["giveaway.random-equipment"]}"#,
        )
        .expect("造停用名单");

        let err = import_server_mod(&root, &source).unwrap_err();
        assert!(err.contains("在服务端停用名单里"), "{err}");
        assert!(
            !dir.join("mods").join("giveaway.lua").exists(),
            "拒绝时一个字节都不落"
        );
        assert!(
            !dir.join("mods").join("giveaway.random-equipment").exists(),
            "拒绝时整个目录也不落"
        );
        let _ = std::fs::remove_dir_all(outer);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 只带 Go 件、一份 .lua 都没有的那一种：位置照样放对，但界面必须说清它现在不生效。
    #[test]
    fn import_server_mod_names_a_go_only_mod_as_inert() {
        let (root, dir) = temp_pack("srv-go-only");
        let outer = std::env::temp_dir().join(format!("dfo-mods-srvonly-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outer);
        let source = outer.join("plain.hook");
        std::fs::create_dir_all(source.join("rules")).expect("rules 子目录");
        std::fs::write(source.join("mod.go"), b"package hook").expect("造 mod.go");

        let message = import_server_mod(&root, &source).expect("导入成功");
        assert!(
            dir.join("mods")
                .join("plain.hook")
                .join("mod.go")
                .is_file(),
            "整个目录要落进脚本目录的子目录"
        );
        assert!(!dir.join("mods").join("mod.go").exists(), "Go 件不该被平铺");
        assert!(message.contains("没取到 .lua 规则脚本"), "{message}");
        assert!(message.contains("放对位置"), "要说明这只是放对位置：{message}");
        let _ = std::fs::remove_dir_all(outer);
        let _ = std::fs::remove_dir_all(dir);
    }
}

