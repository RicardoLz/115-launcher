import type { ComponentType } from "react";
import LogConsole from "./LogConsole";
import MemoryCard from "./MemoryCard";
import ModePanel from "./ModePanel";
import ModkitPanel from "./ModkitPanel";
import ModsPanel from "./ModsPanel";
import type { PanelKey } from "../dock";

export const FLOAT_PREFIX = "panel-";

export const PANELS: Record<PanelKey, { title: string; Body: ComponentType }> = {
  log: { title: "运行日志", Body: LogConsole },
  mode: { title: "游戏内容模式", Body: ModePanel },
  mods: { title: "客户端插件", Body: ModsPanel },
  modkit: { title: "mod 安装层", Body: ModkitPanel },
  memory: { title: "服务端内存", Body: MemoryCard },
};

/// 独立窗口只靠 label 认自己该装哪块面板，URL 一律还是 index.html。
export function panelFromLabel(label: string): PanelKey | null {
  if (!label.startsWith(FLOAT_PREFIX)) return null;
  const key = label.slice(FLOAT_PREFIX.length);
  if (!(key in PANELS)) return null;
  return key as PanelKey;
}
