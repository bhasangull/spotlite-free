'use strict';
const {Playback,uriFrom}=SpotLiteCore;
let reconnectSaved={};try{reconnectSaved=JSON.parse(localStorage.getItem('reconnect-state')||'{}');}catch{}
const reconnect=new SpotLiteCore.ReconnectTracker(reconnectSaved);
function saveReconnect(){localStorage.setItem('reconnect-state',JSON.stringify(reconnect.snapshot()));}
let reconnectStage=localStorage.getItem('reconnect-stage')||'';
let reconnectBlocked=new URLSearchParams(location.search).has('login-failed');
const $=id=>document.getElementById(id);
const instance=Number(document.body.dataset.instance);
let version=0, machine=null, controller=null, controllerReady=false, apiPromise=null, createPromise=null, generation=0, timer=null, pollBusy=false, loginBusy=false;
let ackVersion=0, retryVersion=0, retryAt=0, failures=0, pendingReport=null, reporting=false, healthBusy=false, identityBusy=false;
let lastStartedUri=null;
let compact=false,paintedCompact=null;
let retired=false;
function paintLayout(){const small=compact&&!document.hasFocus();if(small===paintedCompact)return;paintedCompact=small;
  document.body.classList?.toggle('compact-player',small);
  const frame=$('embed').querySelector('iframe');if(frame)frame.setAttribute('height',small?'152':'352');
}
let mode=localStorage.getItem('loop-mode') || 'playlist';
const labels={off:'↻ Kapalı',playlist:'↻ Playlist',track:'↻¹ Şarkı'};
const states={playing:'Çalıyor',paused:'Duraklatıldı',ended:'Parça bitti',buffering:'Yükleniyor',stalled:'İlerlemiyor',unknown:'Veri bekleniyor',loading:'Oynatma bekleniyor',unverified_media:'Kısa/reklam medyası — doğrulanmadı'};
function notice(text){$('message').textContent=text;}
async function request(path,mutate=false){if(retired)throw Error('Eski oynatıcı kapatıldı');const r=await fetch(path,{method:mutate?'POST':'GET',headers:{'X-SpotLite-Player':document.body.dataset.client||'',...(mutate?{'X-SpotLite-Request':'1'}:{})},cache:'no-store',signal:AbortSignal.timeout(60000)});
  if(r.status===410&&!retired){retired=true;pendingReport=null;clearTimeout(timer);controller?.destroy();location.replace('about:blank');}
  if(!r.ok)throw Error(`Yerel bağlantı hatası (${r.status})`);return r;}
function paintMode(){if(!labels[mode])mode='off';$('loop').textContent=labels[mode];$('loop').setAttribute('aria-label',`Döngü: ${mode}`);localStorage.setItem('loop-mode',mode);if(machine)machine.mode=mode;}
$('loop').onclick=()=>{mode={off:'playlist',playlist:'track',track:'off'}[mode];paintMode();};paintMode();
$('url').value=localStorage.getItem('last-url') || '';
function api(){if(!apiPromise) apiPromise=new Promise((resolve,reject)=>{
  const timeout=setTimeout(()=>reject(Error('Spotify oynatıcı API yüklenemedi. Ağ bağlantısını kontrol edin.')),20000);
  window.onSpotifyIframeApiReady=value=>{clearTimeout(timeout);resolve(value);};
  const script=document.createElement('script');script.src='https://open.spotify.com/embed/iframe-api/v1';script.onerror=()=>{clearTimeout(timeout);reject(Error('Spotify API yüklenemedi'));};document.head.appendChild(script);
}).catch(e=>{apiPromise=null;throw e;});return apiPromise;}
async function getController(uri){if(createPromise)return createPromise;if(controller && controllerReady)return controller;
  createPromise=api().then(api=>new Promise((resolve,reject)=>{
    if(controller && !controllerReady){controller.destroy();controller=null;const mount=document.createElement('div');mount.id='embed-mount';$('embed').replaceChildren(mount);}
    const timeout=setTimeout(()=>{controllerReady=false;reject(Error('Spotify oynatıcısı hazır olmadı.'));},20000);
    // The API replaces the mount, not the wrapper. Force eager loading on the
    // resulting iframe: Spotify's default loading=lazy stalls parked windows.
    const eager=()=>{const frame=$('embed').querySelector('iframe');if(frame){frame.loading='eager';frame.setAttribute('loading','eager');}};
    api.createController($('embed-mount'),{uri,width:'100%',height:352},c=>{controller=c;eager();paintedCompact=null;paintLayout();
      c.addListener('playback_update',e=>observe(e.data || {}));
      c.addListener('playback_started',()=>{ /* Start signal is not proof of progress. */ });
      c.addListener('ready',()=>{clearTimeout(timeout);controllerReady=true;resolve(c);});
      c.addListener('error',e=>{notice('Spotify oynatıcı hatası: '+(e.data?.message || 'Oynatma başlatılamadı'));});
    });
    eager();
  })).finally(()=>{createPromise=null;});return createPromise;
}
async function report(s){
  // Backpressure: at most one request + one latest real sample per account.
  pendingReport={...s,version};if(reporting)return;reporting=true;
  try{while(pendingReport){const s=pendingReport;pendingReport=null;
    const q=new URLSearchParams({version:s.version,playing:s.playing?'1':'0',track:s.track,position:Math.round(s.position),duration:Math.round(s.duration),updated:s.updated,reason:s.reason || s.state,state:s.state});
    try{await request('/player-status?'+q,true);}catch{/* Latest real sample replaces obsolete samples. */}
  }}finally{reporting=false;}}
