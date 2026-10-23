import { createContext, useContext, useMemo, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  CloseIcon,
  DockIcon,
  DetachIcon,
  GripIcon,
  MaximizeIcon,
  MinimizeIcon,
  PillIcon,
  RestoreIcon,
} from "./icons";
import { useMaximized } from "./WindowControls";

/// 面板外壳：标题栏就是抓手。停靠时能拖、能浮出；独立窗口里标题栏兼任窗口标题栏。
/// 标题与拖拽接线都由外层（停靠层或独立窗口根）通过 context 注进来，面板自己不写死。
/// key 泛化成任意字符串：开发者页是 PanelKey，GM 页那八张卡是 GmKey。
export type Chrome<K extends string = string> = {
  panel: K;
  title: string;
  floating: boolean;
  /// 浮成独立窗口。没有这枚动作的页（GM 那八张卡彼此共享状态，拆不出第二扇窗）不给这个回调，
  /// 标题栏就不画那枚按钮。
  onLeave?: (panel: K) => void;
  /// 收进右侧摆放栏。只有停靠层给得起这个动作，独立窗口没有（它是另一扇窗，不占开发者页的位置）。
  onCollapse?: (panel: K) => void;
  onHeadPointerDown?: (event: ReactPointerEvent<HTMLElement>) => void;
};

const ChromeCtx = createContext<Chrome | null>(null);

/// 回调参数是逆变的，所以 Chrome<K> 不能直接当 Chrome<string> 用；这里收一次是安全的——
/// panel 与两个回调都来自同一个对象，而下面唯一的调用点传的正是这个对象的 panel。
export function PanelChrome<K extends string>({
  chrome,
  children,
}: {
  chrome: Chrome<K>;
  children: ReactNode;
}) {
  return <ChromeCtx.Provider value={chrome as unknown as Chrome}>{children}</ChromeCtx.Provider>;
}

export function PanelFrame({
  extra,
  bodyClass,
  children,
}: {
  extra?: ReactNode;
  /// 正文那层的额外 class：GM 的卡要自己滚、自己要内边距，开发者页那几块各自带名。
  bodyClass?: string;
  children: ReactNode;
}) {
  const chrome = useContext(ChromeCtx);
  const draggable = Boolean(chrome && !chrome.floating && chrome.onHeadPointerDown);
  return (
    <section className="card panel" data-panel={chrome?.panel}>
      <header
        className="panel-head"
        data-grab={draggable ? "drag" : undefined}
        data-tauri-drag-region={chrome?.floating ? "deep" : undefined}
        onPointerDown={chrome?.onHeadPointerDown}
      >
        {draggable ? <GripIcon size={14} /> : null}
        <h2>{chrome?.title}</h2>
        <span className="spacer" />
        {extra}
        {chrome?.onCollapse ? (
          <button
            className="tool-btn"
            data-act="collapse"
            aria-label={`把${chrome.title}收起成胶囊`}
            onPointerDown={(event) => event.stopPropagation()}
            onClick={() => chrome.onCollapse?.(chrome.panel)}
          >
            <PillIcon size={16} />
          </button>
        ) : null}
        {chrome?.onLeave ? (
          <button
            className={chrome.floating ? "win-btn" : "tool-btn"}
            aria-label={
              chrome.floating ? `把${chrome.title}归位到开发者页` : `把${chrome.title}浮出主窗口`
            }
            onPointerDown={(event) => event.stopPropagation()}
            onClick={() => chrome.onLeave?.(chrome.panel)}
          >
            {chrome.floating ? <DockIcon size={16} /> : <DetachIcon size={16} />}
          </button>
        ) : null}
        {chrome?.floating ? <WindowToggles /> : null}
      </header>
      <div className={bodyClass ? `panel-body ${bodyClass}` : "panel-body"}>{children}</div>
    </section>
  );
}

/// 浮出去的窗口没了系统标题栏，这三个就跟在启动器右上角一样：同一套图标、同一套样式。
/// 关闭只是把这扇窗藏起来（PanelApp 里拦的 onCloseRequested），内容不丢。
function WindowToggles() {
  const win = useMemo(() => getCurrentWindow(), []);
  const [maximized, toggleMaximize] = useMaximized();
  return (
    <div className="win-controls">
      <button className="win-btn" aria-label="最小化" onClick={() => void win.minimize()}>
        <MinimizeIcon />
      </button>
      <button
        className="win-btn"
        aria-label={maximized ? "还原" : "最大化"}
        onClick={toggleMaximize}
      >
        {maximized ? <RestoreIcon /> : <MaximizeIcon />}
      </button>
      <button
        className="win-btn"
        data-kind="close"
        aria-label="关闭"
        onClick={() => void win.close()}
      >
        <CloseIcon />
      </button>
    </div>
  );
}
