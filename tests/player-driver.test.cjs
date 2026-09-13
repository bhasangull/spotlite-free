const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const {Playback,uriFrom,ReconnectTracker}=require('../src/ui/playback-core.js');
const uri='spotify:track:7GqvrVU8R6tBv9DAN1USaG';
function harness(){
  const calls=[],listeners={},timers=[],intervals=[],nodes={};let frame=null,now=1000;
  const node=id=>nodes[id] ||= {id,value:'',textContent:'',disabled:false,replaceChildren(){},setAttribute(){},querySelector:()=>frame};
  const controller={addListener:(n,f)=>listeners[n]=f,play:()=>calls.push('play'),restart:()=>calls.push('restart'),resume:()=>calls.push('resume'),loadEntity:u=>calls.push(['load',u]),destroy:()=>calls.push('destroy')};
  const fakeApi={createController:(mount,options,callback)=>{assert.equal(mount.id,'embed-mount');
    frame={loading:'lazy',setAttribute:(k,v)=>{frame[k]=v;}};callback(controller);}};
  const context=vm.createContext({SpotLiteCore:{Playback,uriFrom,ReconnectTracker},URLSearchParams,AbortSignal,
    document:{body:{dataset:{instance:'1'}},visibilityState:'visible',hasFocus:()=>false,getElementById:node,createElement:()=>({}),head:{appendChild(){}}},
    window:{addEventListener:(name,fn)=>listeners['window-'+name]=fn},location:{search:'',assign:()=>calls.push('login')},localStorage:{getItem:()=>null,setItem(){}},
    fetch:async(path,options)=>({ok:true,json:async()=>path==='/auth-state'?{}:path.startsWith('/command')?{version:0}:{error:'metadata unavailable'},text:async()=>'ok'}),
    setTimeout:(f,ms)=>{const t={f,ms};timers.push(t);return t;},clearTimeout:t=>{if(t)t.cancelled=true;},setInterval:(f,ms)=>intervals.push({f,ms}),
    Date:class extends Date{static now(){return now;}}
  });
  vm.runInContext(fs.readFileSync('src/ui/player.js','utf8'),context);
  return {context,calls,listeners,timers,intervals,get frame(){return frame;},fakeApi,setNow:n=>now=n,
    eval:s=>vm.runInContext(s,context),tick:async()=>{for(let i=0;i<8;i++)await Promise.resolve();}};
}
test('completed ad resumes returned music in background without restarting or login',async()=>{
  const h=harness(),pending=h.eval(`begin('${uri}','track')`);await h.tick();
  h.context.window.onSpotifyIframeApiReady(h.fakeApi);await h.tick();h.listeners.ready();await pending;
  const sample=(time,position,duration,isPaused=false)=>{h.setNow(time);h.listeners.playback_update({data:{playingURI:uri,position,duration,isPaused,isBuffering:false}});};
  sample(1000,140000,151303);sample(2000,150000,151303);
  sample(3000,0,30172);sample(33000,30000,30172);
  assert.ok(!h.calls.includes('resume'));assert.ok(!h.calls.includes('restart'));
  sample(34000,0,151303,true);h.setNow(35500);
  const timer=h.timers.find(t=>t.ms===1500&&!t.cancelled);assert.ok(timer);timer.f();
  assert.equal(h.calls.filter(c=>c==='resume').length,1);
  assert.ok(!h.calls.includes('restart'));assert.ok(!h.calls.includes('login'));assert.ok(!h.calls.includes('destroy'));
  sample(36000,200,151303);sample(37000,1200,151303);
  assert.equal(h.eval('machine.sample.state'),'playing');
});