function observe(data){if(!machine)return;const s=machine.observe(data,Date.now());void report(s);paintStatus();
  const oldCount=reconnect.count,oldKnown=Object.keys(reconnect.known).length;
  reconnect.observe(data,Date.now(),machine.catalog.find(t=>t.uri===data.playingURI)?.duration||0);
  if(oldCount!==reconnect.count || oldKnown!==Object.keys(reconnect.known).length)saveReconnect();
  if(reconnect.healthy && reconnectStage){reconnectStage='';localStorage.removeItem('reconnect-stage');playerEvent('reconnect-verified');saveReconnect();}
  if(!loginBusy && reconnect.take(Date.now())){saveReconnect();void reconnectLogin();return;}
  if((s.state==='ended' || machine.resumePending) && !timer){const epoch=generation;timer=setTimeout(()=>{timer=null;
    if(epoch!==generation || !machine)return;const action=machine.resumeAction(Date.now()) || machine.repeatAction(Date.now());if(!action)return;
    // restart() resets the currently loaded collection as well. loadEntity(same
    // URI) alone is a no-op in Spotify's API, so always explicitly restart.
    try {if(action.type==='resume')controller.resume();else {if(action.type==='load')controller.loadEntity(action.uri);controller.restart();}}
    catch(e){notice('Döngü komutu uygulanamadı: '+e.message);}
  },1500);}
}
function paintStatus(){if(!machine || !document.hasFocus())return;const s=machine.view(Date.now());notice((states[s.state] || s.state)+(s.reason?' · '+s.reason:''));}
function playerEvent(event){void request('/player-event?event='+encodeURIComponent(event),true).catch(()=>{});}
function watchdog(){
  if(retired || loginBusy || !controllerReady || !machine)return;
  const action=machine.watchdogAction(Date.now());if(!action)return;
  playerEvent('repeat-resume-'+action.attempt);
  try{controller.resume();}catch{playerEvent('repeat-resume-error');}
  if(action.attempt===3)playerEvent('repeat-last-recovery-attempt');
}
setInterval(watchdog,1000);
async function reconnectLogin(){
  if(loginBusy)return;
  reconnectStage='connecting';localStorage.setItem('reconnect-stage',reconnectStage);
  try{await request('/player-event?event=reconnect-start',true);await login();}
  catch{reconnectStage='failed';localStorage.setItem('reconnect-stage',reconnectStage);playerEvent('reconnect-failed');}
}
async function begin(url,newMode){const uri=uriFrom(url);if(!uri)throw Error('Geçerli Spotify playlist veya şarkı bağlantısı girin.');
  const epoch=++generation;clearTimeout(timer);timer=null;mode=newMode || mode;paintMode();
  machine=new Playback(uri,mode);$('url').value=url;localStorage.setItem('last-url',url);notice('Spotify oynatıcısı yükleniyor…');
  const c=await getController(uri);if(epoch!==generation)return;
  // There is no native repeat setter in the Embed API.
  c.loadEntity(uri);if(lastStartedUri===uri)c.restart();else c.play();lastStartedUri=uri;
  request('/catalog?uri='+encodeURIComponent(uri)).then(r=>r.json()).then(data=>{
    if(epoch!==generation)return;
    machine.metadata(data);
    if(data.error)$('auth-status').textContent=data.error;
    if(uri.startsWith('spotify:playlist:') && !machine.verified) $('auth-status').textContent+=' · Playlist sonu doğrulanamıyor; doğal Spotify sırası korunuyor.';
  }).catch(e=>{if(epoch===generation)$('auth-status').textContent=e.message;});
}
$('play').onsubmit=async e=>{e.preventDefault();reconnectBlocked=false;try{await request('/set-command?'+new URLSearchParams({url:$('url').value,loop:mode}),true);await poll();}catch(e){notice(e.message);}};
async function login(commandVersion=0){if(loginBusy)return;loginBusy=true;$('login').disabled=true;notice('Spotify giriş sayfası hazırlanıyor…');
  reconnectStage='connecting';localStorage.setItem('reconnect-stage',reconnectStage);
  localStorage.setItem('reconnect-version',String(commandVersion||version));
  try {const result=await (await request('/auth-start',true)).json();if(!result.url)throw Error(result.error || 'Giriş adresi alınamadı.');
    if(commandVersion){await acknowledge(commandVersion);version=commandVersion;}
    location.assign(result.url); // Same Edge account profile, no popup or second shell.
  }catch(e){notice(e.message);loginBusy=false;$('login').disabled=false;throw e;}}
