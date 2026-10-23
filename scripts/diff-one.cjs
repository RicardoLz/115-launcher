const fs = require('fs');
function loadRows(file) {
  const text = fs.readFileSync(file, 'utf8');
  const rows = new Map();
  const re = /\{"ID":(\d+),"Path":"([^"]*)","SHA256":"([0-9a-fA-F]{64})","Fields":/g;
  let m;
  while ((m = re.exec(text)) !== null) {
    let i = re.lastIndex, depth = 0, inStr = false, esc = false;
    for (; i < text.length; i++) {
      const c = text[i];
      if (inStr) {
        if (esc) esc = false;
        else if (c === String.fromCharCode(92)) esc = true;
        else if (c === '"') inStr = false;
        continue;
      }
      if (c === '"') inStr = true;
      else if (c === '{') depth++;
      else if (c === '}') { depth--; if (depth === 0) break; }
    }
    rows.set(Number(m[1]), { path: m[2], sha: m[3], fields: text.slice(re.lastIndex, i + 1) });
  }
  return rows;
}
const unesc = (s) => s.replace(/\\u003c/g, '<').replace(/\\u003e/g, '>').replace(/\\u0026/g, '&');
const N = loadRows(process.argv[2]), O = loadRows(process.argv[3]);
const id = Number(process.argv[4]);
const a = unesc(N.get(id).fields), b = unesc(O.get(id).fields);
console.log(`len new=${a.length} old=${b.length}`);
let i = 0; while (i < a.length && i < b.length && a[i] === b[i]) i++;
console.log(`首个差异 @${i}`);
console.log('new …' + a.slice(Math.max(0, i - 90), i + 170));
console.log('old …' + b.slice(Math.max(0, i - 90), i + 170));
const keys = (s) => [...s.matchAll(/"\[([a-z_ ]+)\]":\[/g)].map((x) => x[1]);
console.log('new keys|' + keys(a).join('|'));
console.log('old keys|' + keys(b).join('|'));