test('watchdog timer recovers a silent failed repeat without another Spotify event',async()=>{
  const h=harness(),pending=h.eval(`begin('${uri}','track')`);await h.tick();
  h.context.window.onSpotifyIframeApiReady(h.fakeApi);await h.tick();h.listeners.ready();await pending;
  const sample=(t,p,paused=false)=>{h.setNow(t);h.listeners.playback_update({data:{playingURI:uri,duration:151303,position:p,isPaused:paused,isBuffering:false}});};
  sample(1000,149800);sample(2000,150950);sample(2500,0,true);
  h.setNow(4000);h.timers.find(t=>t.ms===1500&&!t.cancelled).f();
  h.setNow(10500);h.eval('watchdog()');
  assert.equal(h.calls.filter(c=>c==='restart').length,1);
  assert.equal(h.calls.filter(c=>c==='resume').length,1);
  sample(11000,100);sample(12000,1100);h.setNow(18000);h.eval('watchdog()');
  assert.equal(h.calls.filter(c=>c==='resume').length,1);
  assert.equal(h.eval('machine.repeatCount'),1);
});
test('network return releases command backoff but never resumes an ordinary pause',async()=>{
  const h=harness();await h.tick();h.eval('failures=3;retryAt=999999');
  h.listeners['window-online']();await h.tick();
  assert.equal(h.eval('failures'),0);assert.equal(h.eval('retryAt'),0);
  assert.ok(!h.calls.includes('resume'));assert.ok(!h.calls.includes('restart'));
});
test('three independent previews open same-profile authorize exactly once, no restart',async()=>{
  const h=harness(),requests=[],pending=h.eval(`begin('${uri}','track')`);await h.tick();
  h.context.window.onSpotifyIframeApiReady(h.fakeApi);await h.tick();h.listeners.ready();await pending;
  h.context.fetch=async path=>{requests.push(path);return {ok:true,json:async()=>path==='/auth-start'?{url:'https://accounts.spotify.com/authorize?test=1'}:{},text:async()=>'ok'};};
  const sample=(now,p,d,paused=false)=>{h.setNow(now);h.listeners.playback_update({data:{playingURI:uri,position:p,duration:d,isPaused:paused,isBuffering:false}});};
  sample(1000000,21000,151303);sample(1002000,23000,151303);
  for(let run=0;run<3;run++){for(let p=0;p<=16000;p+=4000)sample(1010000+run*20000+p,p,16000,p===16000);await h.tick();}
  await h.tick();assert.equal(requests.filter(p=>p==='/auth-start').length,1);
  assert.equal(h.calls.filter(c=>c==='login').length,1);
  assert.ok(requests.includes('/player-event?event=reconnect-start'));
  assert.ok(!h.calls.includes('restart'));assert.ok(!h.calls.includes('resume'));
});
test('background iframe is made eager before ready and only plays after ready',async()=>{
  const h=harness(),pending=h.eval(`begin('${uri}','playlist')`);await h.tick();h.context.window.onSpotifyIframeApiReady(h.fakeApi);await h.tick();
  assert.equal(h.frame.loading,'eager');assert.ok(!h.calls.includes('play'));h.listeners.ready();await pending;assert.ok(h.calls.includes('play'));assert.ok(!h.calls.includes('login'));
});
test('natural reset-to-zero repeats without focus, login, reload or resume race',async()=>{
  const h=harness(),pending=h.eval(`begin('${uri}','playlist')`);await h.tick();h.context.window.onSpotifyIframeApiReady(h.fakeApi);await h.tick();h.listeners.ready();await pending;
  const sample=(position,isPaused=false)=>h.listeners.playback_update({data:{playingURI:uri,duration:151265,position,isPaused,isBuffering:false}});
  h.setNow(1000);sample(149800);h.setNow(2000);sample(150950);h.setNow(2500);sample(0,true);
  h.setNow(4000);const t=h.timers.find(t=>t.ms===1500&&!t.cancelled);assert.ok(t);t.f();
  assert.equal(h.calls.filter(c=>c==='restart').length,1);assert.ok(!h.calls.includes('resume'));assert.ok(!h.calls.includes('login'));assert.ok(!h.calls.includes('destroy'));
});
test('new playback command cancels previously scheduled repeat',async()=>{
  const h=harness(),pending=h.eval(`begin('${uri}','track')`);await h.tick();h.context.window.onSpotifyIframeApiReady(h.fakeApi);await h.tick();h.listeners.ready();await pending;
  h.eval(`machine.sample={state:'ended',track:'${uri}',position:151265,duration:151265,updated:1000}; machine.endArmed=true; machine.advanced=true;`);
  h.listeners.playback_update({data:{playingURI:uri,duration:151265,position:151265,isPaused:true,isBuffering:false}});
  const old=h.timers.find(t=>t.ms===1500);assert.ok(old);await h.eval(`begin('${uri}','off')`);const before=h.calls.length;h.setNow(2500);old.f();assert.equal(h.calls.length,before);
});

