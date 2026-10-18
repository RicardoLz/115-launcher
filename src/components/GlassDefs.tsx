import { useEffect } from "react";

/// 玻璃这一轮不需要任何几何量测：折射与靠近光标的弹性整条撤掉，材质全靠 CSS 的
/// backdrop-filter，JS 只剩「把光标位置写成根节点上 -1..1 两个量」，边框那圈高光按它偏角。
/// 开关打开才挂载；主窗与浮出的面板窗口各挂一份——变量是文档内的，跨窗口引不到。
export default function GlassDefs() {
  useEffect(() => {
    const root = document.documentElement;
    const put = (x: number, y: number) => {
      root.style.setProperty("--glass-mx", (window.innerWidth ? (x / window.innerWidth) * 2 - 1 : 0).toFixed(3));
      root.style.setProperty("--glass-my", (window.innerHeight ? (y / window.innerHeight) * 2 - 1 : 0).toFixed(3));
    };
    const onMove = (event: PointerEvent) => put(event.clientX, event.clientY);
    // 光标离开窗口时回到正中，否则高光停在最后一帧的偏角上，看着像边框自己拧了一半。
    const onLeave = () => put(0, 0);
    window.addEventListener("pointermove", onMove, { passive: true });
    document.addEventListener("pointerleave", onLeave);
    return () => {
      window.removeEventListener("pointermove", onMove);
      document.removeEventListener("pointerleave", onLeave);
      put(0, 0);
    };
  }, []);

  return null;
}
