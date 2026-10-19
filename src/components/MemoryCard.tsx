import { useCallback, useEffect, useMemo, useState } from "react";
import {
  commands,
  errText,
  type MemoryConfig,
  type MemoryLive,
  type MemoryTuning,
} from "../bridge";

type Key = keyof MemoryTuning;

/** 三格各自的口径写在一处：硬上限是「超了就杀」，Go 目标只是「回收勤快点」。 */
const FIELDS: { key: Key; label: string; unit: string; blank: string; note: string }[] = [
  {
    key: "server_memory_limit_mib",
    label: "服务端提交内存硬上限",
    unit: "MiB",
    blank: "不限",
    note: "超过它，Windows 直接把这个进程杀掉",
  },
  {
    key: "go_mem_target_mib",
    label: "Go 运行内存目标",
    unit: "MiB",
    blank: "不限",
    note: "软约束：只让回收更早动手，不是上限",
  },
  {
    key: "import_max_procs",
    label: "导入并发上限",
    unit: "个",
    blank: "默认",
    note: "留空就按本机逻辑核心数",
  },
];

/** 空与 0 都是「不限」；其余只能是正整数，且不超过编排器认得的边界。 */
function parseField(
  key: Key,
  text: string,
  cfg: MemoryConfig | null,
): { value: number | null } | { error: string } {
  const trimmed = text.trim();
  if (trimmed === "" || trimmed === "0") return { value: null };
  if (!/^\d+$/.test(trimmed)) return { error: "只能填整数（留空或 0 = 不限）" };
  const value = Number(trimmed);
  const max = key === "import_max_procs" ? (cfg?.max_import_procs ?? 65536) : (cfg?.max_memory_mib ?? 0);
  if (max > 0 && value > max) {
    return {
      error:
        key === "import_max_procs"
          ? `导入并发最大 ${max}（编排器超过它就拒绝启动）`
          : `超出可换算范围（最大 ${max} MiB），编排器会拒绝启动`,
    };
  }
  return { value };
}

function mibText(value: number): string {
  return value >= 1024 ? `${(value / 1024).toFixed(2)} GiB（${value} MiB）` : `${value} MiB`;
}

function Spark({ series }: { series: number[] }) {
  if (series.length < 2) return null;
  const max = Math.max(...series);
  const min = Math.min(...series);
  const span = Math.max(1, max - min);
  const points = series
    .map(
      (value, i) =>
        `${((i / (series.length - 1)) * 100).toFixed(1)},${(32 - ((value - min) / span) * 28).toFixed(1)}`,
    )
    .join(" ");
  return (
    <svg className="mem-spark" viewBox="0 0 100 34" preserveAspectRatio="none" aria-hidden>
      <polyline points={points} />
    </svg>
  );
}

