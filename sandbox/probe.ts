/// 只读探针：把界面上真实渲染出来的东西收成一份可断言的快照。
/// 沙盒里所有「修好了」的结论都得能落到这里的某个字段上，不然就只是看着像。

function pick(selector: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(selector);
}

function num(value: string | null): number | null {
  if (value === null) return null;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : null;
}

export interface Snapshot {
  toggle: {
    exists: boolean;
    label: string;
    mode: string | null;
    state: string | null;
    phase: string | null;
    dim: string | null;
    disabled: boolean | null;
    title: string | null;
    percent: number | null;
    barDash: string | null;
    barColor: string | null;
    width: number;
    height: number;
  };
  account: { label: string; width: number; overflow: boolean } | null;
  dot: boolean;
  checkLabel: string | null;
  toast: { text: string; tone: string | null; lines: number } | null;
  chips: string[];
  footer: string[];
  background: { tag: string; src: string; natural: string } | null;
  page: string;
  theme: string;
  /** 侧栏：条目数、高亮胶囊是否已就位、它与当前项矩形的左上角偏差（px）。 */
  rail: { items: number; ready: boolean; gap: [number, number] };
  /** 换页时抛掷动画的名字与胶囊行程毫秒；空数组 = 动画没挂上（只是位置跳了）。 */
  toss: string[];
  railTravel: number;
  railDir: string | null;
  /** 页面上第一枚开关：滑块 ::after 的动画名与 translate 落点（"none" = 没挂）。 */
  knobAnim: string;
  knobTranslate: string;
  switchOn: string | null;
  /** .page 的滚动位置与可滚范围（px）；range 为 0 说明这一页根本不需要滚。 */
  scroll: number;
  range: number;
  /** 换页入场动画：挂在 .page 直接子节点上的动画名（空数组 = 规则没落上）。 */
  entry: string[];
  /** 根节点上的 data-scheme；空串 = 还没写过，等于默认那套琥珀。 */
  scheme: string;
  /** 开发者页停靠层：树上按渲染顺序挂着的面板、右侧摆放栏里的胶囊、那一栏在不在；null = 这一页没有停靠层。 */
  dock: {
    leaves: string[];
    pills: string[];
    tray: boolean;
  } | null;
  /** 「服务端内存」卡：三格输入值、就地拦下的错误、六个读数、尺宽%、小曲线点数、提醒句、卡内那句回执；null = 卡不在这一页。 */
  memory: {
    inputs: string[];
    errs: string[];
    stats: string[];
    track: string | null;
    points: number;
    warn: string;
    note: string;
    noteTone: string | null;
  } | null;
  animations: { name: string; state: string; time: number | null }[];
  overflowing: string[];
  ellipsised: string[];
  accent: {
    /** 根节点上的 --wall-accent；空串 = 后端没给主题色。 */
    wall: string;
    /** --pill-glow 代入 var() 之后的原样文本（能看出主题色到底混进去没有）。 */
    glowRaw: string;
    /** 悬停那一圈的扩散宽度（px）：靠临时把 --active 拨到 1 量出来，量完还原。 */
    pillRing: number | null;
    toggleRing: number | null;
    /** 根节点算出来的 --hue：配色方案有没有真落到 token 上。 */
    hue: string;
    /** 拨到悬停态后的完整 box-shadow 文本，用来断言混色结果真落成了 rgba(...)。 */
    pillShadow: string;
    /** body 的背景色：亮色主题下整扇窗口跟色的证据。 */
    body: string;
  };
}

/// 最后一层阴影的四个长度是 偏移x 偏移y 模糊 扩散；取倒数第二个就是圈宽。
/// 颜色不拆（rgba 里带逗号，按逗号切会碎），要断言色就整串 includes。
function ringSpread(shadow: string): number | null {
  const lengths = shadow.match(/-?[\d.]+px/g);
  if (lengths === null || lengths.length < 4) return null;
  return Number.parseFloat(lengths[lengths.length - 1]);
}

/// CSS 的 :hover 在脚本里点不出来，但样式只是把 --active 置 1，所以直接临时写这个变量，
/// 量完立刻还原 —— 量的仍是同一条 box-shadow 规则，不是另搭一份。
/// 读之前先挂一枚全局 class 把 transition/animation 关掉：否则 ::before 的 all .3s 过渡
/// 刚建立，getComputedStyle 拿到的是插值起点（圈宽 0px），量出来永远是「没亮」。
const FREEZE = "sandbox-freeze-motion";
let freezeSeeded = false;

