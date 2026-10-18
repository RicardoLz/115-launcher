import { memo, useCallback, useEffect, useState } from "react";
import { PanelFrame } from "./PanelFrame";
import {
  commands,
  errText,
  events,
  type ClientMods,
  type ClientPlugin,
  type LoaderStatus,
  type ModsState,
  type Phase,
} from "../bridge";

/** 这一代服务端只在启动时读一次名单，所以只有停下来时才允许改。 */
function locked(phase: Phase): boolean {
  return phase === "starting" || phase === "running" || phase === "stopping";
}

/** 宿主的口径：认不出形状的清单等同于没有清单，那时盘上每个 DLL 都会被加载。 */
function hostLoadNote(client: ClientMods | undefined): string {
  if (!client) return "…";
  if (client.manifest === "corrupt") return "plugins.json 认不出：盘上 DLL 全部加载";
  if (client.manifest === "missing") return "没有 plugins.json：盘上 DLL 全部加载";
  return client.auto_discover ? "清单外的 DLL 也会被加载" : "只加载清单里列的";
}

/** 统一入口上次启动到底装了几条：这是「勾有没有落地」的唯一实据。 */
function loaderNote(loader: LoaderStatus): string {
  if (!loader.seen) return "统一入口还没写过状态：这一侧还没跑过迁移，或客户端还没启动过。";
  if (loader.ready === false)
    return `统一入口自检未过：${loader.reject_reason || "原因未写"}（它自己没装上，下面这些勾都不会生效）`;
  return `统一入口上次声明 ${loader.declared} 条、装上 ${loader.started} 条、失败 ${loader.failed} 条`;
}

function clientLoadNote(plugin: ClientPlugin): string {
  if (plugin.loaded === true && plugin.start_return !== 0)
    return `装进进程了，但入口拒绝启动：${
      plugin.load_note || `ModStart 返回 ${plugin.start_return}`
    }`;
  if (plugin.loaded === true) return "上次启动确实装上了";
  if (plugin.loaded === false)
    return plugin.load_note ? `上次启动没装上：${plugin.load_note}` : "上次启动没装上";
  return "还没跑过（统一入口没记账）";
}

/** 「装上了」不等于「在干活」：入口返回非 0 的那条要标红，别让它混在 ok 里。 */
function clientLoadTone(plugin: ClientPlugin): "ok" | "err" | undefined {
  if (plugin.loaded !== true) return undefined;
  return plugin.start_return === 0 ? "ok" : "err";
}

/**
 * `ready` 是各 mod 自己算的行为计数，不是安装状态：入口返回 0、补丁在跑，
 * 但这局没玩到用它的那段内容时它同样是 false 且不带原因。把这一种和红分开。
 */
function selfCheckNote(
  plugin: ClientPlugin,
): { text: string; tone: "err" | undefined } {
  if (plugin.reject_reason) return { text: `自检被拒：${plugin.reject_reason}`, tone: "err" };
  if (plugin.loaded === true && plugin.start_return === 0)
    return { text: "已装载，这场没触发它的代码", tone: undefined };
  return { text: "自检未过：原因未写", tone: "err" };
}

/** 清单里没有分组字段，所以「一族」按 id 前缀取：这一代就是末日那四条。 */
function apocalypseGroup(client: ClientMods | undefined): ClientPlugin[] {
  return (client?.plugins ?? []).filter((plugin) => plugin.id.startsWith("apocalypse-"));
}

