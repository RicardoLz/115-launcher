import { useLayoutEffect, useRef, useState, type ComponentType, type CSSProperties } from "react";
import { DevIcon, FeedbackIcon, GmIcon, HomeIcon, ModsIcon, SavesIcon, SettingsIcon, ToolsIcon } from "./icons";

export type PageKey = "home" | "mods" | "tools" | "gm" | "saves" | "settings" | "feedback" | "dev";

type Page = { key: PageKey; label: string; Icon: ComponentType<{ size?: number }>; foot?: boolean };

/// 除「主页」外都只是占位页，不挂任何原有功能。`foot` 决定落在侧栏底部那一组。
export const PAGES: Page[] = [
  { key: "home", label: "主页", Icon: HomeIcon },
  { key: "mods", label: "mod", Icon: ModsIcon },
  { key: "tools", label: "工具", Icon: ToolsIcon },
  { key: "gm", label: "GM工具", Icon: GmIcon },
  { key: "saves", label: "存档", Icon: SavesIcon },
  { key: "settings", label: "设置", Icon: SettingsIcon },
  { key: "feedback", label: "反馈", Icon: FeedbackIcon },
  { key: "dev", label: "开发者", Icon: DevIcon, foot: true },
];

/// 高亮胶囊的一次落位：位置、是否已量到过、第几次换页（用作动画重播的 key）、行程毫秒与方向。
type Marker = { x: number; y: number; ready: boolean; seq: number; travel: number; dir: string };

export default function Rail({ page, onSelect }: { page: PageKey; onSelect: (key: PageKey) => void }) {
  const rail = useRef<HTMLElement | null>(null);
  const buttons = useRef<Record<string, HTMLButtonElement | null>>({});
  // seq 只在真的换页时递增：ResizeObserver 也会重测，那一次不该重播抛掷动画。
  const [marker, setMarker] = useState<Marker>({ x: 0, y: 0, ready: false, seq: 0, travel: 320, dir: "" });

  // 高亮胶囊跟着当前项走：换页时它是「滑过去」而不是「旧的淡掉新的浮起」。
  useLayoutEffect(() => {
    const measure = () => {
      const host = rail.current;
      const active = buttons.current[page];
      if (!host || !active) return;
      const box = host.getBoundingClientRect();
      const dot = active.getBoundingClientRect();
      const x = dot.left - box.left;
      const y = dot.top - box.top;
      setMarker((cur) => {
        if (cur.ready && cur.x === x && cur.y === y) return cur;
        // 行程按时距伸缩：挪一格和跨过底部那一大跳不该一样快。
        const travel = Math.min(460, 280 + Math.abs(y - cur.y) * 1.2);
        const dy = y - cur.y;
        return {
          x,
          y,
          ready: true,
          seq: cur.seq + 1,
          travel,
          dir: cur.ready ? (dy > 0 ? "down" : "up") : "",
        };
      });
    };
    measure();
    const ro = new ResizeObserver(measure);
    if (rail.current) ro.observe(rail.current);
    return () => ro.disconnect();
  }, [page]);

  const item = ({ key, label, Icon }: Page) => (
    <button
      key={key}
      className="rail-item"
      data-tip={label}
      aria-label={label}
      aria-current={page === key ? "page" : undefined}
      ref={(el) => {
        buttons.current[key] = el;
      }}
      onClick={() => onSelect(key)}
    >
      <Icon size={21} />
    </button>
  );

  return (
    <nav className="rail" ref={rail}>
      <span
        aria-hidden
        className="rail-marker"
        data-ready={marker.ready ? "on" : undefined}
        style={
          {
            "--rail-x": `${marker.x}px`,
            "--rail-y": `${marker.y}px`,
            "--rail-travel": `${marker.travel}ms`,
          } as CSSProperties
        }
      >
        <span key={marker.seq} className="rail-marker__body" data-dir={marker.dir || undefined} />
      </span>
      <div className="rail-nav">{PAGES.filter((p) => !p.foot).map(item)}</div>
      <div className="rail-foot">{PAGES.filter((p) => p.foot).map(item)}</div>
    </nav>
  );
}
