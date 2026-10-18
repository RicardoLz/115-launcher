import { useCallback, useEffect, useState } from "react";
import {
  commands,
  errText,
  type ClientMods,
  type ClientPlugin,
  type LoaderStatus,
  type ModsState,
  type Phase,
  type ServerMod,
} from "../bridge";

function humanSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

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

export default function ModsPage({
  phase,
  onNotice,
}: {
  phase: Phase;
  onNotice: (text: string, tone?: "ok" | "err") => void;
}) {
  const [state, setState] = useState<ModsState | null>(null);
  const [busy, setBusy] = useState(false);

  const reload = useCallback(async () => {
    try {
      setState(await commands.modsState());
    } catch (err) {
      onNotice(errText(err), "err");
    }
  }, [onNotice]);

  useEffect(() => {
    void reload();
  }, [reload]);

  const toggle = async (mod: ServerMod) => {
    setBusy(true);
    try {
      const record = await commands.setServerMod(mod.id, !mod.enabled);
      onNotice(record.message);
      await reload();
    } catch (err) {
      onNotice(errText(err), "err");
    } finally {
      setBusy(false);
    }
  };

  const toggleClient = async (plugin: ClientPlugin) => {
    setBusy(true);
    try {
      const record = await commands.setClientMod(plugin.id, !plugin.enabled);
      onNotice(record.message);
      await reload();
    } catch (err) {
      onNotice(errText(err), "err");
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
        onNotice(
          `末日一族整组${enabled ? "启用" : "停用"}：已改 ${done.length}/${targets.length} 条（${done.join(
            "、",
          )}）${stopped ? `，停在 ${stopped}` : ""}。改完要重新「开始游戏」才生效。`,
          stopped ? "err" : undefined,
        );
      else onNotice(stopped || "这一组没有可改的条目", "err");
      await reload();
    } finally {
      setBusy(false);
    }
  };

  const importMod = async () => {
    setBusy(true);
    try {
      const record = await commands.importClientMod();
      onNotice(record.message);
      await reload();
    } catch (err) {
      onNotice(errText(err), "err");
    } finally {
      setBusy(false);
    }
  };

  const server = state?.server;
  const scripts = state?.scripts;
  const client = state?.client;
  const modkit = state?.modkit;
  const frozen = locked(phase) || busy;
  const loaded = server?.mods.filter((mod) => mod.seen).length ?? 0;
  const group = apocalypseGroup(client);
  const groupTotal = group.filter((plugin) => plugin.in_manifest).length;
  const groupOn = group.filter((plugin) => plugin.in_manifest && plugin.enabled).length;
  const groupSwitchable = client?.switchable === true && groupTotal > 0;

  return (
    <div className="body">
      <div className="column">
        <section className="card">
          <h2>服务端 mod</h2>
          <div className="row">
            <span className="chip" data-tone={loaded > 0 ? "ok" : undefined}>
              <i />
              本场装载 {loaded} 个
            </span>
            <span className="chip">
              {server?.enabled_present ? "已有停用名单" : "没有停用名单"}
            </span>
            <span className="spacer" />
            <button className="btn" data-tone="ghost" disabled={busy} onClick={() => void reload()}>
              重新扫描
            </button>
          </div>

          {server?.broken_note ? <p className="hint" data-tone="err">{server.broken_note}</p> : null}
          {server?.ignored_note ? <p className="hint" data-tone="err">{server.ignored_note}</p> : null}

          {server && server.mods.length > 0 ? (
            server.mods.map((mod) => (
              <div className="field" key={mod.id}>
                <button
                  className="switch"
                  data-on={mod.enabled ? "true" : "false"}
                  disabled={frozen}
                  onClick={() => void toggle(mod)}
                >
                  <span>
                    {mod.id}
                    <br />
                    <em className="hint">{mod.purpose || "服务端没报这一条的用途"}</em>
                  </span>
                  <span className="switch-track" />
                </button>
                <div className="row">
                  <span className="chip" data-tone={mod.seen ? "ok" : undefined}>
                    <i />
                    {mod.seen
                      ? mod.enabled
                        ? "本场确实装载了它"
                        : "本场装载了，下次启动就不再装载"
                      : mod.enabled
                        ? "本场没装载（还没跑过）"
                        : "已停用，本场未装载"}
                  </span>
                </div>
              </div>
            ))
          ) : (
            <p className="hint">
              这一场服务端没注册任何服务端 mod。先跑一次游戏再回到本页，网关报的清单会出现在这里。
            </p>
          )}

          <p className="hint">
            {frozen
              ? "服务端正开着。它只在启动时读一次这份名单，所以停服之后这里的开关才让按。"
              : server?.gateway_note
                ? `上一场日志说的：${server.gateway_note}（改完开关要重新「开始游戏」才生效）`
                : "服务端启动时读一次这份名单：改完开关要重新「开始游戏」才生效。"}
          </p>
          <p className="path">{server ? `${server.mods_dir}\\enabled.json` : "…"}</p>
        </section>

        <section className="card">
          <h2>奖励规则脚本</h2>
          <div className="row">
            <span className="chip" data-tone={(scripts?.scripts.length ?? 0) > 0 ? "ok" : undefined}>
              <i />
              盘上 {scripts?.scripts.length ?? 0} 份
            </span>
            <span className="chip" data-tone={(scripts?.provided.length ?? 0) > 0 ? "ok" : undefined}>
              <i />
              mod 自带 {scripts?.provided.length ?? 0} 份
            </span>
          </div>

          {scripts && scripts.scripts.length + scripts.provided.length > 0 ? (
            <ul className="scope">
              {scripts.provided.map((item) => (
                <li key={item}>{item}（由 mod 提供，不落盘）</li>
              ))}
              {scripts.scripts.map((file) => (
                <li key={file.name}>
                  {file.name} · {humanSize(file.size)}
                </li>
              ))}
            </ul>
          ) : null}

          <p className="hint">
            启动器不往这个目录写东西。要加减脚本就把 .lua 放进去，下次「开始游戏」时服务端自己读。
          </p>
          <p className="path">{scripts?.dir ?? "…"}</p>
        </section>
      </div>

      <div className="column">
        <section className="card">
          <h2>客户端插件</h2>
          <div className="row">
            <span className="chip" data-tone={client?.present ? undefined : "err"}>
              <i />
              {client?.present ? "插件目录在场" : "插件目录不在"}
            </span>
            <span className="chip">{hostLoadNote(client)}</span>
            <span className="chip" data-tone={client?.switchable ? "ok" : undefined}>
              <i />
              {client?.switchable ? "开关写在 mods.json" : "还没接上统一入口"}
            </span>
            <span className="chip">{client?.plugins.length ?? 0} 个</span>
            <span className="spacer" />
            <button
              className="btn"
              data-tone="ghost"
              disabled={busy || !client}
              onClick={() => void importMod()}
            >
              导入 mod…
            </button>
          </div>

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
                  <span
                    className="chip"
                    data-tone={clientLoadTone(plugin)}
                  >
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
          <p className="path">
            {client ? `${client.dir} ・ ${client.switch_dir}` : "…"}
          </p>
        </section>

        <section className="card">
          <h2>mod 安装层</h2>
          <div className="row">
            <span className="chip" data-tone={modkit?.present ? "ok" : undefined}>
              <i />
              {modkit?.present ? "有落位登记表" : "没有登记表"}
            </span>
            <span className="chip">{modkit?.total ?? 0} 个 mod</span>
            <span className="chip">{modkit?.entries ?? 0} 处落位</span>
          </div>

          {modkit && modkit.risky.length > 0 ? (
            <div className="field">
              <label>动到客户端本体的</label>
              <ul className="scope" data-tone="never">
                {modkit.risky.map((risk) => (
                  <li key={`${risk.id}-${risk.target}`}>
                    {risk.id} → {risk.target}（{risk.kind}）
                  </li>
                ))}
              </ul>
              <p className="hint" data-tone="err">
                这些件动的是客户端本体。回滚必须成对：数据文件与它的密钥文件是一对，
                汉化代理 DLL 与插件清单是一对；只还原其中半件，客户端可能直接起不来。
                要撤就用 mod 安装工具整对撤，启动器这里只报现状、不放回滚按钮。
              </p>
            </div>
          ) : null}

          <p className="path">{modkit?.registry ?? "…"}</p>
        </section>
      </div>
    </div>
  );
}
