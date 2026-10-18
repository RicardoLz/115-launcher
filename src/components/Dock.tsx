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
import {
  PANEL_KEYS,
  place,
  retune,
  strip,
  zoneAt,
  type Dir,
  type DockNode,
  type PanelKey,
  type Zone,
} from "../dock";
import { PanelChrome } from "./PanelFrame";
import { PANELS } from "./panels";

const MIN_RATIO = 0.15;
const MAX_RATIO = 0.85;
/// 手抖到这么多像素才算真在拖：不然点「浮出」按钮会先把面板拎起来。
const DRAG_SLOP = 4;
/// 幽灵块贴边时留的间距：拖出主窗也要看得见手里这块是什么。
const GHOST_PAD = 12;

type Drag = {
  k: PanelKey;
  sx: number;
  sy: number;
  x: number;
  y: number;
  moved: boolean;
  /// 指针已经越出主窗：这一步松手是开窗，不是换位。
  outside: boolean;
};

export default function Dock({
  node,
  setNode,
  floating,
  hover,
  onDetach,
  onReturn,
  hitRef,
}: {
  node: DockNode | null;
  setNode: Dispatch<SetStateAction<DockNode | null>>;
  floating: Record<PanelKey, boolean>;
  /// 独立窗口正被拖回主窗：按这个落点画窗口内的吸附提示。
  hover: { panel: PanelKey; x: number; y: number } | null;
  onDetach: (panel: PanelKey) => void;
  onReturn: (panel: PanelKey) => void;
  /// 停靠层把命中测试借出来：独立窗口拖回来时按落点挑吸附槽。
  hitRef: RefObject<((x: number, y: number) => Zone | null) | null>;
}) {
  const [drag, setDrag] = useState<Drag | null>(null);
  const [zone, setZone] = useState<Zone | null>(null);
  const leafEls = useRef(new Map<PanelKey, HTMLElement>());
  const dragRef = useRef<Drag | null>(null);
  const zoneRef = useRef<Zone | null>(null);
  const actions = useRef({ setNode, onDetach });
  actions.current = { setNode, onDetach };

  const rects = () => {
    const out: Array<{ k: PanelKey; rect: DOMRect }> = [];
    leafEls.current.forEach((el, k) => {
      if (el.isConnected) out.push({ k, rect: el.getBoundingClientRect() });
    });
    return out;
  };

  useEffect(() => {
    hitRef.current = (x, y) => zoneAt(rects(), x, y);
    return () => {
      hitRef.current = null;
    };
    // rects 只读 ref，不需要依赖。
  }, [hitRef]);

  const drop = (k: PanelKey, where: Zone | null) => {
    actions.current.setNode((prev) => place(prev, k, where));
  };

  const detach = (k: PanelKey) => {
    actions.current.setNode((prev) => strip(prev, k));
    actions.current.onDetach(k);
    dragRef.current = null;
    zoneRef.current = null;
    setDrag(null);
    setZone(null);
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
        // 松手在窗外：就地开一扇独立窗口（窗口开在指针那儿，看着像同一块面板被拎了出去）。
        if (active.outside) detach(active.k);
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

  const startDrag = (event: ReactPointerEvent<HTMLElement>, k: PanelKey) => {
    if (event.button !== 0) return;
    event.preventDefault();
    // 拿到 capture 才收得到越出主窗的移动：窗外松手那一下才算浮出。
    event.currentTarget.setPointerCapture(event.pointerId);
    const seed: Drag = {
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

  const render = (item: DockNode, path: number[]): ReactNode => {
    if (item.t === "leaf") {
      const { title, Body } = PANELS[item.k];
      const isDragged = drag?.moved && drag.k === item.k;
      const snap =
        hint && hint.zone && hint.zone.k === item.k && hint.k !== item.k
          ? hint.zone.edge ?? "center"
          : null;
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
            chrome={{ panel: item.k, title, floating: false, onLeave: detach, onHeadPointerDown: (e) => startDrag(e, item.k) }}
          >
            <Body />
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

  const outs = PANEL_KEYS.filter((k) => floating[k]);

  return (
    <div className="dock" data-drag={drag?.moved ? "true" : undefined}>
      {node ? (
        render(node, [])
      ) : (
        <section className="blank">
          <b>开发者页</b>
          <span>
            运行日志、游戏内容模式与客户端插件都在这里：按住面板标题栏在页内拖动可以换位、分割，
            <br />
            拖出主窗后松手，就在松手那儿开一扇能自由拉伸的独立窗口；再拖回主窗，按提示落位。
          </span>
          {outs.length > 0 ? (
            <div className="row">
              {outs.map((k) => (
                <button className="btn" key={k} data-tone="accent" onClick={() => onReturn(k)}>
                  {PANELS[k].title} 归位
                </button>
              ))}
            </div>
          ) : null}
        </section>
      )}
      {hint ? (
        <div
          className="dock-ghost"
          data-outside={hint.outside ? "true" : undefined}
          style={{
            left: Math.min(Math.max(hint.x, GHOST_PAD), window.innerWidth - GHOST_PAD),
            top: Math.min(Math.max(hint.y, GHOST_PAD), window.innerHeight - GHOST_PAD),
          }}
        >
          {PANELS[hint.k].title}
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
