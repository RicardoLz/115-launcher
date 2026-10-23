// 开发者页也走同一套停靠层：验一次换位 + 一次浮出图标仍在（回归 DockLayer 重构）。
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
await send("Page.navigate", { url: "http://localhost:1430/sandbox/index.html?panel=off&page=开发者" });
await sleep(2600);
for (let i = 0; i < 40 && !(await ev("!!window.__sim")); i += 1) await sleep(200);

const dockNow = () => ev("window.__sim.snapshot().dock");
const rectOf = (sel) =>
  ev(`(() => { const el = document.querySelector(${JSON.stringify(sel)});
      if (!el) return null; const r = el.getBoundingClientRect();
      return { x: r.left, y: r.top, w: r.width, h: r.height }; })()`);
const mouse = (type, x, y) =>
  send("Input.dispatchMouseEvent", {
    type, x, y, button: "left", buttons: type === "mouseReleased" ? 0 : 1, clickCount: 1, pointerType: "mouse",
  });

console.log("起始：", JSON.stringify(await dockNow()));
// 浮出按钮还在不在：开发者页每块标题栏都该有一枚 data-tone 无关的 tool-btn（浮出）。
const floatBtns = await ev(
  "document.querySelectorAll('.dock-main .panel-head > .tool-btn:not([data-act])').length",
);
const head = await rectOf('.panel[data-panel="mode"] .panel-head');
const box = await rectOf('.panel[data-panel="log"]');
await mouse("mousePressed", head.x + 60, head.y + head.h / 2);
await mouse("mouseMoved", head.x + 70, head.y + head.h / 2 + 8);
for (let i = 1; i <= 6; i += 1) {
  await mouse("mouseMoved", head.x + 60 + ((box.x + box.w / 2 - head.x - 60) * i) / 6, head.y + head.h / 2 + ((box.y + box.h / 2 - head.y - head.h / 2) * i) / 6);
  await sleep(30);
}
await sleep(80);
await mouse("mouseReleased", box.x + box.w / 2, box.y + box.h / 2);
await sleep(400);
const after = await dockNow();
console.log("换位后：", JSON.stringify(after));
console.log("浮出按钮数：", floatBtns);
const ok = after.leaves[0] === "mode" && floatBtns === 5;
console.log(ok ? "判定：开发者页正常" : "判定：回归");
ws.close();
process.exit(ok ? 0 : 1);
