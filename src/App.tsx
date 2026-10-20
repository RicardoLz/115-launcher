import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import BackgroundLayer from "./components/BackgroundLayer";
import GlassDefs from "./components/GlassDefs";
import Dock from "./components/Dock";
import FeedbackPage, { type FeedbackTab } from "./components/FeedbackPage";
import LaunchDock, { type UpdateArm } from "./components/LaunchDock";
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
import { useGlideScroll } from "./glide";
import { useTheme } from "./theme";
import {
  commands,
  errText,
  events,
  PHASE_LABEL,
  type AppInfo,
  type BackgroundState,
  type ConfigView,
  type HealthReport,
  type Phase,
  type PortReport,
  type Readiness,
  type SelfUpdateCheck,
  type ServerUpdateCheck,
  type Status,
  type UpdateProgress,
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
  // 主页背景素材：路径与类型都由 Rust 侧给，这里只负责摆出来和刷新。
  const [background, setBackground] = useState<BackgroundState | null>(null);
  const [page, setPage] = useState<PageKey>("home");
  const [feedbackTab, setFeedbackTab] = useState<FeedbackTab>("board");
  // 更新日志合成一张卡，靠这页签切「客户端 / 启动器」。
  const [logTab, setLogTab] = useState<"client" | "launcher">("client");
  const [dock, setDock] = useState<DockNode | null>(DEFAULT_DOCK);
  const [floating, setFloating] = useState<Record<PanelKey, boolean>>({
    log: false,
    mode: false,
    mods: false,
    modkit: false,
    memory: false,
  });
  /// 独立窗口正被拖回主窗：这个落点交给停靠层画窗口内的吸附提示。
  const [panelHover, setPanelHover] = useState<{ panel: PanelKey; x: number; y: number } | null>(
    null,
  );
  // 首次的上传范围说明：一次会话只问一遍，没答就下次再问，不替玩家默认打开。
  const [consentDone, setConsentDone] = useState(false);
  // 更新不再一启动就弹窗：探测到结果先存着，只在「检查更新」上点一颗小红点。
  const [selfCheck, setSelfCheck] = useState<SelfUpdateCheck | null>(null);
  const [serverCheck, setServerCheck] = useState<ServerUpdateCheck | null>(null);
  // 玩家点过「检查更新」且确有新版本，右下角那枚开关才交给更新。
  const [armed, setArmed] = useState<UpdateArm | null>(null);
  const [progress, setProgress] = useState<UpdateProgress | null>(null);
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  // 部署中（首次部署 / 更新部署）的状态文字，显示在顶部状态栏。
  const [deployLabel, setDeployLabel] = useState<string | null>(null);
  const { theme, toggleTheme, scheme, setScheme, glass, setGlass } = useTheme();
  /// 停靠层把命中测试借出来：独立窗口拖回来时按落点挑吸附槽。
  const hitRef = useRef<((x: number, y: number) => Zone | null) | null>(null);
  const noticeTimer = useRef<number | null>(null);
  // 主内容区就是这一页唯一的滚动容器：滚轮与拖拽的缓冲滚动都挂在它身上。
  const pageRef = useRef<HTMLElement | null>(null);
  useGlideScroll(pageRef);

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
          // 只记账，不弹窗：小红点等玩家自己点「检查更新」。
          if (info.has_update) setServerCheck(info);
        }),
        events.onUpdateProgress((next) => setProgress(next)),
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
    take(commands.backgroundState(), setBackground);
    // 启动器自身更新：探测到新版本只记着（界面点小红点），不再自动弹对话框；失败就当没有。
    commands
      .selfUpdateCheck()
      .then((r) => {
        if (alive && r.has_update) setSelfCheck(r);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [flash]);

  // 壁纸主题色挂到根节点：后端取到就写，取不到（没背景/视频/坏图/整张灰）就摘掉，
  // 样式里每一处 var(--wall-accent, 各自原值) 会自动退回原色，所以不需要前端再兜一份默认值。
  useEffect(() => {
    const root = document.documentElement;
    const accent = background?.accent ?? "";
    if (accent === "") root.style.removeProperty("--wall-accent");
    else root.style.setProperty("--wall-accent", accent);
  }, [background]);

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

  // 「检查更新」把两条更新各问一遍，有版本可取就把它交到右下角那枚开关上。
  // 先启动器、后服务端：自更完会重启进程，服务端那一轮重启后自然还能再检。
  const checkUpdates = async () => {
    setCheckingUpdate(true);
    try {
      const self = await commands.selfUpdateCheck();
      setSelfCheck(self.has_update ? self : null);
      if (self.has_update) {
        setArmed({ kind: "self", target: `v${self.latest_version}` });
        flash(`启动器有新版本 v${self.latest_version}，按右下角「更新版本」开始`);
        return;
      }
      const server = await commands.serverUpdateCheck();
      setServerCheck(server.has_update ? server : null);
      if (server.has_update) {
        setArmed({ kind: "server", target: server.latest_tag });
        flash(`服务端有新版本 ${server.latest_tag}，按右下角「更新版本」开始`);
        return;
      }
      setArmed(null);
      flash(`当前已是最新（启动器 v${self.current_version}）`);
    } catch (err) {
      flash(`检查更新失败：${errText(err)}`, "err");
    } finally {
      setCheckingUpdate(false);
    }
  };

  const applyUpdate = async () => {
    if (!armed || progress !== null) return;
    const kind = armed.kind;
    // 先把环点上，下载事件还没来之前也不能让人重复按。
    setProgress({ kind, phase: "download", percent: 0, text: "准备下载…" });
    try {
      const record =
        kind === "self" && selfCheck
          ? await commands.selfUpdateApply(selfCheck)
          : await commands.serverUpdateApply();
      flash(record.message);
      if (kind === "self") {
        // 替换脚本正在后台等这个进程退出：界面停在「正在替换…」，由新进程接管。
        setProgress({ kind: "self", phase: "deploy", percent: 100, text: record.message });
        window.setTimeout(() => void commands.quit("keep"), 1500);
        return;
      }
      setProgress(null);
      setArmed(null);
      setServerCheck(null);
      await refresh();
    } catch (err) {
      setProgress(null);
      flash(`更新失败：${errText(err)}`, "err");
    }
  };

  // 小红点 = 探测到有版本还没处理；接管给开关之后就不再点，避免两处同时催。
  const updateDot = armed === null && (selfCheck !== null || serverCheck !== null);

  const busy = working || status.phase === "starting" || status.phase === "stopping";
  const running = status.phase === "running";
  const canStart = readiness?.can_start ?? false;
  // 按下之前就得知道会起什么，只是不再占主页一块文字，收进开关的悬停提示。
  const startHint = !canStart
    ? (readiness?.detail ?? "还没满足启动条件，先去设置页确认客户端目录")
    : launchMode === "server-only"
      ? "本次：仅服务端（不拉起 DFO.exe，设置页可改）"
      : "本次：原生启动（网关 + 客户端，设置页可改）";

  const current = PAGES.find((item) => item.key === page) ?? PAGES[0];
  const hasBackground = background !== null && background.exists && background.kind !== "none";

  return (
    <div className="app" data-bg={hasBackground ? "on" : "off"} data-page={page}>
      {glass === "on" ? <GlassDefs /> : null}
      <BackgroundLayer view={background} onProblem={(text) => flash(text, "err")} />
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
                  data-dot={updateDot ? "on" : "off"}
                  disabled={checkingUpdate || progress !== null}
                  onClick={() => void checkUpdates()}
                >
                  {checkingUpdate ? "检查中…" : "检查更新"}
                </button>
              </>
            ) : null}
            <WindowControls theme={theme} onToggleTheme={toggleTheme} />
          </header>

          <main className="page" ref={pageRef}>
            {page === "home" ? (
              <div className="home-stage">
                {/* 主页只剩背景、右下角那枚起停开关，与左下这张更新日志卡：状态与自检都收进顶栏和悬停提示里。 */}
                <section className="card log-card" aria-label="更新日志">
                  {/* 条目还没接：页签只管切「客户端」与「启动器」两块待填槽位，等谁来挂日志。 */}
                  <div className="log-tabs" role="tablist" aria-label="更新日志">
                    <button
                      type="button"
                      role="tab"
                      id="log-tab-client"
                      aria-selected={logTab === "client"}
                      aria-controls="log-panel"
                      onClick={() => setLogTab("client")}
                    >
                      客户端
                    </button>
                    <button
                      type="button"
                      role="tab"
                      id="log-tab-launcher"
                      aria-selected={logTab === "launcher"}
                      aria-controls="log-panel"
                      onClick={() => setLogTab("launcher")}
                    >
                      启动器
                    </button>
                  </div>
                  <div
                    className="log-slot"
                    role="tabpanel"
                    id="log-panel"
                    aria-labelledby={`log-tab-${logTab}`}
                  />
                </section>
              </div>
            ) : page === "settings" ? (
              <SettingsPage
                config={config}
                readiness={readiness}
                busy={busy}
                launchMode={launchMode}
                background={background}
                scheme={scheme}
                onScheme={setScheme}
                glass={glass}
                onGlass={setGlass}
                onLaunchMode={setLaunchMode}
                onAutoStop={setAutoStop}
                onAutoReport={setAutoReport}
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
                onPickBackground={async () => {
                  setWorking(true);
                  try {
                    setBackground(await commands.backgroundPick());
                  } catch (err) {
                    flash(errText(err), "err");
                  } finally {
                    setWorking(false);
                  }
                }}
                onClearBackground={async () => {
                  setWorking(true);
                  try {
                    const next = await commands.backgroundClear();
                    setBackground(next);
                    flash(next.note);
                  } catch (err) {
                    flash(errText(err), "err");
                  } finally {
                    setWorking(false);
                  }
                }}
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
            <span title={info?.root ?? ""}>整合包：{info?.root ?? "…"}</span>
            <span className="spacer" />
            <span title={info?.log_dir ?? ""}>日志：{info?.log_dir ?? "…"}</span>
          </footer>
        </div>
      </div>

      {page === "home" ? (
        <LaunchDock
          config={config}
          phase={status.phase}
          busy={busy}
          canStart={canStart}
          startHint={startHint}
          update={armed}
          progress={progress}
          onAccount={async (account) =>
            act("设置账号", async () => {
              await commands.setAccount(account);
              await loadConfig();
            })
          }
          onStart={startGame}
          onStop={stopGame}
          onUpdate={() => void applyUpdate()}
        />
      ) : null}

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

      {notice ? (
        <div className="toast" data-tone={notice.tone}>
          {notice.text}
        </div>
      ) : null}
    </div>
  );
}
