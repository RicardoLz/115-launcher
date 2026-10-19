import { useState } from "react";
import {
  commands,
  errText,
  type BackgroundState,
  type ConfigView,
  type Readiness,
} from "../bridge";
import { SCHEMES, type Glass, type Scheme } from "../theme";
import MemoryCard from "./MemoryCard";

export type LaunchMode = "compat" | "server-only";

/// 上传范围：这是给玩家看的白名单，说一套做一套比什么都不报更糟。
const UPLOADS = [
  "故障发生在哪一步、标题与详情",
  "游戏客户端版本与本机系统环境",
  "启动器版本",
  "退出码（判成崩溃时）",
  "运行日志的尾部，已做大小、编码与隐私过滤",
];

const NEVER = [
  "登录令牌、密码、验证码",
  "完整的用户目录路径与账号信息",
  "整份日志文件",
];

export default function SettingsPage({
  config,
  readiness,
  busy,
  launchMode,
  background,
  scheme,
  onScheme,
  glass,
  onGlass,
  onLaunchMode,
  onAutoStop,
  onAutoReport,
  onPickClient,
  onDiscoverClient,
  onPickBackground,
  onClearBackground,
  onNotice,
}: {
  config: ConfigView | null;
  readiness: Readiness | null;
  busy: boolean;
  launchMode: LaunchMode;
  background: BackgroundState | null;
  scheme: Scheme;
  onScheme: (scheme: Scheme) => void;
  glass: Glass;
  onGlass: (glass: Glass) => void;
  onLaunchMode: (mode: LaunchMode) => void;
  onAutoStop: (enabled: boolean) => Promise<void>;
  onAutoReport: (enabled: boolean) => Promise<void>;
  onPickClient: () => Promise<void>;
  onDiscoverClient: () => Promise<void>;
  onPickBackground: () => Promise<void>;
  onClearBackground: () => Promise<void>;
  onNotice: (text: string, tone?: "ok" | "err") => void;
}) {
  const pending = config?.pending_reports ?? 0;
  // 背景那枚 chip：没设=灰，记着但文件不在了=黄，正常=绿。
  const bgTone =
    background === null || background.path === ""
      ? "idle"
      : background.exists
        ? "ok"
        : "busy";
  // 悬停与聚焦共用一个开关：只认鼠标的话，键盘玩家永远看不到这份白名单。
  const [showScope, setShowScope] = useState(false);

  const retry = () => {
    try {
      commands.reportsRetry();
      onNotice("已催一次队列发送，结果稍后写在运行日志里");
    } catch (err) {
      onNotice(errText(err), "err");
    }
  };

  return (
    <div className="body">
      <div className="column">
        <section className="card">
          <h2>启动</h2>

          <div className="field">
            <label>本次启动方式</label>
            <div className="segmented">
              <button
                aria-pressed={launchMode === "compat"}
                disabled={busy}
                onClick={() => onLaunchMode("compat")}
              >
                原生启动（网关 + 客户端）
              </button>
              <button
                aria-pressed={launchMode === "server-only"}
                disabled={busy}
                onClick={() => onLaunchMode("server-only")}
              >
                仅服务端
              </button>
            </div>
            <p className="hint">
              仅服务端不拉起 DFO.exe，用来单独验证网关；停止时同样会把环境收干净。
            </p>
          </div>

          <div className="field">
            <button
              className="switch"
              data-on={config?.auto_stop_on_client_exit ? "true" : "false"}
              disabled={busy}
              onClick={() => void onAutoStop(!config?.auto_stop_on_client_exit)}
            >
              <span>
                关闭游戏后自动停止服务端
                <br />
                <em className="hint">关掉游戏窗口就收尾，不留残留进程</em>
              </span>
              <span className="switch-track" />
            </button>
          </div>
        </section>

        <section className="card">
          <h2>客户端与存档</h2>

          <div className="field">
            <label>游戏客户端目录</label>
            <div className="path">{readiness?.client_dir ?? "…"}</div>
            <div className="row">
              <button className="btn" disabled={busy} onClick={onDiscoverClient}>
                自动识别
              </button>
              <button className="btn" disabled={busy} onClick={onPickClient}>
                手动选择…
              </button>
            </div>
          </div>

          <div className="field">
            <label>存档</label>
            <div className="path">{config?.storage_detail ?? "…"}</div>
            <p className="hint">启动器只在缺配置时补一份 SQLite 档，绝不覆盖或预建库文件。</p>
          </div>
        </section>

        <MemoryCard onNotice={onNotice} />
      </div>

      <div className="column">
        <section className="card">
          <h2>配色方案</h2>

          <div className="field">
            <div className="scheme-grid" role="group" aria-label="配色方案">
              {SCHEMES.map((item) => (
                <button
                  key={item.key}
                  className="scheme"
                  data-scheme={item.key}
                  title={item.label}
                  aria-label={item.label}
                  aria-pressed={scheme === item.key}
                  onClick={() => onScheme(item.key)}
                >
                  <span className="scheme-plate" />
                </button>
              ))}
            </div>
            <p className="hint">
              当前是「{SCHEMES.find((item) => item.key === scheme)?.label ?? SCHEMES[0].label}」。
              方案只管颜色，明暗另算：暗色改到底色与各处高亮，亮色让整扇窗口跟着偏色。
              设了主页背景时，悬停那圈光改用壁纸取到的主题色，这里的方案只管其余部分。
            </p>
          </div>

          <div className="field">
            <button
              className="switch"
              data-on={glass === "on" ? "true" : "false"}
              onClick={() => onGlass(glass === "on" ? "off" : "on")}
            >
              <span>
                液态玻璃
                <br />
                <em className="hint">卡片、弹层与按钮改用透光磨砂材质：底板透下去一层模糊，边框高光跟着光标走</em>
              </span>
              <span className="switch-track" />
            </button>
            <p className="hint">
              关＝原版（不透明面板，不吃合成开销）。材质要有东西可透才好看：没设主页背景时只剩模糊与边缘高光，
              设了壁纸才看得到背景透过卡片糊成一层。
            </p>
          </div>
        </section>

        <section className="card">
          <h2>自动上报</h2>

          <div className="field" data-pop="true" onMouseLeave={() => setShowScope(false)}>
            <button
              className="switch"
              data-on={config?.auto_report ? "true" : "false"}
              disabled={busy}
              onMouseEnter={() => setShowScope(true)}
              onFocus={() => setShowScope(true)}
              onBlur={() => setShowScope(false)}
              onClick={() => void onAutoReport(!config?.auto_report)}
            >
              <span>
                自动上报故障
                <br />
                <em className="hint">关闭时只记在本机，绝不发出去</em>
              </span>
              <span className="switch-track" />
            </button>

            {showScope ? (
              <div className="scope-pop" role="tooltip">
                <div className="scope-col">
                  <b>会上传这些</b>
                  <ul className="scope">
                    {UPLOADS.map((item) => (
                      <li key={item}>{item}</li>
                    ))}
                  </ul>
                </div>
                <div className="scope-col">
                  <b>绝不上传</b>
                  <ul className="scope" data-tone="never">
                    {NEVER.map((item) => (
                      <li key={item}>{item}</li>
                    ))}
                  </ul>
                </div>
              </div>
            ) : null}
          </div>

          <div className="field">
            <div className="row">
              <span className="spacer" />
              <span className="chip" data-tone={pending > 0 ? "busy" : "ok"}>
                待发送 {pending} 条
              </span>
              <button
                className="btn"
                data-tone="accent"
                disabled={busy || pending === 0 || !config?.auto_report}
                onClick={retry}
              >
                立即重试
              </button>
            </div>
            <p className="hint">
              {config?.auto_report
                ? "断网时报告留在本地队列，恢复后按原来那条的编号重发，不会重复计数。"
                : "自动上报未开启：本机记下的故障都留在这台电脑上，随时可以打开开关后发出去。"}
            </p>
          </div>
        </section>

        <section className="card">
          <h2>主页背景</h2>

          <div className="field">
            <div className="row">
              <span className="chip" data-tone={bgTone}>
                {background?.note ?? "…"}
              </span>
              <span className="spacer" />
              <button className="btn" disabled={busy} onClick={onPickBackground}>
                选择素材…
              </button>
              <button
                className="btn"
                data-tone="danger"
                disabled={busy || background === null || background.path === ""}
                onClick={onClearBackground}
              >
                清除
              </button>
            </div>
            {background && background.path ? (
              <div className="path">{background.path}</div>
            ) : null}
            <p className="hint">
              图片支持 png / jpg / jpeg / webp / gif；视频支持 mp4 / webm / mov，在主页循环静音播放。
              素材留在你自己选的位置，启动器不搬运、不复制，只把当前选中的那一个路径放行给渲染层，
              换一个或清除后旧路径立刻收回。没有背景时主页走纯色主题。
            </p>
          </div>
        </section>
      </div>
    </div>
  );
}
