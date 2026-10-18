import { useState, type CSSProperties } from "react";
import { scenarios } from "./scenarios";
import { snapshot } from "./probe";
import { st } from "./sim";
import { suite } from "./driver";
import { cases } from "./cases";

const PANEL_ID = "sandbox-panel";

const box: CSSProperties = {
  position: "fixed",
  top: 8,
  right: 8,
  bottom: 8,
  width: 320,
  zIndex: 2147483000,
  overflow: "auto",
  padding: "10px 12px",
  background: "rgba(12,14,18,0.94)",
  border: "1px solid #2a2f3a",
  borderRadius: 10,
  color: "#d7dce6",
  font: "12px/1.5 ui-monospace, Consolas, monospace",
};

const groupTitle: CSSProperties = {
  margin: "12px 0 4px",
  color: "#8b93a7",
  letterSpacing: "0.08em",
};

const button: CSSProperties = {
  display: "block",
  width: "100%",
  margin: "3px 0",
  padding: "5px 7px",
  textAlign: "left",
  background: "#171b22",
  border: "1px solid #2a2f3a",
  borderRadius: 6,
  color: "#e6ebf5",
  font: "inherit",
  cursor: "pointer",
};

export default function Panel() {
  // ?panel=0 默认收起：无头截图点不到「隐藏沙盒面板」，而面板会挡住右半张界面。
  const [open, setOpen] = useState(() => new URLSearchParams(window.location.search).get("panel") !== "0");
  const [last, setLast] = useState("");
  const [running, setRunning] = useState(false);
  const groups = Array.from(new Set(scenarios.map((item) => item.group)));

  const run = (key: string) => {
    const item = scenarios.find((scenario) => scenario.key === key);
    if (!item) return;
    item.run();
    setLast(`${key} → ${JSON.stringify(snapshot().toggle)}`);
  };

  return (
    <>
      <button
        id={PANEL_ID + "-toggle"}
        style={{
          position: "fixed",
          top: 8,
          right: 8,
          zIndex: 2147483001,
          padding: "4px 8px",
          background: "#171b22",
          border: "1px solid #2a2f3a",
          borderRadius: 6,
          color: "#d7dce6",
          font: "12px ui-monospace, Consolas, monospace",
          cursor: "pointer",
        }}
        onClick={() => setOpen((next) => !next)}
      >
        {open ? "隐藏沙盒面板" : "显示沙盒面板"}
      </button>
      {open ? (
        <div style={box}>
          <div style={{ marginTop: 26, color: "#8b93a7" }}>
            沙盒后端 ≠ Rust 后端：这里只证明界面在各种返回值下的表现。
          </div>
          {groups.map((group) => (
            <div key={group}>
              <div style={groupTitle}>{group}</div>
              {scenarios
                .filter((item) => item.group === group)
                .map((item) => (
                  <button
                    key={item.key}
                    style={button}
                    title={item.hint}
                    onClick={() => run(item.key)}
                  >
                    <b>{item.label}</b>
                    <span style={{ color: "#727a8c" }}> — {item.hint}</span>
                  </button>
                ))}
            </div>
          ))}
          <div style={groupTitle}>剧本用例</div>
          <button
            style={button}
            onClick={() => {
              setRunning(true);
              setLast(`跑 ${cases.length} 条用例中…`);
              void suite(cases).then(
                (report) => {
                  const bad = Object.entries(report).filter(([, trace]) => trace.some((row) => row.error));
                  setLast(
                    bad.length === 0
                      ? `全部 ${Object.keys(report).length} 条通过`
                      : bad.map(([name, trace]) => name + " -> " + trace.filter((row) => row.error).map((row) => String(row.error)).join("; ")).join(" / "),
                  );
                  setRunning(false);
                },
                (err) => {
                  setLast("跑挂了：" + String(err));
                  setRunning(false);
                },
              );
            }}
          >
            {running ? "用例跑动中…" : `跑全部用例（${cases.length} 条）`}
          </button>
          <div style={groupTitle}>读当前状态</div>
          <button style={button} onClick={() => setLast(JSON.stringify(snapshot(), null, 1))}>
            snapshot()
          </button>
          <button
            style={button}
            onClick={() =>
              setLast(
                JSON.stringify(
                  {
                    phase: st.phase,
                    can_start: st.can_start,
                    self_latest: st.self_latest,
                    server_latest: st.server_latest,
                    update_mode: st.update_mode,
                    log: st.log.slice(0, 8),
                  },
                  null,
                  1,
                ),
              )
            }
          >
            后端状态 + 最近命令
          </button>
          <pre style={{ whiteSpace: "pre-wrap", wordBreak: "break-all", color: "#9fe8b0" }}>{last}</pre>
        </div>
      ) : null}
    </>
  );
}
