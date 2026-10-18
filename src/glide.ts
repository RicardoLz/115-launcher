import { useEffect, type RefObject } from "react";

/* ------------------------------------------------------------- 缓冲滚动
   把 .page 的原生滚动接成「有惯性的一条曲线」：滚轮不再一格一跳，拖拽松手后按速度继续滑。
   做法是自己维护三个量（target 想去哪 / shown 现在写到哪 / vel 甩出去的速度），
   每帧让 shown 以固定比例靠近 target，再把它写进 scrollTop。

   没有做「橡皮筋越界」：scrollTop 天然越不出 [0, max]，要越界只能 transform 内容容器，
   而主页那张更新日志卡是 position:absolute 挂在窗口上的，容器一带 transform 它就会跳锚点。
   所以到头到尾表现为「减速贴边」，不是「拉出回弹」。 */

const EASE = 0.18; // 每帧向 target 靠拢的比例
const SETTLE = 0.4; // 差多少像素以下算到位（顺手停掉 rAF）
const FLICK_DECAY = 0.92; // 甩出后每帧速度衰减
const FLICK_MIN = 0.6; // 低于这个速度就不再甩
const FLICK_CAP = 64; // 单帧最大甩出像素，免得用力一拽飞出半页
const THRESHOLD = 4; // 位移超过几像素才算「真的在拖」，避免点一下就被吞
const SUPPRESS_MS = 300; // 拖完之后多久之内的 click 算这次拖的余波

/// 落在这上面的按下不接管：控件要留给点击，代码块要留给选中文本。
const GUARD = "button, a, input, select, textarea, summary, label, [role='tab'], [data-grab], .dock-bar, code, pre";

/// 从 node 往上找到 .page 之前，有没有一个自己能竖着滚的容器——有就把这次输入整个让给它。
function nestedScrollable(node: EventTarget | null, host: HTMLElement) {
  let cur = node instanceof Element ? node : null;
  while (cur && cur !== host) {
    const style = getComputedStyle(cur);
    const y = style.overflowY;
    if ((y === "auto" || y === "scroll" || y === "overlay") && cur.scrollHeight - cur.clientHeight > 1) {
      return cur;
    }
    cur = cur.parentElement;
  }
  return null;
}

