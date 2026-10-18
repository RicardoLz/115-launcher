import { useEffect, useRef, useState, type CSSProperties } from "react";
import { UserIcon } from "./icons";
import type { ConfigView, UpdateProgress } from "../bridge";

type LaunchState = "idle" | "starting" | "running" | "stopping";

/// 检查过更新、确有新版本时挂到开关上的那件事：kind 决定走哪条 apply，target 只用于提示。
export interface UpdateArm {
  kind: "self" | "server";
  target: string;
}

function stateOf(phase: string): LaunchState {
  if (phase === "running") return "running";
  if (phase === "starting") return "starting";
  if (phase === "stopping") return "stopping";
  return "idle";
}

/// 主页右下角那一组：黑色药丸底板 + 悬停起光（uiverse「mean-bird」那一套），
/// 边缘一圈跑动的虚线与按真实字节填充的描边（uiverse「quiet-panda」那一套）留给进行中的状态。
/// 一个控件承担「开始游戏 / 取消并停止 / 停止游戏」，检查过更新且真有新版本时整枚变成「更新版本」，
/// 按下去就是下载 + 部署，进度直接画在按钮边上。账号缩在左边同一材质的胶囊里。
export default function LaunchDock({
  config,
  phase,
  busy,
  canStart,
  startHint,
  update,
  progress,
  onAccount,
  onStart,
  onStop,
  onUpdate,
}: {
  config: ConfigView | null;
  phase: string;
  busy: boolean;
  canStart: boolean;
  startHint: string;
  update: UpdateArm | null;
  progress: UpdateProgress | null;
  onAccount: (account: string) => Promise<void>;
  onStart: () => void;
  onStop: () => void;
  onUpdate: () => void;
}) {
  const [draft, setDraft] = useState("");
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLDivElement | null>(null);

  const current = config?.account ?? "";
  const used = config?.accounts ?? [];

  useEffect(() => {
    if (!open) setDraft(current);
  }, [current, open]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      const node = anchor.current;
      if (node && !node.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  const apply = (candidate: string) => {
    const value = candidate.trim();
    setOpen(false);
    if (value === "" || value === current) return;
    void onAccount(value);
  };

  const state = stateOf(phase);
  const inFlight = progress !== null;
  const percent = progress === null ? 0 : Math.max(0, Math.min(100, progress.percent));
  const deploying = progress?.phase === "deploy";

  // armed 与起停共用同一枚控件：有更新待处理时它就不管游戏状态，专心做更新。
  const disabled = update
    ? inFlight || state === "starting" || state === "stopping"
    : state === "stopping" || (state === "idle" && (!canStart || busy));
  // 进行中那一档不该被当成「按不动」涂灰，只有真够不着的时候才压暗。
  const dim = disabled && !inFlight && state !== "starting" && state !== "stopping";

  const label = update
    ? inFlight
      ? deploying
        ? update.kind === "self"
          ? "正在替换…"
          : "部署中…"
        : `更新中 ${percent}%`
      : "更新版本"
    : state === "idle"
      ? "开始游戏"
      : state === "starting"
        ? "取消并停止"
        : state === "running"
          ? "停止游戏"
          : "正在收尾…";

  const title = update
    ? inFlight
      ? `${progress?.kind === "server" ? "服务端" : "启动器"}下载中：${progress?.text ?? ""}`
      : `${update.kind === "self" ? "启动器" : "服务端"}有新版本 ${update.target}，按这里下载并更新` +
        (update.kind === "self" ? "；完成后启动器会自己重启" : "")
    : state === "idle"
      ? startHint
      : state === "starting"
        ? "环境还在起来，按这里是取消并把已经拉起的收干净"
        : state === "stopping"
          ? "正在收尾…"
          : "停止游戏与服务端";

  const press = () => {
    if (update) {
      onUpdate();
    } else if (state === "idle") {
      onStart();
    } else {
      onStop();
    }
  };

  return (
    <div className="launch-dock">
      <div className="account-dock" ref={anchor}>
        {open ? (
          <div className="account-pop">
            <label htmlFor="dock-account">登录账号（下次「开始游戏」生效）</label>
            <input
              id="dock-account"
              type="text"
              value={draft}
              spellCheck={false}
              title="字母、数字、下划线与短横线，最长 32；默认 probe"
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") apply(draft);
              }}
            />
            {used.length > 0 ? (
              <>
                <span className="account-pop__title">用过的账号，点一下就换</span>
                <ul className="account-list">
                  {used.map((name) => (
                    <li key={name}>
                      <button
                        type="button"
                        className="account-item"
                        data-current={name === current ? "on" : "off"}
                        onClick={() => apply(name)}
                      >
                        <span className="account-item__name">{name}</span>
                        {name === current ? <span className="account-item__tag">使用中</span> : null}
                      </button>
                    </li>
                  ))}
                </ul>
              </>
            ) : null}
          </div>
        ) : null}
        <button
          type="button"
          className="account-pill"
          data-open={open ? "on" : "off"}
          title={`当前登录账号：${current || "没设置"}，点开可以换`}
          onClick={() => setOpen((next) => !next)}
        >
          <UserIcon size={15} />
          <span className="account-pill__name">{current || "设置账号"}</span>
        </button>
      </div>

      <button
        className="launch-toggle"
        data-mode={update ? "update" : "launch"}
        data-state={state}
        data-phase={inFlight ? (deploying ? "deploy" : "download") : "off"}
        data-dim={dim ? "on" : "off"}
        disabled={disabled}
        title={title}
        onClick={press}
      >
        <svg className="toggle__frame" viewBox="0 0 200 44" aria-hidden="true">
          <rect
            className="frame__march"
            x="1.5"
            y="1.5"
            width="197"
            height="41"
            rx="20.5"
            ry="20.5"
            pathLength={100}
          />
          <rect
            className="frame__bar"
            x="1.5"
            y="1.5"
            width="197"
            height="41"
            rx="20.5"
            ry="20.5"
            pathLength={100}
            style={{ "--p": String(percent) } as CSSProperties}
          />
        </svg>
        <span className="toggle__sweep" aria-hidden="true" />
        <span className="toggle__icon" aria-hidden="true">
          <svg viewBox="0 0 24 24" width="17" height="17">
            <path className="glyph glyph--play" d="M8.6 5.9v12.2L19 12z" />
            <path className="glyph glyph--stop" d="M8.4 8.4h7.2v7.2H8.4z" />
            <path
              className="glyph glyph--update"
              d="M12 4.2v9.1m0 0 3.5-3.5M12 13.3 8.5 9.8M4.8 18.9h14.4"
              strokeWidth="1.9"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        </span>
        <span className="toggle__label">{label}</span>
      </button>
    </div>
  );
}
