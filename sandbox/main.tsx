import { createRoot, type Root } from "react-dom/client";
import App from "../src/App";
import "../src/styles.css";
import Panel from "./panel";
import { scenarios } from "./scenarios";
import { snapshot, stepAnimations } from "./probe";
import { cancelUpdate, dispatch, emit, pump, pumpDeploy, pumpFail, reset, st } from "./sim";
import { hooks, play, suite, type Case, type Trace } from "./driver";
import { cases } from "./cases";

// 沙盒素材：模拟路径 → 浏览器真能加载的 URL。映射不到的走 convertFileSrc 的假地址，
// 正好复现真机上「asset 通道没放行」那条报错文案。
(window as unknown as { __assets: Record<string, string> }).__assets = {
  "C:\\sandbox\\wallpaper.png": "/sandbox/fixtures/bg.svg",
  "C:\\sandbox\\wallpaper-bright.png": "/sandbox/fixtures/bg-bright.svg",
  "C:\\sandbox\\wallpaper.mp4": "/sandbox/fixtures/bg.mp4",
};

interface SimApi {
  state: typeof st;
  scenarios: typeof scenarios;
  dispatch: typeof dispatch;
  emit: typeof emit;
  reset: () => void;
  cancelUpdate: () => void;
  snapshot: () => ReturnType<typeof snapshot>;
  stepAnimations: typeof stepAnimations;
  pump: typeof pump;
  pumpDeploy: typeof pumpDeploy;
  pumpFail: typeof pumpFail;
  run: (key: string) => void;
  play: (item: Case) => Promise<Trace[]>;
  suite: (list: Case[]) => Promise<Record<string, Trace[]>>;
  cases: Case[];
  runCase: (name: string) => Promise<Trace[]>;
  remount: () => void;
}

declare global {
  interface Window {
    __sim?: SimApi;
  }
}

let generation = 0;
let root: Root | null = null;

function render(): void {
  root?.render(<App key={generation} />);
}

const api: SimApi = {
  state: st,
  scenarios,
  dispatch,
  emit,
  reset,
  cancelUpdate,
  snapshot,
  stepAnimations,
  pump,
  pumpDeploy,
  pumpFail,
  run: (key) => scenarios.find((item) => item.key === key)?.run(),
  // 重挂一次 App：启动那批命令会重新走，正好验「开机只亮红点、不弹窗」。
  remount: () => {
    generation += 1;
    render();
  },
  play,
  suite,
  cases,
  runCase: (name) => {
    const item = cases.find((row) => row.name === name);
    if (!item) throw new Error("没有用例 " + name);
    return play(item);
  },
};
window.__sim = api;
hooks.remount = api.remount;

root = createRoot(document.getElementById("root") as HTMLElement);

// 无头渲染（edge --headless --screenshot）注入不了脚本，所以这里认几个查询参数把偏好预置进
// localStorage：?theme=light & ?scheme=violet & ?glass=on & ?bg=bg.image。
// 只有沙盒认它们；真机的偏好仍然只走 localStorage 与设置页。
const params = new URLSearchParams(window.location.search);
const seeds: [string, string | null][] = [
  ["launcher.theme", params.get("theme")],
  ["launcher.scheme", params.get("scheme")],
  ["launcher.glass", params.get("glass")],
];
for (const [key, value] of seeds) {
  if (value) window.localStorage.setItem(key, value);
}
const bgKey = params.get("bg");

render();
if (bgKey) scenarios.find((item) => item.key === bgKey)?.run();

