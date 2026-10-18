import { useCallback, useEffect, useState } from "react";
import {
  commands,
  errText,
  type AutofireConfig,
  type AutofireEntry,
  type AutofireLive,
  type ConfigView,
  type KeyOption,
  type ServerUpdateCheck,
} from "../bridge";

const EMPTY_CONFIG: AutofireConfig = {
  enabled: false,
  mute: false,
  hotkey: "F10",
  targets: ["dfo.exe"],
  entries: [],
};

const newRapid = (): AutofireEntry => ({
  type: "rapid",
  key: "Z",
  intervalMs: 60,
  holdMs: 15,
  trigger: "",
  steps: [],
  enabled: true,
});

/// 默认三步用旧启动器那套「 RIGHT RIGHT Z 」，改起来比从三个空格子开始快。
const newCombo = (): AutofireEntry => ({
  type: "combo",
  key: "",
  intervalMs: 0,
  holdMs: 0,
  trigger: "X",
  steps: [
    { key: "RIGHT", holdMs: 30, gapMs: 60 },
    { key: "RIGHT", holdMs: 30, gapMs: 60 },
    { key: "Z", holdMs: 30, gapMs: 60 },
  ],
  enabled: true,
});

function KeySelect({
  value,
  keys,
  disabled,
  onChange,
}: {
  value: string;
  keys: KeyOption[];
  disabled: boolean;
  onChange: (value: string) => void;
}) {
  // 玩家手填过的名字（0x58、vk_f10 这种）不在候选里也要留得住，否则一改就丢。
  const known = value === "" || keys.some((item) => item.value === value);
  return (
    <select value={value} disabled={disabled} onChange={(e) => onChange(e.target.value)}>
      {value === "" ? <option value="">（选一个键）</option> : null}
      {!known ? <option value={value}>{value}</option> : null}
      {keys.map((item) => (
        <option key={item.value} value={item.value}>
          {item.label}
        </option>
      ))}
    </select>
  );
}

function Num({
  label,
  value,
  suffix,
  min,
  max,
  disabled,
  onChange,
}: {
  label: string;
  value: number;
  suffix: string;
  min: number;
  max: number;
  disabled: boolean;
  onChange: (value: number) => void;
}) {
  return (
    <label className="num">
      <span>{label}</span>
      <input
        type="number"
        value={value}
        min={min}
        max={max}
        disabled={disabled}
        onChange={(e) => onChange(Number(e.target.value) || 0)}
      />
      <em>{suffix}</em>
    </label>
  );
}