function withActive(el: HTMLElement | null, read: (shadow: string) => number | null): { value: number | null; shadow: string } {
  if (el === null) return { value: null, shadow: "" };
  if (!freezeSeeded) {
    const tag = document.createElement("style");
    tag.textContent = `.${FREEZE} *, .${FREEZE} *::before, .${FREEZE} *::after {
  transition: none !important;
  animation: none !important;
}`;
    document.head.append(tag);
    freezeSeeded = true;
  }
  const prev = el.style.getPropertyValue("--active");
  document.documentElement.classList.add(FREEZE);
  el.style.setProperty("--active", "1");
  const shadow = getComputedStyle(el, "::before").boxShadow;
  if (prev === "") el.style.removeProperty("--active");
  else el.style.setProperty("--active", prev);
  document.documentElement.classList.remove(FREEZE);
  return { value: read(shadow), shadow };
}

/// 后台标签页里动画时钟是冻住的（CSSTransition 的 currentTime 恒为 0），
/// 所以截图前必须手动把这一帧推进去，否则看到的永远是过渡起点。
export function stepAnimations(to = 1): number {
  let touched = 0;
  for (const item of document.getAnimations()) {
    const timing = item.effect?.getTiming();
    const duration = typeof timing?.duration === "number" ? timing.duration : null;
    if (item.playState === "running" && duration !== null) {
      item.currentTime = duration * to;
      touched += 1;
    }
  }
  return touched;
}

export function freezeAnimations(): { name: string; state: string; time: number | null }[] {
  return document.getAnimations().slice(0, 12).map((item) => {
    const named = item as Animation & { animationName?: string };
    return {
      name: named.animationName ?? item.constructor.name,
      state: item.playState,
      time: typeof item.currentTime === "number" ? item.currentTime : null,
    };
  });
}

interface TextMetrics {
  own: boolean;
  lines: number;
  width: number;
  inner: number;
  ellipsis: boolean;
}

/// 行数不能按高度除行高算：一段文字里混着不同字号的行内元素（标题 + 版本号）时，
/// 每块各占一个矩形，高度立刻虚高一倍。按矩形 top 归并才是真实折行数。
function measure(node: HTMLElement, range: Range): TextMetrics {
  const cs = getComputedStyle(node);
  const box = node.getBoundingClientRect();
  range.selectNodeContents(node);
  const rects = Array.from(range.getClientRects()).filter((r) => r.width > 0.5 && r.height > 0.5);
  const tallest = rects.reduce((max, r) => Math.max(max, r.height), 0);
  const tops = Array.from(new Set(rects.map((r) => Math.round(r.top)))).sort((a, b) => a - b);
  const lines = tops.reduce((count, top, i) => (i === 0 || top - tops[i - 1] > tallest * 0.4 ? count + 1 : count), 0);
  return {
    own: Array.from(node.childNodes).some((child) => child.nodeType === 3 && (child.textContent ?? "").trim() !== ""),
    lines,
    width: rects.reduce((max, r) => Math.max(max, r.width), 0),
    inner: box.width - Number.parseFloat(cs.paddingLeft) - Number.parseFloat(cs.paddingRight),
    ellipsis: cs.overflow === "hidden" && cs.textOverflow === "ellipsis",
  };
}

