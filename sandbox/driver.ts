/// 驱动器：把「点哪里、等什么、看到什么」写成一条可以整段跑完的剧本。
/// 后台标签页里 setTimeout 会被掐到 1 秒、requestAnimationFrame 干脆不走，所以这里一律用
/// MessageChannel 让出宏任务：React 的提交发生在宏任务里，只 await 微任务读到的永远是旧 DOM。
import { snapshot, stepAnimations, type Snapshot } from "./probe";
import { scenarios } from "./scenarios";
import { pump, pumpDeploy, pumpFail, st } from "./sim";

export type Step =
  | { do: "click"; sel: string; text?: string }
  | { do: "rail"; label: string }
  | { do: "scenario"; key: string }
  | { do: "mode"; want: string; timeout?: number }
  | { do: "dot"; want: boolean }
  | { do: "pump"; percent: number }
  | { do: "deploy" }
  | { do: "fail"; text: string }
  | { do: "toast"; want: string }
  | { do: "scheme"; want: string }
  | { do: "ms"; value: number }
  /** 往 .page 派一发滚轮（deltaY 为像素），验缓冲滚动有没有接管。 */
  | { do: "wheel"; delta: number }
  /** 合成一次拖拽：按住 dy 像素再松手，松手前那几段速度会被换算成甩出。 */
  | { do: "drag"; dy: number }
  /** 往「服务端内存」卡第 at 格写值（走原生 value setter + input 事件，React 才认这是玩家敲的）。 */
  | { do: "fill"; at: number; value: string }
  /** 断言内存卡渲染出来的东西：三格的值、拦下的错误、某个读数、尺那一档、小曲线点数区间、提醒句包含。 */
  | {
      do: "mem";
      inputs?: string[];
      errs?: string[];
      hasStat?: string;
      track?: string | null;
      points?: [number, number];
      warn?: string;
      timeout?: number;
    }
  | { do: "shot"; label: string };

export interface Case {
  name: string;
  scenario: string;
  kind: "self" | "server";
  remount?: boolean;
  steps: Step[];
}

/// 场景里要重挂 App 才能验「开机那一次探测」，由 main.tsx 把重挂函数交进来。
export const hooks: { remount: () => void } = { remount: () => {} };

function mac(): Promise<void> {
  return new Promise((resolve) => {
    const ch = new MessageChannel();
    ch.port1.onmessage = () => resolve();
    ch.port2.postMessage(0);
  });
}

export async function tick(times = 2): Promise<void> {
  for (let i = 0; i < times; i++) await mac();
  stepAnimations();
}

async function waitUntil(pred: () => boolean, label: string, timeout = 5000): Promise<void> {
  const end = performance.now() + timeout;
  while (!pred()) {
    if (performance.now() > end) throw `等不到 ${label}`;
    await mac();
  }
  stepAnimations();
}

