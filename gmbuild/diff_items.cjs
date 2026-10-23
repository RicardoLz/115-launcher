const fs = require("fs");
const go = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const rs = JSON.parse(fs.readFileSync(process.argv[3], "utf8"));
const g = new Map(go.items.map(i => [i.id, i]));
const r = new Map(rs.items.map(i => [i.id, i]));
let common = 0;
const onlyGo = [], onlyRs = [];
for (const [id, it] of g) if (!r.has(id)) onlyGo.push([id, it.name]);
for (const [id, it] of r) if (!g.has(id)) onlyRs.push([id, it.name]);
const fieldDiff = { kind: 0, grade: 0, rarity: 0, stack_type: 0, stack_limit: 0, durability: 0, name: 0 };
const samples = { kind: [], grade: [], rarity: [], stack_type: [], stack_limit: [], durability: [], name: [] };
for (const [id, gi] of g) {
  const ri = r.get(id);
  if (!ri) continue;
  common++;
  for (const f of Object.keys(fieldDiff)) {
    const a = gi[f] === undefined ? "" : gi[f];
    const b = ri[f] === undefined ? "" : ri[f];
    if (a !== b) {
      fieldDiff[f]++;
      if (samples[f].length < 8) samples[f].push(`${id}: go=${JSON.stringify(a)} rs=${JSON.stringify(b)}`);
    }
  }
}
console.log(`go=${g.size} rs=${r.size} common=${common} onlyGo=${onlyGo.length} onlyRs=${onlyRs.length}`);
console.log("common 字段差异：", fieldDiff);
for (const [f, s] of Object.entries(samples)) if (s.length) console.log(`--${f}--\n` + s.join("\n"));
console.log("onlyGo 样例:", onlyGo.slice(0, 10).map(x => x.join(" ")).join(" | "));
console.log("onlyRs 样例:", onlyRs.slice(0, 10).map(x => x.join(" ")).join(" | "));
// 形态核对：Rust 文件 key 集合与 Go 一致
const shapes = new Set(rs.items.map(i => Object.keys(i).join(",")));
console.log("rs shapes:", [...shapes]);
const krs = {};
for (const i of rs.items) krs[i.kind] = (krs[i.kind] || 0) + 1;
console.log("rs kinds:", krs, "count:", rs.count, "source:", rs.source);
