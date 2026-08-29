#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Command, ExitCode},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

const PLAYER_URL: &str = "https://open.spotify.com/";
const TOKENS_CSS: &str = include_str!("../tokens.css");
const AUTH_CALLBACK_HTML: &str = r#"<!doctype html>
<meta charset="utf-8">
<title>Spotify girişi</title>
<style>body{font:15px Segoe UI,sans-serif;background:#111;color:#fff;display:grid;place-items:center;min-height:90vh}b{color:#1ed760}</style>
<p id="status"><b>Spotify girişi tamamlanıyor…</b></p>
<script>
  const status = document.getElementById('status');
  const params = new URLSearchParams(location.search);
  const code = params.get('code');
  const rawState = params.get('state') || '';

  function decode(value) {
    const padded = value.replace(/-/g, '+').replace(/_/g, '/') + '='.repeat((4 - value.length % 4) % 4);
    return JSON.parse(new TextDecoder().decode(Uint8Array.from(atob(padded), c => c.charCodeAt(0))));
  }

  (async () => {
    try {
      const state = decode(rawState);
      if (!code || !state.verifier) throw new Error('Giriş kodu alınamadı.');
      const body = new URLSearchParams({
        client_id: '65b708073fc0480ea92a077233ca87bd',
        grant_type: 'authorization_code',
        code,
        redirect_uri: 'http://127.0.0.1:5588/login',
        code_verifier: state.verifier
      });
      const tokenResponse = await fetch('https://accounts.spotify.com/api/token', {
        method: 'POST',
        headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
        body
      });
      if (!tokenResponse.ok) throw new Error('Spotify oturumu doğrulanamadı.');
      const token = await tokenResponse.json();
      const returnPort = Number(state.returnPort);
      if (!Number.isInteger(returnPort) || returnPort < 43117 || returnPort > 43125) {
        throw new Error('SpotLite penceresi doğrulanamadı.');
      }
      let label = `Hesap ${returnPort - 43116}`;
      for (let attempt = 0; attempt < 3; attempt += 1) {
        try {
          const profileResponse = await fetch('https://api.spotify.com/v1/me', {
            headers: { Authorization: `Bearer ${token.access_token}` },
            signal: AbortSignal.timeout(6000)
          });
          if (profileResponse.ok) {
            const profile = await profileResponse.json();
            label = profile.email || label;
            break;
          }
        } catch (_) {}
        await new Promise(resolve => setTimeout(resolve, 400));
      }
      const sameWindow = state.sameWindow ? '1' : '0';
      location.replace(`http://127.0.0.1:${returnPort}/auth-complete?label=${encodeURIComponent(label)}&same=${sameWindow}`);
    } catch (error) {
      status.textContent = error.message || 'Spotify girişi tamamlanamadı.';
    }
  })();
</script>"#;
const AUTH_COMPLETE_HTML: &str = r#"<!doctype html>
<meta charset="utf-8">
<title>SpotLite Free</title>
<style>body{font:15px Segoe UI,sans-serif;background:#111;color:#fff;display:grid;place-items:center;min-height:90vh}b{color:#1ed760}button{border:0;border-radius:18px;padding:10px 18px;background:#1ed760;font-weight:700}</style>
<div><p><b>Giriş tamamlandı.</b> SpotLite’a dönülüyor…</p><button onclick="window.close()">SpotLite’a dön</button></div>
<script>
  const params = new URLSearchParams(location.search);
  const label = params.get('label') || 'Spotify hesabı';
  localStorage.setItem('spotlite-account', label);
  setTimeout(() => params.get('same') === '1' ? location.replace('/?login-complete=1') : window.close(), 350);
</script>"#;

#[derive(Default)]
struct PlaybackCommand {
    version: u64,
    url: String,
    loop_mode: String,
    login: bool,
    playing: bool,
    starting: bool,
    track: String,
    position_ms: u64,
    duration_ms: u64,
    updated_ms: u64,
}

const CONTROL_HTML: &str = r#"<!doctype html>
<html lang="tr">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>SpotLite Free — Kontrol Paneli</title>
  <link rel="stylesheet" href="/tokens.css">
  <style>
    :root { color-scheme: dark; font-family: var(--font-body); }
    * { box-sizing: border-box; }
    html, body { overflow-x: clip; }
    body { margin: 0; min-height: 100dvh; background: var(--color-paper); color: var(--color-ink); }
    main { width: min(920px, calc(100% - 32px)); margin: 0 auto; padding: 28px 0 40px; }
    header { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 20px; }
    h1 { margin: 0; color: var(--color-accent); font: 700 22px/1.2 var(--font-display); }
    .muted, .status { color: var(--color-ink-2); font-size: 13px; }
    .playback-badge { min-width: 112px; border: var(--rule-thin) solid var(--color-rule); border-radius: var(--radius-pill); padding: 8px 12px; background: var(--color-paper-3); color: var(--color-ink-2); font-size: 12px; font-weight: 800; text-align: center; white-space: nowrap; }
    .playback-badge.playing { border-color: var(--color-accent); color: var(--color-accent); }
    .playback-badge.starting { color: var(--color-ink); }
    .progress-row { display: grid; grid-template-columns: minmax(0, 1fr) auto; align-items: center; gap: 10px; margin-top: 10px; }
    .progress { height: 5px; overflow: hidden; border-radius: var(--radius-pill); background: var(--color-paper-3); }
    .progress span { display: block; width: 100%; height: 100%; border-radius: inherit; background: var(--color-accent); transition: transform var(--dur-short) linear; transform-origin: left center; }
    .time { min-width: 82px; color: var(--color-ink-2); font: 600 12px/1 var(--font-body); text-align: right; }
    .all, .account-card { border: var(--rule-thin) solid var(--color-rule); background: var(--color-paper-2); border-radius: var(--radius-md); }
    .all { padding: 14px; margin-bottom: 12px; }
    .account-card { padding: 14px; margin-top: 8px; }
    .card-head { display: flex; align-items: center; justify-content: space-between; gap: 10px; margin-bottom: 10px; }
    .identity { min-width: 0; }
    .account-heading { display: flex; align-items: center; gap: 9px; min-width: 0; }
    .account-number { flex: 0 0 auto; min-width: 30px; border: var(--rule-thin) solid var(--color-rule); border-radius: var(--radius-pill); padding: 4px 7px; color: var(--color-accent); font-size: 12px; font-weight: 800; text-align: center; }
    .identity strong { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .controls { display: grid; grid-template-columns: minmax(180px, 1fr) 150px auto; gap: 8px; }
    input, select, button { min-height: 40px; border-radius: var(--radius-pill); font: 600 13px/1 var(--font-body); }
    input, select { min-width: 0; border: var(--rule-thin) solid var(--color-rule); background: var(--color-paper-3); color: var(--color-ink); padding: 0 14px; }
    button { border: 0; padding: 0 16px; background: var(--color-accent); color: var(--color-accent-ink); cursor: pointer; white-space: nowrap; }
    button.secondary { background: var(--color-paper-3); color: var(--color-ink); }
    button.danger { color: var(--color-danger); }
    button:disabled { opacity: .5; cursor: wait; }
    input:focus-visible, select:focus-visible, button:focus-visible { outline: 2px solid var(--color-focus); outline-offset: 2px; }
    .actions { display: flex; gap: 6px; }
    .actions button { min-height: 34px; padding: 0 12px; }
    #empty { padding: 32px 12px; text-align: center; color: var(--color-ink-2); }
    dialog { width: min(360px, calc(100% - 32px)); border: var(--rule-thin) solid var(--color-rule); border-radius: var(--radius-md); padding: 20px; background: var(--color-paper-2); color: var(--color-ink); }
    dialog::backdrop { background: var(--color-overlay); }
    dialog h2 { margin: 0 0 12px; font-size: 18px; }
    dialog p { color: var(--color-ink-2); font-size: 14px; }
    dialog input { width: 100%; }
    .dialog-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 16px; }
    @media (max-width: 680px) { .controls { grid-template-columns: 1fr; } header { align-items: flex-start; } .card-head { flex-wrap: wrap; } .actions { width: 100%; } }
  </style>
</head>
<body>
  <main>
    <header>
      <div><h1>SpotLite Free</h1><div class="muted">Bütün hesaplar için ana kontrol paneli</div></div>
      <button id="add" class="secondary">＋ Hesap ekle</button>
    </header>
    <section class="all" aria-label="Bütün hesaplar">
      <div class="card-head"><strong>Bütün hesaplara uygula</strong><span id="panel-status" class="status"></span></div>
      <div class="controls">
        <input id="all-url" placeholder="Spotify playlist veya şarkı bağlantısı" aria-label="Bütün hesapların bağlantısı">
        <select id="all-loop" aria-label="Bütün hesapların döngüsü"><option value="playlist">Playlist döngüsü</option><option value="track">Şarkı döngüsü</option><option value="off">Döngü kapalı</option></select>
        <button id="apply-all">Tümünde başlat</button>
      </div>
    </section>
    <section id="accounts" aria-live="polite"></section>
    <div id="empty" hidden>Henüz hesap yok. “Hesap ekle” ile Spotify girişi yapabilirsin.</div>
  </main>
  <dialog id="delete-dialog"><form method="dialog" id="delete-form"><h2>Hesabı sil</h2><p id="delete-copy"></p><div class="dialog-actions"><button value="cancel" class="secondary">Vazgeç</button><button value="default" class="danger">Hesabı sil</button></div></form></dialog>
  <script>
    const list = document.getElementById('accounts');
    const empty = document.getElementById('empty');
    const panelStatus = document.getElementById('panel-status');
    const deleteDialog = document.getElementById('delete-dialog');
    let accounts = [];
    let selected = null;
    const runtimeCache = new Map();
    const deletingAccounts = new Set();

    function saved(instance, key, fallback = '') { return localStorage.getItem(`spotlite-${key}-${instance}`) || fallback; }
    function setStatus(text) { panelStatus.textContent = text; }
    function formatTime(milliseconds) {
      const seconds = Math.max(0, Math.floor(Number(milliseconds || 0) / 1000));
      return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
    }
    function accountStatus(item) {
      if (item.runtime?.playing) return item.runtime.track ? `● Çalıyor · ${item.runtime.track}` : '● Çalıyor';
      if (item.runtime?.starting) return '◌ Başlatılıyor';
      return item.running ? '○ Çalmıyor' : '— Kapalı';
    }

    function updateProgressBars() {
      const now = Date.now();
      for (const card of list.querySelectorAll('.account-card')) {
        const duration = Number(card.dataset.duration || 0);
        let position = Number(card.dataset.position || 0);
        if (card.dataset.playing === '1') position += Math.max(0, now - Number(card.dataset.updated || now));
        position = duration > 0 ? Math.min(position, duration) : 0;
        const ratio = duration > 0 ? Math.min(1, Math.max(0, position / duration)) : 0;
        const progress = card.querySelector('.progress');
        progress.setAttribute('aria-valuenow', String(Math.round(position)));
        progress.querySelector('span').style.transform = `scaleX(${ratio})`;
        card.querySelector('.time').textContent = `${formatTime(position)} / ${duration > 0 ? formatTime(duration) : '--:--'}`;
      }
    }

    function render() {
      list.replaceChildren();
      empty.hidden = accounts.length > 0;
      for (const item of accounts) {
        const card = document.createElement('article');
        card.className = 'account-card';
        card.dataset.instance = item.instance;
        card.innerHTML = `<div class="card-head"><div class="identity"><div class="account-heading"><span class="account-number"></span><strong></strong></div><span class="status"></span></div><span class="playback-badge"></span><div class="actions"><button class="login secondary">${item.loggedIn ? 'Girişi yenile' : 'Spotify ile giriş'}</button><button class="remove secondary danger">Sil</button></div></div><div class="controls"><input class="url" placeholder="Playlist veya şarkı bağlantısı"><select class="loop"><option value="playlist">Playlist döngüsü</option><option value="track">Şarkı döngüsü</option><option value="off">Döngü kapalı</option></select><button class="start">Başlat</button></div><div class="progress-row"><div class="progress" role="progressbar" aria-label="Şarkı ilerlemesi"><span></span></div><span class="time"></span></div>`;
        card.querySelector('.account-number').textContent = `#${item.instance}`;
        card.querySelector('strong').textContent = item.name;
        card.querySelector('.status').textContent = item.loggedIn ? 'Spotify girişi yapıldı' : 'Spotify oturumu doğrulanmadı';
        const playbackBadge = card.querySelector('.playback-badge');
        playbackBadge.textContent = accountStatus(item);
        playbackBadge.classList.toggle('playing', Boolean(item.runtime?.playing));
        playbackBadge.classList.toggle('starting', Boolean(item.runtime?.starting));
        const basePosition = Number(item.runtime?.position || 0);
        const duration = Number(item.runtime?.duration || 0);
        const updated = Number(item.runtime?.updated || Date.now());
        const playing = Boolean(item.runtime?.playing);
        const position = playing && duration > 0
          ? Math.min(basePosition + Math.max(0, Date.now() - updated), duration)
          : basePosition;
        card.dataset.position = String(basePosition);
        card.dataset.duration = String(duration);
        card.dataset.updated = String(updated);
        card.dataset.playing = playing ? '1' : '0';
        const ratio = duration > 0 ? Math.min(1, Math.max(0, position / duration)) : 0;
        const progress = card.querySelector('.progress');
        progress.setAttribute('aria-valuemin', '0');
        progress.setAttribute('aria-valuemax', String(duration));
        progress.setAttribute('aria-valuenow', String(position));
        progress.querySelector('span').style.transform = `scaleX(${ratio})`;
        card.querySelector('.time').textContent = `${formatTime(position)} / ${duration > 0 ? formatTime(duration) : '--:--'}`;
        const url = card.querySelector('.url');
        const loop = card.querySelector('.loop');
        url.value = saved(item.instance, 'url');
        loop.value = saved(item.instance, 'loop', 'playlist');
        card.querySelector('.start').onclick = () => play(item, url, loop);
        url.onkeydown = event => { if (event.key === 'Enter') play(item, url, loop); };
        const loginButton = card.querySelector('.login');
        loginButton.onclick = async () => {
          loginButton.disabled = true;
          await login(item);
          setTimeout(() => { loginButton.disabled = false; }, 1200);
        };
        card.querySelector('.remove').onclick = () => { selected = item; document.getElementById('delete-copy').textContent = `${item.name} ve kayıtlı Spotify oturumu silinecek.`; deleteDialog.showModal(); };
        list.append(card);
      }
    }

    async function refresh() {
      const response = await fetch('/accounts', { cache: 'no-store' });
      accounts = (await response.json()).filter(item => !deletingAccounts.has(item.instance)).map(item => {
        const previous = runtimeCache.get(item.instance);
        const incoming = item.runtime || { playing: false, starting: false, track: '', position: 0, duration: 0, updated: 0 };
        const incomingUpdated = Number(incoming.updated || 0);
        const previousUpdated = Number(previous?.updated || 0);
        const lostTimeline = previous && !incoming.starting && Number(previous.duration || 0) > 0 && Number(incoming.duration || 0) === 0;
        if (item.running && previous && (lostTimeline || incomingUpdated === 0 || incomingUpdated < previousUpdated)) {
          item.runtime = previous;
        } else {
          item.runtime = incoming;
          runtimeCache.set(item.instance, incoming);
        }
        return item;
      });
      render();
    }

    async function play(item, urlInput, loopSelect) {
      const url = urlInput.value.trim();
      if (!url) { setStatus(`${item.name}: bağlantı gerekli.`); return; }
      localStorage.setItem(`spotlite-url-${item.instance}`, url);
      localStorage.setItem(`spotlite-loop-${item.instance}`, loopSelect.value);
      setStatus(`${item.name} başlatılıyor…`);
      const response = await fetch(`/control-account?instance=${item.instance}&url=${encodeURIComponent(url)}&loop=${loopSelect.value}`, { method: 'POST' });
      const result = await response.text();
      setStatus(result === 'ok' ? `${item.name} arka planda başlatıldı.` : `${item.name} başlatılamadı.`);
      setTimeout(refresh, 500);
    }

    async function login(item) {
      setStatus(`${item.name} için Spotify girişi açılıyor…`);
      try {
        const response = await fetch(`/login-account?instance=${item.instance}`, { method: 'POST' });
        const result = await response.text();
        setStatus(result === 'opened'
          ? `${item.name} için Spotify giriş ekranı açıldı.`
          : `${item.name} giriş ekranı açılamadı. Tekrar deneyin.`);
      } catch (_) {
        setStatus(`${item.name} giriş ekranı açılamadı. Tekrar deneyin.`);
      }
    }

    document.getElementById('apply-all').onclick = async () => {
      const url = document.getElementById('all-url').value.trim();
      const loop = document.getElementById('all-loop').value;
      if (!url || !accounts.length) { setStatus('Bağlantı ve en az bir hesap gerekli.'); return; }
      for (const item of accounts) {
        localStorage.setItem(`spotlite-url-${item.instance}`, url);
        localStorage.setItem(`spotlite-loop-${item.instance}`, loop);
      }
      setStatus(`${accounts.length} hesap başlatılıyor…`);
      const response = await fetch(`/control-all?url=${encodeURIComponent(url)}&loop=${loop}`, { method: 'POST' });
      const result = await response.text();
      setStatus(result === 'ok' ? `${accounts.length} hesaba uygulandı.` : 'Bazı hesaplar başlatılamadı.');
      await refresh();
    };

    document.getElementById('add').onclick = async () => {
      const addButton = document.getElementById('add');
      addButton.disabled = true;
      setStatus('Yeni Spotify hesabı açılıyor…');
      try {
        const response = await fetch('/add-account', { method: 'POST' });
        const result = await response.text();
        if (result.startsWith('opened:')) {
          const number = result.split(':')[1];
          setStatus(`Hesap ${number} için Spotify giriş penceresi açıldı.`);
          setTimeout(refresh, 800);
        } else if (result === 'full') {
          setStatus('Dokuz hesap yuvası dolu.');
        } else {
          setStatus('Spotify giriş penceresi açılamadı. Tekrar deneyin.');
        }
      } catch (_) {
        setStatus('Spotify giriş penceresi açılamadı. Tekrar deneyin.');
      } finally {
        setTimeout(() => { addButton.disabled = false; }, 1200);
      }
    };
    document.getElementById('delete-form').onsubmit = async event => {
      event.preventDefault();
      if (event.submitter?.value === 'cancel') { deleteDialog.close(); return; }
      if (!selected) return;
      const deleting = selected; selected = null; deleteDialog.close();
      deletingAccounts.add(deleting.instance);
      runtimeCache.delete(deleting.instance);
      localStorage.removeItem(`spotlite-url-${deleting.instance}`);
      localStorage.removeItem(`spotlite-loop-${deleting.instance}`);
      accounts = accounts.filter(item => item.instance !== deleting.instance); render();
      setStatus(`${deleting.name} silindi.`);
      try {
        const response = await fetch(`/delete-account?instance=${deleting.instance}`, { method: 'POST' });
        if (!response.ok || await response.text() !== 'deleted') throw new Error('Silme başarısız.');
        for (let attempt = 0; attempt < 20; attempt += 1) {
          await new Promise(resolve => setTimeout(resolve, 250));
          const check = await fetch('/accounts', { cache: 'no-store' });
          const remaining = await check.json();
          if (!remaining.some(item => item.instance === deleting.instance)) {
            deletingAccounts.delete(deleting.instance);
            return;
          }
        }
        throw new Error('Silme tamamlanmadı.');
      } catch (_) {
        deletingAccounts.delete(deleting.instance);
        setStatus(`${deleting.name} silinemedi. Tekrar deneyin.`);
        await refresh();
      }
    };
    deleteDialog.onclick = event => { if (event.target === deleteDialog) deleteDialog.close(); };
    refresh();
    setInterval(refresh, 2000);
    setInterval(updateProgressBars, 250);
  </script>
</body>
</html>"#;
const LITE_HTML: &str = r#"<!doctype html>
<html lang="tr">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>SpotLite Free — Hesap {{INSTANCE}}</title>
  <link rel="stylesheet" href="/tokens.css">
  <style>
    /* Hallmark · genre: modern-minimal · macrostructure: Workbench · theme: Quiet-dark · designed-as-app */
    :root { color-scheme: dark; font-family: var(--font-body); }
    * { box-sizing: border-box; }
    html, body { overflow-x: clip; }
    body { margin: 0; min-height: 100dvh; display: grid; place-items: center; background: var(--color-paper); color: var(--color-ink); }
    main { width: min(700px, calc(100% - var(--space-lg))); padding: var(--space-lg); }
    h1 { margin: 0 0 var(--space-sm); color: var(--color-accent); font-family: var(--font-display); font-size: var(--text-md); letter-spacing: -0.02em; overflow-wrap: anywhere; }
    .row { display: flex; gap: var(--space-xs); min-width: 0; }
    input { flex: 1; min-width: 100px; height: 44px; border: var(--rule-thin) solid var(--color-rule); border-radius: var(--radius-pill); background: var(--color-paper-3); color: var(--color-ink); padding: 0 var(--space-md); outline: 2px solid transparent; outline-offset: 1px; }
    input::placeholder { color: var(--color-muted); }
    input:focus-visible, button:focus-visible { outline: 2px solid var(--color-focus); outline-offset: 2px; }
    button { min-height: 44px; border: 0; border-radius: var(--radius-pill); padding: 0 var(--space-md); background: var(--color-accent); color: var(--color-accent-ink); font: 700 var(--text-sm)/1 var(--font-body); cursor: pointer; white-space: nowrap; transition: transform var(--dur-micro) var(--ease-out), background-color var(--dur-short) var(--ease-out); }
    button:active { transform: translateY(1px); }
    button:disabled { opacity: .5; cursor: not-allowed; }
    button[aria-busy="true"] { color: var(--color-ink-2); cursor: wait; }
    input[aria-invalid="true"] { border-color: var(--color-danger); }
    input[data-state="success"] { border-color: var(--color-accent); }
    #loop { background: var(--color-paper-3); color: var(--color-ink); }
    #loop.active { outline: var(--rule-thin) solid var(--color-accent); color: var(--color-accent); }
    .account-row { display: flex; align-items: center; gap: var(--space-xs); margin-top: var(--space-xs); }
    #account, #status { min-height: 20px; margin: 0 var(--space-3xs); color: var(--color-ink-2); font-size: var(--text-sm); }
    #account { flex: 1; }
    #status { margin-top: var(--space-2xs); }
    #login { min-height: 36px; background: var(--color-ink); color: var(--color-paper); }
    .menu-toggle { position: fixed; inset-block-start: var(--space-sm); inset-inline-start: var(--space-sm); z-index: var(--z-raised); width: 44px; padding: 0; display: grid; place-items: center; background: var(--color-paper-3); color: var(--color-ink); }
    .menu-toggle svg, .drawer-close svg, .icon-button svg { width: 18px; height: 18px; fill: none; stroke: currentColor; stroke-width: 1.8; stroke-linecap: round; stroke-linejoin: round; }
    .drawer-backdrop { position: fixed; inset: 0; z-index: var(--z-raised); background: var(--color-overlay); opacity: 0; pointer-events: none; transition: opacity var(--dur-short) var(--ease-in); }
    .drawer { position: fixed; inset-block: 0; inset-inline-start: 0; z-index: calc(var(--z-raised) + 1); width: min(300px, calc(100% - var(--space-xl))); display: flex; flex-direction: column; background: var(--color-paper-2); border-inline-end: var(--rule-thin) solid var(--color-rule); transform: translateX(-100%); transition: transform var(--dur-long) var(--ease-in); }
    body.drawer-open .drawer { transform: translateX(0); transition-timing-function: var(--ease-out); }
    body.drawer-open .drawer-backdrop { opacity: 1; pointer-events: auto; transition-timing-function: var(--ease-out); }
    .drawer-head { display: flex; align-items: center; justify-content: space-between; min-height: 64px; padding: var(--space-sm) var(--space-md); border-block-end: var(--rule-thin) solid var(--color-rule); }
    .drawer-head strong { font-family: var(--font-display); font-size: var(--text-base); }
    .drawer-close, .icon-button { width: 44px; padding: 0; display: grid; place-items: center; background: transparent; color: var(--color-ink-2); }
    .account-list { display: flex; flex-direction: column; gap: var(--space-2xs); padding: var(--space-sm); overflow-y: auto; }
    .account-item { display: flex; align-items: center; min-height: 48px; border-radius: var(--radius-md); }
    .account-item.current { background: var(--color-paper-3); }
    .account-open { flex: 1; min-width: 0; justify-content: flex-start; overflow: hidden; text-overflow: ellipsis; background: transparent; color: var(--color-ink); font-weight: 600; text-align: start; }
    .account-actions { display: flex; opacity: 0; pointer-events: none; }
    .account-item:focus-within .account-actions { opacity: 1; pointer-events: auto; }
    .icon-button.danger { color: var(--color-danger); }
    .drawer-foot { margin-top: auto; padding: var(--space-sm); border-block-start: var(--rule-thin) solid var(--color-rule); }
    #add-account { width: 100%; justify-content: center; background: var(--color-paper-3); color: var(--color-ink); }
    dialog { width: min(360px, calc(100% - var(--space-lg))); border: var(--rule-thin) solid var(--color-rule); border-radius: var(--radius-md); padding: var(--space-lg); background: var(--color-paper-2); color: var(--color-ink); }
    dialog::backdrop { background: var(--color-overlay); }
    dialog h2 { margin: 0 0 var(--space-sm); font: 700 var(--text-md)/1.2 var(--font-display); }
    dialog p { margin: 0 0 var(--space-md); color: var(--color-ink-2); font-size: var(--text-sm); line-height: 1.5; }
    dialog label { display: grid; gap: var(--space-2xs); color: var(--color-ink-2); font-size: var(--text-sm); }
    dialog input { width: 100%; }
    .dialog-actions { display: flex; justify-content: flex-end; gap: var(--space-xs); margin-top: var(--space-md); }
    .secondary { background: var(--color-paper-3); color: var(--color-ink); }
    .danger-action { background: var(--color-danger); color: var(--color-danger-ink); }
    #embed-shell { position: fixed; left: 0; bottom: 0; width: 400px; height: 352px; transform: scale(.002); transform-origin: left bottom; opacity: .01; pointer-events: none; }
    @media (hover: hover) and (pointer: fine) { .account-item:hover .account-actions { opacity: 1; pointer-events: auto; } }
    @media (pointer: coarse) { .account-actions { opacity: 1; pointer-events: auto; } }
    @media (max-width: 40rem) { .row { flex-direction: column; } main { padding-block-start: var(--space-xl); } }
    @media (prefers-reduced-motion: reduce) { *, *::before, *::after { transition-duration: 150ms !important; animation-duration: 150ms !important; } }
  </style>
</head>
<body>
  <button id="menu-toggle" class="menu-toggle" aria-label="Hesap menüsünü aç" aria-expanded="false">
    <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 7h16M4 12h16M4 17h16"/></svg>
  </button>
  <div id="drawer-backdrop" class="drawer-backdrop"></div>
  <aside id="drawer" class="drawer" aria-label="Hesaplar" aria-hidden="true">
    <div class="drawer-head">
      <strong>Hesaplar</strong>
      <button id="drawer-close" class="drawer-close" aria-label="Hesap menüsünü kapat">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6 6 12 12M18 6 6 18"/></svg>
      </button>
    </div>
    <div id="account-list" class="account-list"></div>
    <div class="drawer-foot"><button id="add-account">＋ Hesap ekle</button></div>
  </aside>
  <main>
    <h1 id="app-title">SpotLite Free — Hesap {{INSTANCE}}</h1>
    <div class="row">
      <input id="url" aria-label="Playlist bağlantısı" placeholder="Spotify playlist bağlantısını yapıştır">
      <button id="start">Başlat ve döngüye al</button>
      <button id="loop" class="active">Döngü: Playlist</button>
    </div>
    <div class="account-row">
      <div id="account">Hesap: giriş durumu henüz doğrulanmadı.</div>
      <button id="login">Spotify ile giriş yap</button>
    </div>
    <div id="status">Playlist bağlantısı bekleniyor.</div>
  </main>
  <dialog id="delete-dialog">
    <form method="dialog" id="delete-form">
      <h2>Hesabı sil</h2>
      <p id="delete-copy"></p>
      <div class="dialog-actions"><button value="cancel" class="secondary">Vazgeç</button><button value="default" id="delete-save" class="danger-action">Hesabı sil</button></div>
    </form>
  </dialog>
  <div id="embed-shell"></div>
  <script src="https://open.spotify.com/embed/iframe-api/v1" async></script>
  <script>
    const input = document.getElementById('url');
    const account = document.getElementById('account');
    const login = document.getElementById('login');
    const loopButton = document.getElementById('loop');
    const status = document.getElementById('status');
    const menuToggle = document.getElementById('menu-toggle');
    const drawer = document.getElementById('drawer');
    const accountList = document.getElementById('account-list');
    const deleteDialog = document.getElementById('delete-dialog');
    let api = null;
    let controller = null;
    let playlist = '';
    let currentTrack = '';
    let endTimer = 0;
    let trackTimer = 0;
    let repeatTarget = '';
    let metadataRequest = 0;
    let pendingStart = false;
    let loadTimer = 0;
    let playbackTimer = 0;
    let commandVersion = 0;
    let lastPlaybackReport = 0;
    let reportedPlaying = null;
    let reportedTrack = '';
    let reportedPosition = 0;
    let playbackIsPlaying = false;
    let lastPlaybackPosition = -1;
    let playbackPosition = 0;
    let playbackDuration = 0;
    let playlistRestarting = false;
    let loginPopup = null;
    let knownAccounts = [];
    const deletingAccounts = new Set();
    const savedAccount = localStorage.getItem('spotlite-account');
    let accountLabel = savedAccount || '';
    let loopMode = localStorage.getItem('spotlite-loop-mode') || 'playlist';
    const currentInstance = {{INSTANCE}};
    let selectedAccount = null;
    if (savedAccount) {
      account.textContent = `Hesap: ${savedAccount}`;
      login.hidden = true;
      rememberSpotifyName(savedAccount);
    }

    function setDrawer(open) {
      document.body.classList.toggle('drawer-open', open);
      menuToggle.setAttribute('aria-expanded', String(open));
      drawer.setAttribute('aria-hidden', String(!open));
      if (open) document.getElementById('drawer-close').focus();
      else menuToggle.focus();
    }

    async function rememberSpotifyName(name) {
      await fetch(`/remember-account?label=${encodeURIComponent(name)}`, { method: 'POST' }).catch(() => {});
    }

    async function openAccount(number) {
      status.textContent = `Hesap ${number} seçiliyor…`;
      const response = await fetch(`/open-account?instance=${number}`, { cache: 'no-store' });
      const result = await response.text();
      status.textContent = result === 'current'
        ? `Zaten bu hesaptasın.`
        : result === 'focused'
          ? `Hesap ${number} öne getirildi.`
          : `Hesap ${number} açıldı.`;
      setDrawer(false);
    }

    function icon(path) {
      return `<svg viewBox="0 0 24 24" aria-hidden="true"><path d="${path}"/></svg>`;
    }

    async function refreshAccounts() {
      const response = await fetch('/accounts', { cache: 'no-store' });
      const accounts = (await response.json()).filter(item => !deletingAccounts.has(item.instance));
      knownAccounts = accounts;
      accountList.replaceChildren();
      for (const item of accounts) {
        if (item.instance === currentInstance) {
          document.getElementById('app-title').textContent = `SpotLite Free — ${item.name}`;
          document.title = `SpotLite Free — ${item.name}`;
        }
        const row = document.createElement('div');
        row.className = `account-item${item.instance === currentInstance ? ' current' : ''}`;
        row.dataset.instance = item.instance;

        const open = document.createElement('button');
        open.className = 'account-open';
        open.textContent = `#${item.instance}  ${item.name}`;
        open.title = item.name;
        open.onclick = () => openAccount(item.instance);

        const actions = document.createElement('div');
        actions.className = 'account-actions';
        const remove = document.createElement('button');
        remove.className = 'icon-button danger';
        remove.setAttribute('aria-label', `${item.name} hesabını sil`);
        remove.innerHTML = icon('M4 7h16M9 7V4h6v3m-8 0 1 13h8l1-13M10 11v5m4-5v5');
        remove.onclick = () => {
          selectedAccount = item;
          document.getElementById('delete-copy').textContent = `${item.name} ve kayıtlı Spotify oturumu kalıcı olarak silinecek.`;
          deleteDialog.showModal();
          document.getElementById('delete-save').focus();
        };
        actions.append(remove);
        row.append(open, actions);
        accountList.append(row);
      }
    }

    menuToggle.onclick = () => setDrawer(true);
    document.getElementById('drawer-close').onclick = () => setDrawer(false);
    document.getElementById('drawer-backdrop').onclick = () => setDrawer(false);
    deleteDialog.onclick = event => { if (event.target === deleteDialog) deleteDialog.close(); };
    document.addEventListener('keydown', event => { if (event.key === 'Escape' && document.body.classList.contains('drawer-open')) setDrawer(false); });
    document.getElementById('add-account').onclick = async () => {
      const response = await fetch('/add-account', { method: 'POST' });
      const result = await response.text();
      if (result.startsWith('opened:')) {
        status.textContent = `Hesap ${result.split(':')[1]} açıldı; Spotify ile giriş yapabilirsin.`;
        setTimeout(refreshAccounts, 900);
      } else status.textContent = 'Yeni hesap eklenemedi; dokuz hesap yuvası dolu.';
    };
    document.getElementById('delete-form').onsubmit = async event => {
      event.preventDefault();
      if (event.submitter?.value === 'cancel') { deleteDialog.close(); return; }
      if (!selectedAccount) return;
      const deleting = selectedAccount;
      const deletingCurrent = deleting.instance === currentInstance;
      const currentPosition = knownAccounts.findIndex(item => item.instance === deleting.instance);
      const fallback = deletingCurrent
        ? knownAccounts[currentPosition - 1] || knownAccounts[currentPosition + 1]
        : null;
      selectedAccount = null;
      deleteDialog.close();
      deletingAccounts.add(deleting.instance);
      knownAccounts = knownAccounts.filter(item => item.instance !== deleting.instance);
      accountList.querySelector(`[data-instance="${deleting.instance}"]`)?.remove();

      const restoreAfterFailure = () => {
        deletingAccounts.delete(deleting.instance);
        refreshAccounts();
      };
      if (fallback) {
        try {
          const response = await fetch(`/open-account?instance=${fallback.instance}`, { cache: 'no-store' });
          if (!response.ok) throw new Error('Hesap geçişi başarısız.');
        } catch (_) {
          restoreAfterFailure();
          return;
        }
      }
      fetch(`/delete-account?instance=${deleting.instance}`, { method: 'POST' }).catch(restoreAfterFailure);
    };
    refreshAccounts();

    function base64url(bytes) {
      return btoa(String.fromCharCode(...bytes)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
    }

    function playlistUrl(value) {
      const text = value.trim();
      const web = text.match(/open\.spotify\.com\/(?:intl-[^/]+\/)?(playlist|track)\/([A-Za-z0-9]+)/i);
      const uri = text.match(/^spotify:(playlist|track):([A-Za-z0-9]+)$/i);
      const type = web?.[1] || uri?.[1];
      const id = web?.[2] || uri?.[2];
      return type && id ? `spotify:${type.toLowerCase()}:${id}` : null;
    }

    async function showTrack(uri) {
      if (!uri || uri === currentTrack) return;
      currentTrack = uri;
      const request = ++metadataRequest;
      status.textContent = 'Çalıyor…';
      const id = uri.split(':').pop();
      try {
        const response = await fetch(`https://open.spotify.com/oembed?url=https://open.spotify.com/track/${id}`);
        const data = await response.json();
        if (request === metadataRequest) {
          status.textContent = `Çalıyor: ${data.title}`;
          reportPlayback(playbackIsPlaying, data.title || uri, playbackPosition, playbackDuration);
        }
      } catch (_) {
        if (request === metadataRequest) status.textContent = 'Playlist çalıyor ve döngü açık.';
      }
    }

    function reportPlayback(playing, track = '', position = playbackPosition, duration = playbackDuration) {
      const now = Date.now();
      if (reportedPlaying === playing && reportedTrack === track && Math.abs(position - reportedPosition) < 900 && now - lastPlaybackReport < 1000) return;
      reportedPlaying = playing;
      reportedTrack = track;
      reportedPosition = position;
      lastPlaybackReport = now;
      fetch(`/player-status?playing=${playing ? '1' : '0'}&track=${encodeURIComponent(track)}&position=${Math.max(0, Math.round(position))}&duration=${Math.max(0, Math.round(duration))}&updated=${Date.now()}`, { method: 'POST' }).catch(() => {});
    }

    function watchPlayback(event) {
      const data = event.data || {};
      const playingUri = data.playingURI || currentTrack;
      const duration = Number(data.duration) || 0;
      const position = Number(data.position) || 0;
      playbackPosition = position;
      playbackDuration = duration;
      if (duration > 0) {
        playbackIsPlaying = typeof data.isPaused === 'boolean'
          ? !data.isPaused
          : lastPlaybackPosition >= 0 && position > lastPlaybackPosition;
        lastPlaybackPosition = position;
        reportPlayback(playbackIsPlaying, playingUri || '', position, duration);
      }
      showTrack(playingUri);
      const remaining = duration - position;
      if (!controller || !playingUri || duration <= 0) return;

      if (loopMode === 'track') {
        clearTimeout(endTimer);
        clearTimeout(trackTimer);
        if (!playbackIsPlaying || remaining < -1000) return;
        repeatTarget = playingUri;
        trackTimer = setTimeout(() => {
          if (loopMode !== 'track' || !controller || currentTrack !== playingUri) return;
          status.textContent = 'Aynı şarkı yeniden başlatılıyor…';
          controller.restart();
          setTimeout(() => {
            if (loopMode === 'track' && controller) controller.resume();
          }, 200);
        }, Math.max(500, remaining + 300));
        return;
      }

      clearTimeout(trackTimer);
      repeatTarget = '';
      if (loopMode !== 'playlist') {
        clearTimeout(endTimer);
        return;
      }

      clearTimeout(endTimer);
      if (!playbackIsPlaying || remaining < -1000) return;
      const endedTrack = playingUri;
      endTimer = setTimeout(() => {
        if (loopMode === 'playlist' && currentTrack === endedTrack) restartPlaylist();
      }, Math.max(1200, remaining + 1000));
    }

    function createPlaybackController() {
      if (!api || !playlist) return;
      let shell = document.getElementById('embed-shell');
      if (!shell) {
        shell = document.createElement('div');
        shell.id = 'embed-shell';
        document.body.append(shell);
      } else {
        shell.replaceChildren();
      }
      api.createController(shell, { uri: playlist, width: 400, height: 352 }, attach);
    }

    function restartPlaylist() {
      if (playlistRestarting || loopMode !== 'playlist' || !api || !playlist) return;
      playlistRestarting = true;
      clearTimeout(endTimer);
      clearTimeout(trackTimer);
      status.textContent = 'Playlist baştan başlatılıyor…';
      const previousController = controller;
      controller = null;
      currentTrack = '';
      try { previousController?.destroy(); } catch (_) {}
      setTimeout(createPlaybackController, 250);
      setTimeout(() => {
        if (playlistRestarting) {
          playlistRestarting = false;
          status.textContent = 'Playlist yeniden başlatılamadı; tekrar deneniyor…';
          if (controller) {
            controller.restart();
            setTimeout(() => controller?.resume(), 200);
          } else {
            createPlaybackController();
          }
        }
      }, 6000);
    }

    function renderLoopMode() {
      const labels = { playlist: 'Döngü: Playlist', track: 'Döngü: Şarkı', off: 'Döngü: Kapalı' };
      loopButton.textContent = labels[loopMode];
      loopButton.classList.toggle('active', loopMode !== 'off');
      localStorage.setItem('spotlite-loop-mode', loopMode);
    }

    function attach(embedController) {
      clearTimeout(loadTimer);
      controller = embedController;
      account.textContent = 'Hesap: oynatıcı hazır; giriş durumu oynatma başlayınca doğrulanacak.';
      controller.addListener('playback_started', event => {
        clearTimeout(playbackTimer);
        playlistRestarting = false;
        account.textContent = accountLabel
          ? `Hesap: ${accountLabel} — yayın yetkisi doğrulandı.`
          : 'Hesap: yayın yetkisi doğrulandı; kullanıcı adı alınamadı.';
        login.hidden = true;
        playbackIsPlaying = true;
        playbackPosition = 0;
        playbackDuration = 0;
        reportPlayback(true, event.data?.playingURI || '', 0, 0);
        showTrack(event.data?.playingURI);
      });
      controller.addListener('playback_update', watchPlayback);
      controller.play();
    }

    window.onSpotifyIframeApiReady = IFrameAPI => {
      api = IFrameAPI;
      if (pendingStart) {
        pendingStart = false;
        start();
      }
    };

    function start() {
      const normalized = playlistUrl(input.value);
      if (!normalized) {
        playbackIsPlaying = false;
        playbackPosition = 0;
        playbackDuration = 0;
        reportPlayback(false, '', 0, 0);
        status.textContent = 'Geçerli bir Spotify playlist veya şarkı bağlantısı gir.';
        return;
      }
      if (!api) {
        pendingStart = true;
        status.textContent = 'Spotify oynatıcı hazırlanıyor…';
        return;
      }

      playlist = normalized;
      currentTrack = '';
      localStorage.setItem('spotlite-playlist', playlist);
      status.textContent = 'Playlist yükleniyor…';
      clearTimeout(playbackTimer);
      playbackTimer = setTimeout(() => {
        playbackIsPlaying = false;
        playbackPosition = 0;
        playbackDuration = 0;
        reportPlayback(false, '', 0, 0);
        login.hidden = false;
        account.textContent = 'Hesap: oynatma yetkisi doğrulanamadı.';
        status.textContent = 'Spotify ile giriş yapıp yeniden deneyin.';
      }, 15000);

      if (controller) {
        controller.loadEntity(playlist);
        setTimeout(() => controller.play(), 900);
      } else {
        clearTimeout(loadTimer);
        loadTimer = setTimeout(() => {
          if (!controller) {
            account.textContent = 'Hesap: giriş yapılmadı veya doğrulanamadı.';
            login.hidden = false;
            status.textContent = 'Spotify ile giriş yapıp yeniden deneyin.';
          }
        }, 12000);
        createPlaybackController();
      }
    }

    document.getElementById('start').onclick = start;
    loopButton.onclick = () => {
      loopMode = loopMode === 'playlist' ? 'track' : loopMode === 'track' ? 'off' : 'playlist';
      clearTimeout(endTimer);
      clearTimeout(trackTimer);
      repeatTarget = '';
      renderLoopMode();
      status.textContent = `${loopButton.textContent} seçildi.`;
    };
    input.onkeydown = event => { if (event.key === 'Enter') start(); };
    input.value = localStorage.getItem('spotlite-playlist') || '';
    renderLoopMode();
    async function beginSpotifyLogin(sameWindow = false) {
      const random = crypto.getRandomValues(new Uint8Array(64));
      const verifier = base64url(random);
      const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(verifier));
      const challenge = base64url(new Uint8Array(digest));
      const state = base64url(new TextEncoder().encode(JSON.stringify({
        verifier,
        returnPort: Number(location.port),
        sameWindow
      })));
      const query = new URLSearchParams({
        client_id: '65b708073fc0480ea92a077233ca87bd',
        response_type: 'code',
        redirect_uri: 'http://127.0.0.1:5588/login',
        code_challenge_method: 'S256',
        code_challenge: challenge,
        state,
        scope: 'user-read-private user-read-email'
      });
      const authUrl = `https://accounts.spotify.com/authorize?${query}`;
      if (sameWindow) {
        status.textContent = 'Spotify girişine yönlendiriliyor…';
        location.assign(authUrl);
        return;
      }
      if (loginPopup && !loginPopup.closed) {
        loginPopup.focus();
        status.textContent = 'Açık Spotify giriş penceresi öne getirildi.';
        return;
      }
      loginPopup = window.open(authUrl, 'spotify-login', 'popup,width=520,height=720');
      status.textContent = loginPopup ? 'Spotify girişi bekleniyor…' : 'Giriş penceresi engellendi.';
    }
    login.onclick = () => beginSpotifyLogin(false);
    window.addEventListener('message', event => {
      if (event.origin !== 'http://127.0.0.1:5588' || event.data?.type !== 'spotlite-auth') return;
      const label = String(event.data.label || 'Spotify hesabı');
      localStorage.setItem('spotlite-account', label);
      accountLabel = label;
      account.textContent = `Hesap: ${label}`;
      login.hidden = true;
      loginPopup = null;
      status.textContent = 'Giriş tamamlandı. Playlist başlatılabilir.';
      window.focus();
    });
    setInterval(() => {
      fetch('/heartbeat', { cache: 'no-store' }).catch(() => {});
      const latestAccount = localStorage.getItem('spotlite-account') || '';
      if (latestAccount && latestAccount !== accountLabel) {
        accountLabel = latestAccount;
        account.textContent = `Hesap: ${latestAccount}`;
        login.hidden = true;
        rememberSpotifyName(latestAccount);
        refreshAccounts();
        status.textContent = 'Giriş tamamlandı. Playlist başlatılabilir.';
        window.focus();
      }
    }, 1000);
    if (new URLSearchParams(location.search).get('login') === '1') {
      setTimeout(() => beginSpotifyLogin(false), 350);
    }
    if (new URLSearchParams(location.search).get('login-complete') === '1') {
      setTimeout(() => fetch('/background-self', { method: 'POST' }).catch(() => {}), 700);
    }
    setInterval(async () => {
      try {
        const response = await fetch(`/command?after=${commandVersion}`, { cache: 'no-store' });
        const command = await response.json();
        if (!command.version || command.version <= commandVersion) return;
        commandVersion = command.version;
        if (command.login) {
          beginSpotifyLogin(false);
          return;
        }
        if (!command.url) return;
        input.value = command.url;
        loopMode = ['playlist', 'track', 'off'].includes(command.loop) ? command.loop : 'playlist';
        renderLoopMode();
        start();
      } catch (_) {}
    }, 700);
  </script>
</body>
</html>"#;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            show_error(&message);
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let edge = find_edge().ok_or_else(|| {
        "Microsoft Edge bulunamadı. SpotLite Free, Spotify'ın korumalı Free yayınını oynatmak için Windows'taki Edge motorunu kullanır.".to_owned()
    })?;

    let local_data = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "LOCALAPPDATA klasörü bulunamadı.".to_owned())?;
    let app_data = local_data.join("SpotLiteFree");
    let full_mode = env::args_os().any(|argument| argument == "--full");
    let login_on_start = env::args_os().any(|argument| argument == "--login");
    let background = env::args_os().any(|argument| argument == "--background");
    let controller_pid = controller_window_pid();
    let replace_window = replacement_window_pid();
    let requested_instance = requested_instance()?;
    let controller_mode = !full_mode && requested_instance.is_none();
    let (instance, listener) = if controller_mode {
        match TcpListener::bind(("127.0.0.1", 43116)) {
            Ok(listener) => (0, Some(listener)),
            Err(_) => {
                if let Ok(mut existing) = TcpStream::connect(("127.0.0.1", 43116)) {
                    let _ = existing.write_all(
                        b"GET /focus HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
                    );
                    return Ok(());
                }
                return Err("Kontrol paneli başlatılamadı.".to_owned());
            }
        }
    } else if full_mode {
        (requested_instance.unwrap_or(1), None)
    } else if let Some(instance) = requested_instance {
        let port = 43116_u16 + u16::from(instance);
        let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|error| {
            format!(
                "Hesap {instance} penceresi başlatılamadı ({error}). Aynı hesap penceresi zaten açık olabilir."
            )
        })?;
        (instance, Some(listener))
    } else {
        unreachable!()
    };
    let main_port = 43116_u16 + u16::from(instance);
    let profile_name = if full_mode {
        format!("ProfileFull{instance}")
    } else if controller_mode {
        "ControlPanel".to_owned()
    } else if instance == 1 {
        "ProfileLite".to_owned()
    } else {
        format!("ProfileLite{instance}")
    };
    let cache_name = if full_mode {
        format!("CacheFull{instance}")
    } else if controller_mode {
        "ControlCache".to_owned()
    } else if instance == 1 {
        "CacheLite".to_owned()
    } else {
        format!("CacheLite{instance}")
    };
    let profile = app_data.join(profile_name);
    let cache = app_data.join(cache_name);
    fs::create_dir_all(&profile).map_err(|error| format!("Profil oluşturulamadı: {error}"))?;
    fs::create_dir_all(&cache).map_err(|error| format!("Önbellek oluşturulamadı: {error}"))?;

    let target_url = if full_mode {
        PLAYER_URL.to_owned()
    } else if controller_mode {
        "http://127.0.0.1:43116/".to_owned()
    } else if login_on_start {
        format!("http://127.0.0.1:{main_port}/?login=1")
    } else {
        format!("http://127.0.0.1:{main_port}/")
    };

    let mut command = Command::new(edge);
    command
        // Edge'in Spotify tarafından desteklenen, DRM özellikli motorunu tek site
        // penceresi olarak kullanır. Şifreli medyaya müdahale edilmez.
        .arg(format!("--app={target_url}"))
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--disk-cache-dir={}", cache.display()))
        .arg("--disk-cache-size=268435456")
        .arg("--media-cache-size=134217728")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-default-apps")
        .arg("--disable-extensions")
        .arg("--disable-sync")
        .arg("--disable-popup-blocking")
        .arg("--autoplay-policy=no-user-gesture-required")
        .arg("--disable-background-mode")
        .arg("--disable-background-timer-throttling")
        .arg("--disable-renderer-backgrounding")
        .arg("--disable-gpu")
        .arg("--renderer-process-limit=2")
        .arg(if controller_mode {
            "--window-size=960,700"
        } else {
            "--window-size=760,260"
        });
    if background || login_on_start {
        command.arg("--window-position=-32000,-32000");
    }

    // Tanılama için URL veya güvenlik davranışını değiştirmeyen ek Edge
    // seçenekleri verilebilir: spotlite-free.exe --edge-arg=--enable-logging
    for argument in env::args_os().skip(1) {
        if let Some(value) = edge_argument(argument) {
            command.arg(value);
        }
    }

    let child = command
        .spawn()
        .map_err(|error| format!("Spotify penceresi başlatılamadı: {error}"))?;
    let edge_pid = child.id();
    if background {
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(1100));
            hide_process_window(edge_pid);
        });
    } else if login_on_start {
        thread::spawn(move || {
            for _ in 0..100 {
                if focus_spotify_login_window_from(edge_pid, controller_pid) {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        });
    } else if let Some(previous_pid) = replace_window {
        thread::spawn(move || {
            for _ in 0..80 {
                if focus_process_window_from(edge_pid, Some(previous_pid)) {
                    thread::sleep(Duration::from_millis(120));
                    hide_process_window(previous_pid);
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        });
    }

    if controller_mode {
        thread::spawn(serve_auth_callback);
    }

    if let Some(listener) = listener {
        let command_state = Arc::new(Mutex::new(PlaybackCommand::default()));
        serve_lite_player(
            listener,
            instance,
            edge_pid,
            controller_pid,
            app_data,
            child,
            command_state,
        )
        .map_err(|error| format!("Hafif oynatıcı bağlantısı kesildi: {error}"))?;
    }

    Ok(())
}

fn serve_auth_callback() {
    loop {
        match TcpListener::bind("127.0.0.1:5588") {
            Ok(listener) => {
                for mut stream in listener.incoming().flatten() {
                    let mut request = [0_u8; 2048];
                    let _ = stream.read(&mut request);
                    let _ = respond_html(&mut stream, AUTH_CALLBACK_HTML);
                }
            }
            Err(_) => thread::sleep(Duration::from_secs(1)),
        }
    }
}

fn requested_instance() -> Result<Option<u8>, String> {
    if env::args_os().any(|argument| argument == "--auto-instance") {
        return Ok(None);
    }
    for argument in env::args_os().skip(1) {
        let text = argument.to_string_lossy();
        if let Some(value) = text.strip_prefix("--instance=") {
            let number = value
                .parse::<u8>()
                .map_err(|_| "Örnek numarası 1 ile 9 arasında olmalı.".to_owned())?;
            return (1..=9)
                .contains(&number)
                .then_some(Some(number))
                .ok_or_else(|| "Örnek numarası 1 ile 9 arasında olmalı.".to_owned());
        }
    }

    let executable = env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .map(|name| name.to_string_lossy().to_lowercase())
        })
        .unwrap_or_default();
    for number in 2..=9 {
        if executable.contains(&format!("hesap-{number}"))
            || executable.contains(&format!("account-{number}"))
        {
            return Ok(Some(number));
        }
    }
    Ok(None)
}

fn replacement_window_pid() -> Option<u32> {
    env::args_os().skip(1).find_map(|argument| {
        argument
            .to_string_lossy()
            .strip_prefix("--replace-window=")?
            .parse::<u32>()
            .ok()
    })
}

fn controller_window_pid() -> Option<u32> {
    env::args_os().skip(1).find_map(|argument| {
        argument
            .to_string_lossy()
            .strip_prefix("--controller-pid=")?
            .parse::<u32>()
            .ok()
    })
}

fn edge_argument(argument: OsString) -> Option<OsString> {
    const PREFIX: &str = "--edge-arg=";
    let text = argument.to_string_lossy();
    text.strip_prefix(PREFIX).map(OsString::from)
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b':' | b'/') {
            encoded.push(*byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn send_local_request(port: u16, path: &str) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes(),
    )?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

fn account_server_running(instance: u8) -> bool {
    let port = 43116_u16 + u16::from(instance);
    TcpListener::bind(("127.0.0.1", port)).is_err()
}

fn account_runtime_json(instance: u8) -> String {
    let port = 43116_u16 + u16::from(instance);
    send_local_request(port, "/status")
        .ok()
        .and_then(|response| {
            response
                .split_once("\r\n\r\n")
                .map(|(_, body)| body.trim().to_owned())
        })
        .filter(|body| body.starts_with('{') && body.ends_with('}'))
        .unwrap_or_else(|| {
            "{\"playing\":false,\"starting\":false,\"track\":\"\",\"position\":0,\"duration\":0,\"updated\":0}"
                .to_owned()
        })
}

fn start_account_process(
    instance: u8,
    controller_pid: u32,
    login: bool,
    background: bool,
) -> std::io::Result<()> {
    // Sunucu yokken aynı profile ait Edge işlemi kalmışsa yeni Edge çağrısı
    // eski, sahipsiz pencereye bağlanıp hemen kapanır. Hesap yardımcısı da portu
    // bıraktığı için panelde hem oynatma hem giriş sessizce başarısız olur.
    stop_stale_account_browser(instance);
    let executable = env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg(format!("--instance={instance}"))
        .arg(format!("--controller-pid={controller_pid}"));
    if login {
        command.arg("--login");
    }
    if background {
        command.arg("--background");
    }
    command.spawn()?;
    Ok(())
}

fn ensure_account_running(instance: u8, controller_pid: u32) -> std::io::Result<()> {
    if account_server_running(instance) {
        return Ok(());
    }
    start_account_process(instance, controller_pid, false, true)?;
    for _ in 0..50 {
        if account_server_running(instance) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "Hesap oynatıcısı hazırlanamadı.",
    ))
}

fn control_account(
    instance: u8,
    controller_pid: u32,
    url: &str,
    loop_mode: &str,
) -> std::io::Result<()> {
    ensure_account_running(instance, controller_pid)?;
    let port = 43116_u16 + u16::from(instance);
    send_local_request(
        port,
        &format!(
            "/set-command?url={}&loop={}",
            percent_encode(url),
            percent_encode(loop_mode)
        ),
    )?;
    Ok(())
}

fn serve_lite_player(
    listener: TcpListener,
    instance: u8,
    edge_pid: u32,
    controller_pid: Option<u32>,
    app_data: PathBuf,
    mut edge_child: std::process::Child,
    command_state: Arc<Mutex<PlaybackCommand>>,
) -> std::io::Result<()> {
    listener.set_nonblocking(true)?;

    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                let request_app_data = app_data.clone();
                let request_command_state = Arc::clone(&command_state);
                thread::spawn(move || {
                    let _ = respond(
                        &mut stream,
                        instance,
                        edge_pid,
                        controller_pid,
                        &request_app_data,
                        &request_command_state,
                    );
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(150));
            }
            Err(error) => return Err(error),
        }
        if edge_child.try_wait()?.is_some() {
            return Ok(());
        }
    }
}