export function snapshot(): Snapshot {
  const toggle = pick(".launch-toggle");
  const bar = document.querySelector<SVGRectElement>(".frame__bar");
  const barStyle = bar === null ? null : getComputedStyle(bar);
  const pill = pick(".account-pill");
  const name = pick(".account-pill__name");
  const toast = pick(".toast");
  const memCard = pick(".mem-card");
  const memTrack = memCard?.querySelector<HTMLElement>(".mem-track i") ?? null;
  const memSpark = memCard?.querySelector("polyline") ?? null;
  const memNote = memCard?.querySelector<HTMLElement>(".mem-note") ?? null;
  // 停靠层里挂着谁、摆放栏里躺着谁，只能按渲染顺序读：胶囊就是「不在树上又没浮出去」的那一块，
  // 所以这两条列表合起来必须正好是全部面板，缺一个就是收起/归位走岔了。
  const dock = pick(".dock");
  const bgImg = document.querySelector(".bg-layer img");
  const bgVideo = document.querySelector(".bg-layer video");
  const bgNode = (bgImg ?? bgVideo) as HTMLImageElement | HTMLVideoElement | null;

  // scrollWidth 会把小红点那种故意出界的角标一起算进来，所以这里只量文字自己的宽度，
  // 再单独认出「折了行」和「被压缩成竖条」这两种真破版；省略号截断是设计内的，不算破。
  const overflowing: string[] = [];
  const ellipsised: string[] = [];
  const range = document.createRange();
  for (const node of Array.from(document.querySelectorAll<HTMLElement>(".topbar *, .statusbar span, .account-pill__name, .toggle__label"))) {
    const m = measure(node, range);
    // 只判自己直接带文字的叶子。.brand 这种容器把子元素各摊成一块矩形，按高度数行数全是假阳性。
    if (!m.own) continue;
    const label = `${node.className}:${node.textContent?.trim().slice(0, 18)}`;
    if (m.ellipsis) {
      if (m.width > m.inner + 1) ellipsised.push(label);
      continue;
    }
    if (m.lines > 1) overflowing.push(`折行(${m.lines} 行) ${label}`);
    else if (m.width > m.inner + 1) overflowing.push(`挤掉 ${Math.round(m.width - m.inner)}px ${label}`);
  }

  // 卡片里塞表格出的不是「文字挤掉」，是整张表的最小宽度顶过卡片（GM 页右栏只有 298px 内容宽）：
  // 逐个量文字永远量不到，只能比卡片自己的 scrollWidth / clientWidth。
  for (const card of Array.from(document.querySelectorAll<HTMLElement>("section.card"))) {
    if (card.scrollWidth > card.clientWidth + 1) {
      const head = card.querySelector("h2")?.textContent?.trim() ?? card.className;
      overflowing.push(`卡片溢出 ${card.scrollWidth - card.clientWidth}px ${head}`);
    }
  }

  // --pill-glow 现在定义在 :root（配色方案那套 token 一起上收了），
  // 自定义属性的计算值是 var() 代入之后的文本，这一行能直接看出主题色到底混没混进边框那一圈。
  const glowRaw = getComputedStyle(document.documentElement).getPropertyValue("--pill-glow").trim();
  const hue = getComputedStyle(document.documentElement).getPropertyValue("--hue").trim();
  const pillRing = withActive(pill, ringSpread);
  const toggleRing = withActive(toggle, ringSpread);

  // 高亮胶囊是靠 translate 送过去的，「有没有跟到新图标」只能量两个矩形的左上角差；
  // 非 0 就是没量到（首帧没就绪）或者 React 没重算。
  const current = pick(".rail-item[aria-current='page']");
  const marker = pick(".rail-marker");
  let markerGap: [number, number] = [-999, -999];
  if (current && marker) {
    const a = marker.getBoundingClientRect();
    const b = current.getBoundingClientRect();
    markerGap = [Math.round(a.left - b.left), Math.round(a.top - b.top)];
  }
  // 抛掷那一下挂在胶囊的内层：换页时它必须带一个 rail-toss-* 动画名和方向，
  // 否则只是位置变了，看不见「被扔过去」。读的是计算值而不是 getAnimations()——
  // 隐藏页里动画时钟是停的，实时列表会是空的，但计算值照样能说「规则挂上了」。
  const markerBody = pick(".rail-marker__body");
  const markerStyle = markerBody ? getComputedStyle(markerBody) : null;
  const markerAnim = markerStyle?.animationName && markerStyle.animationName !== "none" ? [markerStyle.animationName] : [];
  // 开关滑块同理：按完那一下 ::after 上必须挂着 knob-toss-*，行程则读 translate 计算值。
  const aSwitch = pick(".switch");
  const knobTrack = aSwitch?.querySelector(".switch-track") ?? null;
  const knobStyle = knobTrack ? getComputedStyle(knobTrack, "::after") : null;
  const stage = pick(".page > *");
  const scroller = pick(".page");

  return {
    toggle: {
      exists: toggle !== null,
      label: toggle?.textContent?.trim() ?? "",
      mode: toggle?.getAttribute("data-mode") ?? null,
      state: toggle?.getAttribute("data-state") ?? null,
      phase: toggle?.getAttribute("data-phase") ?? null,
      dim: toggle?.getAttribute("data-dim") ?? null,
      disabled: toggle ? (toggle as HTMLButtonElement).disabled : null,
      title: toggle?.getAttribute("title") ?? null,
      percent: num(barStyle?.getPropertyValue("--p").trim() ?? null),
      barDash: barStyle?.strokeDasharray ?? null,
      barColor: barStyle?.stroke ?? null,
      width: toggle?.offsetWidth ?? 0,
      height: toggle?.offsetHeight ?? 0,
    },
    account:
      pill && name
        ? { label: name.textContent ?? "", width: pill.offsetWidth, overflow: name.scrollWidth > name.clientWidth + 1 }
        : null,
    dot: pick('.btn[data-dot="on"]') !== null,
    checkLabel: Array.from(document.querySelectorAll(".btn"))
      .find((node) => node.textContent?.includes("更新") || node.textContent?.includes("检查中"))
      ?.textContent?.trim() ?? null,
    toast: toast ? { text: toast.textContent ?? "", tone: toast.getAttribute("data-tone"), lines: measure(toast, range).lines } : null,
    chips: Array.from(document.querySelectorAll(".chip")).map((node) => node.textContent?.trim() ?? ""),
    footer: Array.from(document.querySelectorAll(".statusbar span")).map((node) => node.textContent?.trim() ?? ""),
    background: bgNode
      ? {
          tag: bgNode.tagName.toLowerCase(),
          src: (bgNode as HTMLImageElement).src ?? "",
          natural:
            bgNode instanceof HTMLImageElement
              ? `${bgNode.naturalWidth}x${bgNode.naturalHeight}`
              : `${bgNode.videoWidth}x${bgNode.videoHeight}`,
        }
      : null,
    page: document.querySelector(".rail-item[aria-current='page']")?.getAttribute("aria-label") ?? "",
    theme: document.documentElement.dataset.theme ?? "",
    rail: {
      items: document.querySelectorAll(".rail-item").length,
      ready: marker?.dataset.ready === "on",
      gap: markerGap,
    },
    toss: markerAnim,
    railTravel: Math.round(Number((markerStyle?.animationDuration ?? "0s").replace("s", "")) * 1000),
    railDir: markerBody?.dataset.dir ?? null,
    knobAnim: knobStyle?.animationName ?? "",
    knobTranslate: knobStyle?.translate ?? "",
    switchOn: aSwitch?.dataset.on ?? null,
    scroll: scroller?.scrollTop ?? -1,
    range: scroller === null ? -1 : Math.max(0, scroller.scrollHeight - scroller.clientHeight),
    entry:
      stage === null
        ? []
        : stage
            .getAnimations()
            .map((item) => (item as Animation & { animationName?: string }).animationName ?? item.constructor.name),
    scheme: document.documentElement.dataset.scheme ?? "",
    dock:
      dock === null
        ? null
        : {
            leaves: Array.from(dock.querySelectorAll<HTMLElement>(".dock-main .panel[data-panel]")).map(
              (node) => node.dataset.panel ?? "",
            ),
            pills: Array.from(dock.querySelectorAll<HTMLElement>(".tray-pill")).map(
              (node) => node.dataset.panel ?? "",
            ),
            tray: dock.querySelector(".pill-tray") !== null,
          },
    memory: memCard
      ? {
          inputs: Array.from(memCard.querySelectorAll<HTMLInputElement>(".num input")).map((node) => node.value),
          errs: Array.from(memCard.querySelectorAll<HTMLElement>(".mem-row .hint[data-tone='err']")).map(
            (node) => node.textContent?.trim() ?? "",
          ),
          stats: Array.from(memCard.querySelectorAll<HTMLElement>(".mem-stats dd")).map((node) => node.textContent?.trim() ?? ""),
          track: memTrack ? `${memTrack.style.width ?? ""}|${memTrack.dataset.tone ?? ""}` : null,
          points: (memSpark?.getAttribute("points") ?? "").trim().split(/\s+/).filter((x) => x !== "").length,
          warn: memCard.querySelector<HTMLElement>(".mem-warn")?.textContent?.trim() ?? "",
          note: memNote?.textContent?.trim() ?? "",
          noteTone: memNote?.dataset.tone ?? null,
        }
      : null,
    animations: freezeAnimations(),
    overflowing,
    ellipsised,
    accent: {
      wall: document.documentElement.style.getPropertyValue("--wall-accent"),
      glowRaw,
      pillRing: pillRing.value,
      toggleRing: toggleRing.value,
      hue,
      pillShadow: pillRing.shadow,
      body: getComputedStyle(document.body).backgroundColor,
    },
  };
}
