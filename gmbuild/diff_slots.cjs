const fs = require("fs");
const go = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const rs = JSON.parse(fs.readFileSync(process.argv[3], "utf8"));
console.log("rs envelope keys:", Object.keys(rs), "built_at:", rs.built_at);
const g = go.slots, r = rs.slots;
const gk = Object.keys(g), rk = Object.keys(r);
let cellSame = 0, cellDiff = 0, lvlSame = 0, lvlDiff = 0;
const onlyGo = new Set(gk), onlyRs = [];
const cellSamples = [], lvlSamples = [];
for (const k of rk) {
  if (!(k in g)) { onlyRs.push(k); continue; }
  onlyGo.delete(k);
  if (g[k].cell === r[k].cell) cellSame++;
  else if (cellSamples.length < 8) cellSamples.push(`${k}: go=${g[k].cell} rs=${r[k].cell}`);
  else cellDiff++;
  if (g[k].level === r[k].level) lvlSame++;
  else if (lvlSamples.length < 8) lvlSamples.push(`${k}: go=${g[k].level} rs=${r[k].level}`);
  else lvlDiff++;
}
console.log(`go=${gk.length} rs=${rk.length} common=${rk.length - onlyRs.length}`);
console.log(`cell: same=${cellSame} diff=${cellDiff + (cellSamples.length ? 0 : 0)} | level: same=${lvlSame} diff=${lvlDiff}`);
console.log("cell 差异样例:\n" + cellSamples.join("\n"));
console.log("level 差异样例:\n" + lvlSamples.join("\n"));
console.log(`仅旧表保留(扫描未覆盖) ${onlyGo.size} 条，新增(旧表没有) ${onlyRs.length} 条`);
console.log("新增样例:", onlyRs.slice(0, 8).join(","));
console.log("保留样例:", [...onlyGo].slice(0, 8).join(","));
const cells = {};
for (const k of rk) cells[r[k].cell] = (cells[r[k].cell] || 0) + 1;
console.log("rs 部位种类:", Object.keys(cells).length);
console.log("Top:", Object.entries(cells).sort((a, b) => b[1] - a[1]).slice(0, 8));