fn respond(
    stream: &mut TcpStream,
    instance: u8,
    edge_pid: u32,
    controller_pid: Option<u32>,
    app_data: &std::path::Path,
    command_state: &Arc<Mutex<PlaybackCommand>>,
) -> std::io::Result<()> {
    let mut request = [0_u8; 2048];
    let read = stream.read(&mut request)?;
    let no_content = request[..read].starts_with(b"GET /favicon.ico ")
        || request[..read].starts_with(b"GET /heartbeat ");
    let path = request_path(&request[..read]).unwrap_or_default();
    let focus = path == "/focus" || path.starts_with("/focus?");
    let auth_complete = request[..read].starts_with(b"GET /auth-complete?")
        || request[..read].starts_with(b"GET /auth-complete ");

    if path == "/tokens.css" {
        respond_css(stream, TOKENS_CSS)?;
    } else if path == "/accounts" {
        respond_json(stream, &accounts_json(app_data, instance))?;
    } else if instance == 0 && path.starts_with("/control-account?") {
        let selected = query_value(&path, "instance").and_then(|value| value.parse::<u8>().ok());
        let url = query_value(&path, "url").unwrap_or_default();
        let loop_mode = query_value(&path, "loop").unwrap_or_else(|| "playlist".to_owned());
        let result = selected
            .filter(|number| (1..=9).contains(number) && account_exists(app_data, *number))
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "Hesap bulunamadı.")
            })
            .and_then(|number| control_account(number, edge_pid, &url, &loop_mode));
        respond_text(stream, if result.is_ok() { "ok" } else { "error" })?;
    } else if instance == 0 && path.starts_with("/control-all?") {
        let url = query_value(&path, "url").unwrap_or_default();
        let loop_mode = query_value(&path, "loop").unwrap_or_else(|| "playlist".to_owned());
        let mut failures = 0_u8;
        for selected in (1_u8..=9).filter(|number| account_exists(app_data, *number)) {
            if control_account(selected, edge_pid, &url, &loop_mode).is_err() {
                failures += 1;
            }
        }
        respond_text(stream, if failures == 0 { "ok" } else { "error" })?;
    } else if instance == 0 && path.starts_with("/login-account?") {
        let result = if let Some(selected) =
            query_value(&path, "instance").and_then(|value| value.parse::<u8>().ok())
        {
            if (1..=9).contains(&selected) && account_exists(app_data, selected) {
                let port = 43116_u16 + u16::from(selected);
                if account_server_running(selected) {
                    send_local_request(port, &format!("/request-login?controller={edge_pid}"))
                        .map(|_| ())
                } else {
                    start_account_process(selected, edge_pid, true, false)
                }
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Hesap bulunamadı.",
                ))
            }
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Hesap numarası geçersiz.",
            ))
        };
        respond_text(stream, if result.is_ok() { "opened" } else { "error" })?;
    } else if path == "/add-account" {
        let selected =
            (1_u8..=9).find(|number| !account_spotify_name_path(app_data, *number).is_file());
        if let Some(selected) = selected {
            let result = if account_server_running(selected) {
                let port = 43116_u16 + u16::from(selected);
                send_local_request(port, &format!("/request-login?controller={edge_pid}"))
                    .map(|_| ())
            } else {
                start_account_process(selected, edge_pid, true, false)
            };
            let response = if result.is_ok() {
                format!("opened:{selected}")
            } else {
                "error".to_owned()
            };
            respond_text(stream, &response)?;
        } else {
            respond_text(stream, "full")?;
        }
    } else if instance > 0 && path.starts_with("/player-status?") {
        let playing = query_value(&path, "playing").as_deref() == Some("1");
        let track = query_value(&path, "track").unwrap_or_default();
        let position_ms = query_value(&path, "position")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default();
        let duration_ms = query_value(&path, "duration")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default();
        let updated_ms = query_value(&path, "updated")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default();
        if let Ok(mut command) = command_state.lock() {
            // Spotify iframe'i şarkı/geçiş sırasında kısa süreli duration=0
            // bildirebiliyor. Bu geçici paket, son geçerli zaman çizelgesini ve
            // paneldeki ilerleme çubuğunu sıfırlamamalı.
            if duration_ms > 0 || command.duration_ms == 0 {
                command.playing = playing;
                command.starting = false;
                command.track = track;
                command.position_ms = position_ms;
                command.duration_ms = duration_ms;
                command.updated_ms = updated_ms;
            } else {
                command.starting = false;
                command.playing |= playing;
                if !track.trim().is_empty() {
                    command.track = track;
                }
            }
        }
        respond_text(stream, "saved")?;
    } else if instance > 0 && path == "/status" {
        let body = if let Ok(command) = command_state.lock() {
            format!(
                "{{\"playing\":{},\"starting\":{},\"track\":\"{}\",\"position\":{},\"duration\":{},\"updated\":{}}}",
                command.playing,
                command.starting,
                json_escape(&command.track),
                command.position_ms,
                command.duration_ms,
                command.updated_ms
            )
        } else {
            "{\"playing\":false,\"starting\":false,\"track\":\"\",\"position\":0,\"duration\":0,\"updated\":0}"
                .to_owned()
        };
        respond_json(stream, &body)?;
    } else if instance > 0 && path.starts_with("/set-command?") {
        let url = query_value(&path, "url").unwrap_or_default();
        let loop_mode = query_value(&path, "loop").unwrap_or_else(|| "playlist".to_owned());
        if let Ok(mut command) = command_state.lock() {
            command.version = command.version.wrapping_add(1).max(1);
            command.url = url;
            command.loop_mode = loop_mode;
            command.login = false;
            command.playing = false;
            command.starting = true;
            command.track.clear();
            command.position_ms = 0;
            command.duration_ms = 0;
            command.updated_ms = 0;
        }
        respond_text(stream, "queued")?;
    } else if instance > 0 && path.starts_with("/command?") {
        let after = query_value(&path, "after")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default();
        let body = if let Ok(mut command) = command_state.lock() {
            let is_new = command.version > after;
            let body = format!(
                "{{\"version\":{},\"url\":\"{}\",\"loop\":\"{}\",\"login\":{}}}",
                command.version,
                if is_new {
                    json_escape(&command.url)
                } else {
                    String::new()
                },
                if is_new {
                    json_escape(&command.loop_mode)
                } else {
                    String::new()
                },
                is_new && command.login
            );
            if is_new {
                command.login = false;
            }
            body
        } else {
            "{\"version\":0,\"url\":\"\",\"loop\":\"\",\"login\":false}".to_owned()
        };
        respond_json(stream, &body)?;
    } else if instance > 0 && path.starts_with("/request-login?") {
        if let Ok(mut command) = command_state.lock() {
            command.version = command.version.wrapping_add(1).max(1);
            command.login = true;
        }
        let panel_pid = query_value(&path, "controller")
            .and_then(|value| value.parse::<u32>().ok())
            .or(controller_pid);
        thread::spawn(move || {
            for _ in 0..100 {
                if focus_spotify_login_window_from(edge_pid, panel_pid) {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        });
        respond_text(stream, "queued")?;
    } else if instance > 0 && path == "/background-self" {
        respond_text(stream, "hidden")?;
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(180));
            hide_process_window(edge_pid);
            if let Some(panel_pid) = controller_pid {
                focus_process_window_from(panel_pid, None);
            }
        });
    } else if path.starts_with("/remember-account?") {
        if let Some(label) = query_value(&path, "label") {
            fs::write(account_spotify_name_path(app_data, instance), label)?;
        }
        respond_text(stream, "saved")?;
    } else if path.starts_with("/rename-account?") {
        if let (Some(selected), Some(name)) = (
            query_value(&path, "instance").and_then(|value| value.parse::<u8>().ok()),
            query_value(&path, "name"),
        ) {
            if (1..=9).contains(&selected) && !name.trim().is_empty() {
                fs::write(account_custom_name_path(app_data, selected), name.trim())?;
            }
        }
        respond_text(stream, "renamed")?;
    } else if path.starts_with("/delete-account?") {
        if let Some(selected) =
            query_value(&path, "instance").and_then(|value| value.parse::<u8>().ok())
        {
            if (1..=9).contains(&selected) {
                if selected == instance {
                    let _ = respond_text(stream, "deleted");
                    delete_account_data(app_data, selected, edge_pid);
                    std::process::exit(0);
                }
                let port = 43116_u16 + u16::from(selected);
                if let Ok(mut target) = TcpStream::connect(("127.0.0.1", port)) {
                    target.write_all(b"POST /delete-self HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")?;
                    let mut reply = [0_u8; 256];
                    let _ = target.read(&mut reply);
                }
                // Hesap yardımcısı Edge kapanınca kendi temizlik iş parçacığı
                // tamamlanmadan sonlanabilir. Kalıcı dosya temizliğini yaşayan
                // kontrol paneli de üstlenir.
                stop_stale_account_browser(selected);
                delete_account_files(app_data, selected);
            }
        }
        respond_text(stream, "deleted")?;
    } else if path == "/delete-self" {
        let _ = respond_text(stream, "deleted");
        delete_account_data(app_data, instance, edge_pid);
        std::process::exit(0);
    } else if focus {
        let previous_pid = query_value(&path, "from").and_then(|value| value.parse().ok());
        focus_process_window_from(edge_pid, previous_pid);
        respond_text(stream, "focused")?;
    } else if no_content {
        stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")?;
    } else if auth_complete {
        respond_html(stream, AUTH_COMPLETE_HTML)?;
    } else if instance == 0 {
        respond_html(stream, CONTROL_HTML)?;
    } else {
        let html = LITE_HTML.replace("{{INSTANCE}}", &instance.to_string());
        respond_html(stream, &html)?;
    }
    stream.flush()
}

