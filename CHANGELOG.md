# 更新日志

按日期倒序。写的是「装上这一版之后玩家会看见什么、以及它凭什么算验过」。
整合包（服务端 / 网关 / 客户端）自己的变更不在这儿，在服务端工作区的 CHANGELOG 里。

哈希都是换上进整合包目录 `服务端\dfo-launcher.exe` 的那枚，不是构建中间产物。

---

## 2026-10-23 01:20 · 出包并换上：七步重建与进度条第一次进壳（`63a30819`）

> TL;DR — 上一档那句「要真人看见进度条得先出包换 exe」当场兑现了：`npx tauri build --no-bundle` 出了一枚
> 同代 exe 并按 §4 的次序换上进整合包。**装上之后玩家能多看见的**只有两样：「物品库查询」卡里那条会走的
> 进度条与「第 N/7 步」回执，以及重建完少一条修不了的红字——代码本身和上一档描述的一致，这一档只做部署。
> 换的时候 `tasklist` 查到启动器**没在跑**，所以没做「在跑时改名」那一步；**换完没有运行它**，
> 请自己启动，**重启启动器才生效**。

**换了哪一枚**

| 文件 | 值 |
| --- | --- |
| `服务端\dfo-launcher.exe` | **9 872 896 B，SHA-256 `63a30819109ccf75dff3b0be40d295c4e6a8e181908ad516ef2c0028dc048d3e`**，与构建输出 `src-tauri\target\release\dfo-launcher.exe` 逐字节相同；内嵌 `index-CzfQj-JQ.js` + `index-EHsHqi77.css` |
| 被顶掉的上一代 | 在整合包外留了正式存档 `%LOCALAPPDATA%\DFO115US-Tauri\exe-archive\dfo-launcher-ca59758a-20261022-2314.exe`（9 842 688 B / `ca59758a`，拷过去之后重新全量哈希过、相等），包内另有就近的 `dfo-launcher.exe.bak-ca59758a`。回滚就把存档拷回正式名 |
| 次序（照 §4 抄，没增删步骤） | `tasklist` → `npx tauri build --no-bundle > out\tauri-build-20261023.log 2>&1; echo exit=$?` → 同代际探针（构建前正好打印本轮改的五个文件、构建后为空）→ 读新 exe 内嵌包名 → 旧 exe 拷包外存档并比哈希 → 新 exe 先落同目录 `dfo-launcher.exe.new-63a30819` 并比哈希 → 两次同卷 `mv` → 读回正式名的大小/哈希/包名 |

**凭什么叫它「含这一轮」——六个探针在两枚 exe 上的 6 比 0**（`node -e` 直接读字节；exe 哈希本身不含代际信息，
grep 对中文也不可信）

| 探针 | 新 `63a30819` | 旧 `ca59758a` |
| --- | --- | --- |
| `gm-rebuild-progress`（进度事件名，`logbus.rs:116`） | YES | no |
| `build-equipment`（新子命令 argv） | YES | no |
| `refresh-bag`（新子命令 argv） | YES | no |
| `背包规则 · gmbuild（只换戳）`（`gmrebuild.rs:81`） | YES | no |
| `经验规则 · 人工策略，不重建`（`gmrebuild.rs:83`） | YES | no |
| `这两件现在都由一键重建产出`（校验判据新话术） | YES | no |
| 内嵌包名 | `index-CzfQj-JQ.js` | `index-C5M7IYkC.js` |

同一轮 `npm run build` 打出的 `dist\assets\index-CzfQj-JQ.js` 里也有 `gm-rebuild-progress` 与进度条那句
placeholder，所以「壳里那代界面」和「沙盒走查过那代界面」是同一个包名——**沙盒测过的东西和现在装上的东西同代**。

