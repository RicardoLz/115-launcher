import type { Dispatch, ReactNode, RefObject, SetStateAction } from "react";
import { PANEL_KEYS, type DockNode, type PanelKey, type Zone } from "../dock";
import DockLayer from "./DockLayer";
import { PANELS } from "./panels";

/// 开发者页那一层薄封装：把五块面板的标题、正文与「浮出成独立窗口」接进通用停靠层。
/// 拖拽、分割、收起成胶囊、右侧摆放栏都在 DockLayer 里，这里只补这页独有的那两件事：
/// 浮出去的窗口（detach/return）与空树时那段说明。
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
  const titles = Object.fromEntries(
    PANEL_KEYS.map((k) => [k, PANELS[k].title]),
  ) as Record<PanelKey, string>;

  const renderBody = (k: PanelKey): ReactNode => {
    const { Body } = PANELS[k];
    return <Body />;
  };

  return (
    <DockLayer<PanelKey>
      keys={PANEL_KEYS}
      node={node}
      setNode={setNode}
      titles={titles}
      renderBody={renderBody}
      detached={PANEL_KEYS.filter((k) => floating[k])}
      onDetach={onDetach}
      onReturn={onReturn}
      hover={hover}
      hitRef={hitRef}
      emptyTitle="开发者页"
      pillHint="拖回开发者页落位，点一下接在末尾"
      emptyText={
        <>
          运行日志、游戏内容模式、客户端插件、mod 安装层与服务端内存都摆在这一页：按住面板标题栏在页内拖动可以换位、分割，
          <br />
          拖出主窗后松手，就在松手那儿开一扇能自由拉伸的独立窗口；再拖回主窗，按提示落位。
          <br />
          标题栏上那枚胶囊图标把面板收进右侧摆放栏，点胶囊立刻归位，也可以把它拖到想落的格子上。
        </>
      }
    />
  );
}