fn request_path(request: &[u8]) -> Option<String> {
    String::from_utf8_lossy(request)
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)
        .map(str::to_owned)
}

fn query_value(path: &str, key: &str) -> Option<String> {
    let query = path.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then(|| percent_decode(value))
    })
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = &value[index + 1..index + 3];
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                decoded.push(byte);
                index += 3;
                continue;
            }
        }
        decoded.push(if bytes[index] == b'+' {
            b' '
        } else {
            bytes[index]
        });
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn account_profile_path(app_data: &std::path::Path, instance: u8) -> PathBuf {
    app_data.join(if instance == 1 {
        "ProfileLite".to_owned()
    } else {
        format!("ProfileLite{instance}")
    })
}

#[cfg(target_os = "windows")]
fn stop_stale_account_browser(instance: u8) {
    use std::os::windows::process::CommandExt;

    let Some(local_data) = env::var_os("LOCALAPPDATA").map(PathBuf::from) else {
        return;
    };
    let profile = account_profile_path(&local_data.join("SpotLiteFree"), instance);
    let escaped_profile = profile.to_string_lossy().replace('\'', "''");
    let script = format!(
        "$spotliteProfile='{escaped_profile}'; Get-CimInstance Win32_Process | Where-Object {{ $_.Name -eq 'msedge.exe' -and $_.CommandLine -like ('*' + $spotliteProfile + '*') }} | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}"
    );
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    thread::sleep(Duration::from_millis(450));
}

