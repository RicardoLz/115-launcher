# 沙盒跑测小工具

界面改动只能靠沙盒证，但沙盒原来只能人工点。这里几个脚本把「起一个无头浏览器 → 连 CDP → 跑 `window.__sim` 的剧本」串起来，可复跑。

## 三步

```powershell
# 1) 沙盒（只绑 IPv6，访问要用 localhost，不能 127.0.0.1）
npm run sandbox

# 2) 无头浏览器。--remote-allow-origins=* 是必须的：没有它 CDP 的 WebSocket 会被拒（403）
& 'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe' `
  --headless=new --disable-gpu --no-first-run --no-default-browser-check `
  --remote-debugging-port=9334 --remote-allow-origins=* `
  --user-data-dir="$env:TEMP\dfo-sandbox" --window-size=1440,900 `
  "http://localhost:1430/sandbox/index.html?panel=off"

# 3) 跑
node sandbox/run/cases.mjs "K03|K04" 9334      # 用例（正则挑名字）
node sandbox/run/drag.mjs 9334                 # GM 页拖拽换位 + 沿边分割 + 拉分割条
node sandbox/run/divider.mjs 9334              # 只验根分割条（读宽度差）
node sandbox/run/dev-drag.mjs 9334             # 开发者页回归：换位 + 五枚浮出按钮
node sandbox/run/shot.mjs 9334 scripts-tmp/gm  # 1440x900 / 1100x720 两张截图 + 溢出量
node sandbox/run/rebuild-bar.mjs 9334          # 真点一次「一键重建」，采进度条走过的每一格
```

**入口是 `/sandbox/index.html`，不是根路径**。根路径挂的是 `src/main.tsx`（真 App、没有 `__sim`），
打开它再跑会报 `Cannot read properties of undefined (reading 'cases')`。

## 判据

- `cases.mjs` 认 `driver.ts` 的口径：步骤抛错才记 `{ error }`，所以「有 error 字段」= 失败。
- `drag.mjs` / `divider.mjs` / `dev-drag.mjs` 用 `Input.dispatchMouseEvent` 派**受信**鼠标事件，
  不是 JS 合成事件 —— `setPointerCapture` 对合成事件会抛，走 CDP 才和真人操作同一条路。
- `shot.mjs` 顺带读 `snapshot().overflowing`：它为空才说明窄栏没被顶破。
- `rebuild-bar.mjs` 用 `MutationObserver` 而不是 `setInterval` 采样：无头页 `visibilityState=hidden`，
  定时器实测掉到 1 Hz 以下（1.6 s 只取到 8 格，条会停在 93%）。它自己发 `Page.bringToFront` +
  `Page.setWebLifecycleState:{state:"active"}` 把页捞回活动态；判据是宽度单调、`last >= 99`、七行回执、按钮回到「一键重建」。

## 注意

- 无头页里 rAF 会被掐，脚本自己用 `MessageChannel` 让出宏任务；截图前调一次 `stepAnimations()`。
- 这台机器的合成点击对**真 App** 无效（见 GM-交接 §5），但沙盒是浏览器，CDP 鼠标事件有效。
- 跑完记得关掉那个无头浏览器进程；CDP 端口固定 9334，重复起会抢端口。
