# 更新日志

按日期倒序。写的是「装上这一版之后玩家会看见什么、以及它凭什么算验过」。
整合包（服务端 / 网关 / 客户端）自己的变更不在这儿，在服务端工作区的 CHANGELOG 里。

哈希都是换上进整合包目录 `服务端\dfo-launcher.exe` 的那枚，不是构建中间产物。

---

## 2026-10-19 · 服务端内存调节面板 + 实时占用

> TL;DR — A third card on the Settings page now edits the orchestrator's three optional memory keys
> (`launcher.local.json`, BOM-free, foreign keys preserved) and shows the gateway's live memory from the
> guard lines it already writes. Blank / `0` means *no limit*; nothing is back-filled.

**新增**

- 设置页第三张卡「服务端内存」：提交内存硬上限（超过就被系统直接杀）、Go 运行内存目标（软约束，只让 GC
  提前动手）、导入并发上限（>65536 编排器拒绝启动）三格，读写 `<服务端>\server\launcher.local.json`。
- 写回是 JSON 往返：只碰这三个键，别人的键（含陌生的嵌套对象）原样留着，输出**无 BOM**——Go 的
  `json.Unmarshal` 见到 BOM 会整份配置读失败。
- 「不限」就是**把这个键删掉**，不是写 `0`，也不会替你回填任何默认值。
- 实时占用块：直接解析最新那场 `runtime/<会话tag>/gateway.err` 里的 `MEMORY_GUARD active/sample/ended` 行
  （2 秒一轮），给硬上限占用尺、私有提交小曲线（最多 90 点）与六个数；本场没设上限时**不画尺**，只写
  「本场没设硬上限（不限）」。
- 读不到就打读不到：没有 `active` 行或一条采样都还没有时面板空着，不摆看着像真数的假数。跑旧编排器
  （`policy=…-r1`）那一场的会额外提醒「本场上限口径按旧版算」。
- 校验在写命令之前：非法值（小数、越界）边打字边转红，按下「写入设置」也不会发出任何东西。
- 生效条件写进卡里：这三项只在编排器下次拉起网关时才读，改了要重开一场（下次「开始游戏」）。

**验收**

| # | 结果 |
| --- | --- |
| 1 非零上限进 `active` 行 + 横幅「内存保护已启用」 | 实机已跑（`hard_commit_mib=4096`，横幅同场可见） |
| 2 三键清空 → `unlimited` + 横幅「内存保护未启用 … 不限 … 默认」 | 实机已跑 |
| 3 往返无损、无 BOM、Go 读得动 | 实机 + 离线双证（清空那次落盘只剩 `client_dir`/`server_binary`，下一场照常起来） |
| 4 曲线/数字与采样行对得上 | 解析器对两场真实日志逐字段核对一致；屏上跟随还欠一张图 |
| 5 不动 Go 源码、不动服务端二进制 | 已证（`bin/dfolauncher.exe` 仍是 `5835d7bf…f7aecb`） |

**门禁** `tsc --noEmit` / `tsc -p tsconfig.sandbox.json --noEmit` / `npm run build` 全 0，`cargo test --lib`
50 通过 0 失败，沙盒 31 用例 0 失败（新增 K01 五种采样状态读法、K02 非法值就地拦下）。产物字节核对：
`4 GiB` / `2 GiB` 各 0 次（红线：不得再出现「默认 4 GiB / 2 GiB」字样）。

**出包与换上** 新 `e9f6a5f822b7c1bc6bb9d25e0b935a501a79443ad028a46a428173a01c74926f`（8 797 184 字节）；
被它换下的 `9384795258e9c10d641b1772b117fd352ab24565b8ea6020ca74bb57bedabac0` 留名为
`dfo-launcher.exe.bak-93847952`。同代判据构建前后各跑一次：前一次只列出本轮 8 个文件，后一次为空。

**还没做** 主页那张「更新日志」卡的条目源仍未接（页签能切，槽位是空的）；#4 的屏上一张截图。

---

## 2026-10-19 · 服务端内存卡片搬进开发者页、卡片可收起成胶囊

> TL;DR — 「服务端内存」不再是设置页的一张卡，而是开发者页停靠层的第五块面板；每块面板标题栏多一枚
> 「收起」按钮，把它折成右侧「摆放栏」里的一枚胶囊，点一下接回末尾、拖着可以摆到任意分割位。

**改动**

- `src/dock.ts`：`PanelKey` 增加 `memory`，默认布局把 `服务端内存` 排在 `客户端插件` 右边
  （`h-split 0.44 { 日志, v-split { {模式, mod}, {插件, 内存} } }`）。
