import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { BackgroundState } from "../bridge";

const CODE_TEXT: Record<number, string> = {
  1: "加载被中止",
  2: "网络层失败",
  3: "解不出来",
  4: "这个源它不认",
};

/// 素材本身没问题时也要能定责：错误码 + 直接对同一个 asset 地址探一次请求。
/// 启动器的 asset 处理器每答都带 `access-control-allow-origin`，所以这一行能分开两种 403：
/// 带＝白名单没放行（启动器拦的），不带＝请求压根没走到启动器，是网络层或系统代理答的。
async function probeAsset(url: string): Promise<string> {
  const ctrl = new AbortController();
  try {
    const timer = setTimeout(() => ctrl.abort(), 3000);
    const res = await fetch(url, { signal: ctrl.signal });
    clearTimeout(timer);
    const range = res.headers.get("content-range");
    const ours = res.headers.get("access-control-allow-origin") !== null;
    ctrl.abort();
    const who = res.status < 400 ? "" : ours ? "，启动器白名单拦的" : "，没走到启动器（网络层或代理答的）";
    return `HTTP ${res.status}${range ? ` ${range}` : ""}${who}`;
  } catch (err) {
    return `请求没走通：${err instanceof Error ? err.name : String(err)}`;
  }
}

/// 主页背景层：玩家自选的壁纸图片或动态壁纸视频，铺在最底下。
/// 素材走 asset 协议（Rust 侧只放行当前选中的那一个文件），所以路径由后端给、这里只拼 URL。
/// 文件被搬走或解码不上就当没有背景，退回纯色主题。
export default function BackgroundLayer({
  view,
  onProblem,
}: {
  view: BackgroundState | null;
  onProblem: (text: string) => void;
}) {
  const live = view && view.exists && view.kind !== "none" ? view : null;
  const path = live?.path ?? "";
  // 素材没出画面之前这一层不显形。开机头几帧窗口尺寸还没定下来，视频的第一帧会按那个旧尺寸
  // 合成在左上角一小块（2026-10-18 实机：每次开起来都这样，过一下才铺满），等它真的播起来
  // 再亮出来就看不到那一帧了；素材读不到时靠兜底时间照常显形，错误提示不受影响。
  const [painted, setPainted] = useState(false);
  useEffect(() => {
    setPainted(false);
    if (!path) return;
    const timer = setTimeout(() => setPainted(true), 1200);
    return () => clearTimeout(timer);
  }, [path]);
  if (!live) return null;
  const src = convertFileSrc(live.path);

  const report = async (what: string, code?: number) => {
    const how = await probeAsset(src);
    onProblem(
      `背景${what}播不出来：${live.path}（${code ? CODE_TEXT[code] ?? `错误码 ${code}` : "读不出来"}，${how}）`,
    );
  };
  return (
    <div className="bg-layer" data-painted={painted ? "yes" : "no"} aria-hidden="true">
      {live.kind === "video" ? (
        <video
          key={live.path}
          src={src}
          autoPlay
          muted
          loop
          playsInline
          onPlaying={() => setPainted(true)}
          onError={(event) => void report("视频", event.currentTarget.error?.code)}
        />
      ) : (
        <img
          key={live.path}
          src={src}
          alt=""
          onLoad={() => setPainted(true)}
          onError={() => void report("图", 0)}
        />
      )}
      <span className="bg-veil" />
    </div>
  );
}
