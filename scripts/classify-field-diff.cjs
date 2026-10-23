// 把「共有 id 的字段差异」分成三类：新增标签（良性）／值漂移（代际）／丢标签（危险）。
// 用法：node scripts/classify-field-diff.cjs <new> <old>
const fs = require('fs');
const BS = String.fromCharCode(92);

function loadRows(file) {
  const text = fs.readFileSync(file, 'utf8');
  const rows = new Map();
  const re = new RegExp('\\{"ID":(\\d+),"Path":"([^"]*)","SHA256":"([0-9a-fA-F]{64})","Fields":', 'g');
  let m;
  while ((m = re.exec(text)) !== null) {
    let i = re.lastIndex, depth = 0, inStr = false, esc = false;
    for (; i < text.length; i++) {
      const c = text[i];
      if (inStr) {
        if (esc) esc = false;
        else if (c === BS) esc = true;
        else if (c === '"') inStr = false;
        continue;
      }
      if (c === '"') inStr = true;
      else if (c === '{') depth++;
      else if (c === '}') { depth--; if (depth === 0) break; }
    }
    rows.set(Number(m[1]), JSON.parse(text.slice(re.lastIndex, i + 1)));
  }
  return rows;
}

const [, , newPath, oldPath] = process.argv;
const N = loadRows(newPath), O = loadRows(oldPath);
let identical = 0, addedTags = 0, valueDrift = 0, droppedTags = 0;
const driftSamples = [], dropSamples = [];
for (const [id, oldF] of O) {
  const newF = N.get(id);
  if (!newF) continue;
  const oldKeys = Object.keys(oldF), newKeys = Object.keys(newF);
  const missing = oldKeys.filter((k) => !newKeys.includes(k));
  const extra = newKeys.filter((k) => !oldKeys.includes(k));
  const changed = oldKeys.filter((k) => newKeys.includes(k) && JSON.stringify(oldF[k]) !== JSON.stringify(newF[k]));
  if (!missing.length && !extra.length && !changed.length) identical++;
  else if (missing.length) {
    droppedTags++;
    if (dropSamples.length < 5) dropSamples.push({ id, missing });
  } else if (changed.length) {
    valueDrift++;
    if (driftSamples.length < 5) driftSamples.push({ id, changed, extra });
  } else addedTags++;
}
console.log(`共有 id=${O.size}`);
console.log(`  完全一致=${identical}`);
console.log(`  只多出新标签=${addedTags}`);
console.log(`  共有标签的值有别=${valueDrift}`);
console.log(`  旧标签在新版里不见了=${droppedTags}  <-- 必须是 0`);
for (const s of dropSamples) console.log('   丢标签', s.id, s.missing.join('|'));
for (const s of driftSamples) console.log('   值漂移', s.id, '改动标签=' + s.changed.join('|'), '新增=' + s.extra.join('|'));
const tagTally = new Map();
for (const [, f] of N) for (const k of Object.keys(f)) tagTally.set(k, (tagTally.get(k) || 0) + 1);
console.log('新产物各标签行数', [...tagTally.entries()].sort((a, b) => b[1] - a[1]).map(([k, v]) => `${k}=${v}`).join(' '));