- `src/components/MemoryCard.tsx` 改成**无 props** 的面板正文：通知不再依赖主窗口的全局 toast，
  改成卡片正文下方一行 6 秒后自动消失的 `.mem-note`——这样 dock 里和独立浮出窗口里表现一致。
  `src/components/SettingsPage.tsx` 里那张卡整体移除，设置页只剩 启动 / 客户端与存档 / 配色方案 /
  液态玻璃 / 自动上报 / 主页背景。
- `tauri.conf.json` 预声明 `panel-memory` 浮窗（520×620，与其余四块同一套 ACL 与无边框配置）。
- 收起成胶囊是**派生状态**，不额外落盘：某块是胶囊 ⇔ 它既没浮出、也不在 dock 树里；收起 =
  `strip(node, k)`，展开 = `place(node, k, null)`（接在文档序末尾）。摆放栏顺序按 `PANEL_KEYS`，
  不受收起先后影响。因此 `leaves ∪ pills` 恒等于五块，不会出现「既不在栏里也不在树上」的孤儿面板。
- 标题栏挤不下的时候折行而不是把标题压成一列竖字：`.panel-head` 改 `flex-wrap: wrap`，标题与状态芯片
  `white-space: nowrap`。加了收起/浮出两枚按钮后，窄格子里「客户端插件」曾竖排成五个字。
- `.tray-pill` 接进液态玻璃引擎（高光、悬停抬起、减弱动效三处规则）与 focus-visible 环；玻璃规则的
  特异性高于 `.tray-pill:hover`，不挂进去胶囊就没有悬停反馈。

**验收**

- 沙盒 32 用例 0 失败。新增 `K03 开发者页：收起成胶囊、摆放栏点数、点胶囊归位`，逐步断言真实序列：
  `log,mode,mods,modkit,memory ▸ 空栏` → 收起内存 → `…modkit ▸ memory` → 收起日志 →
  `mode,mods,modkit ▸ log,memory` → 点内存胶囊 → `…modkit,memory ▸ log` → 点日志胶囊 → 五块全回、空栏
  → 收起/展开 mod 后仍是五块。探针 `sandbox/probe.ts` 新增 `dock.{leaves,pills,tray}` 与 `memory.note`。
- 沙盒 URL 新增 `?collapse=log,modkit`：无头页没有鼠标，只能这样把胶囊态摆出来拍照；真机不读这个参数。
  `?page=` 选择器修成 `.rail .rail-item[aria-label=…]`（「开发者」在侧栏底部组，`.rail-nav` 选不到）。
- 出图四张（暗色五块、暗色三枚胶囊、亮色+玻璃两枚胶囊、设置页已无内存卡）：
  `tmp/shots/dev-dark2.png`、`dev-dark-pills2.png`、`dev-light-glass-pills2.png`、`settings-dark.png`。
- 门禁：`tsc --noEmit` / `tsc -p tsconfig.sandbox.json --noEmit` / `npm run build` 全 0，
  `cargo test --lib` 50 通过 0 失败。界面仍无「默认 4 GiB / 2 GiB」字样（产物字节计数为 0）。

**出包与换上** 新 `854cdafb53e153a7dfc245a450da05f8708b2ed3f1d34588fbf3d8db818246f1`（8 798 208 字节），
已装进 `<服务端>\dfo-launcher.exe`；被它换下的上一轮（服务端内存面板）那枚
`e9f6a5f8…c74926f` 留名为 `dfo-launcher.exe.bak-e9f6a5f8`。同代判据构建前后各跑一次：前一次只列出本轮
10 个文件（含 `tauri.conf.json`），后一次为空。本轮没动 Rust，正面证据换成内嵌资产名——新 exe 内嵌
`index-DV3XqcRT.js`（1 次），换上前的那枚计数 0；`dist/assets/index-DV3XqcRT.js` 里「摆放栏」2 次、
「收起成胶囊」1 次、`4 GiB` / `2 GiB` 各 0 次。（exe 里中文串两侧都是 0：内嵌 dist 被压缩，不是没进包。）
旧那枚的行为结论：`服务端内存` 仍只在设置页，开发者页是四块面板、标题栏没有收起按钮、也就没有右侧摆放栏——
凑合能用，但这一轮的两项改动在它上面都看不见。

