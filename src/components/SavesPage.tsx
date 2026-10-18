import { useCallback, useEffect, useState } from "react";
import { commands, errText, type BackupItem, type SavesState } from "../bridge";
import { TrashIcon } from "./icons";

/// 服务端开着连接时整库换文件等于对着活体动底层，前后端都按这个口径拦。
function locked(phase: string): boolean {
  return phase === "starting" || phase === "running" || phase === "stopping";
}

export default function SavesPage({
  phase,
  onNotice,
}: {
  phase: string;
  onNotice: (text: string, tone?: "ok" | "err") => void;
}) {
  const [state, setState] = useState<SavesState | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [doomed, setDoomed] = useState<BackupItem | null>(null);

  const reload = useCallback(async () => {
    try {
      setState(await commands.savesState());
    } catch (err) {
      onNotice(errText(err), "err");
    }
  }, [onNotice]);

  useEffect(() => {
    void reload();
  }, [reload]);

  const backup = async () => {
    setBusy(true);
    try {
      const record = await commands.savesBackup();
      onNotice(`已备份：${record.message}`);
      await reload();
    } catch (err) {
      onNotice(errText(err), "err");
    } finally {
      setBusy(false);
    }
  };

  const restore = async () => {
    if (!picked) return;
    setBusy(true);
    try {
      const record = await commands.savesRestore(picked);
      onNotice(record.message, "ok");
      setPicked(null);
      setConfirming(false);
      await reload();
    } catch (err) {
      onNotice(errText(err), "err");
      await reload();
    } finally {
      setBusy(false);
    }
  };

  /// 删除只走确认弹窗那一步：点了「删除这份备份」先问，答「确认」才真删。
  const remove = async () => {
    if (!doomed) return;
    setBusy(true);
    try {
      const record = await commands.savesDelete(doomed.path);
      onNotice(record.message, "ok");
    } catch (err) {
      onNotice(errText(err), "err");
    } finally {
      setDoomed(null);
      setBusy(false);
      await reload();
    }
  };

  const save = state?.save;
  const frozen = locked(phase);
  const usable = state?.backups.filter((item) => item.usable).length ?? 0;
  return (
    <div className="body">
      <div className="column">
        <section className="card">
          <h2>当前存档</h2>
          <div className="row">
            <span className="chip" data-tone={save?.exists ? "ok" : "busy"}>
              <i />
              {save?.exists ? "库文件在场" : "还没有库文件"}
            </span>
            {save && save.exists ? (
              <>
                <span className="chip">{save.size_text}</span>
                <span className="chip">{save.modified}</span>
              </>
            ) : null}
            <span className="spacer" />
            <button className="btn" data-tone="ghost" disabled={busy} onClick={() => void reload()}>
              重新读取
            </button>
          </div>

          <p className="hint">
            {save?.detail ?? "…"}
          </p>
          {save && !save.exists ? (
            <p className="hint">
              服务端第一次有角色登录时才会建这个库。现在备份不了，也不用备份。
            </p>
          ) : null}

          <div className="kv">
            <span>库文件</span>
            <code>{save?.db_path ? save.db_path : "尚未定下库文件路径"}</code>
            <span>存档目录</span>
            <code>{save?.storage_dir ?? "…"}</code>
            <span>备份目录</span>
            <code>{save?.backup_dir ?? "…"}</code>
          </div>

          {save && save.sidecars.length > 0 ? (
            <p className="hint">随库文件：{save.sidecars.join("、")}</p>
          ) : null}

          <div className="field">
            <div className="row">
              <span className="spacer" />
              <button
                className="btn"
                data-tone="accent"
                disabled={busy || save?.exists !== true}
                onClick={() => void backup()}
              >
                备份一份
              </button>
            </div>
            <p className="hint">
              备份是只读地把整库另写一份到备份目录，服务端在跑也能备；不改原文件，
              也不碰 <code>local.json</code>。
            </p>
          </div>
        </section>
      </div>

      <div className="column">
        <section className="card">
          <h2>已备份的存档</h2>
          <div className="row">
            <span className="chip" data-tone={usable > 0 ? "ok" : undefined}>
              可用 {usable} 份
            </span>
            <span className="chip">共 {state?.backups.length ?? 0} 份</span>
          </div>

          {state && state.backups.length === 0 ? (
            <p className="hint">备份目录还是空的。先在左边点「备份一份」。</p>
          ) : null}

          <div className="save-list">
            {(state?.backups ?? []).map((item: BackupItem) => (
              <div className="save-row" key={item.path}>
                <button
                  className="save-item"
                  data-on={picked === item.path ? "true" : "false"}
                  disabled={!item.usable || busy}
                  title={item.usable ? "选它做替换源" : item.note}
                  onClick={() => {
                    setPicked(picked === item.path ? null : item.path);
                    setConfirming(false);
                  }}
                >
                  <b>{item.name}</b>
                  <span>
                    {item.modified} · {item.size_text} · 账号 {item.accounts} / 角色 {item.characters}
                  </span>
                  {item.note ? <em>{item.note}</em> : null}
                </button>
                <button
                  className="tool-btn save-kill"
                  aria-label={`删除备份 ${item.name}`}
                  title={item.is_current ? "这份就是当前正在用的存档，删不掉" : "删除这份备份"}
                  disabled={busy || item.is_current}
                  onClick={() => setDoomed(item)}
                >
                  <TrashIcon size={15} />
                </button>
              </div>
            ))}
          </div>

          {picked ? (
            <div className="field">
              {confirming ? (
                <>
                  <p className="hint" data-tone="err">
                    确认替换？现在这份存档会整库换成选中那份，账号与角色都跟着换。
                    替换前会先把当前那份备份进备份目录，出错也能退回去。
                  </p>
                  <div className="row">
                    <span className="spacer" />
                    <button
                      className="btn"
                      data-tone="ghost"
                      disabled={busy}
                      onClick={() => setConfirming(false)}
                    >
                      取消
                    </button>
                    <button
                      className="btn"
                      data-tone="danger"
                      disabled={busy || frozen}
                      onClick={() => void restore()}
                    >
                      {busy ? "正在替换…" : "确认替换"}
                    </button>
                  </div>
                </>
              ) : (
                <div className="row">
                  <span className="hint">已选：{picked.split(/[\\/]/).pop()}</span>
                  <span className="spacer" />
                  <button
                    className="btn"
                    data-tone="accent"
                    disabled={busy || frozen}
                    onClick={() => setConfirming(true)}
                  >
                    替换为这份
                  </button>
                </div>
              )}
            </div>
          ) : null}

          <p className="hint">
            {frozen
              ? "服务端正在用这份存档，替换要先停止服务端 —— 备份随时可以。"
              : "只能选备份目录里的文件，替换前会验文件头、跑一遍完整性检查、确认三张必备表都在。"}
          </p>
        </section>
      </div>

      {doomed ? (
        <div className="scrim" onMouseDown={() => setDoomed(null)}>
          <div
            className="sheet sheet--narrow"
            role="dialog"
            aria-modal="true"
            onMouseDown={(event) => event.stopPropagation()}
          >
            <header className="sheet-head">
              <h3>删除这份备份</h3>
              <span className="spacer" />
              <button className="btn" data-tone="ghost" disabled={busy} onClick={() => setDoomed(null)}>
                返回
              </button>
            </header>
            <div className="sheet-body">
              <p className="hint">
                要删的是 <code>{doomed.name}</code>（{doomed.size_text}，{doomed.modified}）。
              </p>
              <p className="hint" data-tone="err">
                删除不经过回收站，直接从备份目录抹掉，删了就找不回来。当前存档不受影响；
                但要是这份「替换前的自动快照」，删掉之后就退不回替换前那一面了。
              </p>
            </div>
            <footer className="sheet-foot">
              <button
                className="btn"
                data-tone="ghost"
                disabled={busy}
                onClick={() => setDoomed(null)}
              >
                取消
              </button>
              <button className="btn" data-tone="danger" disabled={busy} onClick={() => void remove()}>
                {busy ? "正在删除…" : "确认删除"}
              </button>
            </footer>
          </div>
        </div>
      ) : null}
    </div>
  );
}
