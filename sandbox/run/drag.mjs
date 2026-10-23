// 合成一次真鼠标拖拽（CDP Input，受信事件），验 GM 页卡片换位与沿边分割。
// 用法：node scripts-tmp/drag-test.mjs [port]
import { setTimeout as sleep } from "node:timers/promises";

const PORT = Number(process.argv[2] ?? 9334);
const list = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
const target = list.find((t) => t.type === "page" && t.url.includes("localhost:1430"));
if (!target) throw new Error("没有 1430 的页面");

const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((res, rej) => {
  ws.onopen = res;
  ws.onerror = () => rej(new Error("ws 连接失败"));
});
let id = 0;
const pending = new Map();
const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const n = ++id;
    pending.set(n, { resolve, reject });
    ws.send(JSON.stringify({ id: n, method, params }));
  });
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) {
    const p = pending.get(m.id);
    pending.delete(m.id);
    m.error ? p.reject(new Error(JSON.stringify(m.error))) : p.resolve(m.result);
  }
};
await send("Runtime.enable");
await send("Page.enable");

const ev = async (expression, awaitPromise = false) => {
  const r = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? "eval fail");
  return r.result.value;
};

await send("Emulation.setDeviceMetricsOverride", {
  width: 1440,
  height: 900,
  deviceScaleFactor: 1,
  mobile: false,
});
await send("Page.navigate", {
  url: "http://localhost:1430/sandbox/index.html?panel=off&page=GM工具",
});
await sleep(2600);
for (let i = 0; i < 40 && !(await ev("!!window.__sim")); i += 1) await sleep(200);

const dockNow = () => ev("window.__sim.snapshot().dock");
const rectOf = (sel) =>
  ev(`(() => { const el = document.querySelector(${JSON.stringify(sel)});
      if (!el) return null; const r = el.getBoundingClientRect();
      return { x: r.left, y: r.top, w: r.width, h: r.height }; })()`);

const mouse = (type, x, y, extra = {}) =>
  send("Input.dispatchMouseEvent", {
    type,
    x,
    y,
    button: "left",
    buttons: type === "mouseReleased" ? 0 : 1,
    clickCount: 1,
    pointerType: "mouse",
    ...extra,
  });

async function dragHeadTo(fromSel, toSel, at = "center") {
  const head = await rectOf(`${fromSel} .panel-head`);
  const box = await rectOf(toSel);
  if (!head || !box) throw new Error(`找不到 ${fromSel} / ${toSel}`);
  const sx = head.x + Math.min(80, head.w / 2);
  const sy = head.y + head.h / 2;
  const tx = at === "left" ? box.x + box.w * 0.12 : box.x + box.w / 2;
  const ty = at === "top" ? box.y + box.h * 0.12 : box.y + box.h / 2;
  await mouse("mousePressed", sx, sy);
  // 先跨过 4px 的抖动门，再分段走过去：每一段都要真发 move，落点才会被重算。
  await mouse("mouseMoved", sx + 8, sy + 8);
  for (let i = 1; i <= 6; i += 1) {
    await mouse("mouseMoved", sx + ((tx - sx) * i) / 6, sy + ((ty - sy) * i) / 6);
    await sleep(30);
  }
  await sleep(80);
  await mouse("mouseReleased", tx, ty);
  await sleep(400);
  await ev("window.__sim.stepAnimations()");
}

console.log("起始：", JSON.stringify(await dockNow()));

// 1) 正中落点＝两块交换：把 materials 拖到 search 身上。
await dragHeadTo('.panel[data-panel="materials"]', '.panel[data-panel="search"]', "center");
const swapped = await dockNow();
console.log("交换后：", JSON.stringify(swapped));

// 2) 沿左边缘＝在这一块左边切一刀：把 quests 拖到 results 左缘。
await dragHeadTo('.panel[data-panel="quests"]', '.panel[data-panel="results"]', "left");
const split = await dockNow();
console.log("分割后：", JSON.stringify(split));

// 3) 拉分割条：根分割是 dir=h 且贯穿整列的那一条（内层还有两条短的 h），
//    拖它再看根分割左右两格的宽度有没有真的变。
const rootBars = await ev(`(() => {
  const host = document.querySelector('.dock-main');
  return Array.from(host.querySelectorAll('.dock-bar')).map((b) => {
    const r = b.getBoundingClientRect();
    return { dir: b.dataset.dir, x: r.left, y: r.top, w: r.width, h: r.height };
  });
})()`);
const root = rootBars.filter((b) => b.dir === "h").sort((a, b) => b.h - a.h)[0];
const cells = () =>
  ev(
    "Array.from(document.querySelector('.dock-main').querySelectorAll(':scope > .dock-split > .dock-cell')).map((c) => Math.round(c.getBoundingClientRect().width))",
  );
const before = await cells();
if (root) {
  const bx = root.x + root.w / 2;
  const by = root.y + Math.min(root.h / 2, 200);
  await mouse("mousePressed", bx, by);
  for (let i = 1; i <= 5; i += 1) {
    await mouse("mouseMoved", bx + i * 25, by);
    await sleep(30);
  }
  await mouse("mouseReleased", bx + 125, by);
  await sleep(300);
}
const after = await cells();
console.log("分割条拖动：", JSON.stringify({ before, after }));

const ok =
  swapped.leaves.join(",") !== "search,results,target,detail,bag,stats,quests,materials" &&
  split.tray === true &&
  before.join() !== after.join();
console.log(ok ? "判定：拖拽与分割条都生效" : "判定：拖拽或分割条没生效");
ws.close();
process.exit(ok ? 0 : 1);