$('login').onclick=()=>{void login().catch(()=>{});};
$('web-login').onclick=e=>{e.preventDefault();const back=location.origin+'/';const spotify='https://open.spotify.com/';
  notice('Spotify aynı profil içinde açılıyor. Tarayıcının Geri düğmesiyle veya Alt+Sol ile SpotLite’a dönebilirsiniz.');
  location.assign('https://accounts.spotify.com/login?continue='+encodeURIComponent(spotify));};
async function identity(){ $('identity').textContent='Hesap '+instance;
  $('auth-status').textContent='Oturum yönetimi Spotify oynatıcısındadır.'; }
async function acknowledge(v){await request('/command-ack?version='+v,true);ackVersion=v;}
async function poll(){if(pollBusy || loginBusy)return;pollBusy=true;
  try{if(ackVersion<version)await acknowledge(version);
    const c=await(await request('/command?after='+version)).json();
    if(reconnectBlocked){if(c.version<=Number(localStorage.getItem('reconnect-version')||0))return;reconnectBlocked=false;}
    compact=!!c.compact;paintLayout();if(c.version>version){
      if(c.version!==retryVersion){retryVersion=c.version;failures=0;retryAt=0;}
      if(Date.now()<retryAt || failures>=3)return;
      if(c.login){await login(c.version);return;}
      if(c.loopOnly && machine){mode=c.loop;paintMode();}else if(c.url)await begin(c.url,c.loop);
      version=c.version;failures=0;await acknowledge(version);
    }}
  catch(e){failures++;retryAt=Date.now()+Math.min(30000,2000*2**failures);notice(e.message+(failures>=3?' · Yeni Başlat komutu bekleniyor.':''));}finally{pollBusy=false;}}
async function health(){if(healthBusy)return;healthBusy=true;try{await request('/player-health?'+new URLSearchParams({visibility:document.visibilityState,focused:document.hasFocus()?'1':'0',ready:controllerReady?'1':'0',loading:$('embed').querySelector('iframe')?.loading || '',repeats:machine?.repeatCount || 0}),true);}catch{}finally{healthBusy=false;}}
void identity();void health();setInterval(()=>{if(document.hasFocus())void identity();},30000);setInterval(poll,1000);setInterval(paintStatus,1000);setInterval(health,3000);
window.addEventListener?.('focus',()=>{paintLayout();paintStatus();void identity();});
window.addEventListener?.('blur',paintLayout);
window.addEventListener?.('online',()=>{
  failures=0;retryAt=0;
  void poll();
  // Restore only an interrupted repeat; never undo an ordinary user pause.
  if(machine?.repeatPending){machine.watchAttempts=0;machine.watchAt=0;watchdog();}
});
if(new URLSearchParams(location.search).has('login'))void login().catch(()=>{});else {
  if(new URLSearchParams(location.search).has('login-failed')){
    reconnectStage='failed';localStorage.setItem('reconnect-stage',reconnectStage);
    notice('Yeniden bağlantı tamamlanmadı. Spotify girişini kontrol edin.');playerEvent('reconnect-failed');
  }else {
    if(new URLSearchParams(location.search).has('login-complete')){void request('/park-self',true);if(reconnectStage)playerEvent('reconnect-returned');}
    void poll();
  }
}
