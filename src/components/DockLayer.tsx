import {
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  type RefObject,
  type SetStateAction,
} from "react";
import { leaves, place, retune, strip, zoneAt, type Dir, type DockNode, type Zone } from "../dock";
import { PanelChrome } from "./PanelFrame";

const MIN_RATIO = 0.15;
const MAX_RATIO = 0.85;
/// 手抖到这么多像素才算真在拖：不然点标题栏上的按钮会先把面板拎起来。
const DRAG_SLOP = 4;
/// 幽灵块贴边时留的间距：拖出主窗也要看得见手里这块是什么。
const GHOST_PAD = 12;

type Drag<K extends string> = {
  k: K;
  sx: number;
  sy: number;
  x: number;
  y: number;
  moved: boolean;
  /// 指针已经越出主窗：这一步松手是开窗，不是换位。
  outside: boolean;
};

/// 通用停靠层：一棵二分树 + 右侧摆放栏。开发者页那五块面板与 GM 页那八张卡共用。
/// 区别只在 `onDetach`：没有这枚回调的页（GM 那八张卡彼此共享状态，拆不出第二扇窗）
/// 标题栏不画「浮出」按钮，拖出主窗也只按落点摆回树里，不开窗。
export default function DockLayer<K extends string>({
  keys,
  node,
  setNode,
  titles,
  renderBody,
  detached = [],
  onDetach,
  onReturn,
  hover = null,
  hitRef,
  emptyTitle,
  emptyText,
  pillHint = "拖回这一页落位，点一下接在末尾",
  trayHint = "面板收起来排在这里",
}: {
  keys: K[];
  node: DockNode<K> | null;
  setNode: Dispatch<SetStateAction<DockNode<K> | null>>;
  titles: Record<K, string>;
  /// 叶子正文：外壳、抓手、收起与浮出都在这层，内容各页自己给（各页自己包 PanelFrame）。
  renderBody: (k: K) => ReactNode;
  /// 已经浮成独立窗口的那些：它们既不在树上，也不该在摆放栏里排着。
  detached?: K[];
  onDetach?: (k: K) => void;
  /// 空树时那几枚「归位」按钮的走法：独立窗口要通知 Rust 侧藏窗，跟单纯 expand 不是一回事。
  onReturn?: (k: K) => void;
  /// 独立窗口正被拖回主窗：按这个落点画窗口内的吸附提示。
  hover?: { panel: K; x: number; y: number } | null;
  /// 停靠层把命中测试借出来：独立窗口拖回来时按落点挑吸附槽。
  hitRef?: RefObject<((x: number, y: number) => Zone<K> | null) | null>;
  emptyTitle?: string;
  emptyText?: ReactNode;
  /// 胶囊的悬停提示：开发者页能浮出成窗口、GM 页不能，同一句话在两页说法不一样。
  pillHint?: string;
  trayHint?: string;
}) {
  const [drag, setDrag] = useState<Drag<K> | null>(null);
  const [zone, setZone] = useState<Zone<K> | null>(null);
  const leafEls = useRef(new Map<K, HTMLElement>());
  const dragRef = useRef<Drag<K> | null>(null);
  const zoneRef = useRef<Zone<K> | null>(null);
  /// 拖完那一下浏览器还会补一次 click：这一枚标记用来把它吞掉。
  const justDragged = useRef(false);
  const actions = useRef({ setNode, onDetach });
  actions.current = { setNode, onDetach };

  const rects = () => {
    const out: Array<{ k: K; rect: DOMRect }> = [];
    leafEls.current.forEach((el, k) => {
      if (el.isConnected) out.push({ k, rect: el.getBoundingClientRect() });
    });
    return out;
  };

  useEffect(() => {
    if (!hitRef) return;
    hitRef.current = (x, y) => zoneAt(rects(), x, y);
    return () => {
      hitRef.current = null;
    };
    // rects 只读 ref，不需要依赖。
  }, [hitRef]);

  const drop = (k: K, where: Zone<K> | null) => {
    actions.current.setNode((prev) => place(prev, k, where));
  };

  const detach = (k: K) => {
    actions.current.setNode((prev) => strip(prev, k));
    actions.current.onDetach?.(k);
    dragRef.current = null;
    zoneRef.current = null;
    setDrag(null);
    setZone(null);
  };

  /// 收起只是把它从树上摘掉：谁「既不在树上、又没浮出去」，谁就是右侧那一枚胶囊，
  /// 所以不用再存一份收起状态，摆在哪一栏永远能从树里推出来。
  const collapse = (k: K) => {
    actions.current.setNode((prev) => strip(prev, k));
  };

  const expand = (k: K) => {
    // 拖动松手后补的那次 click 不能当成「再归位一次」，否则刚选好的落点会被追加到末尾。
    if (justDragged.current) return;
    actions.current.setNode((prev) => place(prev, k, null));
  };

  // 拖拽期间的事件挂在 window 上：标题栏元素拿到 pointer capture 后，越出窗口的移动照样能收到。
  const dragging = drag !== null;
  useEffect(() => {
    if (!dragging) return;

    const move = (event: PointerEvent) => {
      const active = dragRef.current;
      if (!active) return;
      const { clientX: x, clientY: y } = event;
      const moved = active.moved || Math.hypot(x - active.sx, y - active.sy) > DRAG_SLOP;
      const outside = x < 0 || y < 0 || x > window.innerWidth || y > window.innerHeight;
      // 越出主窗不再当场浮出：还是原来那套拖动样式，等松手。窗外也就不提示吸附。
      dragRef.current = { ...active, x, y, moved, outside };
      zoneRef.current = outside ? null : zoneAt(rects(), x, y);
      setDrag(dragRef.current);
      setZone(zoneRef.current);
    };

    const up = () => {
      const active = dragRef.current;
      if (active?.moved) {
        // 真拖过：把紧随其后的那次 click 吞掉（拖的是右侧胶囊时才用得上）。
        justDragged.current = true;
        window.setTimeout(() => {
          justDragged.current = false;
        }, 0);
        // 松手在窗外：这页给得起「浮出」才开窗，给不起就照落点摆回树里。
        if (active.outside && actions.current.onDetach) detach(active.k);
        else drop(active.k, zoneRef.current);
      }
      dragRef.current = null;
      zoneRef.current = null;
      setDrag(null);
      setZone(null);
    };

    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
    };
    // 监听器只读 ref，装一次就够。
  }, [dragging]);

  const startDrag = (event: ReactPointerEvent<HTMLElement>, k: K) => {
    if (event.button !== 0) return;
    event.preventDefault();
    // 拿到 capture 才收得到越出主窗的移动：窗外松手那一下才算浮出。
    // 合成事件（沙盒 / 无障碍工具）没有活动指针，这一句会抛 NotFoundError：拿不到就退化成不收 capture，
    // 拖动照样按 window 上的监听走完。
    try {
      event.currentTarget.setPointerCapture(event.pointerId);
    } catch {
      /* 没有活动指针就没有 capture，不影响下面的落点判定 */
    }
    const seed: Drag<K> = {
      k,
      sx: event.clientX,
      sy: event.clientY,
      x: event.clientX,
      y: event.clientY,
      moved: false,
      outside: false,
    };
    dragRef.current = seed;
    setDrag(seed);
  };

  const tune = (path: number[], r: number) => {
    const ratio = Math.min(MAX_RATIO, Math.max(MIN_RATIO, r));
    setNode((prev) => (prev ? retune(prev, path, ratio) : prev));
  };

  /// 提示的来源：本窗里正拖着的，或独立窗口正拖回来的 —— 画的是同一套幽灵块加吸附块。
  const hint = drag?.moved
    ? { k: drag.k, x: drag.x, y: drag.y, zone, outside: drag.outside }
    : hover
      ? { k: hover.panel, x: hover.x, y: hover.y, zone: zoneAt(rects(), hover.x, hover.y), outside: false }
      : null;

  const render = (item: DockNode<K>, path: number[]): ReactNode => {
    if (item.t === "leaf") {
      const isDragged = drag?.moved && drag.k === item.k;
      const snap =
        hint && hint.zone && hint.zone.k === item.k && hint.k !== item.k
          ? hint.zone.edge ?? "center"
          : null;
      const body = renderBody(item.k);
      return (
        <div
          className="dock-leaf"
          data-dragging={isDragged ? "true" : undefined}
          ref={(el) => {
            if (el) leafEls.current.set(item.k, el);
            else leafEls.current.delete(item.k);
          }}
        >
          {snap ? <i className="snap" data-edge={snap} /> : null}
          <PanelChrome
            chrome={{
              panel: item.k,
              title: titles[item.k],
              floating: false,
              onLeave: onDetach ? detach : undefined,
              onCollapse: collapse,
              onHeadPointerDown: (e) => startDrag(e, item.k),
            }}
          >
            {/* 外壳由 renderBody 自己给：开发者页各面板内部就带 PanelFrame（正文各有专属 class 与滚动），
                GM 那八张卡则由本页包一层 PanelFrame。这层只管位置和拖拽，不重复画标题栏。 */}
            {body}
          </PanelChrome>
        </div>
      );
    }
    return (
      <div className="dock-split" data-dir={item.dir}>
        <div className="dock-cell" style={{ flexGrow: item.r, flexBasis: 0 }}>
          {render(item.a, [...path, 0])}
        </div>
        <Divider dir={item.dir} path={path} onTune={tune} />
        <div className="dock-cell" style={{ flexGrow: 1 - item.r, flexBasis: 0 }}>
          {render(item.b, [...path, 1])}
        </div>
      </div>
    );
  };

  const outs = keys.filter((k) => detached.includes(k));
  /// 右侧摆放栏那一列：不在停靠树上、又没浮成独立窗口的，就是收起着的那一枚。
  const docked = node ? leaves(node) : [];
  const tray = keys.filter((k) => !detached.includes(k) && !docked.includes(k));

  return (
    <div className="dock" data-drag={drag?.moved ? "true" : undefined}>
      <div className="dock-main">
        {node ? (
          render(node, [])
        ) : (
          <section className="blank">
            {emptyTitle ? <b>{emptyTitle}</b> : null}
            <span>{emptyText}</span>
            {outs.length > 0 ? (
              <div className="row">
                {outs.map((k) => (
                  <button
                    className="btn"
                    key={k}
                    data-tone="accent"
                    onClick={() => (onReturn ? onReturn(k) : expand(k))}
                  >
                    {titles[k]} 归位
                  </button>
                ))}
              </div>
            ) : null}
          </section>
        )}
      </div>

      <aside className="pill-tray" aria-label="已收起的面板">
        <b className="tray-title">摆放栏</b>
        {tray.map((k) => (
          <button
            className="tray-pill"
            data-panel={k}
            key={k}
            data-dragging={drag?.moved && drag.k === k ? "true" : undefined}
            title={pillHint}
            onPointerDown={(event) => startDrag(event, k)}
            onClick={() => expand(k)}
          >
            {titles[k]}
          </button>
        ))}
        {tray.length === 0 ? <span className="tray-hint">{trayHint}</span> : null}
      </aside>

      {hint ? (
        <div
          className="dock-ghost"
          data-outside={hint.outside ? "true" : undefined}
          style={{
            left: Math.min(Math.max(hint.x, GHOST_PAD), window.innerWidth - GHOST_PAD),
            top: Math.min(Math.max(hint.y, GHOST_PAD), window.innerHeight - GHOST_PAD),
          }}
        >
          {titles[hint.k]}
        </div>
      ) : null}
    </div>
  );
}

function Divider({
  dir,
  path,
  onTune,
}: {
  dir: Dir;
  path: number[];
  onTune: (path: number[], r: number) => void;
}) {
  const bar = useRef<HTMLDivElement>(null);

  const start = (event: ReactPointerEvent<HTMLDivElement>) => {
    const host = bar.current?.parentElement;
    if (!host) return;
    event.preventDefault();
    const rect = host.getBoundingClientRect();
    const move = (ev: PointerEvent) => {
      const ratio =
        dir === "v" ? (ev.clientY - rect.top) / rect.height : (ev.clientX - rect.left) / rect.width;
      onTune(path, ratio);
    };
    const stop = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", stop);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", stop);
  };

  return <div className="dock-bar" data-dir={dir} ref={bar} onPointerDown={start} />;
}
