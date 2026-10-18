# DFO-115US 单机启动器（Tauri 重构版）

一个 Windows 桌面的图形启动器：把 DFO 115US 单机整合包的「起服务端 → 拉客户端 → 盯日志 → 收残留」收成一个按钮，
外加 mod 开关、存档备份、连发工具与故障上报。前端 React 19 + TypeScript + Vite，后端 Tauri 2（Rust）。

> **English TL;DR** — A Windows GUI launcher for the DFO 115US single-player pack. It is a *driver*, not the
> server: this repository contains **only the launcher's source code**. The game client, the gateway
> (`wireprobe-*.exe`) and the orchestrator CLI (`dfolauncher.exe`) come from the pack itself and are not
> distributed here. Built without a pack next to it, the launcher simply reports 「未找到」.

---

## 先说清楚这个仓库里没有什么

| 不在本仓库 | 从哪来 |
| --- | --- |
| 游戏客户端 `DFO.exe` / `Script.pvf` / `sk.dat` | 整合包 |
| 服务端网关 `wireprobe-*.exe` | 整合包 `server/work/dfo-lan/bin/` |
| **编排程序 `dfolauncher.exe`** | 整合包 `server/work/dfo-lan/bin/` |
| 存档数据库 `dfolan.sqlite3` | 服务端首次连接时自己建 |
| 任何 DLL mod、PVF 数据、Lua 脚本 | 整合包 |

启动器不下载、不编译、也不替代上面任何东西。它与整合包之间**只有一条契约**：

```
<root>/server/work/dfo-lan/bin/dfolauncher.exe launch --root <root> [--server-only]
<root>/server/work/dfo-lan/bin/dfolauncher.exe stop   --root <root>
```

网关的启动顺序、WFP 回环隔离、PVF 拆壳、客户端载荷——全在那条 CLI 里。启动器负责的是它周围那一圈工程问题：
端口被谁占了、启动到底成没成、客户端是自己崩的还是被关的、崩了要留什么证据、玩家的存档不能被顺手写坏。

---

## 功能

侧栏七个页面，全部对着真实整合包跑通过（不是界面壳子）。

- **主页** — 一个「开始游戏」大按钮 + 四项就绪检查（服务端编排 / 游戏客户端 / 存档 / 网关端口 7001），
  外加账号、客户端目录、环境体检、端口与残留清理。
- **mod** — 四族互不相干的 mod 各有各的口径：服务端 mod（写 `mods/enabled.json` 禁用名单）、
  Lua 奖励脚本、客户端 DLL 插件（`<客户端>/.115us-mods/plugins.json`）、modkit 安装层
  （`.launcher-mods/modkit/registry.json`）。运行中拒绝改服务端名单——网关只在 boot 时读一次。
- **工具** — 键盘连发引擎（WH_KEYBOARD_LL 低级钩子 + SendInput 扫描码），带热键、目标窗口门、节拍与连招编辑。
- **存档** — 只读 `VACUUM INTO` 备份 + 整库替换；替换前自动快照、按大小与 SHA-256 校验暂存件、
  换完清掉 `-wal` / `-shm` / `-journal`。服务端正连着库时一律拒绝。
- **设置** — 启动方式（原生 / 仅服务端）、关游戏后是否自动停服、自动上报开关与上传范围说明。
- **反馈** — 问题看板、单条时间线、我的反馈、提交新问题。
- **开发者** — 运行日志与游戏内容模式两块面板，可停靠成二分树，也能浮成独立窗口再拖回来归位。

---

## 构建

### 前置

只支持 Windows。构建机需要：

1. **Rust**（实测 1.99.0，MSVC 工具链）+ **Visual Studio Build Tools** 的 *Desktop development with C++* 工作负载
2. **Node.js**（实测 v24.19.0）与 npm
3. **WebView2 Runtime**（Win11 / 较新 Win10 自带；缺了界面起不来）
4. `curl.exe` 与 `reg.exe`（系统自带，上报与反馈走它们，不额外引 TLS 依赖）

依赖已经锁死：`package-lock.json` 与 `src-tauri/Cargo.lock` 都在库里，`rusqlite` 用 `bundled` 特性自带 SQLite，
不需要额外的 C 库。

```bash
npm install            # .npmrc 把 registry 指到 npmmirror，国内直连；换网络可自行删掉这一行
```

### 开发

```bash
npx tauri dev
```

Vite 固定占用 `1420`（`strictPort`），被占就直接失败而不是悄悄换端口。
`tauri dev` 需要一个整合包当根目录，否则每个页面都会报「未找到」——这是预期的，不是构建坏了：

```bash
set DFO_ROOT=D:\115us\服务端        # 或者 npx tauri dev -- --root "D:\115us\服务端"
```

### 出包

```bash
npx tauri build
```

产物：

