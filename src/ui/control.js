'use strict';
const $=id=>document.getElementById(id), rows=new Map(), deleted=new Set();
const loopLabels={off:'↻ Kapalı',playlist:'↻ Playlist',track:'↻¹ Şarkı'};
const stateLabels={playing:'● Şarkı çalıyor',paused:'○ Duraklatıldı',ended:'Döngü geçişi bekleniyor',buffering:'Bağlantı / medya bekleniyor',stalled:'İlerleme bekleniyor',unknown:'Güncel bağlantı verisi bekleniyor',loading:'Oynatma başlatılıyor',unverified_media:'Reklam / kısa medya — türü doğrulanmadı'};
let startupChecked=false;
$('autostart').checked=localStorage.getItem('auto-start')==='1';
$('autostart').onchange=()=>localStorage.setItem('auto-start',$('autostart').checked?'1':'0');
async function startup(data){
  if(startupChecked)return;startupChecked=true;
  if(localStorage.getItem('auto-start')!=='1')return;
  try{const s=await(await request('/session')).json();if(!s.boot || localStorage.getItem('auto-boot')===String(s.boot))return;
    localStorage.setItem('auto-boot',String(s.boot));
    for(const a of data){const row=rows.get(a.instance);if(a.running || !row || !SpotLiteCore.uriFrom(row.input.value))continue;
      try{await action('/control-account?'+new URLSearchParams({instance:a.instance,url:row.input.value,loop:row.mode}));}
      catch{message('Otomatik başlatılamadı: Hesap '+a.instance+'. Başlat ile yeniden deneyin.');}}
  }catch{startupChecked=false;}
}
let allMode=localStorage.getItem('all-mode') || 'playlist', polling=false,revision=0;
function cycle(m){return {off:'playlist',playlist:'track',track:'off'}[m] || 'off';}
function message(t){$('message').textContent=t;}
async function request(path,mutate=false){const r=await fetch(path,{method:mutate?'POST':'GET',headers:mutate?{'X-SpotLite-Request':'1'}:{},cache:'no-store',signal:AbortSignal.timeout(60000)});if(!r.ok)throw Error('Bağlantı hatası ('+r.status+')');return r;}
async function action(path){const text=await(await request(path,true)).text();if(text==='error'||text==='full'||text==='ipv6_unavailable')throw Error(text==='full'?'En fazla 9 hesap eklenebilir.':text==='ipv6_unavailable'?'IPv6 havuzunda boş ve bu bilgisayara atanmış bir adres bulunamadı.':'İşlem uygulanamadı; tanılama günlüğünü kontrol edin.');return text;}
function time(ms){if(!(ms>=0))return '--:--';const s=Math.floor(ms/1000);return Math.floor(s/60)+':'+String(s%60).padStart(2,'0');}
function storedHistory(key){try{const value=JSON.parse(localStorage.getItem(key)||'[]');return Array.isArray(value)?value.filter(e=>e&&Number.isFinite(e.at)&&typeof e.kind==='string').slice(-20):[];}catch{return [];}}
$('all-url').value=localStorage.getItem('all-url') || '';
$('all-loop').textContent=loopLabels[allMode] || loopLabels.off;
$('all-loop').onclick=()=>{allMode=cycle(allMode);$('all-loop').textContent=loopLabels[allMode];localStorage.setItem('all-mode',allMode);};
$('all').onsubmit=async e=>{e.preventDefault();const url=$('all-url').value;if(!SpotLiteCore.uriFrom(url)){message('Geçerli Spotify bağlantısı girin.');return;}
  localStorage.setItem('all-url',url);const b=e.submitter;b.disabled=true;message('Oynatma istekleri gönderiliyor…');
  try{const targets=[...rows].filter(([id])=>!deleted.has(id));
    const results=await Promise.allSettled(targets.map(async([id,row])=>{row.input.value=url;row.mode=allMode;row.loop.textContent=loopLabels[allMode];row.save();await action('/control-account?'+new URLSearchParams({instance:id,url,loop:allMode}));}));
    const failed=results.flatMap((r,i)=>r.status==='rejected'?[targets[i][0]]:[]);
    message(failed.length?'Başlatılamayan hesaplar: '+failed.join(', ')+'. Diğer hesaplara komut iletildi.':'Tüm hesaplara sessiz arka plan oynatma komutu iletildi. Gerçek ilerleme bekleniyor.');}
  catch(e){message(e.message);}finally{b.disabled=false;void refresh();}};
