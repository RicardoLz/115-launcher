import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import ControlPanel from "./components/ControlPanel";
import Dock from "./components/Dock";
import FeedbackPage, { type FeedbackTab } from "./components/FeedbackPage";
import ModsPage from "./components/ModsPage";
import QuitSheet, { type QuitChoice } from "./components/QuitSheet";
import Rail, { PAGES, type PageKey } from "./components/Rail";
import ReportConsentSheet from "./components/ReportConsentSheet";
import SavesPage from "./components/SavesPage";
import SettingsPage, { type LaunchMode } from "./components/SettingsPage";
import ToolsPage from "./components/ToolsPage";
import { PANELS } from "./components/panels";
import { HealthSheet, PortsSheet } from "./components/ReportSheet";
import WindowControls from "./components/WindowControls";
import { DEFAULT_DOCK, leaves, place, type DockNode, type PanelKey, type Zone } from "./dock";
import { useTheme } from "./theme";
import {
  commands,
  errText,
  events,
  PHASE_LABEL,
  type AppInfo,
  type ConfigView,
  type HealthReport,
  type Phase,
  type PortReport,
  type Readiness,
  type SelfUpdateCheck,
  type ServerUpdateCheck,
  type Status,
} from "./bridge";

type SheetKind = "health" | "ports" | "quit" | null;

const TONE: Record<Phase, "ok" | "busy" | "err" | "idle"> = {
  idle: "idle",
  starting: "busy",
  running: "ok",
  stopping: "busy",
  error: "err",
};