#[cfg(not(target_os = "windows"))]
fn stop_stale_account_browser(_instance: u8) {}

fn account_cache_path(app_data: &std::path::Path, instance: u8) -> PathBuf {
    app_data.join(if instance == 1 {
        "CacheLite".to_owned()
    } else {
        format!("CacheLite{instance}")
    })
}

fn account_custom_name_path(app_data: &std::path::Path, instance: u8) -> PathBuf {
    app_data.join(format!("AccountName{instance}.txt"))
}

fn account_spotify_name_path(app_data: &std::path::Path, instance: u8) -> PathBuf {
    app_data.join(format!("SpotifyName{instance}.txt"))
}

fn account_exists(app_data: &std::path::Path, instance: u8) -> bool {
    account_profile_path(app_data, instance).is_dir()
        || account_custom_name_path(app_data, instance).is_file()
        || account_spotify_name_path(app_data, instance).is_file()
}

fn account_display_name(app_data: &std::path::Path, instance: u8) -> String {
    fs::read_to_string(account_spotify_name_path(app_data, instance))
        .ok()
        .filter(|name| !name.trim().is_empty())
        .map(|name| {
            let name = name.trim();
            if name == format!("Hesap {instance}") {
                "E-posta için girişi yenile".to_owned()
            } else {
                name.to_owned()
            }
        })
        .unwrap_or_else(|| "Spotify girişi yapılmadı".to_owned())
}

fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn accounts_json(app_data: &std::path::Path, current_instance: u8) -> String {
    let entries = (1_u8..=9)
        .filter(|instance| account_exists(app_data, *instance))
        .map(|instance| {
            let running = current_instance == instance || account_server_running(instance);
            let runtime = if running && current_instance == 0 {
                account_runtime_json(instance)
            } else {
                "{\"playing\":false,\"starting\":false,\"track\":\"\",\"position\":0,\"duration\":0,\"updated\":0}".to_owned()
            };
            format!(
                "{{\"instance\":{instance},\"name\":\"{}\",\"loggedIn\":{},\"running\":{},\"runtime\":{runtime}}}",
                json_escape(&account_display_name(app_data, instance)),
                account_spotify_name_path(app_data, instance).is_file(),
                running
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{entries}]")
}

fn delete_account_files(app_data: &std::path::Path, instance: u8) {
    for _ in 0..16 {
        let _ = fs::remove_dir_all(account_profile_path(app_data, instance));
        let _ = fs::remove_dir_all(account_cache_path(app_data, instance));
        let _ = fs::remove_file(account_custom_name_path(app_data, instance));
        let _ = fs::remove_file(account_spotify_name_path(app_data, instance));
        if !account_exists(app_data, instance) {
            return;
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn delete_account_data(app_data: &std::path::Path, instance: u8, edge_pid: u32) {
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &edge_pid.to_string(), "/T", "/F"])
            .output();
    }
    thread::sleep(Duration::from_millis(500));
    delete_account_files(app_data, instance);
}

fn respond_text(stream: &mut TcpStream, body: &str) -> std::io::Result<()> {
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

fn respond_json(stream: &mut TcpStream, body: &str) -> std::io::Result<()> {
    respond_with_type(stream, body, "application/json; charset=utf-8")
}

fn respond_css(stream: &mut TcpStream, body: &str) -> std::io::Result<()> {
    respond_with_type(stream, body, "text/css; charset=utf-8")
}

fn respond_with_type(
    stream: &mut TcpStream,
    body: &str,
    content_type: &str,
) -> std::io::Result<()> {
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

#[cfg(target_os = "windows")]
fn focus_process_window_from(process_id: u32, previous_process_id: Option<u32>) -> bool {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct WindowRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    struct WindowTarget {
        process_id: u32,
        window: *mut c_void,
        visible_only: bool,
    }

    unsafe extern "system" fn visit(window: *mut c_void, data: isize) -> i32 {
        let target = &mut *(data as *mut WindowTarget);
        let mut owner = 0_u32;
        GetWindowThreadProcessId(window, &mut owner);
        if owner == target.process_id && (!target.visible_only || IsWindowVisible(window) != 0) {
            target.window = window;
            return 0;
        }
        1
    }

    extern "system" {
        fn EnumWindows(
            callback: unsafe extern "system" fn(*mut c_void, isize) -> i32,
            data: isize,
        ) -> i32;
        fn GetWindowThreadProcessId(window: *mut c_void, process_id: *mut u32) -> u32;
        fn GetWindowRect(window: *mut c_void, rect: *mut WindowRect) -> i32;
        fn IsWindowVisible(window: *mut c_void) -> i32;
        fn MoveWindow(
            window: *mut c_void,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            repaint: i32,
        ) -> i32;
        fn ShowWindow(window: *mut c_void, command: i32) -> i32;
        fn SetForegroundWindow(window: *mut c_void) -> i32;
    }

    let mut target = WindowTarget {
        process_id,
        window: std::ptr::null_mut(),
        visible_only: false,
    };
    unsafe {
        EnumWindows(visit, &mut target as *mut WindowTarget as isize);
    }
    if target.window.is_null() {
        return false;
    }

    if let Some(previous_process_id) = previous_process_id {
        let mut previous = WindowTarget {
            process_id: previous_process_id,
            window: std::ptr::null_mut(),
            visible_only: true,
        };
        unsafe {
            EnumWindows(visit, &mut previous as *mut WindowTarget as isize);
            if !previous.window.is_null() {
                let mut rect = WindowRect::default();
                if GetWindowRect(previous.window, &mut rect) != 0 {
                    MoveWindow(
                        target.window,
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        1,
                    );
                }
            }
        }
    }

    unsafe {
        ShowWindow(target.window, 9);
        SetForegroundWindow(target.window);
    }
    true
}

#[cfg(not(target_os = "windows"))]
fn focus_process_window_from(_process_id: u32, _previous_process_id: Option<u32>) -> bool {
    true
}

#[cfg(target_os = "windows")]
fn focus_spotify_login_window_from(process_id: u32, panel_process_id: Option<u32>) -> bool {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct WindowRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    struct LoginTarget {
        process_id: u32,
        login_window: *mut c_void,
    }

    unsafe extern "system" fn visit_login(window: *mut c_void, data: isize) -> i32 {
        let target = &mut *(data as *mut LoginTarget);
        let mut owner = 0_u32;
        GetWindowThreadProcessId(window, &mut owner);
        if owner != target.process_id {
            return 1;
        }

        let mut title = [0_u16; 256];
        let length = GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32);
        let title = String::from_utf16_lossy(&title[..length.max(0) as usize]);
        let lower = title.to_lowercase();
        if lower.starts_with("spotlite free") {
            ShowWindow(window, 0);
        } else if lower.contains("spotify") {
            target.login_window = window;
        }
        1
    }

    struct PanelTarget {
        process_id: u32,
        window: *mut c_void,
    }

    unsafe extern "system" fn visit_panel(window: *mut c_void, data: isize) -> i32 {
        let target = &mut *(data as *mut PanelTarget);
        let mut owner = 0_u32;
        GetWindowThreadProcessId(window, &mut owner);
        if owner == target.process_id && IsWindowVisible(window) != 0 {
            target.window = window;
            return 0;
        }
        1
    }

    extern "system" {
        fn EnumWindows(
            callback: unsafe extern "system" fn(*mut c_void, isize) -> i32,
            data: isize,
        ) -> i32;
        fn GetWindowThreadProcessId(window: *mut c_void, process_id: *mut u32) -> u32;
        fn GetWindowTextW(window: *mut c_void, text: *mut u16, maximum: i32) -> i32;
        fn GetWindowRect(window: *mut c_void, rect: *mut WindowRect) -> i32;
        fn IsWindowVisible(window: *mut c_void) -> i32;
        fn MoveWindow(
            window: *mut c_void,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            repaint: i32,
        ) -> i32;
        fn ShowWindow(window: *mut c_void, command: i32) -> i32;
        fn SetForegroundWindow(window: *mut c_void) -> i32;
    }

    let mut login = LoginTarget {
        process_id,
        login_window: std::ptr::null_mut(),
    };
    unsafe {
        EnumWindows(visit_login, &mut login as *mut LoginTarget as isize);
    }
    if login.login_window.is_null() {
        return false;
    }

    let mut rect = WindowRect {
        left: 120,
        top: 80,
        right: 880,
        bottom: 780,
    };
    if let Some(panel_process_id) = panel_process_id {
        let mut panel = PanelTarget {
            process_id: panel_process_id,
            window: std::ptr::null_mut(),
        };
        unsafe {
            EnumWindows(visit_panel, &mut panel as *mut PanelTarget as isize);
            if !panel.window.is_null() {
                let _ = GetWindowRect(panel.window, &mut rect);
            }
        }
    }

    unsafe {
        MoveWindow(
            login.login_window,
            rect.left,
            rect.top,
            (rect.right - rect.left).max(520),
            (rect.bottom - rect.top).max(640),
            1,
        );
        ShowWindow(login.login_window, 9);
        SetForegroundWindow(login.login_window);
    }
    true
}

#[cfg(not(target_os = "windows"))]
fn focus_spotify_login_window_from(_process_id: u32, _panel_process_id: Option<u32>) -> bool {
    true
}

#[cfg(target_os = "windows")]
fn hide_process_window(process_id: u32) {
    use std::ffi::c_void;

    unsafe extern "system" fn visit(window: *mut c_void, data: isize) -> i32 {
        let process_id = data as u32;
        let mut owner = 0_u32;
        GetWindowThreadProcessId(window, &mut owner);
        if owner == process_id {
            ShowWindow(window, 0);
        }
        1
    }

    extern "system" {
        fn EnumWindows(
            callback: unsafe extern "system" fn(*mut c_void, isize) -> i32,
            data: isize,
        ) -> i32;
        fn GetWindowThreadProcessId(window: *mut c_void, process_id: *mut u32) -> u32;
        fn ShowWindow(window: *mut c_void, command: i32) -> i32;
    }

    unsafe {
        EnumWindows(visit, process_id as isize);
    }
}

#[cfg(not(target_os = "windows"))]
fn hide_process_window(_process_id: u32) {}

fn respond_html(stream: &mut TcpStream, html: &str) -> std::io::Result<()> {
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        html.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(html.as_bytes())?;
    stream.flush()
}

fn find_edge() -> Option<PathBuf> {
    let candidates = [
        env::var_os("PROGRAMFILES(X86)")
            .map(PathBuf::from)
            .map(|root| root.join(r"Microsoft\Edge\Application\msedge.exe")),
        env::var_os("PROGRAMFILES")
            .map(PathBuf::from)
            .map(|root| root.join(r"Microsoft\Edge\Application\msedge.exe")),
        env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|root| root.join(r"Microsoft\Edge\Application\msedge.exe")),
    ];

    candidates.into_iter().flatten().find(|path| path.is_file())
}

#[cfg(target_os = "windows")]
fn show_error(message: &str) {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(
            window: *mut std::ffi::c_void,
            text: *const u16,
            caption: *const u16,
            kind: u32,
        ) -> i32;
    }

    let text: Vec<u16> = OsStr::new(message).encode_wide().chain(Some(0)).collect();
    let caption: Vec<u16> = OsStr::new("SpotLite Free")
        .encode_wide()
        .chain(Some(0))
        .collect();

    // SAFETY: Both pointers reference valid, null-terminated UTF-16 buffers for
    // the duration of this synchronous Windows API call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            0x0000_0010,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn show_error(message: &str) {
    eprintln!("{message}");
}

#[cfg(test)]
mod tests {
    use super::{edge_argument, json_escape, percent_decode};
    use std::ffi::OsString;

    #[test]
    fn accepts_explicit_edge_argument() {
        assert_eq!(
            edge_argument(OsString::from("--edge-arg=--enable-logging")),
            Some(OsString::from("--enable-logging"))
        );
    }

    #[test]
    fn ignores_unscoped_arguments() {
        assert_eq!(edge_argument(OsString::from("--enable-logging")), None);
    }

    #[test]
    fn decodes_turkish_account_names() {
        assert_eq!(percent_decode("Ge%C3%A7ici%20Hesap"), "Geçici Hesap");
    }

    #[test]
    fn escapes_account_names_for_json() {
        assert_eq!(json_escape("Ev \\\" hesabı"), "Ev \\\\\\\" hesabı");
    }
}