**这一档没有动的**：`服务端\gm-tool\bin\gmbuild.exe`（00:30 已是 `4a97c0ff…`，`find_exe` 前两个候选
`服务端\gmbuild.exe`、`服务端\bin\gmbuild.exe` 当场确认都不存在，不会被截走）；
`源码\115US当前源码\gm-tool\configs\` 那十一件（`equipment.current37.json` 42 584 797 B、
`inventory.current37.json` 358 B、`experience.compat90.json` 355 B 的时间戳都还是 2026-10-12 05:38）。
构建只往 `src-tauri\target\` 和 `dist\` 写。

**仍未证的只有一条**：真人点一次「一键重建」、看见进度条走到 100（#20）。换完**没有运行过**这枚 exe，
本机合成点击对真 App 无效，这一步替不了。别点界面里的「更新版本」——它会覆盖 `服务端\`，把这枚新 exe 冲成
发布机上更旧的一代（见 GM-交接 §5）。

**人话版**：进度条和「重建也把服务端那两件管上」这套逻辑，昨天写完之后一直只待在源码和沙盒里，
你机屏幕上看到的还是 23:14 那版。刚才重新打了个包、把启动器换掉了，所以下次你打开启动器，
「GM 工具 → 物品库查询」里点「一键重建」就能看到一条一格一格的长条和「第 3/7 步」这样的回执。
换的过程中旧的没删，放在启动器目录旁边的存档文件夹里，名字带着它自己的哈希，想退回去拷回来就行。

**日志里盯这一行**（`%LOCALAPPDATA%\DFO115US-Tauri\logs`）：

```
GM 产物已换新：9 件写入，归档校验 32dd76978ff0…，用时 …s
```

看到「9 件」就说明这枚新包在跑（旧那枚最多报 7 件）；界面里那七行回执每行末尾还带这一步跑了几秒。

---

## 2026-10-23 00:35 · 「一键重建」把服务端那两件导出也接管了，并且第一次有了进度条

> TL;DR — 「一键重建」原来是**五步 / 七件**，跑完仍然会留一条红字「装备目录与背包规则不同源，点一次就补齐」
> ——但那两件根本不在重建清单里，那句话修不了任何东西。这一档把它变成**七步 / 九件**：
> `build-equipment` 直读 PVF 全量重导 `equipment.current37.json`，`refresh-bag` 只把
> `inventory.current37.json` 的代际戳换成当次归档（策略字段逐键不动）；第三件 `experience.compat90.json`
> **刻意不重建**（它整份文件没有来源戳）。同时在「物品库查询」卡顶部加了一条**真**进度条——子进程输出
> 改成逐行流式收，`进度 n/m` 换算成步内百分比，按各步实测秒数摊进总进度。
> **玩家看得见的变化**：重建按钮下方多一条走着的长条和「第 N/7 步 · P% · …」；重建完的红字条数少一条。
> **本轮没出启动器包**——进度条与这两步在源码与沙盒里，装着的 `dfo-launcher.exe` 还是 23:14 那代（见末段）。
> **（这一句是当时的快照：半小时后 01:20 出了包并换上 `63a30819`，见更上面那一档。）**

**先纠一句「那三件」的账**（这是本轮成立的前提，不是顺手加的）

| 件 | 里面有什么 | 有没有「代际」 | 本轮处置 |
| --- | --- | --- | --- |
| `equipment.current37.json` | 67,874 行全量 `.equ` 枚举 + `source.checksum` | **有**，就是那枚 checksum | `build-equipment` 重导 |
| `inventory.current37.json` | 358 B，`model`/`slots`/`missing_stack_limit`/`equipment_slots`/`quick_slots` 全是人工策略，只有 `source` 是 64 位戳 | **只有那 64 个字符** | `refresh-bag` 只换戳 |
| `experience.compat90.json` | 355 B，顶层只有 `reference_sha256`（指向别的东西）、`level_cap 115`、`named_multiplier 3` 与倍率表 | **没有** | **不进重建**，`ARTIFACTS` 里写明「人工策略，不重建」 |

**为什么装备目录必须是全量而不是白名单**（决定实现形状的那一条）

`gmgrant.rs:413` 按 ID 查这张表，查不到就硬报「装备 X 不在装备目录里」。`gmbuild` 里现成的
`ImportEquipmentSelection` 只有 **3,174** 行（那是掉落白名单），导成它就等于把可发的装备面缩掉 **21 倍**。
所以新增的 `gmbuild/src/equipmentexport.rs` 逐字段照 Go `internal/toolcmd/equipmentfull/main.go` 的 `Run()`：
扫全部 `equipment/**.equ`、跳过路径含 `avatar` 的、ID **优先**取自 `list/equipment.lst`（文件名解析只兜底，
早期装备文件名不是纯数字，硬解析会整条丢 43,349 条）、十条标签按字典序落 `Fields`、行键序
`ID`/`Path`/`SHA256`/`Fields`、顶层 `source` 在 `rows` 之前（`load_artifact` 只读头 8 KB，键序是硬要求）、紧凑写出。
**逐字节复现旧文件不可能**——旧那件的戳是 2026-09-11 那次解包的内层归档 `runtime/pvf_source/Script.inner.pvf`，
归档本身早被换掉；能复现的是「同一套枚举规则 + 当代归档」。

**验到哪一步**

| 断言 | 证据 |
| --- | --- |
| 重导的覆盖面不倒退 | `node scripts/compare-equipment-catalog.cjs`（只读）：旧 67 874 行 / 新 **106 254** 行，**旧有新缺 0 个 id**；标签 census 十条全升（如 `[durability]` 46 193→69 840、`[name]` 67 874→106 250），**旧标签在新版里不见了 0 个** |
| 共有 id 的残差可解释 | `node scripts/classify-field-diff.cjs`：67 874 条共有里 42 534 条完全一致、25 338 条**只多出新标签**（`[creation rate]`/`[item category]` 正是比旧白名单多的那两条）、共有标签值漂移 **2** 条、`Path` 差 2 条、`SHA256` 差 2 条——全是 PVF 代际漂移，与既有记忆「残差皆代际漂移」同形 |
| 单跑一次的时间与形状 | 整合包那枚 `gmbuild.exe build-equipment` → **18 s**、106 254 行、66 886 046 B，计数行 `candidate=116391 unreadable=0 bad_name=10134 id_from_index=104115 id_from_filename=2142 dup_id=3` |
| 可复现 | 同一枚 PVF 连跑两次，两份 66 886 046 B 产物**只差 `source.loaded_at` 的 2 个字节**（分钟位；`node` 逐字节 diff 只有偏移 178 与 180 两处），其余逐字节相同 |
| 换戳真的只动戳 | 对现网那件跑 `refresh-bag` 到临时目录：输出与旧文件只差那 64 个字符那一段，`slots`/`missing_stack_limit`/`equipment_slots`/`quick_slots` 逐键相等；实现里改前改后各解析一遍逐键比对，比对不过**整条拒**（`bag_refresh_touches_only_the_stamp` 等 4 条用例）。两次独立运行的换戳输出 `cmp` 逐字节相同 |
| 重建清单自洽 | `build_steps_cover_every_artifact_they_replace`：七步产出九件、每步超时 ≥60 s、权重 >0、需要旧产物目录那第 4 个参数的只有 `refresh-bag` 一步；替换前的同源校验从三件加到**五件**（任一件对不上整套丢弃、旧产物不动） |
| 进度解析 | `progress_lines_become_fractions`：三条真实打印形状（物品库 / 旁表 / 装备目录）各验一条，`进度 5/0`、`进度 5`、结论行与表格行一律 `None`，`进度 900/100` 夹到 1.0。**这条用例当场抓出一个真 bug**——解析器原本先读两段数字再找分母，`n/m` 会读错 |
| 进度不污染日志 | `bus.progress` 只 emit `gm-rebuild-progress`，**不进 2000 格环形、不落盘**——一轮能打出几百行 `进度 n/m`，走 `push` 会把真告警行挤出日志，而进度是瞬时状态、重启后没有回看价值。整数百分比相同的事件丢掉（自节流） |
| 四道门禁 | `cargo test --lib` 启动器 **154 通过 / 0 失败 / 7 忽略**；gmbuild **56 / 0**；`npx tsc --noEmit`、`npm run build`（`index-CzfQj-JQ.js` + `index-EHsHqi77.css`）、`npm run check:sandbox` 全 0 |
| 界面 | 沙盒 `sim.ts` 的 `gm_rebuild` 改成 async，按同一张权重表发假事件；新增走查脚本 `node sandbox/run/rebuild-bar.mjs 9334` **实测**：长条宽度依次 `0 → 11 → 31 → 54 → 65 → 76 → 93 → 100`（单调不回退），说明行七条齐全（`第 6/7 步 · 76% · 第 6/7 步：build-equipment` 这种形状），跑完落下七行步骤回执（`build-equipment · 2.0s`…）与一句「已重建：归档校验 32dd76978ff0…、用时 11.9s、旧产物备份在 …\\.backup-…」，按钮从「重建中…」回到「一键重建」；K01–K04 同时回归全过 |
| 走查这条踩到的两个坑 | ① 无头页默认 `visibilityState=hidden`，沙盒那 40 ms 假节拍会被冻住（第一次跑只走到 93% 且按钮永远「重建中…」）——脚本里补了 `Page.bringToFront` + `Page.setWebLifecycleState:active`，浏览器那侧也带上了 `--disable-background-timer-throttling` 一族；② 第七步的 100% 与「解除忙碌」如果落在同一帧，那一格根本画不出来（采到的是 93%），所以沙盒在七步走完后补了 150 ms——真机那 150 ms 本来就是「校验 → 备份 → 替换 → 重载」，不是假造的 |
| **真人点一次「一键重建」、看见进度条走完** | **未证**（#20；「本轮没出包」这半句已被 01:20 那档顶掉，见更上面） |

**整合包里换了一枚：`gmbuild.exe`**（`D:\115us` 没有 `.git`，这就是一份文件清单）

| 文件 | 值 |
| --- | --- |
| `服务端\gm-tool\bin\gmbuild.exe` | **1 317 376 B，SHA-256 `4a97c0ffd321adb766998b2f533184dcbd6c7280ef961db6214c864f973feb3f`**（= `gmbuild/target/release/gmbuild.exe` 逐字节相同；`find gmbuild/src -name '*.rs' -newer` 探针为空） |
| 为什么必须换 | 上一枚（1 240 064 B / `ecc04c31…`，本档上面那条 23:25 才放的）**不认这两个新子命令**：拿现网 PVF 探它 → `panic: 未知子命令 build-equipment`（`main.rs:171`）。而 `find_exe` 把整合包那枚排在开发树 `target/release` **之前**，不换就等着跑到第六步断 |
| 上一枚留档 | `out/gmbuild-2325-ecc04c31.exe`（仓库内、未入库，可从源码重链复现） |
| 换完实跑 | 从**整合包那枚**跑 `build-equipment` → 18 s / 106 254 行；跑 `refresh-bag` → 「换戳完成（策略字段逐键未动）」。**两份都只写临时目录** |
| **没有动的** | `源码\115US当前源码\gm-tool\configs\` **一个字节都没被本轮改写**（对拍、计时、换戳全部落在仓库的 `out/` 下）。这句里原本并列的那句「`服务端\` 下的 `dfo-launcher.exe` 仍是 23:14 那代 `ca59758a`」在 01:20 作废，见上面那一档 |

**人话版**：以前点「一键重建」，它会把物品库那套五件重烘一遍，然后仍然告诉你「装备目录和背包规则不同源、
发装备会被拒」——而那两件它自己管不着。现在这两件它也管了：装备目录整份重扫 PVF 重导（比旧的多出近四万件，
旧的任何一件都不会在新版里丢失），背包规则那份只把里面那串 64 位版本号换成新的、别的字一个都不改。
第三件「经验规则」压根没有版本号，所以不动它。另外按钮下面多了一条会走的进度条。
**要在界面上看见这条进度条，得先出包换 exe，再重启启动器才生效**（当时还没打进启动器 exe —— 半小时后的
01:20 那档做了，见上面）。

**日志里盯这一行**（`%LOCALAPPDATA%\DFO115US-Tauri\logs`）：

```
GM 产物已换新：9 件写入，归档校验 32dd76978ff0…，用时 …s
```

`9 件`就是这一轮的判据——读出 `7` 说明装的还是旧壳。它前面每一步各有一行
`<子命令> 完成（…s）：…`，其中 `build-equipment` 那行的尾巴是 `写出 106254 行（candidate=…）`。
中途断掉时是 `重建中止在 <cmd>：…` + `旧产物没有动（暂存目录已清掉）。`

---

## 2026-10-22 23:25 · 「一键重建」在玩家机上从「必然失败」变成可用：`gmbuild.exe` 进整合包

> TL;DR — 上一档里写明「`gmbuild.exe` 不在整合包里，所以「一键重建」在玩家机上会整条拒」。这一档补上它：
> 重链一枚**同代际**的离线构建器放进 `D:\115us\服务端\gm-tool\bin\gmbuild.exe`。玩家可见的变化只有一句——
> GM 页那枚「一键重建」按钮不再一上来就报「找不到 gmbuild.exe」。**没动启动器本体，没动七件产物，没跑重建。**

**为什么不是直接拷手上那枚**

手上的 `gmbuild/target/release/gmbuild.exe` 停在 **23:14 之前的 16:54**，而 `gmbuild/src/gmindex.rs` 是 **22:18**
（时装谓词那一轮）。这条 `-newer` 探针不是形式：同一天上午它已经因为「exe 比源码旧」骗过一次判读
（见 GM-交接 §2.6）。所以先 `cargo build --release --manifest-path gmbuild/Cargo.toml`（7.03 s，
`default = ["build"]` 会带上 PVF 内核），探针复跑为空，再谈放哪儿。

| 项 | 值 |
| --- | --- |
| `D:\115us\服务端\gm-tool\bin\gmbuild.exe` | 1 240 064 B，SHA256 `ecc04c31d1646b7899662fdd37e5635f6ca29324196428263d5f2246bd8db05b` |
| `gmbuild/target/release/gmbuild.exe` | **同一串哈希**（逐字节相同，23:24 重链） |
| `find_exe` 命中位 | 第 3 候选；前两个位置 `<root>\gmbuild.exe`、`<root>\bin\gmbuild.exe` 当场 `ls` 确认都不存在，不会被截走 |

**验到哪一步**

| 断言 | 证据 |
| --- | --- |
| PE 加载并真能打开现网 PVF | `gmbuild.exe smoke D:\115us\客户端\DFO\Script.pvf` → 退出 0，`format=dfo_20260901 files=5752281 checksum=32dd76978ff0…`（与 GM-交接 §3.2 量到的同一枚归档、同一校验值），`ui.uv.str` 2 446 492 B、`list/equipment.lst` 4 755 190 B |
| 构建器本体没退化 | `cargo test --lib --manifest-path gmbuild/Cargo.toml` → **49 passed / 0 failed / 0 ignored**，EXIT=0 |
| 同代际 | `find gmbuild/src -name '*.rs' -newer gmbuild/target/release/gmbuild.exe` → 空 |
| 整合包里落到位 | 两次 `certutil -hashfile … SHA256` 相等（上表） |
| **真人点一次「一键重建」** | **未证**。本轮只跑了只读的 `smoke`，没有跑任何 `build-*` 子命令，`gm-tool\configs` 一个字节都没动 |

**两条要记住的**

- **`一键重建` 替换的不是 `服务端` 下的文件**。`find_configs` 在 `<root>/gm-tool/configs` 没命中，命中的是
  `源码\115US当前源码\gm-tool\configs\`，所以七件产物、`.backup-<时戳>` 全在那棵树里；这次新建的
  `服务端\gm-tool\` 只有一个 `bin`，不会改变命中目录。
- **整合包里的这枚副本不会跟着源码走**。以后每次动 `gmbuild/src/**`，重链之后要把新那枚再拷进去一次，
  否则「点得动但烘的是旧逻辑」——这比找不到 exe 更难查。它也不在 git、不在 `tauri build` 的产物里，发布时要单独带上。
  **这条预言在一小时零九分钟后成真**：本档放的 `ecc04c31…` 到 2026-10-23 00:30 已经被上一档那枚
  `4a97c0ff…` 顶掉，理由正是「新子命令只在源码里，整合包那枚不认」。以后读到本档那张哈希表，
  请当它是**当时**的快照，不当它是现状（现状看最新那档）。

---

## 2026-10-22 23:14 · 出包并换上：此前七轮的改动第一次进壳（含耐久 / 属性面板 / 发放记录 / 写前备份 / SP 算满 / 邮件发放）

> TL;DR — `npx tauri build --no-bundle` 出了一枚同代 exe 并换上进整合包目录。**装上之后玩家能看见的**
> 是 GM 页这九张卡的当前形态：装备行改耐久、属性面板中文对照 + 精选/折叠、发放记录（最近 100 笔）、
> 写存档前落一份改前原文、SP 按当前等级算满、**发邮件到邮箱**、以及换 PVF 后的「同源校验」。
> 本轮**只动启动器本体**，没碰服务端、没碰产物目录、没跑起来冒烟。

**换进去的那枚**

| 项 | 值 |
| --- | --- |
| `D:\115us\服务端\dfo-launcher.exe` | 9 842 688 B，SHA256 `ca59758a…`，内嵌 `index-C5M7IYkC.js` + `index-EHsHqi77.css`（与当前 `dist` 同代） |
| 上一代就近档（整合包内） | `dfo-launcher.exe.bak-552e1c7a`，9 683 456 B |
| **正式存档（整合包外）** | `%LOCALAPPDATA%\DFO115US-Tauri\exe-archive\dfo-launcher-552e1c7a-20261021-2014.exe`，换之前逐字节比过哈希相等 |

**次序**（下次照抄）：`tasklist` 确认没在跑 → 构建 → 读新 exe 的内嵌包名确认同代 → **旧那枚整份拷到整合包外并比哈希**
→ 新 exe 先落成同目录 `dfo-launcher.exe.new-<hash8>` 并比哈希 → 两次同卷 `mv` → 读回正式名那枚的大小/哈希/包名。
这样中途任何一步失败，整合包都不会缺一枚启动器。

**验到哪一步**

| # | 结果 |
| --- | --- |
| 换进去的 exe 与当前源码同代（判据 = 内嵌 `assets/index-*.js` 包名，不是 exe 哈希） | 已证（`node -e` 读字节，换代前后各一次） |
| 五条门禁在换之前当场复跑全绿 | 已证（src-tauri 153 / gmbuild 49 / tsc 0 / build 0 / check:sandbox 0） |
| 存档副本与磁盘上被换掉的那枚逐字节相同 | 已证（两份 SHA256 全等，非「大概一样」） |
| **真人点一遍这七档 + 游戏内表现** | **未证**（#20，本机合成点击无效）。这一条现在**只剩它**——「装着的 exe 里还没有这一档」那句话到本轮为止作废 |

**两条要盯着的**

- 本地构建永远显示「有更新」小红点，**别点「更新版本」**：那条分支会覆盖 `D:\115us\服务端`，把刚换上的这枚连同实机确认过的修复一起冲掉。要回滚就用上面两处存档，不要指望整合包里的 `.bak`（2026-10-22 复核时那批 `.bak` 就消失过一次）。
- `gmbuild.exe` **仍然不在整合包里**，所以「一键重建」这枚按钮在玩家机上会整条拒（「同源校验」不受影响）。要发就把它放到 `<root>/gm-tool/bin/`，见交接 §4 末。

---

## 2026-10-22 22:27 · GM 页：邮件发放（落点 `character_mail`，附件 JSON 逐字节对 Go）

> TL;DR — 「物品详情」那张卡多一枚「发放为邮件」和一个正文框：选中的东西不再只能塞进背包，
> 而是往玩家侧那张 `character_mail` 插一行系统邮件（署名固定 `GM`），玩家重新进游戏时在邮箱里看到。
> 附件 JSON 的键序、大小写、期限值全部逐字抄现役服务端的写法，运行时不读 PVF、不叫 Go。
> 本轮**没有出包**。

**先撤一条上一版的说法**

- 交接文档此前建议落 `gm_mail`，**作废**。那张表是旧管理台自己的待发放队列（建表迁移
  `0037_gm_mail.sql` 自己注明「Management dashboard queue; independent of the game's character_mail」），
  现役服务端对它零 SQL 读写——Go 链接期把没人调用的函数连同里面的 SQL 字面量一起剥掉了，玩家永远看不见。
  `character_mail` 才是 CMD96 读取、NOTI99 提醒未读的那张表。

**新增 / 改动**

- `src-tauri/src/gmmail.rs`（252 行，本轮唯一的新文件）：常量、三道容量/条数门、附件取号、
  `build_items()` 出附件 JSON、`send()` 插行与回执。契约来源 `cmd/gmtool/edition_operations.go`
  的 `grantGMItem`（`Mail:true` 那半）与 `GMSystemMail` / `insertSystemMailTx`。
- 新管理动作 `mail.send`：`normalize_manage` 认它（`gmgrant.rs:2023`，白名单并列在 2024-2034），
  两条纯参数门放在**开事务之前**（2048-2055），执行分支在 `run_manage_action` 里先于数值那半走（2311）。
  `ManageInput` 为此多 `gold` / `body` 两格；这里的 `gold` 跟发放那条 `GrantRequest.gold` 是两回事，别混。
- 它不碰 `characters.state`，所以 `writes_character_state` 把它排除（`gmgrant.rs:1374`）：
  不留改前备份文件、回执不带 `state_backup`——上一轮那条「写存档先落一份原文」的规则不该被邮件误触发。
- 界面不新增卡：沿用已有的金币 / 数量 / 选中物品三格（Go 那边也是同一张表单 + `req.Mail` 开关）。
  `mailBlocked`（`GmPage.tsx:844`）把每一条**按下去必然被拒**的组合在按钮层先讲出来，
  发放记录因此多一种动作名「发邮件」。

**这一轮真正花时间的那条：真产物里一件 `kind == "avatar"` 都没有**

- 第一版按 `kind` 判时装，写完拿真产物对拍当场 panic「真产物里一件时装都没有？」。
  `node -e` 数了一下现役 `items.index.json`：676,442 行，`kind` 只有 `stackable`（200,923）和
  `equipment`（475,519）两种值，行里根本没有 `slot` 键。时装是旁表 `equipment.slots.json`
  （481,403 条里 375,887 条长 `[hat avatar]` / `[hair avatar]` 这样）在加载时把 `slot` 覆盖成「时装」那一格认出来的。
- 于是判据收成一条，住在 `gmbuild/src/gmindex.rs:311` 的 `GmItem::avatar()`（`slot` 为主，`kind` 只算兜底），
  `gm.rs` 给每行带出 `avatar` 那一格，Rust 邮件侧、界面侧、沙盒 mock 三处都读它，**没有任何一处复制「slot 是不是时装」这条规则**。
- 这条只有只读对拍才暴露得出来（`live_mail_shapes_on_real_artifacts`，`--ignored` 手工跑，全程只读）：
  真产物 `kind` 标 avatar 的 **0** 行 / 判据认出的时装 **371,468** 行 / 总行数 **676,442**。

**验收**

| # | 结果 |
| --- | --- |
| 附件 JSON 与服务端 reader 逐字节相等：金币无 `item` 键、堆叠的 `Template`/`Amount` 是**大写首字母**（Go 没给 json tag）、时装 `space:1`、期限 `2147483647` | 已证 |
| 一封 = 一行；`sender_id` NULL、`status` 走 DDL 默认 1、`expires_at` 落在整数**微秒**域 | 已证（写 86400000000 当毫秒读会得出 2001 年的行，容量门当场失真） |
| 附件先取号、消息号最后：两者共用 `sqlite_sequence` 里 `character_mail` 那一个号段，`mailbox_id_sequence` 只向前对齐、绝不往后拉 | 已证（本包现网没这张表，那条对齐在本机空转，算术照抄） |
| 门次序与话术照 Go：正文 >512 字节、金币与物品都空、附件 >11、收件箱已满（活消息 ≥255 或未领附件 + 本批 >255）、缺真实模板、超堆叠上限、时装每批 >9 | 已证（兜底堆叠上限取自现网 `inventory.current37.json` 的 `missing_stack_limit` = **1000**，实测而非抄测试夹具） |
| 三种拒绝整笔回滚，且**连流水号都不留**（门在事务之前 ⇒ 同号重试照旧能成） | 已证 |
| 装备实例（要真实例字节那一族）明确拒成「装备实例发放未支持（见 #27）」，不硬凑成堆叠件 | 已证 |
| 时装判据在真产物上可用（`slot` 覆盖那一支） | 已证（只读对拍 + `gmbuild` 新用例点名两种形态各一断言） |
| 真库里真发过一笔 | **未证**（`DFO_REHEARSAL_ROOT` 彩排这轮只跑了只读那半） |
| 玩家在游戏里看见那封邮件、界面按钮真按一次 | **未证**（#20，只能真人；本机合成点击无效，本轮又没出包） |

**门禁** `cargo test --lib`：src-tauri **153** 通过 0 失败（7 忽略），gmbuild **49** 通过 0 失败；
`tsc --noEmit` / `npm run check:sandbox` / `npm run build` 全 0（产物名 `index-C5M7IYkC.js`）。
本轮未出包，所以装着的 `服务端\dfo-launcher.exe`（`552e1c7a`，内嵌 `index-CiokB90Y.js`）里
连耐久 / 属性面板 / 发放记录 / 写前备份 / SP 算满 / 邮件发放这六档都没有——见交接 §4。
（**这句到同日 23:14 那档作废**：那一档出了包并换上，七轮改动现在都在壳内。）

---

## 2026-10-22 19:48 · GM 页：SP「按当前等级重算为满值」（判据是产物里的每级授予表）

> TL;DR — 「技能点 / 技巧点」那张卡多一枚按钮「按 Lv.X 重算为满值」。它写的不是「加 N 点」，
> 也不是旧工具那句 9999，而是**服务端把这个角色升到当前等级本该累计到的总量**：
> 判据全在离线产物 `gm-growth.json` 的 `skill_points`（`etc/sptable.etc` 烘出来的每级授予），
> 求和从 2 级起（1 级不发点，所以 1 级算出来就是 0），两格各写同一份，TP 一个字节都不碰。
> 运行时不读 PVF、不叫 Go。本轮**没有出包**。

**新增 / 改动**

- `gmchar::apply_sp_recalc(state, skill_points, level_cap)`：只写 `skill_points` 这一列，
  回执记 `level` / `levels_summed` / `total` / `before` / `after`，顶层键集合原样保留。
- 新管理动作 `sp.recalculate`：`normalize_manage` 认它，`manage()` 为它载养成产物，
  `run_manage_action` 在数值那半之前先走它。它**不读界面填的金额**，所以 `normalize_manage`
  那条「要写入的数值必须大于 0」不给它设——界面那格留 0 也发得出去。
- 因为它是排除式谓词的默认分支，#40 那条「改前先把 state 落一份原文」自动覆盖这一档（有用例点名）。
- 界面：`GmPage.tsx` 那枚按钮用 `data-tone="ghost"`，标签里直接带当前等级（`按 Lv.115 重算为满值`），
  卡片那段说明写清算式、「已花掉的也一并回到满值」以及越界即拒；发放记录的动作中文名加「SP 按等级算满」。

**这轮最先搞错的一条：旧工具根本没按等级算**

- 旧工具那一档的真实代码是 `gm_dashboard_proxy.py:161`：`if recalc: sp = [9999, 9999]`；
  它自己的 README 第 457 行也写明「一键把两条 SP 写成 9999」——按钮上那句「按当前 Lv.X」**只是标签**，
  等级从头到尾没参与计算。所以「复刻旧工具」和「按当前等级算满」是两件事，本轮由使用方点名选了后者（2026-10-22 定）。
- 求和的两条口径不是编的，都从服务端算法抄：`growth_rules.go:174` 只在**跨过**某一级时查表
  （⇒ 1 级不发点；某一级在表里没有行就整笔拒，Go 原文 `missing crossed-level SP row`），
  `progression_state.go:56-61` 升级时把同一份点数加到两格（⇒ 两格各写）。
- 刻意**不套** `characterCatalog` 那三道身份门（存档身份 / 职业存在 / `source_sha256`）：
  这张每级授予表是全局的 `catalog.Progression.SkillPoints`，服务端算升级奖励时既不看职业也不看来源指纹。
- 旧工具那半还往 state 里塞 `gm_locked_sp` / `gm_locked_tp` 两个键，给它自己的「恢复 GM 加成」按钮用。
  我们不搬：这一列的顶层键集合必须原样，凭空新增服务端不认的键是另一回事。

**这一包的真实数字（核过真产物，`gm-growth.json` sha256 `c7fb3da5…`，与 `gmbuild/out` 同一份）**

- 每级授予表有 200 档，`etc/sptable.etc` 给的是 2 级起每级 30 点（11 级起 60…），1 级那档是 0。
- 满值：Lv.1 = **0**，Lv.70 = **7470**，Lv.115 = **19320**，Lv.150 = **32670**（都远够不着 65535 那把尺）。
- 这一包 `experience.compat90.json` 的 `level_cap` 是 **115**，不是旧工具界面写的 150
  ——所以 Lv.115 是这一档的顶端，比它高的存档直接被「不在成长表范围内」拒，不凑数。

**验收**

| # | 结果 |
| --- | --- |
| 求和只算跨过的级（Lv.3 ⇒ 7、Lv.5 ⇒ 18、Lv.1 ⇒ 0），两格同写，回执五键齐全 | 已证（`gmchar` 三条用例 + `gmgrant` 端到端） |
| 一个字节都不碰别的列：TP / 经验 / 属性 / 装备栏标志 / 背包，且顶层键集合原样、不新增 `gm_locked_*` | 已证 |
| 产物没就绪 ⇒ 拒在写之前，`admin_grants` 零行、存档一字不动 | 已证（`gmgrant` 端到端，`ManageArtifacts::default()` 那条 grow 是 `None`） |
| 表里缺某一级 / 等级越出上限 / 总和落不进 uint16 ⇒ 三种都整笔拒 | 已证（各自点名错误原文） |
| 金额 0 不撞「要写入的数值必须大于 0」那条门 | 已证 |
| 真产物对拍：Lv.1/70/115/150 的满值 = 本轮用 `node` 独立扫同一份 JSON 求出的四个数 | 已证（`live_sp_recalc_on_real_growth`，`--ignored` 手工跑，只读） |
| 界面上真按一次这枚按钮（含按钮 enabled 口径与写后回读） | **未证**（#20：本轮没出包；沙盒的 `gm_manage` 只假了 `quests.*`，`gm_character_detail` / `gm_growth_options` 没有夹具，所以这张卡在沙盒里也点不动） |

**门禁** `cargo test --lib`：src-tauri **146** 通过 0 失败（6 忽略），gmbuild 48 通过 0 失败；
`tsc --noEmit` / `npm run check:sandbox` / `npm run build` 全 0（产物名 `index-BM3BMWbS.js`）。

---

## 2026-10-22 19:30 · GM 页：写存档前先把角色 state 落一份原文（备份失败整笔拒）

> TL;DR — 旧工具那条「写之前把这一行的 state 原样落 `backups/`，落不出去就不写」搬过来了。
> 现在任何会覆盖 `characters.state` 的动作（发放、背包改数/删除、装备耐久、SP/TP/装备栏、等级/转职/觉醒）
> 都在同一个事务里、动手写之前先落一份**那一列的原文**到
> `…\runtime\storage\backup\state\role-<账号>-<角色>-<时戳>.json`，
> 写文件失败 ⇒ `ROLLBACK`，整笔拒，库里连流水号都不留。回执里多一个 `state_backup` 键给落点。
> 运行时不读 PVF、不叫 Go。本轮**没有出包**。

**新增 / 改动**

- `saves::role_state_backup_dir(root)`：落点是**整库备份那个 `backup/` 底下的 `state/` 子目录**，
  不是另起一处。为什么不跟整库档平铺：`list_backups` 按 `.sqlite3` 认「能替换回去的档」，
  一份单角色 JSON 混进那张列表里，看起来就像一份可用的整库备份——那是误导。
- `saves::unique_path` → `unique_backup_path(dir, base, mark, ext)`：同一条「撞名补 `-2`、绝不覆盖」的规则
  现在两边共用（整库档 `.sqlite3`、改前原文 `.json`）。
- `gmgrant::backup_state_before` + `writes_character_state` + `stamp_state_backup`，
  接进 `manage_inner`（`read_role` 之后、`run_manage_action` 之前）与 `grant_inner`（读到角色行之后、`apply_to_character` 之前）。
- `Receipt` 多一枚 `state_backup`（空则不写进 JSON）；管理动作的回执是 JSON 对象，直接盖一个键。
- 界面：发放记录卡底部那段说明加了一句「回执里带 `state_backup` 的那一笔，改前原文就在
  `storage\backup\state\`」——展开那一笔本来就能看到那串路径。

**三条口径，都是这轮自己想错的地方**

- **文件名只用数字**（账号号 + 角色号 + 时戳）。第一版想把流水号也拼进去，但流水号是**用户传进来的字符串**
  （只校验 `launcher-` 前缀与长度 16–100），角色名同理能带 `..`、`:`、`/`。路径由数字拼，
  流水号与名字的对应关系走 `admin_grants`，不走文件名。
- **谓词写成排除式**：只有任务五档与 `materials.set` 明确不碰那一列（`gmquest::apply` 只写 `character_quests`，
  仓库写 `account_material_storage`），其余一律先备份。这样以后新增动作忘了改这里，
  代价是多一份没人用的文件，而不是少一份能救命的。
- **拒在哪一步有讲究**：备份在 `claim_flow` 之后、任何写入之前，而那个 INSERT 还没提交，
  所以失败回滚之后「这笔流水号没被占住」——换个能写的落点同号可以原样重来（有用例钉这一条）。
  旧工具的「失败整笔拒」我们是照做的，不是「备份失败但照样写、只是警告一句」。

**验收**

| # | 结果 |
| --- | --- |
| 管理链路：改前原文逐字节落盘 + 落点进回执 + 文件名形状 | 已证（内存库 + 临时目录用例） |
| 备份写不出来 ⇒ 整笔拒：state 一字节没动、`admin_grants` 零行、同号重试照旧能成 | 已证（同一组用例；落点父目录放一个普通文件让 `create_dir_all` 必失败） |
| 发放链路同样两半（金币入账数与流水行数都断） | 已证 |
| 不碰那一列的动作不留文件、回执不带 `state_backup`；排除式谓词逐档点名 | 已证 |
| 落点公式 = 整合包 `<root>\server\work\dfo-lan\runtime\storage\backup\state`，且那批 JSON 既不进整库备份列表、也够不到删除按钮 | 已证（`saves` 两条用例；删除那边靠 `.sqlite3` 白名单，实测拒） |
| 真机上按一次界面按钮、去 `backup\state\` 看那份文件 | **未证**（#20：本轮没出包，装着的 exe 里还没有耐久/属性/记录/备份这几档；也没重跑 `DFO_REHEARSAL_ROOT` 彩排） |

**门禁** `cargo test --lib`：src-tauri **141** 通过 0 失败（5 忽略），gmbuild 48 通过 0 失败；
`tsc --noEmit` / `npm run check:sandbox` / `npm run build` 全 0（产物名 `index-Bk2oeBTM.js`）。

---

## 2026-10-22 19:20 · GM 页：发放记录卡（最近 100 笔流水，只读）

> TL;DR — GM 页第九张卡：读服务端那张审计表 `admin_grants`，列最近 100 笔（新的在前），
> 默认只铺 时间 / 动作 / 角色 / 流水号，点「看请求」才展开请求与回执两份 JSON。
> 一条新命令 `gm_grant_log`，**只读打开存档**：不领管理租约、不写一个字节，服务端在跑也读得动。
> 运行时不读 PVF、不叫 Go。本轮**没有出包**。

**新增**

- `gmgrant::grant_log` / `grant_log_at` / `gm_grant_log`（命令总数 18 → **19**）：
  `LEFT JOIN characters` 把角色名一起带出来，`ORDER BY created_at DESC, rowid DESC LIMIT 100`。
  表不在这份存档里就**报错**，不给空表——空表和「一笔都没发过」在界面上长得一样，该说的话完全不同。
- `src/components/GmPage.tsx` 的 `records` 卡 + 动作中文对照 `ACTION_LABELS`（17 条，认不出的原样列，不猜）；
  `grantTime()` 把微秒时戳换成 `2026-10-22 17:40`。
- 停靠层：`GmKey` 多一枚 `records`，`DEFAULT_GM_DOCK` 改成「上半两列八块 + 整条底栏给发放记录」。
  叶子顺序不变（`… quests, materials, records`），所以 K04 钉的还是同一串，只是多一枚。

**这张表到底是什么（两条实测纠了第一版的想当然）**

- **`reason` 列不是请求体**。第一版按「reason 存的是整份请求 JSON」写，读真档才发现：
  `request` 才是那份 BLOB 请求体，`reason` 是人话（`启动器发放` / `实机彩排`）。
  管理动作的 `action` 藏在 `request` 的 `reason` **键**里那一层——`manage()` 把整份 `ManageInput`
  序列化后同时写进 `g.reason` 与 `request.reason`，发放请求两处都没有 `action`，于是写 `grant`。
  只往里探一层，不递归。单测夹具原来把同一份 JSON 塞进两列，**读错列也照样绿**，已改成两列给不同内容。
- **`admin_grants` 不是 GM 专用表，是幂等闸门表**：真档 7 笔里 4 笔是服务端自己记的建角点券
  （`grant_id = reward:character_create:newchar.lua:N:cera`、`operator = reward`、没有角色）。
  所以这张卡上「账号级 + 操作者不是 launcher」的那些行是游戏自己的流水，不是漏发的 GM 操作。
- `created_at` 建表默认值就是微秒时戳（`(julianday('now')-2440587.5)*86400000000`），真档 7 笔全是 `integer`；
  `request`/`receipt` 是带 `CHECK(json_valid)` 的 BLOB，取原文用 `CAST(... AS TEXT)`——合法 JSON 不等于对象，
  数组/标量照样解得动，`.get("action")` 为空就退回 `grant`，不因一行顶掉整条命令。
- **条数口径照旧工具（100 条、新的在前），范围不照**：旧工具的 `GrantHistory` 按 `account_id` 查，
  这张卡给的是**整包**最近 100 笔。理由：点券那类是账号级、没有角色，按账号筛会把同一场操作切成两半；
  角色名已经跟着流水一起给，要看谁的时候够用。

**验收**

| # | 结果 |
| --- | --- |
| 只读、不领租约、表缺失时报错而不是空表 | 已证（`grant_log_cuts_at_a_hundred_and_refuses_a_save_without_the_table`） |
| 新的在前 / 100 笔截断 / 账号级 `character_id=NULL → 0` / 角色名 JOIN / 脏 `request` 退回 `grant` | 已证（`grant_log_lists_newest_first_and_labels_the_action`，夹具两列不同值） |
| 同一条 SQL 打在真档复制件上：7 笔、顺序、角色名（魔灵 / test2）、时间换算 | 已证（`node:sqlite` 只读临时副本跑完整 SELECT，读完即删） |
| 界面画得出来：5 笔铺开、中文动作 + 原键并列、「账号级 / 角色 12」两种退路、展开两行 JSON、窄栏无溢出 | 已证（无头浏览器跑沙盒，CDP 读回 `innerText` + 1440×900 / 1100×720 两张图，`overflowing` 为空） |
| K04 九张卡摆放 / 收起 / 归位 / 换页记住 | 已证（`node sandbox/run/cases.mjs K04` 判定通过） |
| 真人打开 GM 页看到这张卡 | **未证**（#20：本轮没出包，装着的 exe 里还没有这一档） |

**门禁** `cargo test --lib`：src-tauri **135** 通过 0 失败（5 忽略，较上轮 +2），gmbuild 48 通过 0 失败；
`tsc --noEmit` / `npm run check:sandbox` / `npm run build` 全 0（产物名 `index-DOwe-8Xs.js`）。

---

## 2026-10-22 18:25 · GM 页：属性面板（中文对照 + 精选/全部折叠 + 未汉化）

> TL;DR — 「角色详情与背包」那张卡原来只写一句「属性 N 项」，键名长什么样就摆什么。
> 现在展开成一张表：默认铺**精选 12 条**（HP/MP 上限、物理/魔法攻击力、物理/魔法防御力、
> 攻击/施放/移动速度、跳跃力、硬直、负重上限），抗性/恢复速度连同认不出的键一起折叠进
> 「展开全部属性」，认不出的原样列出并标**未汉化**。数值是**存档原值，不做换算**。
> 纯读侧，一个新字段都没往存档里写；运行时不读 PVF、不叫 Go。本轮**没有出包**。

**新增**

- `src/attrLabel.ts`：键名 → 中文的对照表（18 条）+ `attributeRows()` 行列器。界面只吃这一个入口。
- 界面：`属性 N 项` / `未汉化 M 项` 两枚徽标、`展开全部属性（含其余 M 项）` 与 `收起` 的开关、
  两列表（属性名 / 存档原值），内部键名放进行悬停提示（`title`），跟旧工具「键只放在鼠标悬停里」同口径。
  换角色时折叠状态复位。精选组一条都没有时不摆空表，改说一句「点展开全部属性看」。
- 说明段把千分位这件事讲开：速度类存档 850 即面板上的 85%、负重 50100 即 50.1kg，
  但**表里仍铺原值**——换算系数只有服务端装备符号表那一套口径，角色面板不是它的读者，不拿来改数。

**口径从哪来（这轮改过一次，值得记）**

- 一开始照的是 `internal/inventory/symbol_display.go` 那 15 条（`[physical attack]` → 力量、速度类
  按 0.1 换算加 `%`）。对着旧工具 README「属性面板」一节复核后**推翻**了：那张表是**装备符号**的显示表，
  角色面板用词是「物理攻击力」一族，而且旧工具明写「数值一律是存档原值，不做换算」。
  分组也照 README 的精选清单重排——抗性与恢复速度在旧工具里本来就是折叠组，不是精选。
- 精选 12 条与 README 列的那一串逐条对上；每条真档实测正好铺出 12 条精选（4 个角色，只读取
  `characters.state` 的 BLOB JSON，读完即删临时副本）。

**边界**

- 未汉化的键不猜：实测每档都出现的 `[weight]`、`[jump speed]` 就留在折叠组里原样摆着。
  唯一一处**已知可能是错的**：`[water resistance]` 写作「水属性抗性」，而旧工具记录客户端原文是
  「火/冰/光/暗属性抗性」，游戏里那一行很可能写作「冰属性抗性」。本机能读的客户端文本投影
  (`gmbuild/out/names.client.json` 只有 `name_*` 物品名) 里火/光/暗各命中一次、水与冰零出现，钉不死；
  这条在折叠组、纯显示，已写进 `attrLabel.ts` 的注释与交接。
- `attributeRows` 只认 `typeof value === "number"` 且有限的值：`null` / `true` / `"abc"` / `NaN` 整条丢掉。
  这里踩过一次——`Number(null)` 是 0，硬换算会把一个空键变成一条看起来真的「MP上限 0」。

**验收**

| # | 结果 |
| --- | --- |
| 精选/折叠/未汉化三档的分法与旧工具 README 一致（12 条精选、抗性归折叠、原值不换算） | 已证（README 逐条对照 + 4 份真档实跑） |
| 真档 4 个角色各自行列：15/17/17/19 键，精选恒 12，未汉化恒 2（`[weight]`、`[jump speed]`） | 已证（只读临时副本 `node:sqlite` 跑 `attributeRows`，输出逐行核过） |
| 非数字值丢掉、空档/数组/`undefined` 返回空表、键序固定 | 已证（同一次实跑） |
| 沙盒里画不出这张卡（无 `gm_character_detail` mock），界面像素与折叠动画 | **未证**（这台机器合成点击无效；`check:sandbox` 只保证类型不破） |
| 真人打开 GM 页看到这张表 | **未证**（#20：本轮没出包，装着的 exe 里还没有这一档） |

**门禁** `cargo test --lib`：src-tauri 133 通过 0 失败（5 忽略，本轮零 Rust 改动），gmbuild 48 通过 0 失败；
`tsc --noEmit` / `npm run check:sandbox` / `npm run build` 全 0（产物名 `index-CLSEyfX1.js`）。

---

## 2026-10-22 18:05 · GM 页：装备行改耐久（`equipment.durability`）

> TL;DR — 「角色详情与背包」那张卡里，装备两张表（装备栏 / 已穿戴）的行多了一枚**改耐久**按钮：
> 名称后面直接显示当前耐久，点开只换 `durability` 这一个字段，精炼、属性、别的行一个字都不动。
> 上限不由界面说了算，只认离线产物 `equipment.current37.json` 里这件装备的 `[durability]`
> （跟着 `[import script]` 继承链取）；产物缺这件、或这件压根没有耐久上限，这一笔**整条拒**。
> 运行时仍然不读 PVF、不叫 Go。本轮**没有出包**。

**新增**

- 读侧：`GmBagRow.durability`（只对 `equipment` / `worn` 两类容器解这一键，`items` 行即使带着也不给）。
  解不出数字（字符串 / null / 超 u16）就当这一行没有耐久——界面不给按钮，写侧照样拒。
  这一键**不并进**那一圈「只认 slot/template/group/amount」的循环：那一圈兼管重复与合法性判定，
  Go 的 `bagEntry` 读耐久却不因为它重复就报错，两边口径一致。
- 写侧第三把乐观锁：`expected_durability`（界面读到的当前耐久），与 `revision`、`template` 并列。
  耐久档**不吃** `expected_amount`：装备行的数量既不是界面看的东西也不是它改的东西，
  硬让界面补一个 1 只会多一个能出错的地方（整行指纹仍由 `revision` 盯着）。
- 门：动作白名单加 `equipment.durability`，请求门要「模板 + 64 位修订号 + 带着当前耐久 + 新值 > 0」，
  少一样在领取流水号之前就拒。当前耐久**可以是 0**（打坏的装备要能修回来），所以只要求「带」不要求「非 0」。
- 上限只从装备目录来：`manage()` 里对这一档现读 `EquipCatalog`，走 `reward(template)`
  （就是发放装备行时算耐久用的那一条，含继承链）。查不到 → `缺少装备目录` / `装备目录里查不到这件装备的耐久上限`，
  不放行成「65535 以内随便填」——那是往存档里写一个游戏里不存在的耐久。
- 回执 `action` 从两值变三值：`inventory.amount` / `inventory.delete` / `equipment.durability`，
  `before`/`after` 仍是整行改前改后，进 `admin_grants` 审计。
- 界面：表格里耐久直接跟在名称后（`· 耐久 48`），按钮与「改数量」并列但互斥出现；
  编辑器按 `edit.mode` 分两套文案，保存后照旧顺手重读详情。
  输入框**不设 max**：上限在服务端产物里，界面不假装知道，超了由 Rust 报「耐久须为 1 至 N」。

**边界**

- 只改这一行的 `durability` 键（按键名大小写不敏感定位，与数量档同一套单键替换），所以
  `refine`、`record`、期限那些原样留着；别的容器、别的行、`state` 里的未知键同样不动。
- 上限取的是**模板声明的耐久**（满耐久值）。若某件装备在游戏里有高于模板的当前耐久，
  往上调会被拒、往下调照旧放行——宁可少给一次修改，也不写没依据的数。
- 老规矩：改完要**重新进游戏**才生效（存档是服务端启动时读进内存的），这一条走的是 `ONLINE_NOTE`。

**验收**

| # | 结果 |
| --- | --- |
| 只改耐久、精炼与别的键留下（含 `items` 行不受影响、未知键不丢） | 已证（`gmchar` 单元用例） |
| 拒绝：`items` 档 / 无耐久键的行 / 当前耐久过期 / 0 / 超上限 / 改了个相同值 / 目录查不到上限 | 已证（逐条断言到话术，含「耐久为 0 的坏装备可以修好」这条正向） |
| 整链路（真事务 + 审计两列 + 两道拒绝门后存档不动 + 幂等回放） | 已证（`gmgrant::manage_durability_round_trip_keeps_refine`） |
| 请求门（缺模板 / 缺修订号 / 缺当前耐久 / 新值 0 都拒在领取之前） | 已证（`manage_input_rules_match_go_preconditions`） |
| 真机点这个按钮、游戏里耐久真的变了 | **未证**（#20：这台机器合成点击无效，只能真人实机；本轮没出包，装着的 exe 里还没有这一档） |

**门禁** `cargo test --lib`：src-tauri **133** 通过 0 失败（5 忽略，较上轮 +3），gmbuild 48 通过 0 失败；
`tsc --noEmit` / `npm run check:sandbox` / `npm run build` 全 0（产物名 `index-DvETBDkk.js`）。
沙盒不模拟 `gm_character_detail`，所以这张卡在沙盒里本来就画不出来——`check:sandbox` 只保证类型不破。

---

## 2026-10-21 23:10 · GM 页：产物同源校验 + 一键重建（换 PVF 不再靠手敲命令）

> TL;DR — 换过 PVF 之后，旧产物不会自己失效：查询照开，发放按旧分类走，养成和任务却被指纹门
> 整条拒。现在「物品库查询」卡顶上多了一行状态与按钮：**产物同源 / PVF 快照**两枚徽标、
> **校验 / 选择 PVF… / 一键重建**。重建走外部 `gmbuild.exe` 子进程，先写暂存、三件校验相等才替换，
> 旧的进备份；启动器运行期**仍然不读 PVF**。

**新增**

- `gm_audit`：扫产物目录里那十件的元数据（大文件只读头/尾，不整份解析），提前把两条判据摊开——
  ① `items.index.json` / `gm-growth.json` / `gm-quests.json` 的归档校验必须相等；
  ② `inventory.current37.json` 的 `source` 必须等于 `equipment.current37.json` 的 `source.checksum`。
  现场 `Script.pvf` 只比**大小与时间**（不做哈希）：大小对不上 / 比产物新，都提示「像换过 PVF」。
- `gm_rebuild`：找 `gmbuild.exe`（`DFO_GMBUILD` > 启动器旁边 > `<root>/gm-tool/bin` >
  `<root>/server/work/dfo-lan/bin` > 开发时 `target/{release,debug}`），依次跑
  `build-names → build-items → build-slots → build-growth → build-quests`，全部落在
  `<configs>/.gmbuild-staging`；三件校验相等才备份并替换七件，然后自动重载物品库。
  中途任何一步失败都**不替换**；替换阶段失败则按备份回滚。
- `gm_pick_pvf`：手挑一份 `Script.pvf`（换包、或客户端目录还没指对时）。
- 界面：问题清单按 `err`（写链路会整条拒）/ `warn`（代际漂移，能用）分色，PVF 大小与时间直接显示。

**边界（别指望点一下就全绿）**

- `inventory.current37.json`、`equipment.current37.json`、`experience.compat90.json` 是服务端旧代际的导出，
  `gmbuild` 不产它们：第 ② 条判据只**报错**，要修得按同一代际重新导出那两件。
- 契约没破：`src-tauri/Cargo.toml` 里 `gmbuild` 仍是 `default-features = false`，PVF 内核没有链进启动器。
- 带外层保护的新 PVF 仍要 `DFO.exe + sk.dat` 配套，否则 gmbuild 自报「客户端资源解密校验失败」。

**验收**

| # | 结果 |
| --- | --- |
| 单元用例（扫描三种 `source` 形状 / 两条判据 / 缺件 / 换包 / 替换与回滚） | 已证（`gmrebuild` 9 条，含「替换中途失败要回滚」） |
| 真产物目录跑一遍 | 已证（`DFO_GM_CONFIGS=… live_audit`：三件 32dd76… 同源，服务端导出 7ef2db… 只报代际 warn，与现场一致） |
| 沙盒 1440×900 / 1100×720 | 已证（`overflowing` 为空；点「校验」「一键重建」回执正常，截图 `scripts-tmp/gm-*.png`） |
| 真 App 里点这个按钮、真跑一次重建 | **未证**（只能真人实机；本轮没有改动你部署的产物目录） |

**门禁** `cargo test --lib`（src-tauri）130 通过 0 失败（5 忽略），`gmbuild` 48 通过 0 失败；
`tsc --noEmit` / `-p tsconfig.sandbox.json --noEmit` / `npm run build` 全 0。本轮**没有出包**。

---

## 2026-10-21 20:14 · GM 页八张卡自由摆放（与开发者面板同一套停靠层）

> TL;DR — GM 页不再是写死的两列：八张卡进了和开发者页同一套停靠层，拖标题栏换位、拖边缘分割、拉分割条
> 调大小，标题栏胶囊把它们收进右侧新加的「摆放栏」。摆法挂在 App 上，换页回来还是你摆的样子。
> 本轮**出了包并换上了** `服务端\dfo-launcher.exe`。

**新增**

- 停靠层抽成通用件 `src/components/DockLayer.tsx`：`src/dock.ts` 的树函数泛型化到任意 key，
  开发者页那五块面板与 GM 页八张卡共用一套「换位 / 分割 / 分割条 / 胶囊 / 幽灵块」。
- GM 页八张卡全部入树：物品库查询、结果、发放目标、物品详情、角色详情与背包、角色数值改写、
  任务与一键完成、账号晶体仓库。右侧新增「摆放栏」，收起的卡排成胶囊，点一下接回末尾，也能直接拖回格子。
- 默认摆法照旧版两列：左列查询 / 结果各半；右列上排发放目标 + 物品详情，下排 2×2。
- 卡片正文改由 `.gm-card-body` 自己滚：标题栏不跟着滚，格子窄了也不再让宽表格顶破卡片。

**一处有意的不一致**

- GM 这几张卡**不给「浮出成独立窗口」**：它们共享一份状态（选中角色、选中物品、背包修订号），
  拆成第二扇窗就得跨窗同步，代价与风险都不划算。所以标题栏不画那枚按钮，拖出主窗只按落点摆回树里。

**验收**

| # | 结果 |
| --- | --- |
| 换位（正中=交换）、沿边分割、拉根分割条 | 已证（沙盒 K04 + `sandbox/run/drag.mjs` 用 CDP 派**受信**鼠标事件：交换、分割均成立，根分割 576/576 → 698/453） |
| 收起成胶囊、点胶囊归位、换页回来摆法还在 | 已证（K04 逐条断言，含「主页 → GM 工具」来回一趟） |
| 开发者页没被改坏 | 已证（K03 全过；`sandbox/run/dev-drag.mjs` 换位成立且五枚浮出按钮都在） |
| 窄栏不横溢出 | 沙盒实测 1440×900 与 1100×720，`overflowing` 都是空 |
| 真机表现 | 还没做（#20）：这台机器合成点击无效，只能由你实机确认 |

**门禁** `cargo test --lib`：src-tauri 121 通过 0 失败（4 忽略），gmbuild 48 通过 0 失败；
`tsc --noEmit` / `-p tsconfig.sandbox.json --noEmit` / `npm run build` 全 0。

**出包** `npx tauri build --no-bundle` → `dfo-launcher.exe`（9 683 456 字节，SHA256 `552e1c7a…`），
内嵌 `assets/index-CiokB90Y.js` 与 `index-Cpb8rC4J.css`。按换 exe 规程留档 `dfo-launcher.exe.bak-72927335`、
`dfo-launcher-verify.exe.bak-96b57d35`；两枚在用 exe 都是这一份构建。

**顺带** 沙盒以前只能人工点，现在 `sandbox/run/` 是一套无头浏览器跑测（用例 / 拖拽 / 分割条 / 截图+溢出），
界面改动以后可复跑，用法见那里的 README。

---

## 2026-10-21 · GM 页：等级/转职/觉醒改写 + 任务与一键完成

> TL;DR — 最后两件离线产物（`gm-growth.json`、`gm-quests.json`）落成了，随之而来的是四条养成改写
> 与整张任务卡：列表、按目标等级预览主线范围、按预览执行。判据一律从产物读，启动器运行时不碰 PVF
> 也不叫 Go；「一键完成」只补完成记录，不发奖励、不伪造通关。

**新增**

- `gmbuild build-growth` 烘职业成长 / 觉醒档 / 经验阈值，`gmbuild build-quests` 烘任务目录（前置组、
  互斥、答案分支、进度模型、等级门槛、`[epic]` 标记与条件/完成/说明三段文案）。缺任一件对应链路整条拒，
  不降级成「看着像真的」。
- 角色数值改写补上 `level` / `advancement` / `awakening` / `profession`（转职分支）四条写链路。这四条
  认存档身份**或**归档校验和，对齐 Go 的 `BindGMIdentity`。
- GM 页多一张「任务与一键完成」卡：三档筛选（全部 / 进行中 / 主线）+ 勾选批量；逐条动作
  `quests.ready`（置为可交付）、`quests.complete`（标记完成）、`quests.abandon`（删完成记录，走应用内
  弹窗确认）；预览式动作 `quests.mainline`（按目标等级算主线范围）、`quests.prerequisites`（追溯某一条
  的前置，目标本身保留）。
- 预览与执行之间是签名锁：执行时库里必须还是预览那一批，角色或任务变了整笔拒
  （「角色或任务已变化，请重新预览任务范围」）；三条逐 id 动作另带 `character_quests` 行集的 revision
  做乐观锁，顺序与 Go 一致（先版本门、后身份门）。
- 完成记录只写 `status='completed', progress=0, progress_model='gm-mainline-skip-v1'`。Go 那句
  RowsAffected 口径（只有 `config_version` 相同且 `status='accepted'` 才计数）由一条直打内存库的用例钉住；
  合法计划本来碰不到 0 行那两种形状，所以「整组撤销」那道总量门是纵深防御，不是主判据。
- 任务这一侧的身份门比养成更严：只认当前存档契约版本（`save_identity()`），不认归档校验和；不匹配就整档
  不给按钮，界面上写「任务目录与角色版本不匹配」。
- 租约拒绝仍停在按钮层：服务端持有 `.admin-guard` 期间三个动作按钮都不可点，不是写进去再报错。

**顺带修的破版**

`.af-live input { width: 84px }` 本来是给数字格定的，套到任务表复选框上把「选」列顶到 103px；前置名那颗
chip 的 `white-space: nowrap` 让一枚 chip 的最小宽度直接顶破卡片（右栏卡片内容宽只有 298px）。现在数字格
宽度只认 `input[type="number"]`，复选框回原生尺寸，任务表里的 chip 与长 id 串允许换行（新增 `.mono-wrap`）；
答案分支那句从 chip 挪成一行可换行的说明。沙盒探针多一项「卡片溢出 Npx <卡名>」——表格最小宽度顶破卡片
这种，逐个量文字永远量不到。

**验收**

| # | 结果 |
| --- | --- |
| 主线闭包、答案分支取最低 id、互斥与前置不满足即挡住 | 已证（`gmquest` 10 条纯算法用例逐条对齐 Go 规则） |
| 五条写动作的入库 / 回执 / 审计 / 幂等（同 id 重放不重复执行） | 已证（`gmgrant` 内存库新增 7 条，含被拒批次整笔回滚） |
| 拒因文案与 Go 一致（1 至 100 个任务、产物未就绪、版本不匹配、预览过期） | 已证（逐条断言原文） |
| 任务链路不动 `characters.state` | 已证（写前后逐字节相等断言） |
| 窄栏里不横溢出 | 沙盒实测：修前「卡片溢出 118px」，修后 0；把两条规则还原成 nowrap/84px 会重新报出 99px（阳性对照） |
| 真机按钮与游戏内表现 | 还没做（#20）：这台机器合成点击无效，真 App 里那几下点不了，只能由你实机确认 |

**门禁** `cargo test --lib`：src-tauri 121 通过 0 失败（4 忽略），gmbuild 48 通过 0 失败；
`tsc --noEmit` / `tsc -p tsconfig.sandbox.json --noEmit` / `npm run build` 全 0。
本轮只改代码与样式，**没出包、没换** `服务端\dfo-launcher.exe`，所以没有哈希可记。

**还没做** 邮件发放（#26）、装备实例发放的强化/增幅/精炼（#27）、物品图标与 PVF 懒补（#32）；
上面那张表的最后一行（#20）在你实机之前不能划掉。

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
