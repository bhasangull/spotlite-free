// Read-only analysis; a short interval is a candidate ad, not proof of logout.
const fs = require('node:fs');
const data = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
const accounts = new Map();
for (const entry of data.trace) for (const sample of entry.records || []) {
  if (!accounts.has(sample.id)) accounts.set(sample.id, []);
  accounts.get(sample.id).push({...sample, elapsed:entry.elapsed});
}
const result = [...accounts].map(([id, samples]) => {
  const transitions = []; let short = null, returned = null;
  for (const s of samples) {
    if (s.duration > 0 && s.duration <= 35000) {
      if (!short) short = {start:s.elapsed, duration:s.duration, maxPosition:0};
      short.maxPosition = Math.max(short.maxPosition, s.position || 0);
      short.last = s.elapsed; returned = null;
    } else if (short && s.duration > 35000) {
      if (!returned) returned = s;
      if (s.playing && s.position > 3000 && s.observed > returned.observed) {
        transitions.push({...short, returnAt:returned.elapsed, progressedAt:s.elapsed,
          returnState:returned.state, musicDuration:s.duration, position:s.position});
        short = null; returned = null;
      }
    }
  }
  return {id, transitions, unresolvedShort:short,
    staleSamples:samples.filter(s => s.state === 'unknown').length,
    last:samples.at(-1)};
});
console.log(JSON.stringify(result, null, 2));