function slim(snap: Snapshot) {
  return {
    text: snap.toggle.label,
    mode: snap.toggle.mode,
    phase: snap.toggle.phase,
    pct: snap.toggle.percent,
    dash: snap.toggle.barDash,
    color: snap.toggle.barColor,
    disabled: snap.toggle.disabled,
    dot: snap.dot,
    check: snap.checkLabel,
    toast: snap.toast ? `${snap.toast.tone ?? "?"}:${snap.toast.text}` : null,
    account: snap.account ? snap.account.label : null,
    page: snap.page,
    // 侧栏这一组：items 是图标数、gap 是高亮胶囊与当前项矩形的左上角差（应为 [0,0]）、
    // entry 是挂在 .page 直接子节点上的入场动画名；scroll/range 用来判缓冲滚动有没有落到位。
    railItems: snap.rail.items,
    railReady: snap.rail.ready,
    railGap: snap.rail.gap,
    // toss/railTravel/railDir 是「滑过去」还是「直接跳过去」的判据；knob* 是开关滑块那一下。
    toss: snap.toss,
    railTravel: snap.railTravel,
    railDir: snap.railDir,
    knobAnim: snap.knobAnim,
    knobAt: snap.knobTranslate,
    switchOn: snap.switchOn,
    entry: snap.entry,
    scroll: snap.scroll,
    range: snap.range,
    bg: snap.background ? `${snap.background.tag} ${snap.background.natural}` : null,
    over: snap.overflowing,
    ell: snap.ellipsised,
    // 主题色这一组：wall 是根节点变量，glow 是代入 var() 后的 --pill-glow 原样文本，
    // ring 是悬停那一圈的扩散宽度，shadow 用来断言混色真落成了 rgba()，body 是窗口底色。
    wall: snap.accent.wall,
    glow: snap.accent.glowRaw,
    ring: [snap.accent.pillRing, snap.accent.toggleRing],
    shadow: snap.accent.pillShadow,
    body: snap.accent.body,
    theme: snap.theme,
    // 配色方案这一组：scheme 是根节点属性，hue 是它带出来的 --hue，派生色全靠这一个数。
    scheme: snap.scheme,
    hue: snap.accent.hue,
    // 内存卡：三格输入值 | 六个读数 | 尺那一档（宽%|档名）| 小曲线点数 | 提醒句。null = 这一页没这张卡。
    mem:
      snap.memory === null
        ? null
        : `${snap.memory.inputs.join("/")} | ${snap.memory.stats.join("/")} | 尺 ${snap.memory.track ?? "不画"} | ${snap.memory.points} 点${snap.memory.warn === "" ? "" : ` | ${snap.memory.warn}`}`,
  };
}

/// 追平用的等待：等不到也不算失败，只是这一帧读起来还不算数。
async function quiet(pred: () => boolean, timeout = 3000): Promise<void> {
  const end = performance.now() + timeout;
  while (!pred() && performance.now() < end) await mac();
  stepAnimations();
}

export type Trace = Record<string, unknown>;

/// 开机探测是异步回来的，background_state 排在最后一条；重挂后界面会先有一帧 background 还是 null。
/// 用「这条命令发出过」当落地图腾——它是请求侧证据，不是被测字段本身，拿断言目标当同步信号会自证。
function bootProbes(cmd: string): number {
  return st.log.filter((line) => line.startsWith(`→ ${cmd}`)).length;
}