$('add').onclick=async()=>{const b=$('add');b.disabled=true;try{await action('/add-account');message('Hesap penceresinde Spotify girişini tamamlayın.');await refresh();}catch(e){message(e.message);}finally{b.disabled=false;}};
function createRow(id){const el=document.createElement('section');el.dataset.instance=id;
  el.innerHTML='<div class="line"><div><span class="number"></span><span class="name"></span></div><span class="badge"></span><div><button class="open">Aç</button> <button class="login">Spotify ile giriş</button> <button class="danger">Sil</button></div></div><div class="detail auth"></div><form class="commands"><input aria-label="Spotify bağlantısı" placeholder="Spotify bağlantısı" required><button type="button" class="loop"></button><button class="primary">Başlat</button></form><div class="timeline"><progress max="1" value="0"></progress><span class="time">0:00 / --:--</span></div><div class="detail reason"></div>';
  const get=s=>el.querySelector(s),row={el,input:get('input'),loop:get('.loop'),mode:'playlist',runtime:null,
    badge:get('.badge'),progress:get('progress'),time:get('.time'),reason:get('.reason'),name:get('.name'),auth:get('.auth')};
  try{const saved=JSON.parse(localStorage.getItem('account-'+id)||'{}');row.input.value=saved.url||'';row.mode=loopLabels[saved.mode]?saved.mode:'playlist';}catch{}
  get('.number').textContent='#'+id;row.loop.textContent=loopLabels[row.mode];
  row.save=()=>localStorage.setItem('account-'+id,JSON.stringify({url:row.input.value,mode:row.mode}));row.input.onchange=row.save;
  row.loop.onclick=async()=>{row.mode=cycle(row.mode);row.loop.textContent=loopLabels[row.mode];row.save();if(row.running){try{await action('/control-loop?'+new URLSearchParams({instance:id,loop:row.mode}));}catch(e){message(e.message);}}};
  get('form').onsubmit=async e=>{e.preventDefault();if(!SpotLiteCore.uriFrom(row.input.value)){message('Geçerli Spotify bağlantısı girin.');return;}row.save();e.submitter.disabled=true;
    try{await action('/control-account?'+new URLSearchParams({instance:id,url:row.input.value,loop:row.mode}));message('Komut iletildi; gerçek oynatma doğrulaması bekleniyor.');}catch(e){message(e.message);}finally{e.submitter.disabled=false;}};
  for(const cls of ['.login','.open'])get(cls).onclick=async()=>{try{await action((cls==='.login'?'/login-account?':'/open-account?')+'instance='+id);}catch(e){message(e.message);}};
  get('.danger').onclick=async()=>{if(!confirm('Bu hesabı ve kayıtlı oturumunu silmek istiyor musunuz?'))return;revision++;deleted.add(id);el.remove();
    try{await action('/delete-account?instance='+id);rows.delete(id);deleted.delete(id);localStorage.removeItem('account-'+id);localStorage.removeItem('health-'+id);message('Hesap silindi.');}
    catch(e){deleted.delete(id);$('accounts').append(el);message(e.message);}finally{revision++;void refresh();}};
  const credentials=document.createElement('details');
  credentials.innerHTML='<summary>Giriş bilgileri</summary><form class="credentials"><input class="email" aria-label="E-posta veya kullanıcı adı" placeholder="E-posta veya kullanıcı adı" autocomplete="off" required><input class="password" type="password" aria-label="Şifre" placeholder="Şifre" autocomplete="new-password" required><button>Kaydet</button></form><div class="detail credential-status">Bilgiler bu bilgisayarda düz metin olarak saklanır. Kaydetmek tek başına Spotify girişini tamamlamaz.</div>';
  const email=credentials.querySelector('.email'),password=credentials.querySelector('.password'),status=credentials.querySelector('.credential-status');
  credentials.ontoggle=async()=>{if(!credentials.open){password.value='';return;}try{const data=await(await request('/credentials?instance='+id+'&action=status',true)).json();email.value=data.email||'';password.placeholder=data.saved?'Kayıtlı şifreyi değiştirmek için yazın':'Şifre';status.textContent=data.saved?'Giriş bilgileri bu bilgisayarda kayıtlı.':'Henüz bilgi kaydedilmedi. Düz metin olarak bu bilgisayarda saklanacak.';}catch{status.textContent='Kayıt durumu alınamadı.';}};
  credentials.querySelector('form').onsubmit=async e=>{e.preventDefault();e.submitter.disabled=true;try{const r=await fetch('/credentials?instance='+id,{method:'POST',headers:{'X-SpotLite-Request':'1','Content-Type':'application/json'},body:JSON.stringify({email:email.value,password:password.value}),signal:AbortSignal.timeout(10000)});if(!r.ok||await r.text()!=='ok')throw Error();password.value='';status.textContent='Kaydedildi. Spotify oturumu ayrıca doğrulanacak.';}catch{status.textContent='Kaydedilemedi. Bilgileri kontrol edip tekrar deneyin.';}finally{e.submitter.disabled=false;}};
  el.append(credentials);
  const history=document.createElement('details');history.innerHTML='<summary>Son 20 olay</summary><div class="event-list"></div>';
  row.history=history.querySelector('.event-list');el.append(history);
  $('accounts').append(el);rows.set(id,row);return row;
}
function text(node,value){if(node.textContent!==value)node.textContent=value;}
function paintRuntime(row){const r=row.runtime||{},age=Date.now()-(r.updated||0),fresh=age<8000 && age>=-2000;
  const state=!row.running?'closed':fresh?(r.state||'unknown'):'unknown';
  const reconnectLabels={connecting:'Spotify’a yeniden bağlanıyor',interaction_required:'Spotify giriş ekranını kontrol et',captcha_required:'CAPTCHA gerekiyor — Aç ile tamamla',verification_required:'Doğrulama kodu gerekiyor — Aç ile tamamla',credentials_rejected:'Giriş bilgileri reddedildi',credentials_required:'Giriş bilgilerini kaydet',helper_unavailable:'Giriş yardımcısı bağlanamadı — uygulamayı yeniden aç',verifying:'Tam oynatma doğrulanıyor',failed:'Yeniden bağlantı tamamlanamadı'};
  const badge=row.badge;text(badge,state==='closed'?'Kapalı':reconnectLabels[r.reconnect]||(r.recovering?'Toparlanıyor':stateLabels[state]||'Güncel veri yok'));const cls='badge'+(fresh&&r.playing?' playing':'');if(badge.className!==cls)badge.className=cls;
  const pos=Math.min(r.duration||Infinity,(r.position||0)+(fresh&&r.playing?Math.min(2000,Math.max(0,age)):0));
  const progress=row.progress;const value=r.duration?pos/r.duration:0;if(progress.value!==value)progress.value=value;
  text(row.time,time(pos)+' / '+(r.duration?time(r.duration):'--:--'));
  const ago=r.lastAdvance?Math.max(0,Math.floor((Date.now()-r.lastAdvance)/1000))+' sn önce':'Henüz doğrulanmadı';
  text(row.auth,'Açık kalma: '+(r.started?time(Date.now()-r.started):'--:--')+' · Son gerçek ilerleme: '+ago+' · Döngü: '+(r.health?.repeats||0));
  const events=r.events||[];
  if(events.length){const key='health-'+row.el.dataset.instance,previous=storedHistory(key);
    const merged=[...previous,...events].filter((e,i,all)=>all.findIndex(x=>x.at===e.at&&x.kind===e.kind)===i).slice(-20);
    const serialized=JSON.stringify(merged);if(serialized!==localStorage.getItem(key))localStorage.setItem(key,serialized);
  }
  const history=storedHistory('health-'+row.el.dataset.instance);
  const names={buffering:'Medya beklendi',stalled:'İlerleme durdu',unverified_media:'Reklam/kısa medya',recovered:'İlerleme yeniden doğrulandı','repeat-resume-1':'Toparlama 1','repeat-resume-2':'Toparlama 2','repeat-resume-3':'Toparlama 3','repeat-resume-error':'Toparlama komutu hatası','repeat-last-recovery-attempt':'Son toparlama denemesi'};
  Object.assign(names,{'reconnect-start':'3 kısa oynatma: yeniden bağlantı','reconnect-failed':'Bağlantı yenilenemedi','reconnect-returned':'Yetkilendirmeden dönüldü','reconnect-verified':'Tam oynatma doğrulandı'});
  Object.assign(names,{'login-captcha_required':'CAPTCHA gerekiyor','login-verification_required':'Doğrulama kodu gerekiyor','login-credentials_rejected':'Giriş bilgileri reddedildi','login-credentials_required':'Kayıtlı giriş bilgisi yok','login-connecting':'Giriş yardımcısı başladı','login-verifying':'Giriş dönüşü: oynatma doğrulanıyor','login-interaction_required':'Giriş ekranında kullanıcı işlemi gerekiyor'});
  const formatted=history.map(e=>new Date(e.at).toLocaleString('tr-TR')+' '+(names[e.kind]||e.kind));
  text(row.reason,formatted.at(-1)||'Henüz hata veya toparlama olayı kaydedilmedi.');
  text(row.history,formatted.join('\n')||'Kayıt yok.');
}
async function refresh(){if(polling)return;polling=true;const epoch=revision;try{const data=await(await request('/accounts')).json();if(epoch!==revision)return;const seen=new Set();
  for(const a of data){if(deleted.has(a.instance))continue;seen.add(a.instance);const row=rows.get(a.instance)||createRow(a.instance);
    row.running=a.running;row.runtime=a.runtime;text(row.name,'Hesap '+a.instance+(a.ipv6?' · IPv6 '+a.ipv6:''));
    paintRuntime(row);}
  void startup(data);
  for(const [id,row]of rows)if(!seen.has(id)&&!deleted.has(id)){row.el.remove();rows.delete(id);}
}catch(e){message(e.message);}finally{polling=false;}}
void refresh();setInterval(()=>{if(!document.hidden)void refresh();},3000);setInterval(()=>{if(!document.hidden)for(const row of rows.values())paintRuntime(row);},1000);
document.addEventListener('visibilitychange',()=>{if(!document.hidden)void refresh();});
