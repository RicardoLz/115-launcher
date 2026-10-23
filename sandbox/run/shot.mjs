// 给 GM 页拍照并按两种窗口尺寸量一次溢出：node scripts-tmp/shot-gm.mjs [port] [out前缀]
import { writeFileSync } from "node:fs";
import { setTimeout as sleep } from "node:timers/promises";

const PORT = Number(process.argv[2] ?? 9334);
const prefix = process.argv[3] ?? "scripts-tmp/gm";

async function json(path) {
  const r = await fetch(`http://127.0.0.1:${PORT}${path}`);
  return r.json();
}

const list = await json("/json/list");
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

for (const [w, h] of [
  [1440, 900],
  [1100, 720],
]) {
  await send("Emulation.setDeviceMetricsOverride", {
    width: w,
    height: h,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await send("Page.navigate", { url: "http://localhost:1430/sandbox/index.html?panel=off&page=GM工具" });
  await sleep(2600);
  await ev("window.__sim?.stepAnimations?.()");
  const over = await ev(
    "JSON.stringify({page: window.__sim?.snapshot().page, over: window.__sim?.snapshot().overflowing, dock: window.__sim?.snapshot().dock})",
  );
  console.log(`${w}x${h}`, over);
  const shot = await send("Page.captureScreenshot", { format: "png" });
  const file = `${prefix}-${w}x${h}.png`;
  writeFileSync(file, Buffer.from(shot.data, "base64"));
  console.log("存图", file);
}

ws.close();
