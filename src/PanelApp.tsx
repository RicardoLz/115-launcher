import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import GlassDefs from "./components/GlassDefs";
import { PanelChrome } from "./components/PanelFrame";
import { PANELS } from "./components/panels";
import { commands } from "./bridge";
import { useTheme } from "./theme";
import type { PanelKey } from "./dock";

/// 浮出去的独立窗口：无边框，靠标题栏拖、靠边缘拉伸，界面只管这一块面板。
export default function PanelApp({ panel }: { panel: PanelKey }) {
  const { glass } = useTheme();
  const { title, Body } = PANELS[panel];

  // 标题栏的关闭不真关：窗口只是藏起来，界面把它收回停靠区，下次浮出还是这块内容。
  useEffect(() => {
    let stop: (() => void) | null = null;
    let disposed = false;
    void getCurrentWindow()
      .onCloseRequested((event) => {
        event.preventDefault();
        void commands.panelClosed(panel);
      })
      .then((unlisten) => {
        if (disposed) unlisten();
        else stop = unlisten;
      });
    return () => {
      disposed = true;
      stop?.();
    };
  }, [panel]);

  return (
    <div className="app app-panel">
      {/* 边框高光的偏角写在根节点上，而每个窗口是自己的文档，引不到主窗那份；
          所以浮出的窗口也要自己挂一个监听，不能只靠主窗。 */}
      {glass === "on" ? <GlassDefs /> : null}
      <PanelChrome
        chrome={{
          panel,
          title,
          floating: true,
          onLeave: () => void commands.panelDock(panel),
        }}
      >
        <Body />
      </PanelChrome>
    </div>
  );
}