test('failed start remains unacknowledged and retries after backoff',async()=>{
  const h=harness();await h.tick();let attempts=0;const acknowledgements=[];
  h.context.fetch=async path=>({ok:true,json:async()=>({version:1,url:uri,loop:'playlist'}),text:async()=>{acknowledgements.push(path);return 'ok';}});
  h.context.tryBegin=async()=>{if(++attempts===1)throw Error('temporary');};h.eval('begin=tryBegin');
  await h.eval('poll()');assert.equal(h.eval('version'),0);assert.equal(attempts,1);
  await h.eval('poll()');assert.equal(attempts,1);
  h.setNow(6000);await h.eval('poll()');assert.equal(attempts,2);assert.equal(h.eval('version'),1);
});

test('lost acknowledgement retries acknowledgement without replaying track',async()=>{
  const h=harness();await h.tick();let starts=0,acks=0;
  h.context.fetch=async path=>{if(path.startsWith('/command-ack') && ++acks===1)throw Error('offline');return {ok:true,json:async()=>({version:1,url:uri,loop:'playlist'})};};
  h.context.tryBegin=async()=>{starts++;};h.eval('begin=tryBegin');
  await h.eval('poll()');await h.eval('poll()');assert.equal(starts,1);assert.equal(acks,2);assert.equal(h.eval('ackVersion'),1);
});

test('fresh document rebuilds playback even when latest command is loop-only',async()=>{
  const h=harness();await h.tick();let starts=0;
  h.context.fetch=async()=>({ok:true,json:async()=>({version:3,url:uri,loop:'track',loopOnly:true})});
  h.context.tryBegin=async()=>{starts++;};h.eval('begin=tryBegin');await h.eval('poll()');assert.equal(starts,1);
});

test('slow telemetry holds at most one request and one newest sample',async()=>{
  const h=harness();await h.tick();const pending=[],paths=[];
  h.context.fetch=path=>{paths.push(path);return new Promise(resolve=>pending.push(()=>resolve({ok:true})));};
  const first=h.eval("report({playing:true,track:'x',position:1,duration:1000,updated:1,state:'playing'})");
  for(let i=2;i<=100;i++)h.eval(`void report({playing:true,track:'x',position:${i},duration:1000,updated:${i},state:'playing'})`);
  assert.equal(paths.length,1);pending.shift()();await h.tick();assert.equal(paths.length,2);assert.match(paths[1],/position=100&/);
  pending.shift()();await first;assert.equal(h.eval('pendingReport'),null);
});

test('compact layout restores on focus without reloading or stopping Spotify',async()=>{
  const h=harness(),pending=h.eval(`begin('${uri}','playlist')`);await h.tick();h.context.window.onSpotifyIframeApiReady(h.fakeApi);await h.tick();h.listeners.ready();await pending;
  const before=h.calls.length;h.eval('compact=true;paintLayout()');assert.equal(h.frame.height,'152');
  h.context.document.hasFocus=()=>true;h.eval('paintLayout()');assert.equal(h.frame.height,'352');assert.equal(h.calls.length,before);
});

test('retired page releases its Spotify iframe and stops making requests',async()=>{
  const h=harness();await h.tick();let count=0;const navigations=[];
  h.context.location.replace=url=>navigations.push(url);
  h.context.fetch=async()=>{count++;return {ok:false,status:410};};
  await h.eval("request('/command').catch(()=>{})");await h.eval('poll()');await h.eval('health()');
  assert.equal(count,1);assert.deepEqual(navigations,['about:blank']);assert.equal(h.eval('retired'),true);
});