export function useGlideScroll(ref: RefObject<HTMLElement | null>) {
  useEffect(() => {
    const host = ref.current;
    if (!host) return;
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;

    let target = host.scrollTop;
    let shown = target;
    let vel = 0;
    let flicking = false;
    let down = false;
    let dragging = false;
    let moved = 0;
    let startY = 0;
    let startTarget = 0;
    let suppressUntil = 0;
    let raf = 0;
    // 我们上一次写进 scrollTop 的值。scroll 事件是异步派发的，等它到达时同步的 flag 早就复位了，
    // 所以认「这次事件的位置等不等于我们最后写的那个数」，别拿布尔量当证据。
    let lastWrite = target;
    const samples: { t: number; y: number }[] = [];

    const limit = () => Math.max(0, host.scrollHeight - host.clientHeight);
    const clamp = (v: number) => Math.min(limit(), Math.max(0, v));

    // 指针已经不在（松手在外头、被系统取消、脚本合成的事件）时 setPointerCapture 会直接抛，
    // 一句捕获失败不该把整次拖拽打断，所以统一走这里。
    const capture = (id: number, on: boolean) => {
      try {
        if (on) host.setPointerCapture(id);
        else if (host.hasPointerCapture(id)) host.releasePointerCapture(id);
      } catch {
        /* 捕获没成就各走各的：move/up 本来就挂在 window 上 */
      }
    };

    const write = () => {
      host.scrollTop = shown;
      // 浏览器会把自己的位置截成整数（也可能 clamp），按读回值校正，免得两边长期错开
      shown = host.scrollTop;
      lastWrite = shown;
    };

    const step = () => {
      raf = 0;
      if (flicking) {
        const want = target + vel;
        target = clamp(want);
        vel *= FLICK_DECAY;
        // 速度耗尽，或这一帧被边界截住（截住之后继续衰减没意义，位置已经不动了）
        if (Math.abs(vel) < FLICK_MIN || target !== want) flicking = false;
      }
      if (dragging) {
        // 拖拽时位置是跟着指针直接落的（onPointerMove 里写），这里只负责把剩下的惯性清掉
        shown = target;
        write();
        return;
      }
      const gap = target - shown;
      if (Math.abs(gap) < SETTLE) {
        shown = target;
        write();
        return;
      }
      shown += gap * EASE;
      write();
      raf = requestAnimationFrame(step);
    };

    const kick = () => {
      // 页面被挡住时 rAF 根本不回调，直接把位置落到位：看不见动画，但状态仍是对的
      if (document.hidden) {
        flicking = false;
        shown = target;
        write();
        return;
      }
      if (!raf) raf = requestAnimationFrame(step);
    };

    const onWheel = (e: WheelEvent) => {
      if (e.deltaY === 0 || nestedScrollable(e.target, host)) return;
      if (limit() <= 0) return;
      e.preventDefault();
      const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? host.clientHeight : 1;
      flicking = false;
      vel = 0;
      target = clamp(target + e.deltaY * unit);
      kick();
    };

    const onPointerDown = (e: PointerEvent) => {
      suppressUntil = 0;
      if (e.button !== 0 || e.pointerType === "touch" || e.pointerType === "pen") return;
      const hit = e.target instanceof Element ? e.target : null;
      if (!hit || hit.closest(GUARD) || nestedScrollable(hit, host)) return;
      if (limit() <= 0) return;
      down = true;
      dragging = false;
      moved = 0;
      startY = e.clientY;
      startTarget = target;
      flicking = false;
      vel = 0;
      samples.length = 0;
      samples.push({ t: e.timeStamp, y: e.clientY });
    };

    const onPointerMove = (e: PointerEvent) => {
      if (!down) return;
      const dy = startY - e.clientY;
      moved = Math.max(moved, Math.abs(dy));
      if (!dragging) {
        if (moved < THRESHOLD) return;
        dragging = true;
        capture(e.pointerId, true);
        host.dataset.glide = "drag";
        window.getSelection()?.removeAllRanges();
      }
      target = clamp(startTarget + dy);
      samples.push({ t: e.timeStamp, y: e.clientY });
      while (samples.length > 4) samples.shift();
      shown = target;
      write();
    };

    const onPointerUp = (e: PointerEvent) => {
      if (!down) return;
      down = false;
      const real = dragging;
      dragging = false;
      delete host.dataset.glide;
      capture(e.pointerId, false);
      if (!real) return;
      suppressUntil = e.timeStamp + SUPPRESS_MS;
      const a = samples[0];
      const b = samples[samples.length - 1];
      const dt = b.t - a.t;
      if (dt > 0 && dt < 180) {
        // 用最后这一段的速度换算成「每帧像素」，再收个上限
        vel = Math.max(-FLICK_CAP, Math.min(FLICK_CAP, ((a.y - b.y) / dt) * 16.7));
        if (Math.abs(vel) >= FLICK_MIN) {
          flicking = true;
          kick();
        }
      }
    };

    const onClick = (e: MouseEvent) => {
      if (!suppressUntil || e.timeStamp > suppressUntil) {
        suppressUntil = 0;
        return;
      }
      suppressUntil = 0;
      e.stopPropagation();
      e.preventDefault();
    };

    const onScroll = () => {
      // 不是我们写的那一下（键盘、聚焦滚动、内容高度变化）才需要重新对齐
      if (Math.abs(host.scrollTop - lastWrite) <= 1) return;
      const at = host.scrollTop;
      lastWrite = at;
      shown = at;
      if (raf === 0) {
        // 静止时被别的东西挪走 → 目的地也跟着走
        target = clamp(at);
        return;
      }
      // 正在滑行时只接「现在的位置」，目的地留着：异步落数据会让内容高度变一下，
      // 浏览器随之做一次滚动锚定修正，若把 target 也一起改掉，这一格滚轮就被掐断在半路。
    };

    const onResize = () => {
      target = clamp(target);
      kick();
    };

    host.addEventListener("wheel", onWheel, { passive: false });
    host.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", onPointerUp);
    window.addEventListener("pointercancel", onPointerUp);
    window.addEventListener("click", onClick, true);
    host.addEventListener("scroll", onScroll);
    window.addEventListener("resize", onResize);

    return () => {
      if (raf) cancelAnimationFrame(raf);
      host.removeEventListener("wheel", onWheel);
      host.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", onPointerUp);
      window.removeEventListener("pointercancel", onPointerUp);
      window.removeEventListener("click", onClick, true);
      host.removeEventListener("scroll", onScroll);
      window.removeEventListener("resize", onResize);
      delete host.dataset.glide;
    };
  }, [ref]);
}