/// 没有 props：这一块的盘上事实和「现在能不能改」都自己取，
/// 停靠层重排、拖成独立窗口两条路共用同一份状态。
export default memo(function ModsPanel() {
  const [state, setState] = useState<ModsState | null>(null);
  const [phase, setPhase] = useState<Phase>("idle");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ text: string; tone: "ok" | "err" } | null>(null);

  const reload = useCallback(async () => {
    try {
      setState(await commands.modsState());
    } catch (err) {
      setMessage({ text: errText(err), tone: "err" });
    }
  }, []);

  // phase 决定闸门：面板可能在浮出的独立窗口里，那里收不到主窗的 props，只能自己订。
  useEffect(() => {
    let alive = true;
    let stop: (() => void) | null = null;
    commands
      .status()
      .then((got) => {
        if (alive) setPhase(got.phase);
      })
      .catch(() => {});
    void events
      .onPhase((got) => setPhase(got.phase))
      .then((unlisten) => {
        if (alive) stop = unlisten;
        else unlisten();
      });
    return () => {
      alive = false;
      stop?.();
    };
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const say = (text: string, tone: "ok" | "err" = "ok") => setMessage({ text, tone });

  const toggleClient = async (plugin: ClientPlugin) => {
    setBusy(true);
    try {
      const record = await commands.setClientMod(plugin.id, !plugin.enabled);
      say(record.message);
      await reload();
    } catch (err) {
      say(errText(err), "err");
    } finally {
      setBusy(false);
    }
  };

  /** 整组同开同关：逐条 await 现有命令，mods.json 一次只被一个人改。 */
  const setGroup = async (members: ClientPlugin[], enabled: boolean) => {
    const targets = members.filter((m) => m.in_manifest && m.enabled !== enabled);
    if (targets.length === 0) return;
    setBusy(true);
    const done: string[] = [];
    let stopped = "";
    try {
      for (const member of targets) {
        try {
          await commands.setClientMod(member.id, enabled);
          done.push(member.id);
        } catch (err) {
          stopped = `${member.id}：${errText(err)}`;
          break;
        }
      }
      if (done.length > 0)
        say(
          `末日一族整组${enabled ? "启用" : "停用"}：已改 ${done.length}/${targets.length} 条（${done.join(
            "、",
          )}）${stopped ? `，停在 ${stopped}` : ""}。改完要重新「开始游戏」才生效。`,
          stopped ? "err" : "ok",
        );
      else say(stopped || "这一组没有可改的条目", "err");
      await reload();
    } finally {
      setBusy(false);
    }
  };

  const importMod = async () => {
    setBusy(true);
    try {
      const record = await commands.importClientMod();
      say(record.message);
      await reload();
    } catch (err) {
      say(errText(err), "err");
    } finally {
      setBusy(false);
    }
  };

  const client = state?.client;
  const frozen = locked(phase) || busy;
  const group = apocalypseGroup(client);
  const groupTotal = group.filter((plugin) => plugin.in_manifest).length;
  const groupOn = group.filter((plugin) => plugin.in_manifest && plugin.enabled).length;
  const groupSwitchable = client?.switchable === true && groupTotal > 0;

  return (
    <PanelFrame
      extra={
        <>
          <span className="chip" data-tone={client?.present ? undefined : "err"}>
            <i />
            {client?.present ? "插件目录在场" : "插件目录不在"}
          </span>
          <span className="chip">{client?.plugins.length ?? 0} 个</span>
        </>
      }
    >
      <div className="mods-body">
        <div className="row">
          <span className="chip">{hostLoadNote(client)}</span>
          <span className="chip" data-tone={client?.switchable ? "ok" : undefined}>
            <i />
            {client?.switchable ? "开关写在 mods.json" : "还没接上统一入口"}
          </span>
          <span className="spacer" />
          <button className="btn" data-tone="ghost" disabled={busy} onClick={() => void reload()}>
            重新扫描
          </button>
          <button
            className="btn"
            data-tone="ghost"
            disabled={busy || !client}
            onClick={() => void importMod()}
          >
            导入 mod…
          </button>
        </div>

        {message ? <p className="hint" data-tone={message.tone === "err" ? "err" : undefined}>{message.text}</p> : null}
        {client?.bypass_note ? <p className="hint" data-tone="err">{client.bypass_note}</p> : null}

        {groupTotal > 0 ? (
          <div className="row">
            <span className="chip" data-tone={groupOn === groupTotal ? "ok" : undefined}>
              <i />
              末日一族 {groupOn}/{groupTotal} 条开着
            </span>
            <button
              className="btn"
              data-tone="ghost"
              disabled={frozen || !groupSwitchable || groupOn === groupTotal}
              onClick={() => void setGroup(group, true)}
            >
              本组全开
            </button>
            <button
              className="btn"
              data-tone="ghost"
              disabled={frozen || !groupSwitchable || groupOn === 0}
              onClick={() => void setGroup(group, false)}
            >
              本组全关
            </button>
            <span className="spacer" />
          </div>
        ) : null}

        {groupTotal > 0 ? (
          <p className="hint">
            整组开是给「hard 要等它依赖的那两条先起」用的：单开 hard、依赖没开，它会明确拒绝并把原因写进下面那条。
            清单上 entrypoint=false 的那条（如 third-card）开着也只是加载进进程，不会调它的入口点。
          </p>
        ) : null}

        {client && client.plugins.length > 0 ? (
          client.plugins.map((plugin) => {
            const check = selfCheckNote(plugin);
            return (
              <div className="field" key={`${plugin.id}#${plugin.dll}`}>
                <button
                  className="switch"
                  data-on={plugin.enabled ? "true" : "false"}
                  disabled={frozen || !client.switchable || !plugin.in_manifest}
                  onClick={() => void toggleClient(plugin)}
                >
                  <span>
                    {plugin.id}
                    {plugin.in_manifest ? "" : "（清单外）"}
                    <br />
                    <em className="hint">{plugin.description || "这条没有内置说明"}</em>
                  </span>
                  <span className="switch-track" />
                </button>
                <div className="row">
                  <span className="chip" data-tone={clientLoadTone(plugin)}>
                    <i />
                    {clientLoadNote(plugin)}
                  </span>
                  {plugin.ready === true ? <span className="chip">自检就绪</span> : null}
                  {plugin.ready === false ? (
                    <span className="chip" data-tone={check.tone}>
                      {check.text}
                    </span>
                  ) : null}
                  {plugin.deps.length > 0 ? (
                    <span className="chip">先加载 {plugin.deps.join("、")}</span>
                  ) : null}
                  <span className="chip">{plugin.dll}</span>
                </div>
              </div>
            );
          })
        ) : (
          <p className="hint">
            {client && !client.present
              ? "插件目录不在：客户端这一侧没有 mod 插件。"
              : "两个根里一个 DLL 都没有：客户端这一侧没有 mod 插件。"}
          </p>
        )}

        {client && client.host_tail.length > 0 ? (
          <div className="field">
            <label>客户端宿主日志尾部</label>
            <ul className="scope">
              {client.host_tail.map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
          </div>
        ) : null}

        <p className="hint">{client ? loaderNote(client.loader) : "…"}</p>
        <p className="hint">
          {client?.switchable
            ? "这里的勾写的是 <客户端>\\mods\\mods.json：改完要重新「开始游戏」才生效，客户端只在启动时读一次这份清单。"
            : "这个整合包还没接上统一入口，所以这里只列不放开关。跑 analysis/tasks/mod-loader-unify-20261016 的迁移之后这里就能勾了。"}
        </p>
        <p className="path">{client ? `${client.dir} ・ ${client.switch_dir}` : "…"}</p>
      </div>
    </PanelFrame>
  );
});
