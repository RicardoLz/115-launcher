/**
 * 一键重建的两件新产物与现网那份对拍（只读）：
 *   node scripts/compare-equipment-catalog.cjs
 * 比的是 ID 集合、每个 id 的 Path/SHA256 与共有 id 的 Fields。
 */
const fs = require("fs");

const OLD = "D:/115us/源码/115US当前源码/gm-tool/configs/equipment.current37.json";
const NEW = "D:/115us-Tauri/out/equip-check2/equipment.current37.json";

function rowsOf(file) {
  const text = fs.readFileSync(file, "utf8");
  const key = text.indexOf('"rows"');
  const start = text.indexOf("[", key);
  let depth = 0;
  for (let i = start; i < text.length; i++) {
    if (text[i] === "[") depth++;
    else if (text[i] === "]") {
      depth--;
      if (depth === 0) return JSON.parse(text.slice(start, i + 1));
    }
  }
  throw new Error(`${file} 里找不到完整的 rows`);
}

const oldRows = rowsOf(OLD);
const newRows = rowsOf(NEW);
const oldById = new Map(oldRows.map((r) => [r.ID, r]));
const newById = new Map(newRows.map((r) => [r.ID, r]));

const missing = [];
const pathDiff = [];
const shaDiff = [];
const fieldDiff = [];
for (const [id, r] of oldById) {
  const x = newById.get(id);
  if (!x) {
    missing.push(id);
    continue;
  }
  if (r.Path !== x.Path) pathDiff.push([id, r.Path, x.Path]);
  if (r.SHA256 !== x.SHA256) shaDiff.push(id);
  const a = JSON.stringify(r.Fields);
  const b = JSON.stringify(x.Fields);
  if (a !== b) fieldDiff.push([id, a, b]);
}

const extra = newRows.filter((r) => !oldById.has(r.ID)).map((r) => r.ID);

console.log(`旧 ${oldRows.length} 行 / 新 ${newRows.length} 行`);
console.log(`旧有新缺 ${missing.length} 个 id`);
console.log(`新有旧无 ${extra.length} 个 id`);
console.log(`共有 id 里 Path 不同 ${pathDiff.length}、SHA 不同 ${shaDiff.length}、Fields 不同 ${fieldDiff.length}`);
for (const [id, p, q] of pathDiff.slice(0, 5)) console.log(`  Path 不同 ${id}: ${p} -> ${q}`);
for (const [id, a, b] of fieldDiff.slice(0, 2)) {
  console.log(`  Fields 不同 ${id}:`);
  console.log(`    旧 ${a.slice(0, 260)}`);
  console.log(`    新 ${b.slice(0, 260)}`);
}

// 每个标签有多少行带着：新那份不该丢掉旧那份在用的任何一个标签。
const census = (rows) => {
  const m = new Map();
  for (const r of rows) for (const k of Object.keys(r.Fields)) m.set(k, (m.get(k) || 0) + 1);
  return m;
};
const cOld = census(oldRows);
const cNew = census(newRows);
console.log("标签计数（旧 -> 新）：");
for (const [k, v] of [...cOld].sort()) {
  const now = cNew.get(k) || 0;
  console.log(`  ${k.padEnd(18)} ${String(v).padStart(7)} -> ${String(now).padStart(7)}${now < v ? "  <-- 覆盖率倒退" : ""}`);
}
for (const [k, v] of [...cNew].sort()) if (!cOld.has(k)) console.log(`  ${k.padEnd(18)}      0 -> ${String(v).padStart(7)}  (新增标签)`);