// 无头截图点不到侧边栏，所以再认两个导航参数：?page=设置 直接切页，?focus=1 把第一枚开关聚焦
// （上报范围那层浮层是 hover/focus 才出的，聚焦等于替鼠标按一下）。真机不读这两个参数。
const pageKey = params.get("page");
if (pageKey || params.get("focus")) {
  setTimeout(() => {
    if (pageKey) {
      document
        .querySelector<HTMLButtonElement>(`.rail-nav button[aria-label="${pageKey}"]`)
        ?.click();
    }
    setTimeout(() => {
      if (params.get("focus")) document.querySelector<HTMLButtonElement>('.field[data-pop="true"] .switch')?.focus();
    }, 60);
  }, 60);
}
// ?glide=240 推一发滚轮进 .page，并把 scrollTop 逐帧抄进画面左上角。
// 隐藏页与无头页里 rAF 干脆不回调（glide 自己也据此走「直接落位」那条），所以曲线只有真窗口铺得开；
// 这两种页面里改用宏任务链抄端点，至少能看出「这一页确实被接管了」而不是「探针没跑」。
const glideBy = params.get("glide");
if (glideBy) {
  const next = (run: () => void) => {
    const ch = new MessageChannel();
    ch.port1.onmessage = run;
    ch.port2.postMessage(0);
  };
  setTimeout(() => {
    const box = document.querySelector<HTMLElement>(".page");
    if (!box) return;
    const tag = document.createElement("div");
    tag.style.cssText = "position:fixed;left:78px;top:6px;z-index:99;font:11px monospace;color:#7fe3ff";
    document.body.append(tag);
    const trace: string[] = [];
    const paint = () => {
      tag.textContent = `hidden=${document.hidden} glide ${glideBy}: ${trace.join(" ")}`;
    };
    const sample = () => {
      trace.push(String(Math.round(box.scrollTop)));
      paint();
      if (trace.length < 40) requestAnimationFrame(sample);
    };
    const churn = () => {
      if (document.hidden === false || trace.length >= 12) return;
      trace.push(`<${Math.round(box.scrollTop)}>`);
      paint();
      next(churn);
    };
    // 先等这一页「真的能滚」：无头的虚拟时间跑得快，那一刻设置页可能还没铺开，
    // 而 glide 对不需要滚的页面本就该放手，所以拿 range>0 当发事件的门，别把「没接管」读成「没反应」。
    const fire = (tries: number) => {
      if (box.scrollHeight - box.clientHeight <= 0 && tries > 0) {
        next(() => fire(tries - 1));
        return;
      }
      trace.push(`range=${box.scrollHeight - box.clientHeight}`);
      box.dispatchEvent(new WheelEvent("wheel", { deltaY: Number(glideBy), bubbles: true, cancelable: true }));
      requestAnimationFrame(sample);
      paint();
      churn();
    };
    fire(600);
  }, 120);
}

// ?panel=off 只留真界面，右侧这块调试面板不挂——无头截图要的是干净画面。
if (params.get("panel") !== "off") {
  createRoot(document.getElementById("sandbox-panel-host") as HTMLElement).render(<Panel />);
}

// ?hover=1 把「光标」放到第一张卡片右外沿 30px 处，再把玻璃本身的状态念在画面左上角：
// 无头截图没有鼠标，也读不到合成结果，只能把计算样式逐条打出来验。真机不读这个参数。
if (params.get("hover")) {
  setTimeout(() => {
    const card = document.querySelector<HTMLElement>(".card");
    if (!card) return;
    const rect = card.getBoundingClientRect();
    window.dispatchEvent(
      new PointerEvent("pointermove", { clientX: rect.right + 30, clientY: rect.top + 20 }),
    );
    setTimeout(() => {
      const root = getComputedStyle(document.documentElement);
      const at = (name: string) => (root.getPropertyValue(name).trim() || "-");
      const of = (el: Element | null, part?: string) =>
        el ? getComputedStyle(el, part) : getComputedStyle(document.body);
      const sheet = of(card, "::before");
      const rim = of(card, "::after");
      const btn = document.querySelector<HTMLElement>(".btn");
      const btnSelf = of(btn);
      const btnRim = of(btn, "::before");
      const box = document.createElement("pre");
      box.style.cssText =
        "position:fixed;left:8px;top:8px;z-index:9999;margin:0;padding:6px 8px;background:#000;color:#0f0;font:12px/1.5 monospace";
      box.textContent =
        `sheen ${at("--glass-mx")} ${at("--glass-my")} page ${document.querySelector(".app")?.getAttribute("data-page") ?? "-"}\n` +
        `card veil ${sheet.backgroundColor} filter ${sheet.backdropFilter} overflow ${of(card).overflow} transform ${of(card).transform}\n` +
        `card rim ${rim.backgroundImage.slice(0, 46)} blend ${rim.mixBlendMode}\n` +
        `btn bg ${btnSelf.backgroundColor} filter ${btnSelf.backdropFilter} radius ${btnSelf.borderRadius}\n` +
        `btn rim opacity ${btnRim.opacity} image ${btnRim.backgroundImage.slice(0, 40)}\n` +
        `swatch ${of(document.querySelector(".scheme")).borderRadius} plate ${of(document.querySelector(".scheme-plate")).borderRadius}`;
      document.body.appendChild(box);
    }, 200);
  }, 200);
}