export default function ToolsPage({
  config,
  phase,
  onAutoPickup,
  onNotice,
}: {
  config: ConfigView | null;
  phase: string;
  onAutoPickup: (enabled: boolean) => Promise<void>;
  onNotice: (text: string, tone?: "ok" | "err") => void;
}) {
  const [cfg, setCfg] = useState<AutofireConfig>(EMPTY_CONFIG);
  const [keys, setKeys] = useState<KeyOption[]>([]);
  const [live, setLive] = useState<AutofireLive | null>(null);
  const [targets, setTargets] = useState("dfo.exe");
  const [busy, setBusy] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [update, setUpdate] = useState<ServerUpdateCheck | null>(null);
  const [updateBusy, setUpdateBusy] = useState(false);

  const reload = useCallback(async () => {
    try {
      const view = await commands.autofireGet();
      setCfg(view.config);
      setKeys(view.keys);
      setLive(view.live);
      setTargets(view.config.targets.join(", "));
      setLoaded(true);
    } catch (err) {
      onNotice(errText(err), "err");
    }
  }, [onNotice]);

  useEffect(() => {
    void reload();
  }, [reload]);

  // 实况 1s 一跳：这一页要回答的是「此刻到底有没有在打」，不能只信盘上配置。
  useEffect(() => {
    const timer = window.setInterval(() => {
      commands.autofireLive().then(setLive, () => undefined);
    }, 1000);
    return () => window.clearInterval(timer);
  }, []);

  const patch = (changes: Partial<AutofireConfig>) => setCfg((prev) => ({ ...prev, ...changes }));

  const patchEntry = (index: number, changes: Partial<AutofireEntry>) =>
    setCfg((prev) => ({
      ...prev,
      entries: prev.entries.map((entry, i) => (i === index ? { ...entry, ...changes } : entry)),
    }));

  const patchStep = (index: number, stepIndex: number, changes: Partial<AutofireEntry["steps"][number]>) =>
    setCfg((prev) => ({
      ...prev,
      entries: prev.entries.map((entry, i) =>
        i === index
          ? {
              ...entry,
              steps: entry.steps.map((step, s) => (s === stepIndex ? { ...step, ...changes } : step)),
            }
          : entry,
      ),
    }));

  const save = async (over?: Partial<AutofireConfig>) => {
    const parsed = targets
      .split(",")
      .map((item) => item.trim().toLowerCase())
      .filter((item) => item.length > 0);
    const body: AutofireConfig = { ...cfg, ...over, targets: parsed.length > 0 ? parsed : ["dfo.exe"] };
    setBusy(true);
    try {
      const record = await commands.autofireSave(body);
      onNotice(record.message);
      await reload();
    } catch (err) {
      onNotice(errText(err), "err");
    } finally {
      setBusy(false);
    }
  };

  const stopAll = async () => {
    setBusy(true);
    try {
      const record = await commands.autofireStop();
      onNotice(record.message);
      patch({ enabled: false });
      setLive(await commands.autofireLive());
    } catch (err) {
      onNotice(errText(err), "err");
    } finally {
      setBusy(false);
    }
  };

  const checkUpdate = async () => {
    setUpdateBusy(true);
    try {
      const info = await commands.serverUpdateCheck();
      setUpdate(info);
      if (!info.has_update) {
        onNotice(`已是最新（${info.latest_tag}）`);
      }
    } catch (err) {
      onNotice(errText(err), "err");
    } finally {
      setUpdateBusy(false);
    }
  };

  const applyUpdate = async () => {
    setUpdateBusy(true);
    try {
      const record = await commands.serverUpdateApply();
      onNotice(record.message);
      // 重新查一次，刷新版本号显示。
      try {
        setUpdate(await commands.serverUpdateCheck());
      } catch {
        /* 不阻塞主流程 */
      }
    } catch (err) {
      onNotice(errText(err), "err");
    } finally {
      setUpdateBusy(false);
    }
  };

  const frozen = busy || !loaded;
  const on = live?.running === true;
  // 界面那个开关显示的就是引擎里那个总开关：跑着的时候听 live，没跑的时候听盘上的意图。
  const armed = live ? live.running && live.master : cfg.enabled;

  const toggleEnable = async () => {
    if (!live?.running) {
      // 还没挂监听：点一下直接按新意图保存并生效，别留「显示开着一个没跑的东西」。
      await save({ enabled: !armed });
      return;
    }
    const next = !armed;
    setBusy(true);
    try {
      // 已经在跑：这一等价于按一下热键，翻的是同一个状态。
      setLive(await commands.autofireSetMaster(next));
      patch({ enabled: next });
      onNotice(next ? "连发已打开" : "连发已关闭", "ok");
    } catch (err) {
      onNotice(errText(err), "err");
    } finally {
      setBusy(false);
    }
  };
  // 三个计数各管一段：钩子收没收到、发没发出去、被哪道闸挡下。照这句就知道该动哪里。
  const verdict = (() => {
    if (!live || !live.running) return null;
    if (live.hook_events === 0)
      return "启动器一个物理按键都没收到：多半是游戏以更高权限运行，请以「以管理员身份运行」重开启动器。";
    if (!live.master) return `总开关是关的：按一下 ${live.hotkey} 或点上面「启用连发」才开（按住不放不会来回跳）。`;
    if (live.sends === 0 && live.skip_focus > 0)
      return `按键收到了，但游戏不在前台：最前面的窗口得属于 ${live.targets.join("、")}。`;
    if (live.send_fails > 0) return live.last_fail;
    if (live.sends === 0) return "按键收到了，但还没按下配置里那个键：进游戏按住它才会连发。";
    return null;
  })();

  return (
    <div className="body">
      <div className="column">
        <section className="card">
          <h2>连发与一键搓招</h2>

          <div className="field">
            <button
              className="switch"
              data-on={armed ? "true" : "false"}
              disabled={frozen}
              onClick={() => void toggleEnable()}
            >
              <span>
                启用连发
                <br />
                <em className="hint">这一点就是总开关：开着才连发，热键 {cfg.hotkey || "F10"} 翻的也是它</em>
              </span>
              <span className="switch-track" />
            </button>
            <p className="hint">
              中文输入法（拼音）下长按连发键会掉帧：合成器要把收到的每一个键都再算一遍，我们注入的那份也算。
              连发期间请把输入法切到英文档（ENG），停下再切回来。启动器不会替你改输入法。
            </p>
          </div>

          <div className="field">
            <label>总开关热键（按一下关、再按一下开）</label>
            <KeySelect
              value={cfg.hotkey}
              keys={keys}
              disabled={frozen}
              onChange={(value) => patch({ hotkey: value })}
            />
            <p className="hint">
              这个键翻的就是上面那个开关，两个是同一个状态：开着按一下变关，关着按一下变开，界面最迟一秒跟上。
              一次按键只翻一次，按住不放不会来回跳。这个键被启动器吃掉、不会传进游戏，所以别选成技能键；
              被游戏先绑走时保存会提示换键。每次翻都会在运行日志里记一句「连发总开关已开 / 已关」。
            </p>
          </div>

          <div className="field">
            <label>目标进程</label>
            <input
              type="text"
              value={targets}
              disabled={frozen}
              onChange={(e) => setTargets(e.target.value)}
            />
            <p className="hint">逗号分隔的进程名。只有焦点在这些进程上时才发键，切回启动器就自动停。</p>
          </div>

          <div className="field">
            <button
              className="switch"
              data-on={cfg.mute ? "true" : "false"}
              disabled={frozen}
              onClick={() => patch({ mute: !cfg.mute })}
            >
              <span>
                切换时不响提示音
                <br />
                <em className="hint">响一声是为了确认热键真被接住了</em>
              </span>
              <span className="switch-track" />
            </button>
          </div>
        </section>

        <section className="card">
          <h2>条目</h2>
          {cfg.entries.length === 0 ? (
            <p className="hint">还没有条目。先加一条连发，或加一套连招。</p>
          ) : (
            cfg.entries.map((entry, index) => (
              <div className="af-entry" key={index}>
                <div className="row">
                  <span className="af-index">{index + 1}</span>
                  <select
                    value={entry.type}
                    disabled={frozen}
                    onChange={(e) =>
                      patchEntry(index, { type: e.target.value as AutofireEntry["type"] })
                    }
                  >
                    <option value="rapid">按住连发</option>
                    <option value="combo">一键连招</option>
                  </select>
                  <button
                    className="btn"
                    data-tone="ghost"
                    disabled={frozen}
                    onClick={() =>
                      patch({ entries: cfg.entries.filter((_, i) => i !== index) })
                    }
                  >
                    删除
                  </button>
                  <span className="spacer" />
                  <button
                    className="btn"
                    data-tone={entry.enabled ? undefined : "ghost"}
                    disabled={frozen}
                    title={entry.enabled ? "这一条生效中，点击停用" : "这一条先存着不生效，点击启用"}
                    onClick={() => patchEntry(index, { enabled: !entry.enabled })}
                  >
                    {entry.enabled ? "生效中" : "已停用"}
                  </button>
                </div>

                {entry.type === "rapid" ? (
                  <div className="af-grid">
                    <label className="af-cell">
                      <span>连发键</span>
                      <KeySelect
                        value={entry.key}
                        keys={keys}
                        disabled={frozen}
                        onChange={(value) => patchEntry(index, { key: value })}
                      />
                    </label>
                    <Num
                      label="间隔"
                      value={entry.intervalMs}
                      suffix="ms"
                      min={10}
                      max={1000}
                      disabled={frozen}
                      onChange={(value) => patchEntry(index, { intervalMs: value })}
                    />
                    <Num
                      label="每次按下"
                      value={entry.holdMs}
                      suffix="ms"
                      min={1}
                      max={1000}
                      disabled={frozen}
                      onChange={(value) => patchEntry(index, { holdMs: value })}
                    />
                  </div>
                ) : (
                  <>
                    <div className="af-grid">
                      <label className="af-cell">
                        <span>触发键</span>
                        <KeySelect
                          value={entry.trigger}
                          keys={keys}
                          disabled={frozen}
                          onChange={(value) => patchEntry(index, { trigger: value })}
                        />
                      </label>
                    </div>
                    {entry.steps.map((step, stepIndex) => (
                      <div className="af-step" key={stepIndex}>
                        <span className="af-index">{stepIndex + 1}</span>
                        <KeySelect
                          value={step.key}
                          keys={keys}
                          disabled={frozen}
                          onChange={(value) => patchStep(index, stepIndex, { key: value })}
                        />
                        <Num
                          label="按下"
                          value={step.holdMs}
                          suffix="ms"
                          min={1}
                          max={500}
                          disabled={frozen}
                          onChange={(value) => patchStep(index, stepIndex, { holdMs: value })}
                        />
                        <Num
                          label="隔"
                          value={step.gapMs}
                          suffix="ms"
                          min={0}
                          max={1000}
                          disabled={frozen}
                          onChange={(value) => patchStep(index, stepIndex, { gapMs: value })}
                        />
                        <span className="spacer" />
                        <button
                          className="btn"
                          data-tone="ghost"
                          disabled={frozen}
                          onClick={() =>
                            patchEntry(index, {
                              steps: entry.steps.filter((_, s) => s !== stepIndex),
                            })
                          }
                        >
                          去掉这动
                        </button>
                      </div>
                    ))}
                    <div className="row">
                      <button
                        className="btn"
                        data-tone="ghost"
                        disabled={frozen}
                        onClick={() =>
                          patchEntry(index, {
                            steps: [...entry.steps, { key: "Z", holdMs: 30, gapMs: 60 }],
                          })
                        }
                      >
                        加一动
                      </button>
                    </div>
                  </>
                )}
              </div>
            ))
          )}

          <div className="row">
            <button
              className="btn"
              data-tone="ghost"
              disabled={frozen}
              onClick={() => patch({ entries: [...cfg.entries, newRapid()] })}
            >
              加一条连发
            </button>
            <button
              className="btn"
              data-tone="ghost"
              disabled={frozen}
              onClick={() => patch({ entries: [...cfg.entries, newCombo()] })}
            >
              加一套连招
            </button>
            <span className="spacer" />
            <button className="btn" disabled={frozen} onClick={() => void stopAll()}>
              立即停用
            </button>
            <button
              className="btn"
              data-tone="accent"
              disabled={frozen}
              onClick={() => void save()}
            >
              保存并生效
            </button>
          </div>
          <p className="hint">
            间隔与按下时长超出可用范围时，保存那一步会按上下限收回来，不会原样写进配置。
          </p>
        </section>
      </div>

      <div className="column">
        <section className="card">
          <h2>本场实况</h2>
          <div className="row">
            <span className="chip" data-tone={on ? "ok" : undefined}>
              <i />
              {on ? "监听已挂上" : "未监听"}
            </span>
            <span className="chip" data-tone={live?.master ? "ok" : "busy"}>
              总开关 {live?.master ? "开" : `关 · 按一下 ${live?.hotkey || "热键"} 开`}
            </span>
            <span className="chip" data-tone={live?.focused ? "ok" : undefined}>
              {live?.focused ? "焦点在游戏" : "焦点不在游戏"}
            </span>
          </div>
          <div className="row">
            <span className="chip">收到物理键 {live?.hook_events ?? 0}</span>
            <span className="chip" data-tone={(live?.sends ?? 0) > 0 ? "ok" : undefined}>
              注入 {live?.sends ?? 0}
            </span>
            {(live?.send_fails ?? 0) > 0 ? (
              <span className="chip" data-tone="err">注入失败 {live?.send_fails}</span>
            ) : null}
            {(live?.skip_focus ?? 0) + (live?.skip_master ?? 0) > 0 ? (
              <span className="chip" data-tone="busy">
                按住被挡 {(live?.skip_focus ?? 0) + (live?.skip_master ?? 0)}
              </span>
            ) : null}
          </div>
          {verdict ? (
            <p className="hint" data-tone="err">
              {verdict}
            </p>
          ) : null}
          {live?.hotkey_error ? <p className="hint" data-tone="err">{live.hotkey_error}</p> : null}
          {live && live.pids.length > 0 ? (
            <p className="hint">目标进程 PID：{live.pids.join("、")}</p>
          ) : (
            <p className="hint">现在没有目标进程在跑，发键条件不成立。</p>
          )}

          {live && live.entries.length > 0 ? (
            <table className="af-live">
              <thead>
                <tr>
                  <th>条目</th>
                  <th>状态</th>
                  <th>已发</th>
                </tr>
              </thead>
              <tbody>
                {live.entries.map((item) => (
                  <tr key={`${item.kind}-${item.label}`}>
                    <td>
                      {item.kind === "combo" ? "连招" : "连发"} 第 {item.label + 1} 条
                    </td>
                    <td>
                      {item.kind === "combo"
                        ? item.busy
                          ? "正在打"
                          : "空"
                        : item.held
                          ? "按着"
                          : "空"}
                    </td>
                    <td>{item.taps}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : null}
          <p className="hint">
            只有在焦点落到目标进程、且总开关开着时才计数；数字只活在这一场，重启启动器归零。
          </p>
        </section>

        <section className="card">
          <h2>服务端更新</h2>
          <p className="hint">
            从 GitHub Release 拉取最新服务端（dfolauncher / wireprobe / 配置），自动测速选最快镜像，
            保留 runtime\storage\ 存档。更新前请先停止服务端。
          </p>
          {update ? (
            <div className="field">
              <p className="hint">
                本机版本：<strong>{update.current_tag ?? "未部署"}</strong>
                <br />
                最新版本：<strong>{update.latest_tag}</strong>
                <br />
                包大小：{(update.asset_size / 1048576).toFixed(1)} MB（{update.asset_name}）
              </p>
            </div>
          ) : null}
          <div className="row">
            <button className="btn" disabled={updateBusy} onClick={() => void checkUpdate()}>
              {updateBusy ? "检查中…" : "检查更新"}
            </button>
            {update?.has_update ? (
              <button className="btn" data-tone="accent" disabled={updateBusy} onClick={() => void applyUpdate()}>
                {updateBusy ? "下载部署中…" : `更新到 ${update.latest_tag}`}
              </button>
            ) : null}
          </div>
          <p className="hint">
            下载与部署的详细进度会出现在「运行日志」面板。部署完成后下次点「开始游戏」即生效。
          </p>
        </section>

        <section className="card">
          <h2>游戏内辅助</h2>
          <div className="field">
            <button
              className="switch"
              data-on={config?.auto_pickup ? "true" : "false"}
              disabled={phase === "starting" || phase === "running" || phase === "stopping"}
              onClick={() => void onAutoPickup(!config?.auto_pickup)}
            >
              <span>
                自动拾取
                <br />
                <em className="hint">服务端侧的开关，只在本机回环内生效；下次「开始游戏」时才换</em>
              </span>
              <span className="switch-track" />
            </button>
          </div>
          <p className="hint">
            它不是按键连打：真正的拾取由服务端决定，启动器只是把它作为环境变量交给网关进程，
            所以运行中不让改，改了也不会立刻变。
          </p>
        </section>
      </div>
    </div>
  );
}