**换下又换回（同日 02:28）** 上面那次装上之后，点了一次「更新版本」：自更新把发布在
`RicardoLz/115-launcher` releases/latest 的那枚覆盖进 `<服务端>\dfo-launcher.exe`
（`32c90ed2…4b729685`，8 815 616 字节）。按内嵌前端资产名判它的代次：它是 `index-I6QjgH73.js` /
`index-CVQ4oQBE.css`，且带 `MEMORY_GUARD` + `launcher.local.json` ⇒ 源码是 `09415dd`（上一轮），
**没有**这一轮的胶囊与摆放栏。这条判断已复现而不只是推断：本轮 `git diff 09415dd..HEAD -- src-tauri/src` 为空
（Rust 一行没动，所以差异只可能在 `dist`），而在 detach worktree 里对 `09415dd` 重跑一次 vite 构建，打出的资产名
正是 `index-I6QjgH73.js` / `index-CVQ4oQBE.css`——与那枚 exe 内嵌的逐字相同。（单看 `MEMORY_GUARD` 只能证「≥ 09415dd」，
不能定代次。）重新构建后换成 `72927335e0165f212aa01250474945fc98429496de2451d423cda4528e4bafaa`
（8 798 208 字节，内嵌 `index-DV3XqcRT.js`，与本轮 `npm run build` 打印的资产名一致）；
`32c90ed2` 留名为 `dfo-launcher.exe.bak-32c90ed2`。同代判据这次跑的是「构建前为空」（源码自分代构建后未再改动）
+ 构建后再次为空。哈希与上一次装机的 `854cdafb…8246f1` 不等是 `tauri build` 两次 patch bundle-type 与内嵌
时间戳造成的，不是源码差——代次只认资产名。
（另：先前留的 `dfo-launcher.exe.bak-e9f6a5f8` 这次在整合包目录里已经没有了，不是自更新代码删的——
`self_updater.rs` 只 `remove_file` 它自己下载的临时件——但具体谁删的没证据，记为未知。）

**还没做** 沙盒只走「点胶囊归位」这条路径；拖放落位复用停靠层已有的
拖动管线（`justDragged` 闸门保证拖完不会顺手触发一次归位点击），本轮没有在真机上拖过。

---

## 2026-10-18 · 启动界面重做、背景层与沙盒台（`535fda9`）

- `ControlPanel` 拆成 `LaunchDock` / `ModsPanel` / `ModkitPanel`；「开始游戏」改右下角悬浮开关，账号改胶囊按钮 +
  上弹面板，窗口默认 16:9。
- 主页背景层：壁纸经 asset protocol 流出（`protocol-asset`），并从壁纸取一枚主题色贯穿卡片/开关/侧栏高亮；
  新增原版 / 液态玻璃两套引擎（位移图 + SVG 滤镜 + 光标光泽）与配色方案选择器，偏好都落盘。
- 动效一轮：页面切换过渡、滚轮与拖拽的缓冲滚动、侧栏高亮的抛掷感滑动（曲线随后调柔）、开关同款手感、
  下拉菜单不越界与圆角开合。
- 面板窗口：自绘标题栏、可最大/最小/关闭、拖出改松手才浮出、拖回在停靠层显示落点吸附提示。
- 存档页可备份 / 替换 / 删除（删除走应用内确认弹窗 + Rust 侧窄白名单）；工具页收连发引擎与自动拾取。
- `sandbox/` 独立预览台：mock 掉 Tauri 三个模块、挂真实 `App`，一套状态机场景 + 驱动器 + 用例断言，
  这一轮起成为每次改界面的回归门。
- 连发总开关热键「按一下只翻一次」，冷启动为关；中文输入法下长按掉帧改为界面提示，**不碰玩家输入法**。
- 服务端 mod 导入按内容分流，停用名单一律拒绝；`sk.dat` 全链路改成可选件（缺它是哨兵值，不是删字段）。

## 2026-10-17 · 客户端 mod 管理、连发主状态、自更新与部署向导

- 客户端 mod 管理与连发引擎共用同一份总开关状态（`b623923`）。
- 合入上游 `c1c655e`：服务端自动更新 + 启动器自更新 + 首次部署向导（`9b1fd13`）。
- 版本号回退到 `0.1.0`——主版本号 >255 会让 MSI 打包阶段失败（`9d7d680`）；同时移除每日构建工作流（`a176b3c`）。
- 更新不再启动即弹窗：探到新版只在「检查更新」上点一颗小红点，凭证 `receipt` 只走私有存储，不进 URL / 日志 / 界面。

## 2026-10-15 · Tauri 重写落地

- 从旧的 Go + go-webview2 启动器重写为 Tauri 2 + React 19 + Vite + TS strict：端口闸门、起停服务端与客户端、
  盯日志与收残留、无边框窗口与窗口内控制按钮、明暗双主题。
- 与整合包只留一条契约：`<root>/server/work/dfo-lan/bin/dfolauncher.exe launch|stop --root <root>`；
  启动器不下载、不编译、不替代任何包内二进制，也**从不**改玩家的 `runtime/storage/local.json` 或预建 `.sqlite3`，
  不会自动杀任何名字不认识的进程。