export async function play(item: Case): Promise<Trace[]> {
  const out: Trace[] = [];
  const kind = item.kind;
  const probesBefore = bootProbes("background_state");
  scenarios.find((row) => row.key === item.scenario)?.run();
  if (item.remount) hooks.remount();
  await tick(3);
  // 追上「开机那一次探测」：先等命令真发出去，再多让几帧，React 提交发生在宏任务里。
  if (item.remount) {
    await quiet(() => bootProbes("background_state") > probesBefore, 4000);
    await tick(6);
  }
  // 界面得先追上后端相位，否则读到的还是上一条用例留下的状态。
  await quiet(() => snapshot().toggle.mode === "update" || snapshot().toggle.state === st.phase);
  // 背景素材在隐藏标签页里是慢加载的，naturalWidth 要等一次解码才不是 0x0。
  const wantBg = st.background !== null && st.background.exists && st.background.kind !== "none";
  if (wantBg) await quiet(() => (snapshot().background?.natural ?? "0x0") !== "0x0", 4000);
  out.push({ step: "setup", label: item.scenario, ...slim(snapshot()) });

  for (const step of item.steps) {
    try {
      if (step.do === "click") {
        const nodes = Array.from(document.querySelectorAll<HTMLElement>(step.sel));
        const node = step.text === undefined ? nodes[0] : nodes.find((x) => x.textContent?.trim() === step.text);
        if (!node) throw `找不到 ${step.sel}${step.text ? ` 「${step.text}」` : ""}`;
        node.click();
        await tick();
      } else if (step.do === "rail") {
        const node = Array.from(document.querySelectorAll<HTMLElement>(".rail-item")).find(
          (x) => x.getAttribute("aria-label") === step.label,
        );
        if (!node) throw `侧栏没有「${step.label}」`;
        node.click();
        await tick(3);
      } else if (step.do === "scenario") {
        scenarios.find((row) => row.key === step.key)?.run();
        await tick();
      } else if (step.do === "mode") {
        await waitUntil(() => snapshot().toggle.mode === step.want, `开关变成 ${step.want}`, step.timeout ?? 5000);
      } else if (step.do === "dot") {
        await waitUntil(() => snapshot().dot === step.want, `小红点 ${step.want ? "亮起" : "熄灭"}`);
      } else if (step.do === "toast") {
        await waitUntil(() => (snapshot().toast?.text ?? "").includes(step.want), `提示含「${step.want}」`);
      } else if (step.do === "scheme") {
        await waitUntil(() => snapshot().scheme === step.want, `配色方案落到 ${step.want}`);
      } else if (step.do === "pump") {
        pump(kind, step.percent);
        await tick();
      } else if (step.do === "deploy") {
        pumpDeploy(kind);
        await tick();
      } else if (step.do === "fail") {
        pumpFail(step.text);
        await tick(3);
      } else if (step.do === "wheel") {
        const box = document.querySelector<HTMLElement>(".page");
        if (!box) throw "没有 .page";
        box.dispatchEvent(new WheelEvent("wheel", { deltaY: step.delta, bubbles: true, cancelable: true }));
        await tick();
      } else if (step.do === "drag") {
        const box = document.querySelector<HTMLElement>(".page");
        if (!box) throw "没有 .page";
        const rect = box.getBoundingClientRect();
        const x = rect.left + 12;
        const y = rect.top + rect.height - 40;
        const at = (dy: number) => ({
          bubbles: true,
          cancelable: true,
          pointerId: 7,
          pointerType: "mouse",
          isPrimary: true,
          button: 0,
          buttons: 1,
          clientX: x,
          clientY: y + dy,
        });
        // 四段位移：第一段刚过 4px 阈值（判「真在拖」），后三段构成松手时的速度样本
        box.dispatchEvent(new PointerEvent("pointerdown", at(0)));
        for (const dy of [8, Math.round(step.dy * 0.4), Math.round(step.dy * 0.7), step.dy]) {
          box.dispatchEvent(new PointerEvent("pointermove", at(dy)));
          await mac();
        }
        box.dispatchEvent(new PointerEvent("pointerup", at(step.dy)));
        await tick();
      } else if (step.do === "fill") {
        const nodes = Array.from(document.querySelectorAll<HTMLInputElement>(".mem-card .num input"));
        const node = nodes[step.at];
        if (node === undefined) throw `内存卡没有第 ${step.at + 1} 格输入`;
        // 受控组件只能走原生 setter + input 事件：直接写 node.value 会被 React 当作没变。
        const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
        if (setter === undefined) throw "取不到 input 的 value setter";
        setter.call(node, step.value);
        node.dispatchEvent(new Event("input", { bubbles: true }));
        await tick();
      } else if (step.do === "mem") {
        await waitUntil(
          () => {
            const m = snapshot().memory;
            if (m === null) return false;
            if (step.inputs !== undefined && m.inputs.join("|") !== step.inputs.join("|")) return false;
            if (step.errs !== undefined && m.errs.join("|") !== step.errs.join("|")) return false;
            if (step.hasStat !== undefined && !m.stats.some((x) => x.includes(step.hasStat ?? ""))) return false;
            if (step.track !== undefined && m.track !== step.track) return false;
            if (step.points !== undefined && (m.points < step.points[0] || m.points > step.points[1])) return false;
            if (step.warn !== undefined && !m.warn.includes(step.warn)) return false;
            return true;
          },
          `内存卡 ${JSON.stringify(step)}`,
          step.timeout ?? 6000,
        );
      } else if (step.do === "ms") {
        const end = performance.now() + step.value;
        while (performance.now() < end) await mac();
      }
      out.push({ step: step.do, label: "label" in step ? String(step.label) : "", ...slim(snapshot()) });
    } catch (err) {
      out.push({ step: step.do, error: String(err), ...slim(snapshot()) });
    }
  }
  return out;
}

export async function suite(cases: Case[]): Promise<Record<string, Trace[]>> {
  const done: Record<string, Trace[]> = {};
  for (const item of cases) {
    st.log.length = 0;
    done[item.name] = await play(item);
  }
  return done;
}
