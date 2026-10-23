// 打开沙盒入口（/sandbox/index.html）并跑指定用例。
// 用法：node scripts-tmp/run-cases.mjs "K03|K04" [port]
import { setTimeout as sleep } from "node:timers/promises";

const PORT = Number(process.argv[3] ?? 9334);
const filter = process.argv[2] ? new RegExp(process.argv[2]) : null;
const ENTRY = "http://localhost:1430/sandbox/index.html?panel=off";

async function json(path) {
  const r = await fetch(`http://127.0.0.1:${PORT}${path}`);
  return r.json();
}

const list = await json("/json/list");
const target = list.find((t) => t.type === "page" && t.url.includes("localhost:1430"));
if (!target) {
  console.error("没有 1430 的页面");
  process.exit(1);
}

const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((res, rej) => {
  ws.onopen = res;
  ws.onerror = () => rej(new Error("ws 连接失败（缺 --remote-allow-origins=* ？）"));
});

let id = 0;
const pending = new Map();
const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const n = ++id;
    pending.set(n, { resolve, reject });
    ws.send(JSON.stringify({ id: n, method, params }));
  });
ws.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) {
    const p = pending.get(m.id);
    pending.delete(m.id);
    m.error ? p.reject(new Error(JSON.stringify(m.error))) : p.resolve(m.result);
  }
};

await send("Runtime.enable");
await send("Page.enable");
await send("Page.navigate", { url: ENTRY });
await sleep(2500);

async function ev(expression, awaitPromise = false) {
  const r = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise });
  if (r.exceptionDetails) {
    throw new Error(r.exceptionDetails.exception?.description ?? JSON.stringify(r.exceptionDetails));
  }
  return r.result.value;
}

let ready = false;
for (let i = 0; i < 80; i += 1) {
  if (await ev("!!window.__sim")) {
    ready = true;
    break;
  }
  await sleep(300);
}
if (!ready) {
  console.error("沙盒没起来，__sim 仍缺失");
  process.exit(1);
}

const names = await ev(
  `window.__sim.cases.map(c => c.name).filter(n => ${filter ? `/${filter.source}/.test(n)` : "true"})`,
);
console.log("要跑的用例：", names.join(" | ") || "(没有匹配)");

let bad = 0;
for (const name of names) {
  let trace;
  try {
    trace = await ev(`window.__sim.runCase(${JSON.stringify(name)})`, true);
  } catch (err) {
    bad += 1;
    console.log(`\n=== ${name} === 跑不起来：${String(err.message).slice(0, 400)}`);
    continue;
  }
  const fails = trace.filter((t) => t && typeof t.error === "string");
  console.log(`\n=== ${name} === 步数 ${trace.length}`);
  for (const t of trace) {
    const line = `  [${t.step}] ${t.label || ""} | dock=${t.dock ?? "-"} | over=${JSON.stringify(t.over ?? [])}`;
    console.log(t.error ? `${line}\n      ✗ ${t.error}` : line);
  }
  if (fails.length) {
    bad += 1;
    console.log("判定：失败");
  } else {
    console.log("判定：通过");
  }
}

console.log(`\n总判定：${bad === 0 ? "全部通过" : `${bad} 条失败`}`);
ws.close();
process.exit(bad === 0 ? 0 : 1);
