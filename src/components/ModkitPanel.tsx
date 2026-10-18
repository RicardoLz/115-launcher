import { memo, useCallback, useEffect, useState } from "react";
import { PanelFrame } from "./PanelFrame";
import { commands, errText, type ModkitMods, type ModsState } from "../bridge";

/// 没有 props：这块只有盘上事实，停靠层重排与拖成独立窗口共用同一条取数路径。
export default memo(function ModkitPanel() {
  const [state, setState] = useState<ModsState | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ text: string; tone: "ok" | "err" } | null>(null);

  const reload = useCallback(async () => {
    try {
      setState(await commands.modsState());
    } catch (err) {
      setMessage({ text: errText(err), tone: "err" });
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const say = (text: string, tone: "ok" | "err" = "ok") => setMessage({ text, tone });

  const openDir = async () => {
    setBusy(true);
    try {
      const record = await commands.openModkitDir();
      say(record.message);
    } catch (err) {
      say(errText(err), "err");
    } finally {
      setBusy(false);
    }
  };

  const modkit: ModkitMods | undefined = state?.modkit;

  return (
    <PanelFrame
      extra={
        <>
          <span className="chip" data-tone={modkit?.present ? "ok" : undefined}>
            <i />
            {modkit?.present ? "有落位登记表" : "没有登记表"}
          </span>
          <span className="chip">{modkit?.total ?? 0} 个 mod</span>
          <span className="chip">{modkit?.entries ?? 0} 处落位</span>
        </>
      }
    >
      <div className="mods-body">
        <div className="row">
          <span className="spacer" />
          <button className="btn" data-tone="ghost" disabled={busy} onClick={() => void reload()}>
            重新扫描
          </button>
          <button
            className="btn"
            data-tone="ghost"
            disabled={busy || !modkit?.present}
            onClick={() => void openDir()}
          >
            打开所在文件夹
          </button>
        </div>

        {message ? (
          <p className="hint" data-tone={message.tone === "err" ? "err" : undefined}>
            {message.text}
          </p>
        ) : null}

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
        ) : (
          <p className="hint">
            这里只读 modkit 自己写的落位登记表，启动器不碰它，也不放回滚按钮。
          </p>
        )}

        <p className="path">{modkit?.registry ?? "…"}</p>
      </div>
    </PanelFrame>
  );
});