export default function MemoryCard({
  onNotice,
}: {
  onNotice: (text: string, tone?: "ok" | "err") => void;
}) {
  const [cfg, setCfg] = useState<MemoryConfig | null>(null);
  const [draft, setDraft] = useState<Record<Key, string>>({
    server_memory_limit_mib: "",
    go_mem_target_mib: "",
    import_max_procs: "",
  });
  const [live, setLive] = useState<MemoryLive | null>(null);
  const [busy, setBusy] = useState(false);

  // 校验是派生的：一边打字一边就地转红，而不必等到按下「写入设置」。
  const errors = useMemo(() => {
    const found: Partial<Record<Key, string>> = {};
    for (const item of FIELDS) {
      const parsed = parseField(item.key, draft[item.key], cfg);
      if ("error" in parsed) found[item.key] = parsed.error;
    }
    return found;
  }, [cfg, draft]);

  const reload = useCallback(async () => {
    try {
      const view = await commands.memoryConfig();
      setCfg(view);
      setDraft({
        server_memory_limit_mib: view.tuning.server_memory_limit_mib?.toString() ?? "",
        go_mem_target_mib: view.tuning.go_mem_target_mib?.toString() ?? "",
        import_max_procs: view.tuning.import_max_procs?.toString() ?? "",
      });
    } catch (err) {
      onNotice(errText(err), "err");
    }
  }, [onNotice]);

  useEffect(() => {
    void reload();
  }, [reload]);

  // 编排器本身 2 秒才打一条采样，轮得再快也只是把同一行读两遍。
  useEffect(() => {
    let alive = true;
    const pull = () =>
      commands
        .memoryLive()
        .then((next) => {
          if (alive) setLive(next);
        })
        .catch(() => undefined);
    void pull();
    const timer = window.setInterval(pull, 2000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);

  const save = async () => {
    const next: MemoryTuning = {
      server_memory_limit_mib: null,
      go_mem_target_mib: null,
      import_max_procs: null,
    };
    for (const item of FIELDS) {
      const parsed = parseField(item.key, draft[item.key], cfg);
      if (!("error" in parsed)) next[item.key] = parsed.value;
    }
    // 只要有一格不合格，一个字节都不往外写。
    if (Object.keys(errors).length > 0) return;
    setBusy(true);
    try {
      const record = await commands.memorySave(next);
      onNotice(record.message);
      await reload();
    } catch (err) {
      // 后端守着同一套边界，它拒绝时就照它那句话显示，不另编一份文案。
      onNotice(errText(err), "err");
    } finally {
      setBusy(false);
    }
  };

  const ratio =
    live !== null && live.hard_limit_mib !== null && live.hard_limit_mib > 0
      ? Math.min(100, Math.round((live.private_mib / live.hard_limit_mib) * 100))
      : null;

  return (
    <section className="card mem-card">
      <h2>服务端内存</h2>

      <div className="field">
        {FIELDS.map((item) => (
          <div className="mem-row" key={item.key}>
            <label className="num">
              <span>{item.label}</span>
              <input
                type="text"
                inputMode="numeric"
                placeholder={item.blank}
                value={draft[item.key]}
                disabled={busy || cfg === null}
                onChange={(e) => setDraft((prev) => ({ ...prev, [item.key]: e.target.value }))}
              />
              <em>{item.unit}</em>
            </label>
            <p className="hint" data-tone={errors[item.key] === undefined ? undefined : "err"}>
              {errors[item.key] ?? item.note}
            </p>
          </div>
        ))}

        <div className="row">
          <span className="spacer" />
          <button className="btn" data-tone="accent" disabled={busy || cfg === null} onClick={() => void save()}>
            {busy ? "写入中…" : "写入设置"}
          </button>
          <button className="btn" data-tone="ghost" disabled={busy} onClick={() => void reload()}>
            重读
          </button>
        </div>

        {cfg ? <div className="path">{cfg.path}</div> : null}
        <p className="hint">
          留空或 0 = 不限；填 MiB 数字 = 启用。硬上限超了服务端会被系统直接杀掉，慎用低值。
          <br />
          这三项只写进上面那个文件（其它键原样保留），编排器下次拉起网关时才读它：改了要重开一场（下次「开始游戏」）才生效。
          导入并发超过 {cfg?.max_import_procs ?? 65536} 编排器会拒绝启动；不填就是它自己的默认值（本机 {cfg?.cpu_count ?? "…"} 个逻辑核心）。
        </p>
      </div>

      <div className="field">
        {live ? (
          <>
            <div className="row">
              <span className="chip" data-tone={live.ended ? "idle" : live.stale ? "busy" : "ok"}>
                {live.ended ? "本场已收尾" : live.stale ? "采样停了" : `采样中 · ${live.seconds.toFixed(0)} 秒`}
              </span>
              <span className="spacer" />
              <span className="mono">进程 {live.pid}</span>
            </div>
            <div className="path">{live.session}</div>

            {ratio === null ? <p className="hint">本场没设硬上限（不限）。下面这几个数照常打点。</p> : (
              <div className="mem-track" title={`${live.private_mib} / ${live.hard_limit_mib ?? 0} MiB`}>
                <i data-tone={ratio >= 90 ? "err" : ratio >= 70 ? "busy" : "ok"} style={{ width: `${ratio}%` }} />
                <span className="mono mem-cap">
                  私有提交占硬上限 {ratio}%（{mibText(live.hard_limit_mib ?? 0)}）
                </span>
              </div>
            )}

            <Spark series={live.series} />

            <dl className="mem-stats">
              <div>
                <dt>私有提交</dt>
                <dd>{mibText(live.private_mib)}</dd>
              </div>
              <div>
                <dt>工作集</dt>
                <dd>{mibText(live.working_mib)}</dd>
              </div>
              <div>
                <dt>系统可用</dt>
                <dd>{mibText(live.system_available_mib)}</dd>
              </div>
              <div>
                <dt>本场峰值</dt>
                <dd>{live.peak_private_mib === null ? "还在跑" : mibText(live.peak_private_mib)}</dd>
              </div>
              <div>
                <dt>硬上限</dt>
                <dd>{live.hard_limit_mib === null ? "不限" : mibText(live.hard_limit_mib)}</dd>
              </div>
              <div>
                <dt>低内存警戒线</dt>
                <dd>{live.low_memory_mib === 0 ? "—" : mibText(live.low_memory_mib)}</dd>
              </div>
            </dl>

            <p className="hint">
              读的是 <span className="mono">gateway.err</span> 里编排器打的 MEMORY_GUARD 采样，2 秒一条；
              小曲线画最近 {live.series.length} 条私有提交，采样共 {live.samples} 条。
              只有「私有提交」是拿去跟硬上限比的那个数。
              {live.note ? <b className="mem-warn">{live.note}</b> : null}
            </p>
          </>
        ) : (
          <p className="hint">
            实时占用：现在读不到采样。它只在服务端由启动器拉起之后才有数——没起过服务端、
            或那一场的日志里没有打点行，这里就空着，不摆假数。
          </p>
        )}
      </div>
    </section>
  );
}
