// Read-only acceptance probe: never starts/restarts playback or manufactures state.
const fs=require('node:fs');
const path=require('node:path');
const seconds=Number(process.argv[2]||340), started=Date.now(), trace=[], latest=new Map();
let previous='',lastLog=0;
(async()=>{
  while(Date.now()-started<seconds*1000){
    try{const a=await(await fetch('http://127.0.0.1:43116/accounts',{signal:AbortSignal.timeout(5000)})).json();
      const records=a.map(a=>{const r=a.runtime||{};return {id:a.instance,playing:r.playing,state:r.state,position:r.position,duration:r.duration,observed:r.updated,
        repeats:r.health?.repeats,ready:r.health?.ready,visibility:r.health?.visibility,focused:r.health?.focused,parked:r.window?.parked,foreground:r.window?.foreground,toolWindow:r.window?.toolWindow,silent:r.silentConfigured};});
      trace.push({elapsed:Math.round((Date.now()-started)/1000),records});
      for(const r of records)latest.set(r.id,r);
      const key=records.map(r=>[r.id,r.state,r.repeats,r.focused,r.parked].join(':')).join('|');
      if(key!==previous || Date.now()-lastLog>=30000){console.log(JSON.stringify(trace.at(-1)));previous=key;lastLog=Date.now();}
    }catch(e){trace.push({elapsed:Math.round((Date.now()-started)/1000),error:e.message});console.log(e.message);}
    await new Promise(r=>setTimeout(r,2000));
  }
  const summary=[...latest.keys()].map(id=>{const samples=trace.flatMap(t=>t.records||[]).filter(r=>r.id===id);return {id,samples:samples.length,
    advancingUpdates:samples.filter((s,i)=>i>0&&s.observed>samples[i-1].observed&&s.position>samples[i-1].position).length,
    maxRepeats:Math.max(...samples.map(s=>s.repeats||0)),repeatDelta:(samples.at(-1).repeats||0)-(samples[0].repeats||0),
    repeatCounterMonotonic:samples.every((s,i)=>!i||(s.repeats||0)>=(samples[i-1].repeats||0)),neverFocused:samples.every(s=>s.focused===false&&s.foreground===false),
    alwaysParked:samples.every(s=>s.parked===true&&s.toolWindow===true),silentConfigured:samples.every(s=>s.silent===true),last:samples.at(-1)};});
  const out=path.resolve(process.argv[3]||'work/audit-2026-09-08/live-0.2.2-results.json');fs.writeFileSync(out,JSON.stringify({started,summary,trace},null,2));
  console.log(JSON.stringify({summary,artifact:out}));
  if(!summary.length || !summary.every(s=>s.advancingUpdates>5&&s.repeatDelta>=1&&s.repeatCounterMonotonic&&s.neverFocused&&s.alwaysParked&&s.silentConfigured))process.exitCode=1;
})().catch(e=>{console.error(e.message);process.exitCode=1;});
