const {test}=require('node:test');
const assert=require('node:assert/strict');
const {Playback,uriFrom}=require('../src/ui/playback-core.js');
const {ReconnectTracker}=require('../src/ui/playback-core.js');
const song='spotify:track:7GqvrVU8R6tBv9DAN1USaG', other='spotify:track:1111111111111111111111', playlist='spotify:playlist:2222222222222222222222';
function packet(position,more={}){return {playingURI:song,isPaused:false,isBuffering:false,position,duration:151265,...more};}
function finish(m,track=song){m.observe(packet(1000,{playingURI:track}),1000);m.observe(packet(150000,{playingURI:track}),2000);m.observe(packet(151265,{playingURI:track,isPaused:true}),3000);}
function previewRun(r,start,duration=16000,expected=151303){
  for(let p=0;p<=duration;p+=4000)r.observe(packet(p,{duration,isPaused:p===duration}),start+p,expected);
}
test('preview end resetting to zero counts once, stale or mid-song reset does not',()=>{
  const r=new ReconnectTracker();
  for(let run=0;run<3;run++){
    const t=1000000+run*20000;
    for(const p of [0,4000,8000,12000,15000])r.observe(packet(p,{duration:16000}),t+p,151303);
    r.observe(packet(0,{duration:16000,isPaused:true}),t+16000,151303);
    assert.equal(r.count,run+1);
  }
  assert.equal(r.take(1057000),true);
  const stale=new ReconnectTracker();
  stale.observe(packet(0,{duration:16000}),1000000,151303);
  stale.observe(packet(15000,{duration:16000}),1002000,151303);
  stale.observe(packet(0,{duration:16000,isPaused:true}),1010000,151303);
  assert.equal(stale.count,0);
});
test('three distinct completed previews trigger once, duplicate packets do not count',()=>{
  const r=new ReconnectTracker();previewRun(r,1000000);
  for(let i=0;i<10;i++)r.observe(packet(16000,{duration:16000,isPaused:true}),1016100+i*100,151303);
  assert.equal(r.count,1);assert.equal(r.take(1018000),false);
  previewRun(r,1020000);assert.equal(r.count,2);previewRun(r,1040000);
  assert.equal(r.count,3);assert.equal(r.take(1057000),true);assert.equal(r.take(1058000),false);
});
test('unknown duration, genuine short song, 30 second ad and buffering never trigger login',()=>{
  for(const expected of [0,16000]){const r=new ReconnectTracker();for(let i=0;i<3;i++)previewRun(r,1000000+i*20000,16000,expected);assert.equal(r.count,0);}
  const ad=new ReconnectTracker();for(let i=0;i<3;i++)previewRun(ad,1000000+i*40000,32000);assert.equal(ad.count,0);
  const buffering=new ReconnectTracker();for(let p=0;p<=16000;p+=4000)buffering.observe(packet(p,{duration:16000,isBuffering:true}),1000000+p,151303);assert.equal(buffering.count,0);
});
test('reconnect cooldown survives reload and verified full progress clears suspicion',()=>{
  const r=new ReconnectTracker();for(let i=0;i<3;i++)previewRun(r,1000000+i*20000);
  assert.equal(r.take(1057000),true);const restored=new ReconnectTracker(r.snapshot());
  for(let i=0;i<3;i++)previewRun(restored,1060000+i*20000);
  assert.equal(restored.take(1117000),false);
  restored.observe(packet(22000),1120000);restored.observe(packet(24000),1122000);
  assert.equal(restored.count,0);assert.equal(restored.healthy,true);
});
test('long sleep gap keeps telemetry stale but permits bounded non-seeking recovery',()=>{
  const m=new Playback(song,'track');finish(m);m.repeatAction(3500);
  assert.equal(m.view(3600000).state,'unknown');
  assert.equal(m.repeatAction(3600000),null);
  assert.deepEqual(m.watchdogAction(3600000),{type:'resume',attempt:1});
  m.observe(packet(0),3601000);m.observe(packet(1000),3602000);
  assert.equal(m.sample.state,'playing');assert.equal(m.watchdogAction(3610000),null);
});
test('missing Spotify callbacks trigger bounded resume without stale restart',()=>{
  const m=new Playback(song,'track');finish(m);m.repeatAction(3500);
  assert.equal(m.watchdogAction(8000),null);
  for(const [time,attempt] of [[9500,1],[15500,2],[21500,3]])
    assert.deepEqual(m.watchdogAction(time),{type:'resume',attempt});
  assert.equal(m.watchdogAction(28000),null);assert.equal(m.repeatCount,0);
});
test('watchdog stops on advertisement, buffering, loop off and confirmed progress',()=>{
  for(const change of ['ad','buffer','off','progress']) {
    const m=new Playback(song,'track');finish(m);m.repeatAction(3500);
    if(change==='ad')m.observe(packet(0,{duration:30172}),4000);
    if(change==='buffer')m.observe(packet(0,{isBuffering:true}),4000);
    if(change==='off')m.mode='off';
    if(change==='progress'){m.observe(packet(0),4000);m.observe(packet(1000),5000);}
    assert.equal(m.watchdogAction(10000),null,change);
  }
});
function adReturn(m, more={}) {
  m.observe(packet(140000),1000);m.observe(packet(150000),2000);
  m.observe(packet(0,{duration:30172}),3000);
  m.observe(packet(30000,{duration:30172}),33000);
  assert.equal(m.repeatAction(33001),null);
  m.observe(packet(0,{isPaused:true,...more}),34000);
}
test('completed interstitial returning to known music resumes once, never restarts ad',()=>{
  const m=new Playback(song,'track');adReturn(m);
  assert.deepEqual(m.resumeAction(35500),{type:'resume'});
  assert.equal(m.resumeAction(35501),null);
  m.observe(packet(0,{isPaused:true}),36000);assert.equal(m.resumeAction(36500),null);
});
test('ad recovery does not override off, buffering, stale, different song or natural resume',()=>{
  for(const more of [{isBuffering:true},{playingURI:other},{isPaused:false}]) {
    const m=new Playback(song,'track');adReturn(m,more);assert.equal(m.resumeAction(35500),null);
  }
  const off=new Playback(song,'off');adReturn(off);assert.equal(off.resumeAction(35500),null);
  const stale=new Playback(song,'track');adReturn(stale);assert.equal(stale.resumeAction(40000),null);
});
test('preview alone and ordinary pause never authorize recovery',()=>{
  const m=new Playback(song,'track');m.observe(packet(0,{duration:16000}),1000);
  m.observe(packet(0,{isPaused:true}),2000);assert.equal(m.resumeAction(3500),null);
  m.observe(packet(1000),4000);m.observe(packet(2000),5000);
  m.observe(packet(0,{isPaused:true}),6000);assert.equal(m.resumeAction(7500),null);
});
test('another ad before pending recovery cancels resume',()=>{
  const m=new Playback(song,'track');adReturn(m);
  m.observe(packet(0,{duration:30120}),34500);assert.equal(m.resumeAction(35500),null);
});
test('normal playback requires real advancing Spotify positions',()=>{const m=new Playback(song);assert.equal(m.observe(packet(0),1000).playing,false);assert.equal(m.observe(packet(1000),2000).playing,true);});
test('view interpolation never mutates or refreshes real telemetry',()=>{const m=new Playback(song);m.observe(packet(1000),1000);m.observe(packet(2000),2000);assert.equal(m.view(3000).position,3000);assert.equal(m.sample.position,2000);assert.equal(m.sample.updated,2000);assert.equal(m.view(15000).playing,false);});
test('constant nonpaused samples become stalled instead of forever playing',()=>{const m=new Playback(song);m.observe(packet(0),1000);m.observe(packet(1000),2000);assert.equal(m.observe(packet(1000),9000).state,'stalled');});
test('zero-duration stopped packet clears playing, cannot trigger repeat',()=>{const m=new Playback(song,'track');finish(m);m.observe(packet(0,{isPaused:true,duration:0}),4000);assert.equal(m.sample.playing,false);assert.equal(m.repeatAction(4001),null);});
test('buffering packet is not progress or track completion',()=>{const m=new Playback(song,'track');finish(m);m.observe(packet(151265,{isBuffering:true}),4000);assert.equal(m.sample.state,'buffering');assert.equal(m.repeatAction(4001),null);});
test('16 second media never becomes a forced preview loop',()=>{const m=new Playback(song,'playlist');for(let i=0;i<3;i++)m.observe(packet(i*8000,{duration:16000,isPaused:i===2}),1000+i*1000);assert.equal(m.sample.state,'unverified_media');assert.equal(m.repeatAction(4000),null);});
test('16 second media conflicting with known duration does not loop',()=>{const m=new Playback(song,'track');m.metadata({verified:true,tracks:[{uri:song,duration:151265}]});m.observe(packet(0,{duration:16000}),1000);m.observe(packet(16000,{duration:16000,isPaused:true}),2000);assert.equal(m.repeatAction(2500),null);});
test('genuine short track can repeat when duration is verified',()=>{const m=new Playback(song,'track');m.metadata({verified:true,tracks:[{uri:song,duration:16000}]});m.observe(packet(0,{duration:16000}),1000);m.observe(packet(15000,{duration:16000}),2000);m.observe(packet(16000,{duration:16000,isPaused:true}),3000);assert.deepEqual(m.repeatAction(3500),{type:'restart'});});
test('advertisement URI and buffering cannot invoke restart',()=>{const m=new Playback(song,'track');finish(m);m.observe(packet(30000,{playingURI:'spotify:ad:advertisement',duration:30000,isPaused:true}),4000);assert.equal(m.repeatAction(4500),null);});
test('single track repeats once for one verified end',()=>{const m=new Playback(song,'playlist');finish(m);assert.deepEqual(m.repeatAction(3500),{type:'restart'});assert.equal(m.repeatAction(4000),null);});
test('native next song cancels pending playlist restart',()=>{const m=new Playback(playlist,'playlist');m.metadata({verified:true,tracks:[{uri:song,duration:151265},{uri:other,duration:151265}]});finish(m);m.observe(packet(1000,{playingURI:other}),3500);assert.equal(m.repeatAction(4500),null);});
test('intermediate playlist track must not restart whole playlist',()=>{const m=new Playback(playlist,'playlist');m.metadata({verified:true,tracks:[{uri:song,duration:151265},{uri:other,duration:151265}]});finish(m);assert.equal(m.repeatAction(3500),null);});
test('verified last playlist track restarts playlist without destroying player',()=>{const m=new Playback(playlist,'playlist');m.metadata({verified:true,tracks:[{uri:other,duration:151265},{uri:song,duration:151265}]});finish(m);assert.deepEqual(m.repeatAction(3500),{type:'load',uri:playlist});});
test('unknown playlist order and duplicate final track do not guess completion',()=>{const m=new Playback(playlist,'playlist');finish(m);assert.equal(m.repeatAction(3500),null);m.metadata({verified:true,tracks:[{uri:song,duration:151265},{uri:song,duration:151265}]});assert.equal(m.repeatAction(3500),null);});
test('stale end and loop off do not restart',()=>{const m=new Playback(song,'off');finish(m);assert.equal(m.repeatAction(3500),null);m.mode='track';assert.equal(m.repeatAction(10000),null);});
test('missing playingURI never inherits last song for ad/end decisions',()=>{const m=new Playback(song,'track');finish(m);m.observe({isPaused:true,duration:151265,position:151265},3500);assert.equal(m.sample.playing,false);assert.equal(m.repeatAction(4000),null);});
test('only HTTPS Spotify track/playlist links accepted',()=>{assert.equal(uriFrom('https://open.spotify.com/intl-tr/track/7GqvrVU8R6tBv9DAN1USaG?si=test'),song);assert.equal(uriFrom(song),song);for(const v of ['javascript:alert(1)','https://evil.com/track/7GqvrVU8R6tBv9DAN1USaG','http://open.spotify.com/track/7GqvrVU8R6tBv9DAN1USaG'])assert.equal(uriFrom(v),null);});
test('observed natural end resetting to zero loops without a focus event',()=>{const m=new Playback(song,'track');m.observe(packet(149800),1000);m.observe(packet(150950),2000);m.observe(packet(0,{isPaused:true}),2500);assert.equal(m.sample.state,'ended');m.observe(packet(0,{isPaused:true}),3000);assert.deepEqual(m.repeatAction(4000),{type:'restart'});assert.equal(m.repeatAction(4100),null);});
test('reset after stale or mid-song observation is not an end',()=>{for(const [pos,now]of [[30000,2000],[151000,10000]]){const m=new Playback(song,'track');m.observe(packet(pos-1000),500);m.observe(packet(pos),1000);m.observe(packet(0,{isPaused:true}),now);assert.equal(m.repeatAction(now+1500),null);}});
test('ad duration change after reset cancels pending repeat',()=>{const m=new Playback(song,'track');m.observe(packet(149800),1000);m.observe(packet(150950),2000);m.observe(packet(0,{isPaused:true}),2500);m.observe(packet(0,{isPaused:true,duration:16000}),3000);assert.equal(m.repeatAction(4000),null);});
test('repeat count requires actual reset and advancement',()=>{const m=new Playback(song,'playlist');finish(m);assert.ok(m.repeatAction(3500));assert.equal(m.repeatCount,0);m.observe(packet(0),4000);m.observe(packet(1000),5000);assert.equal(m.repeatCount,1);m.observe(packet(150950),6000);m.observe(packet(0,{isPaused:true}),6500);assert.ok(m.repeatAction(7000));assert.equal(m.repeatCount,1);m.observe(packet(0),8000);m.observe(packet(1000),9000);assert.equal(m.repeatCount,2);});

test('failed repeat retries only on fresh ended music and is bounded',()=>{const m=new Playback(song,'track');finish(m);assert.ok(m.repeatAction(3500));assert.equal(m.repeatAction(4500),null);m.observe(packet(151265,{isPaused:true}),8500);assert.ok(m.repeatAction(8500));m.observe(packet(151265,{isPaused:true}),14000);assert.ok(m.repeatAction(14000));m.observe(packet(151265,{isPaused:true}),20000);assert.equal(m.repeatAction(20000),null);assert.equal(m.repeatCount,0);});
test('ad after failed repeat cancels retry and cannot count success',()=>{const m=new Playback(song,'track');finish(m);m.repeatAction(3500);m.observe(packet(0,{duration:16000}),8500);m.observe(packet(1000,{duration:16000}),9500);assert.equal(m.repeatAction(9500),null);assert.equal(m.repeatCount,0);});