- `src-tauri/target/release/dfo-launcher.exe` — 单文件绿色版，直接丢进整合包根目录就能跑
- `src-tauri/target/release/bundle/nsis/DFO115USLauncher_0.1.0_x64-setup.exe`
- `src-tauri/target/release/bundle/msi/DFO115USLauncher_0.1.0_x64_en-US.msi`

**必须走 `npx tauri build`，不能 `cargo build --release`。** `Cargo.toml` 里的 `prod` feature 挂着
`tauri/custom-protocol`；漏了它，release 二进制会去载 `devUrl`（`localhost:1420`），双击只剩一张
「localhost 拒绝连接」错误页，而且大小和内容看起来都正常。要用 cargo 就自己补上：

```bash
cargo build --release --features prod     # 等价于 tauri build 里 Rust 那一步
```

图标由 `npx tauri icon < master.png >` 重生成，产物提交在 `src-tauri/icons/`。
`build.rs` 里显式声明了 `cargo:rerun-if-changed=icons/icon.ico`——tauri-build 自己不管这个，
不加就会静默链回旧图标、exe 字节数还一模一样。

### 权限等级

`build.rs` 按构建配置注入应用清单：发布件 `highestAvailable`（网关的 WFP 回环隔离需要管理员，
拿不到时编排 CLI 自己降级），开发件 `asInvoker`。dev 一旦提权，UAC 会切断 `tauri dev` 的 stdout 管道，热重载直接哑掉。

---

## 运行时的盘上契约

### 根目录怎么定

优先级从高到低，**刻意不做「向上查找」**——找错根目录会让下面每一条相对路径都静默指到别处：

1. `--root <路径>`
2. 环境变量 `DFO_ROOT`
3. 启动器 exe 自己所在目录
4. 当前工作目录

整合包根目录必须可写（启动第一刻就探针验证）。两种布局都认：

```
经典： <root>/client/                     <root>/server/
拆分： <root>/客户端/DFO/（root 常为 …/服务端）  <root>/server/
```

「是客户端目录」的判定只认 `DFO.exe` + `Script.pvf` 两件。`sk.dat` **不在**判定里：它是外层 PVF 保护的
段密钥密文，已拆壳的包根本不带它，需要时由编排 CLI 报出精确错误。小于 2,560 B 的 `sk.dat` 只可能是占位件，
健康检查据此改措辞，但永远不构成阻塞。

### 读写的配置（三份，全在整合包里）

| 文件 | 用途 | 启动器动它的方式 |
| --- | --- | --- |
| `<root>/launcher.settings.json` | 账号 / 模式 / 自动停服 / 自动拾取 / 上报 / 连发 | 只改自己那一个键，其余原样保留 |
| `<root>/server/launcher.local.json` | `client_dir`、`server_binary` | 同上，客户端目录优先存相对形式 |
| `<root>/server/work/dfo-lan/runtime/storage/local.json` | 存档路由 `driver` / `sqlite_path` | **只在缺 `sqlite_path` 时补一份**；写的是别的驱动就报出来让人决定，绝不替玩家改 |

全部原子写（`.partial-<pid>` 落盘再改名），并保留历史缩进风格。

启动器自己的数据在 `%LOCALAPPDATA%\DFO115US-Tauri`（`DFO_LAUNCHER_HOME` 可覆盖）：
`logs/launcher-YYYYMMDD.log`、`reports/queue/`、`reports/sent.json`。

### 注入给编排 CLI 的环境变量

| 变量 | 何时 |
| --- | --- |
| `DFO_ODYSSEY_MODE=1\|0` | 模式选了奥德赛 / 剧情；`auto` 不注入，交给服务端自己判 |
| `DFO_ACCOUNT` | 账号不是默认的 `probe` 时 |
| `DFO_REWARD_SCRIPTS_DIR` | `<root>/mods`，Lua 奖励脚本目录 |
| `DFO_AUTO_PICKUP=1\|0` | 自动拾取；服务端只在环回连接时认 `"1"` |

---

## 状态机与时序

`idle → starting → running → stopping → idle`，出错落 `error`。判定「起来了」用的是**网关 TCP 连通 +
`DFO.exe` 进程在场**，不是编排 CLI 的退出码。

| 环节 | 预算 |
| --- | --- |
| 启动总超时 | 180 s（轮询步长 500 ms） |
| 客户端 PID 确认 | 30 s |
| 停止脚本 | 90 s |
| 停止后等端口释放 | 10 s |
| 关游戏自动停服的 PID 锁定窗口 | 180 s |

实测：**点下「开始游戏」到 `DFO.exe` 存在 ≈ 70 s**。所有等客户端的窗口都必须超过这个数——
之前那个「关游戏后不自动停服」的缺陷就是这么来的：窗口只有 60 s，永远错过进程。
网关在绑定 7001 **之前**要花 ~50 s 准备 PVF（受保护归档的代价；已拆壳的包约 2 s 过），
所以「20 秒了端口还空着」是正常的。

