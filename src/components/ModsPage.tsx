import { useCallback, useEffect, useState } from "react";
import {
  commands,
  errText,
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

  const importServerMod = async () => {
    setBusy(true);
    try {
      const record = await commands.importServerMod();
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
  const frozen = locked(phase) || busy;
  const loaded = server?.mods.filter((mod) => mod.seen).length ?? 0;

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
            <button className="btn" data-tone="accent" disabled={busy} onClick={() => void importServerMod()}>
              导入 mod…
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
          <p className="hint">
            「导入 mod…」按内容分流：mod 里的 .lua 规则脚本拷到 {scripts?.dir ?? "…"}
            这一层（服务端启动时自己读，不用重编译），整个目录另存一份到它的子目录里。
            Go 型服务端 mod 是编译进网关二进制的，不重编译就不会装载 —— 导入只负责把它放对位置。
            已经在上面这条名单里停用的 mod 不让导入：平铺脚本会绕过停用名单，那个开关就成了假话。
          </p>
        </section>
      </div>

      <div className="column">
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
            这一层的 .lua 由服务端启动时自己读。「导入 mod…」会把 mod 自带的脚本拷进来，
            同名的一律不覆盖；要手动加减也放这里，下次「开始游戏」生效。
          </p>
          <p className="path">{scripts?.dir ?? "…"}</p>
        </section>
      </div>
    </div>
  );
}