export default function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [status, setStatus] = useState<Status>({ phase: "idle", detail: "未运行" });
  const [readiness, setReadiness] = useState<Readiness | null>(null);
  const [config, setConfig] = useState<ConfigView | null>(null);
  const [launchMode, setLaunchMode] = useState<LaunchMode>("compat");
  const [working, setWorking] = useState(false);
  const [sheet, setSheet] = useState<SheetKind>(null);
  const [health, setHealth] = useState<HealthReport | null>(null);
  const [ports, setPorts] = useState<PortReport | null>(null);
  const [portHint, setPortHint] = useState("");
  const [notice, setNotice] = useState<{ text: string; tone: "ok" | "err" } | null>(null);
  const [page, setPage] = useState<PageKey>("home");
  const [feedbackTab, setFeedbackTab] = useState<FeedbackTab>("board");
  const [dock, setDock] = useState<DockNode | null>(DEFAULT_DOCK);
  const [floating, setFloating] = useState<Record<PanelKey, boolean>>({ log: false, mode: false });
  /// 独立窗口正被拖回主窗：这个落点交给停靠层画窗口内的吸附提示。
  const [panelHover, setPanelHover] = useState<{ panel: PanelKey; x: number; y: number } | null>(
    null,
  );
  // 首次的上传范围说明：一次会话只问一遍，没答就下次再问，不替玩家默认打开。
  const [consentDone, setConsentDone] = useState(false);
  // 启动时后台探测到可更新的服务端：弹层问要不要现在更。
  const [updatePrompt, setUpdatePrompt] = useState<ServerUpdateCheck | null>(null);
  // 启动器自身更新提示。
  const [selfUpdatePrompt, setSelfUpdatePrompt] = useState<SelfUpdateCheck | null>(null);
  const [checkingSelfUpdate, setCheckingSelfUpdate] = useState(false);
  // 部署中（首次部署 / 更新部署）的状态文字，显示在顶部状态栏。
  const [deployLabel, setDeployLabel] = useState<string | null>(null);
  const { theme, toggleTheme } = useTheme();
  /// 停靠层把命中测试借出来：独立窗口拖回来时按落点挑吸附槽。
  const hitRef = useRef<((x: number, y: number) => Zone | null) | null>(null);
  const noticeTimer = useRef<number | null>(null);

  const flash = useCallback((text: string, tone: "ok" | "err" = "ok") => {
    setNotice({ text, tone });
    if (noticeTimer.current !== null) window.clearTimeout(noticeTimer.current);
    noticeTimer.current = window.setTimeout(() => setNotice(null), 6000);
  }, []);

  const refresh = useCallback(async () => {
    const [phase, ready] = await Promise.allSettled([commands.status(), commands.readiness()]);
    if (phase.status === "fulfilled") setStatus(phase.value);
    if (ready.status === "fulfilled") setReadiness(ready.value);
  }, []);

  const loadConfig = useCallback(async () => {
    try {
      setConfig(await commands.config());
    } catch (err) {
      flash(errText(err), "err");
    }
  }, [flash]);

  const dockBack = useCallback((panel: PanelKey, zone: Zone | null) => {
    setPanelHover(null);
    setFloating((prev) => (prev[panel] ? { ...prev, [panel]: false } : prev));
    // 归位与「窗口被直接关掉」两条路可能先后到，落点不同也只认第一次：已经在停靠区就不动。
    setDock((prev) => (prev && leaves(prev).includes(panel) ? prev : place(prev, panel, zone)));
  }, []);

  const detachPanel = useCallback(
    (panel: PanelKey) => {
      setFloating((prev) => ({ ...prev, [panel]: true }));
      void commands.panelOpen(panel).catch((err) => {
        setFloating((prev) => ({ ...prev, [panel]: false }));
        flash(errText(err), "err");
      });
    },
    [flash],
  );

  // 首屏：拉一次全量，之后 4 秒一次只刷「盘上事实」那几项。
  useEffect(() => {
    let alive = true;
    const stops: Array<() => void> = [];

    const bind = async () => {
      const got = await Promise.all([
        events.onPhase((next) => setStatus(next)),
        events.onReadiness((next) => setReadiness(next)),
        events.onAlert((alert) => flash(alert.text, "err")),
        events.onPanelReturned((moved) => {
          const panel = moved.panel as PanelKey;
          if (!(panel in PANELS)) return;
          dockBack(panel, hitRef.current?.(moved.x, moved.y) ?? null);
          flash(`「${PANELS[panel].title}」已归位到开发者页`);
        }),
        events.onPanelHover((hover) => {
          const panel = hover.panel as PanelKey;
          if (!(panel in PANELS)) return;
          setPanelHover(hover.inside ? { panel, x: hover.x, y: hover.y } : null);
        }),
        events.onPanelClosed((name) => {
          const panel = name as PanelKey;
          if (!(panel in PANELS)) return;
          dockBack(panel, null);
        }),
        events.onPortBlocked((blocked) => {
          setPorts(blocked.report);
          setPortHint(blocked.hint);
          setSheet("ports");
        }),
        events.onFirstRunNoServer(() => {
          // 首次部署：先弹原生目录选择框让用户选 DFO.exe，选完自动下载服务端。
          setDeployLabel("首次部署中");
          (async () => {
            try {
              await commands.pickClientDir();
              await loadConfig();
              const rec = await commands.serverUpdateApply();
              flash(rec.message);
            } catch (err) {
              flash(`首次部署失败：${errText(err)}`, "err");
            } finally {
              setDeployLabel(null);
              void refresh();
            }
          })();
        }),
        events.onServerUpdateAvailable((info) => {
          setUpdatePrompt(info);
        }),
      ]);
      if (alive) stops.push(...got);
      else got.forEach((stop) => stop());
    };

    // 首屏那几件不再等这里：七个 listen() 里任何一个不落地，那批数据就永远不会发出去，
    // 页脚的「整合包：…」会一直停在 `??` 兜底上。事件订阅自己收尾即可。
    void bind();

    const poll = window.setInterval(() => void refresh(), 4000);
    return () => {
      alive = false;
      window.clearInterval(poll);
      stops.forEach((stop) => stop());
      if (noticeTimer.current !== null) window.clearTimeout(noticeTimer.current);
    };
  }, [refresh, flash, dockBack]);

  // 首屏四件各拉一次、各自落各自的 state：一条命令失败或卡住，不该把其余三块一起带走。
  // 单独立一条 effect 是有意的 —— 它原先挂在上面那条的 `bind()` 之后，订阅没落地就整批不发。
  useEffect(() => {
    let alive = true;
    const take = <T,>(task: Promise<T>, put: (value: T) => void) => {
      task.then(
        (value) => {
          if (alive) put(value);
        },
        (err: unknown) => {
          if (alive) flash(errText(err), "err");
        },
      );
    };
    take(commands.appInfo(), setInfo);
    take(commands.status(), setStatus);
    take(commands.readiness(), setReadiness);
    take(commands.config(), setConfig);
    // 启动器自身更新：探测失败就当没有，不打扰玩家。
    commands
      .selfUpdateCheck()
      .then((r) => {
        if (alive && r.has_update) setSelfUpdatePrompt(r);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [flash]);

  // 点标题栏的关闭：先问三选一，别把环境丢在那儿就走。
  useEffect(() => {
    let stop: (() => void) | null = null;
    let disposed = false;
    void getCurrentWindow()
      .onCloseRequested((event) => {
        event.preventDefault();
        setSheet("quit");
      })
      .then((unlisten) => {
        if (disposed) unlisten();
        else stop = unlisten;
      });
    return () => {
      disposed = true;
      stop?.();
    };
  }, []);

  const act = useCallback(
    async (label: string, task: () => Promise<void>) => {
      setWorking(true);
      try {
        await task();
      } catch (err) {
        flash(`${label}：${errText(err)}`, "err");
      } finally {
        setWorking(false);
        await refresh();
      }
    },
    [flash, refresh],
  );

  const startGame = () =>
    act("开始游戏", async () => {
      const outcome = await commands.start(launchMode === "server-only" ? "server-only" : undefined);
      setStatus({ phase: outcome.phase, detail: outcome.detail });
    });

  const stopGame = () =>
    act("停止游戏", async () => {
      const touched = await commands.stop();
      if (!touched) flash("没有正在运行的环境，不需要停止");
    });

  const openHealth = async () => {
    setHealth(null);
    setSheet("health");
    try {
      setHealth(await commands.envCheck());
    } catch (err) {
      flash(errText(err), "err");
      setSheet(null);
    }
  };

  const loadPorts = useCallback(async () => {
    try {
      setPorts(await commands.ports());
      setPortHint("");
    } catch (err) {
      flash(errText(err), "err");
    }
  }, [flash]);

  const openPorts = () => {
    setPorts(null);
    setSheet("ports");
    void loadPorts();
  };

  const forceClean = () =>
    act("强制清理", async () => {
      setPorts(await commands.forceClean());
      setPortHint("");
    });

  const setAutoReport = useCallback(
    async (enabled: boolean) => {
      await act(enabled ? "开启自动上报" : "关闭自动上报", async () => {
        const record = await commands.setAutoReport(enabled);
        await loadConfig();
        flash(record.message);
      });
    },
    [act, loadConfig, flash],
  );

  const setAutoPickup = useCallback(
    async (enabled: boolean) => {
      await act(enabled ? "开启自动拾取" : "关闭自动拾取", async () => {
        const record = await commands.setAutoPickup(enabled);
        await loadConfig();
        flash(record.message);
      });
    },
    [act, loadConfig, flash],
  );

  const setAutoStop = useCallback(
    async (enabled: boolean) => {
      await act(enabled ? "开启自动停服" : "关闭自动停服", async () => {
        const record = await commands.setAutoStop(enabled);
        await loadConfig();
        flash(record.message);
      });
    },
    [act, loadConfig, flash],
  );

  const showConsent = config !== null && !config.auto_report_prompted && !consentDone;

  const chooseConsent = async (enabled: boolean) => {
    setConsentDone(true);
    await setAutoReport(enabled);
  };

  const quit = (choice: QuitChoice) =>
    act("退出", async () => {
      await commands.quit(choice);
    });

  const applyPromptedUpdate = async () => {
    if (!updatePrompt) return;
    setUpdatePrompt(null);
    setDeployLabel("更新部署中");
    await act("更新服务端", async () => {
      const rec = await commands.serverUpdateApply();
      flash(rec.message);
    });
    setDeployLabel(null);
    void refresh();
  };

  const busy = working || status.phase === "starting" || status.phase === "stopping";
  const running = status.phase === "running";
  const canStart = readiness?.can_start ?? false;

  const current = PAGES.find((item) => item.key === page) ?? PAGES[0];

  return (
    <div className="app">
      <div className="shell">
        <Rail page={page} onSelect={setPage} />
        <div className="pane">
          <header className="topbar" data-tauri-drag-region="deep">
            <div className="brand">
              <b>{info?.name ?? "DFO-115US 启动器"}</b>
              <span>v{info?.version ?? "0.1.0"}</span>
            </div>
            <span className="chip" data-tone={TONE[status.phase]}>
              <i />
              {PHASE_LABEL[status.phase]}
            </span>
            <span
              className="chip"
              data-tone={readiness?.gateway_listening ? (running || busy ? "ok" : "err") : "ok"}
            >
              网关 7001 {readiness?.gateway_listening ? "已监听" : "空闲"}
            </span>
            {deployLabel ? (
              <span className="chip" data-tone="busy">
                <i className="spin" />
                {deployLabel}
              </span>
            ) : null}
            <span className="spacer" />
            {page === "home" ? (
              <>
                <button className="btn" onClick={openHealth}>
                  环境体检
                </button>
                <button className="btn" onClick={openPorts}>
                  端口与残留
                </button>
                <button
                  className="btn"
                  disabled={checkingSelfUpdate}
                  onClick={async () => {
                    setCheckingSelfUpdate(true);
                    try {
                      const info = await commands.selfUpdateCheck();
                      if (info.has_update) setSelfUpdatePrompt(info);
                      else flash(`当前已是最新（v${info.current_version}）`);
                    } catch (err) {
                      flash(`检查更新失败：${errText(err)}`, "err");
                    } finally {
                      setCheckingSelfUpdate(false);
                    }
                  }}
                >
                  {checkingSelfUpdate ? "检查中…" : "检查更新"}
                </button>
              </>
            ) : null}
            <WindowControls theme={theme} onToggleTheme={toggleTheme} />
          </header>

          <main className="page">
            {page === "home" ? (
              <div className="body">
                <div className="column">
                  <section className="card hero">
                    <div className="power">
                      {running ? (
                        <button className="power-btn" data-kind="stop" onClick={stopGame} disabled={working}>
                          停止游戏
                        </button>
                      ) : (
                        <button
                          className="power-btn"
                          onClick={startGame}
                          disabled={busy || !canStart}
                          title={canStart ? "" : (readiness?.detail ?? "")}
                        >
                          {status.phase === "starting"
                            ? "正在启动…"
                            : status.phase === "stopping"
                              ? "正在停止…"
                              : "开始游戏"}
                        </button>
                      )}
                      <div className="power-side">
                        <div className="power-state">{PHASE_LABEL[status.phase]}</div>
                        {status.detail !== PHASE_LABEL[status.phase] ? (
                          <div className="power-detail">{status.detail}</div>
                        ) : null}
                        {/* 启动方式挪进设置页了，这里留一行回声：按下之前就得知道会起什么。 */}
                        <div className="power-detail">
                          {launchMode === "server-only"
                            ? "本次：仅服务端（不拉起 DFO.exe，设置页可改）"
                            : "本次：原生启动（网关 + 客户端，设置页可改）"}
                        </div>
                        {busy ? (
                          <div className="row">
                            <button className="btn" data-tone="danger" onClick={stopGame} disabled={working}>
                              取消并停止
                            </button>
                          </div>
                        ) : null}
                      </div>
                    </div>

                    <div className="checklist">
                      <span className="check" data-level={readiness?.server_ready ? "ok" : "error"}>
                        <b>服务端编排</b>
                        <em>{readiness?.server_ready ? "已就绪" : (readiness?.server_detail ?? "…")}</em>
                      </span>
                      <span className="check" data-level={readiness?.client_ready ? "ok" : "error"}>
                        <b>游戏客户端</b>
                        <em>{readiness?.client_ready ? "已找到" : "未找到"}</em>
                      </span>
                      <span
                        className="check"
                        data-level={readiness?.storage_ready ? "ok" : "warn"}
                      >
                        <b>存档</b>
                        <em>{readiness?.storage_detail ?? "…"}</em>
                      </span>
                      <span
                        className="check"
                        data-level={
                          readiness?.gateway_listening && !(running || busy) ? "error" : "ok"
                        }
                      >
                        <b>网关端口</b>
                        <em>
                          {readiness?.gateway_listening
                            ? running || busy
                              ? "本会话已监听"
                              : "已被监听"
                            : "空闲"}
                        </em>
                      </span>
                    </div>
                  </section>
                </div>

                <div className="column">
                  <ControlPanel
                    config={config}
                    readiness={readiness}
                    busy={busy}
                    onAccount={async (account) =>
                      act("设置账号", async () => {
                        await commands.setAccount(account);
                        await loadConfig();
                      })
                    }
                    onPickClient={() =>
                      act("选择客户端目录", async () => {
                        await commands.pickClientDir();
                        await loadConfig();
                      })
                    }
                    onDiscoverClient={() =>
                      act("识别客户端", async () => {
                        await commands.discoverClient();
                        await loadConfig();
                      })
                    }
                  />
                </div>
              </div>
            ) : page === "settings" ? (
              <SettingsPage
                config={config}
                busy={busy}
                launchMode={launchMode}
                onLaunchMode={setLaunchMode}
                onAutoStop={setAutoStop}
                onAutoReport={setAutoReport}
                onNotice={flash}
              />
            ) : page === "tools" ? (
              <ToolsPage
                config={config}
                phase={status.phase}
                onAutoPickup={setAutoPickup}
                onNotice={flash}
              />
            ) : page === "saves" ? (
              <SavesPage phase={status.phase} onNotice={flash} />
            ) : page === "feedback" ? (
              <FeedbackPage tab={feedbackTab} onTab={setFeedbackTab} onNotice={flash} />
            ) : page === "mods" ? (
              <ModsPage phase={status.phase} onNotice={flash} />
            ) : page === "dev" ? (
              <Dock
                node={dock}
                setNode={setDock}
                floating={floating}
                hover={panelHover}
                onDetach={detachPanel}
                onReturn={(panel) => void commands.panelDock(panel)}
                hitRef={hitRef}
              />
            ) : (
              <section className="blank">
                <current.Icon size={26} />
                <b>{current.label}</b>
                <span>这一页还只是空界面，功能后续再加。</span>
              </section>
            )}
          </main>

          <footer className="statusbar">
            <span>整合包：{info?.root ?? "…"}</span>
            <span className="spacer" />
            <span>日志：{info?.log_dir ?? "…"}</span>
          </footer>
        </div>
      </div>

      {sheet === "health" ? (
        <HealthSheet
          report={health}
          onClose={() => setSheet(null)}
          onRecheck={() => void openHealth()}
        />
      ) : null}

      {sheet === "ports" ? (
        <PortsSheet
          report={ports}
          hint={portHint}
          busy={working}
          onClose={() => setSheet(null)}
          onRecheck={() => void loadPorts()}
          onForceClean={forceClean}
        />
      ) : null}

      {sheet === "quit" ? (
        <QuitSheet
          running={running || readiness?.gateway_listening === true}
          detail={status.detail}
          onChoose={quit}
          onCancel={() => setSheet(null)}
        />
      ) : null}

      {showConsent ? (
        <ReportConsentSheet onChoose={chooseConsent} onLater={() => setConsentDone(true)} />
      ) : null}

      {updatePrompt ? (
        <div className="scrim">
          <div className="sheet sheet--narrow" role="dialog" aria-modal="true">
            <header className="sheet-head">
              <h3>发现服务端新版本</h3>
              <span className="spacer" />
            </header>
            <div className="sheet-body">
              <p className="hint">
                最新版本：<strong>{updatePrompt.latest_tag}</strong>
                <br />
                本机版本：{updatePrompt.current_tag ?? "未部署"}
                <br />
                包大小：{(updatePrompt.asset_size / 1048576).toFixed(1)} MB
                <br />
                更新会自动选最快镜像下载，保留存档。建议先停止游戏再更新。
              </p>
              <div className="row">
                <button className="btn" onClick={() => setUpdatePrompt(null)}>
                  稍后再说
                </button>
                <button
                  className="btn"
                  data-tone="accent"
                  disabled={working || status.phase === "running"}
                  onClick={() => void applyPromptedUpdate()}
                  title={status.phase === "running" ? "请先停止游戏" : ""}
                >
                  立即更新
                </button>
              </div>
            </div>
          </div>
        </div>
      ) : null}

      {selfUpdatePrompt ? (
        <div className="scrim">
          <div className="sheet sheet--narrow" role="dialog" aria-modal="true">
            <header className="sheet-head">
              <h3>启动器有新版本</h3>
              <span className="spacer" />
            </header>
            <div className="sheet-body">
              <p className="hint">
                当前版本：<strong>v{selfUpdatePrompt.current_version}</strong>
                <br />
                最新版本：<strong>v{selfUpdatePrompt.latest_version}</strong>
                <br />
                包大小：{(selfUpdatePrompt.asset_size / 1048576).toFixed(1)} MB
                <br />
                更新后启动器会自动重启。
              </p>
              <div className="row">
                <button className="btn" onClick={() => setSelfUpdatePrompt(null)}>
                  稍后再说
                </button>
                <button
                  className="btn"
                  data-tone="accent"
                  onClick={async () => {
                    const info = selfUpdatePrompt;
                    setSelfUpdatePrompt(null);
                    try {
                      await commands.selfUpdateApply(info);
                      // bat 脚本已启动，等它接管
                      setTimeout(() => {
                        void commands.quit("keep");
                      }, 1500);
                    } catch (err) {
                      flash(`更新失败：${errText(err)}`, "err");
                    }
                  }}
                >
                  立即更新
                </button>
              </div>
            </div>
          </div>
        </div>
      ) : null}

      {notice ? (
        <div className="toast" data-tone={notice.tone}>
          {notice.text}
        </div>
      ) : null}
    </div>
  );
}
