// 「一键重建」那条进度条的沙盒走查：连 CDP → 进 GM 页 → 装采样探针 → 点按钮 → 读回走过的格子。
// 用法：node sandbox/run/rebuild-bar.mjs [cdpPort]
// 前提：npm run sandbox（1430）+ 无头浏览器带 --remote-allow-origins=*
import { setTimeout as sleep } from "node:timers/promises";

const PORT = Number(process.argv[2] ?? 9334);

const list = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
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
// 无头页默认 visibilityState=hidden，`setTimeout(40)` 会被冻住（沙盒那七步的假节拍就停在第 7 步前）。
// 这三条把页捞回活动态，节拍才是真的。
await send("Page.enable");
await send("Page.bringToFront");
try {
  await send("Page.setWebLifecycleState", { state: "active" });
} catch {
  /* 老版本没有这条，忽略 */
}

async function ev(expression) {
  const r = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  if (r.exceptionDetails) {
    throw new Error(r.exceptionDetails.exception?.description ?? JSON.stringify(r.exceptionDetails));
  }
  return r.result.value;
}

for (let i = 0; i < 80; i += 1) {
  if (await ev("!!window.__sim")) break;
  await sleep(300);
}

// 进 GM 页：侧栏按 aria-label 点（记忆里那条口径）。
await ev(`
  (() => {
    const rail = [...document.querySelectorAll('[aria-label]')].find(e => /GM/.test(e.getAttribute('aria-label')));
    if (!rail) throw new Error('侧栏没有 GM 入口');
    rail.click();
    return rail.getAttribute('aria-label');
  })()
`);
await sleep(1200);

// 探针：无头页会被节流（`setInterval` 掉到 1 Hz 以下，实测 1.6 s 只取到 8 格），
// 所以不轮询——挂 MutationObserver，宽度每变一次记一次，节流也记得全。
await ev(`
  (() => {
    const btn = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === '一键重建');
    if (!btn) throw new Error('页面上没有「一键重建」');
    const card = btn.closest('section');
    const read = () => {
      const i = card.querySelector('.mem-track i');
      if (!i) return null;
      const hint = i.closest('.mem-track').nextElementSibling;
      return { w: i.style.width, tone: i.dataset.tone, hint: hint ? hint.textContent.slice(0, 64) : null };
    };
    window.__bar = { samples: [], visibility: document.visibilityState };
    const push = () => { const s = read(); if (s) window.__bar.samples.push(s); };
    window.__bar.mo = new MutationObserver(push);
    window.__bar.mo.observe(card, { childList: true, subtree: true, attributes: true, attributeFilter: ['style', 'data-tone'] });
    btn.click();
    return 'clicked';
  })()
`);

// 等到按钮回到「一键重建」（回执落地）或超时。
let settled = null;
for (let i = 0; i < 120; i += 1) {
  settled = await ev(`
    (() => {
      const b = [...document.querySelectorAll('button')].map(x => x.textContent.trim());
      return b.includes('一键重建') ? 'done' : (b.find(t => /重建/.test(t)) ?? '?');
    })()
  `);
  if (settled === "done") break;
  await sleep(250);
}
await sleep(300);

const out = await ev(`
  (() => {
    window.__bar.mo.disconnect();
    const s = window.__bar.samples;
    const widths = s.map(x => parseFloat(x.w));
    const nonDecreasing = widths.every((v, i) => i === 0 || v >= widths[i - 1]);
    const hints = [...new Set(s.map(x => x.hint).filter(Boolean))];
    const stepLines = [...document.querySelectorAll('section p.hint')]
      .map(p => p.textContent)
      .filter(t => /^build-|^refresh-bag/.test(t));
    return { visibility: window.__bar.visibility, sampled: s.length,
             first: widths[0], last: widths[widths.length - 1], nonDecreasing,
             distinctWidths: [...new Set(widths)], hints, stepLines,
             note: [...document.querySelectorAll('section p.hint')].map(p => p.textContent).filter(t => /重建/.test(t)) };
  })()
`);
console.log("落地：", settled);
console.log(JSON.stringify(out, null, 2));
const ok =
  out.sampled > 3 &&
  out.nonDecreasing &&
  out.last >= 99 &&
  out.stepLines.length === 7 &&
  settled === "done";
console.log(`\n总判定：${ok ? "进度条走完七步、回执七行" : "不合格"}`);
ws.close();
process.exit(ok ? 0 : 1);