崩溃分类结合退出码与 `client.log` 里的 `SUMMARY exit=0x… normal_run=1`：有 `normal_run=1` 就不算崩。
失败时把 `gateway.err`、`helper.err`、`client.log` 一起附进报告。

`auto_stop_on_client_exit` 只在开始游戏时读一次，中途改不动本场。

---

## 端口与进程治理

只关心 **7001**。占用的可能是别的启动器（旧版 Go 启动器驱动同一个整合包，抢同一个端口），所以：

- 端口被监听但不是自己起的 → **认领并显示**「运行中（网关已在监听，非本启动器发起）」，不冒充自己的会话。
- 自动清理只认本项目自己的镜像名前缀（`dfolauncher*`、`wireprobe*` / `probe*`）。
- **玩家自己开着的 `DFO.exe` 或任何认不出的进程，一律不动**；「强制清理并继续」也只打白名单镜像名。
- 停止顺序：客户端进程树 →（自己拉起过才按名清）`DFO.exe` → 8 个服务端镜像名 → 编排 CLI `stop` → 等端口释放。

---

## 遥测与隐私

自动上报**默认关闭**，且首次开启必须先看过一次上传范围说明。没点同意，一个字节都不发。

- 采集范围：启动失败 / 游戏崩溃 / 启动器自身 panic 三类事件，加 Windows 版本、客户端文件版本、复现步骤。
- 出门前一律脱敏：`USERPROFILE` / `LOCALAPPDATA` / `APPDATA` / `USERNAME` / 整合包路径 → `<本机路径>`，
  账号 → `<账号>`，含 `token` / `password` / `secret` / `cookie` / `验证码` 的行在第一个 `=` 或 `:` 处截断为
  `：<已隐去>`；剥 ANSI、限长、附件跳过含 NUL 的内容。
- 传输走系统 `curl.exe` POST 到 `src-tauri/src/site.rs` 里的 `SITE`（默认 `https://134.175.57.52`），
  证书由系统根证书校验，不加 `-k`、不做 pinning。离线时留在 `reports/queue/`（上限 40 条），
  插网后手动催或按退避重试（30/120/600/1800/3600 s，最多 8 次）。
- 玩家自己提的反馈，回执凭证只落本机私有留档，日志里只出现编号。

> 这个仓库是公开的，`SITE` 常量会连同源码一起公开。想换上报服务器，改这一处即可。
> 反馈与上报接口都没有账号体系，也不带任何凭据——「我的反馈」靠提交时发回、只存在本机的回执配对去查。

---

## 架构

```
src/                 React 前端
  bridge.ts            全部 invoke / event 的唯一入口，组件不手写命令名
  App.tsx              主窗：路由、状态轮询（4 s）、弹层
  PanelApp.tsx         浮出面板按窗口 label 认领内容（同一份 bundle）
  dock.ts              停靠层的纯数据二分树，命中测试只用 CSS 像素
  components/          七个页面 + 面板 + 图标
src-tauri/src/       Rust 后端
  lib.rs               装配：状态托管、命令表、退出钩子、后台环境准备
  commands.rs          面向界面的全部命令（一律 spawn_blocking，主线程一卡窗口就未响应）
  session.rs           状态机 + 编排 CLI 调用 + 客户端退出看守 + 崩溃归类
  paths.rs             根目录解析与全部固定路径
  config.rs            三份盘上配置的读写
  health.rs            环境体检（哪些是阻塞项、哪些只是提示）
  portguard.rs ports.rs procutil.rs   端口归属与进程名/进程树治理
  mods.rs saves.rs     mod 四族 /  SQLite 备份替换
  autofire.rs          低级钩子连发引擎
  report.rs site.rs    诊断报告与反馈站接口
  logbus.rs panels.rs  日志环形缓冲 + 面板窗口生命周期
```

### 改代码前要知道的四件事

1. **同步命令跑在主线程**：`with_webview` 在 Windows 主线程上会死锁，面板逻辑一律走已有的窗口句柄与事件。
2. **面板窗口是预先声明的**，运行时不新建窗口（`tauri.conf.json` 里 `panel-log` / `panel-mode` 一开始就在，
   默认 `visible: false`）。运行时建窗在 WebView2 上会挂。收面板用 `hide` 而非 `destroy`，
   且要重试——拖拽的模态循环会吃掉第一次 hide。
3. **`"about:blank"` 是 Windows 上唯一预声明的空白窗**，想要别的地址得自己配进 `windows`。
4. **`tauri build` 会往 exe 里补打包信息**，同一份源码两次构建哈希不同。别拿哈希相等当「源码没动」的证据，
   要证明某个常量真进了包，直接在二进制里搜字符串。

前端样式与旧版启动器**没有继承关系**——行为照旧版抄，像素一律自己画。改界面时别去对齐旧版截图。

---

## 许可

AGPL-3.0（见 `LICENSE`）。游戏客户端、服务端与整合包内容不在本仓库的授权范围内，本许可只覆盖启动器源码。
