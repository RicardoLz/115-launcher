// 只验「拉分割条」：找到根分割（.dock-main 下第一个 .dock-split 的 bar），拖它并读左右宽度。
import { setTimeout as sleep } from "node:timers/promises";

const PORT = Number(process.argv[2] ?? 9334);
const list = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
const target = list.find((t) => t.type === "page" && t.url.includes("localhost:1430"));
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

await send("Emulation.setDeviceMetricsOverride", { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
await send("Page.navigate", { url: "http://localhost:1430/sandbox/index.html?panel=off&page=GM工具" });
await sleep(2600);
for (let i = 0; i < 40 && !(await ev("!!window.__sim")); i += 1) await sleep(200);

const bars = await ev(`(() => {
  const host = document.querySelector('.dock-main');
  const bars = Array.from(host.querySelectorAll('.dock-bar')).map((b) => {
    const r = b.getBoundingClientRect();
    return { dir: b.dataset.dir, x: r.left, y: r.top, w: r.width, h: r.height };
  });
  const cells = Array.from(host.querySelectorAll(':scope > .dock-split > .dock-cell')).map((c) => {
    const r = c.getBoundingClientRect();
    return Math.round(r.width);
  });
  return { bars, cells };
})()`);
console.log("分割条：", JSON.stringify(bars.bars));
console.log("根分割左右宽：", JSON.stringify(bars.cells));

// 根分割是 dir=h 且贯穿整列的那条（内层还有两条短的 h）。
const first = bars.bars.filter((b) => b.dir === "h").sort((a, b) => b.h - a.h)[0];
if (!first) throw new Error("没有分割条");
const bx = first.x + first.w / 2;
const by = first.y + Math.min(first.h / 2, 200);

const mouse = (type, x, y) =>
  send("Input.dispatchMouseEvent", {
    type, x, y, button: "left", buttons: type === "mouseReleased" ? 0 : 1, clickCount: 1, pointerType: "mouse",
  });

await mouse("mousePressed", bx, by);
for (let i = 1; i <= 8; i += 1) {
  await mouse("mouseMoved", bx + i * 25, by);
  await sleep(40);
}
await mouse("mouseReleased", bx + 200, by);
await sleep(400);

const after = await ev(`(() => {
  const host = document.querySelector('.dock-main');
  return Array.from(host.querySelectorAll(':scope > .dock-split > .dock-cell')).map((c) => Math.round(c.getBoundingClientRect().width));
})()`);
console.log("拖动后左右宽：", JSON.stringify(after));
const ok = bars.cells[0] !== after[0];
console.log(ok ? "判定：分割条生效" : "判定：分割条没动");
ws.close();
process.exit(ok ? 0 : 1);
