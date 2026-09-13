(function(root) {
  'use strict';
  // Pure state machine. No timers, DOM, authentication or synthetic telemetry.
  class Playback {
    constructor(uri, mode = 'off') {
      this.uri = uri; this.mode = mode; this.catalog = []; this.verified = false;
      this.sample = {state:'loading', playing:false, track:'', position:0, duration:0, updated:0, reason:'Oynatıcı hazırlanıyor'};
      this.lastAdvance = 0; this.advanced = false; this.endArmed = false;
      this.repeatConsumed = false; this.repeatCount = 0;
      this.repeatPending = null; this.repeatAttempts = 0;
      this.lastMusic = null; this.interruption = null; this.resumePending = null;
      this.watchAttempts=0; this.watchAt=0;
    }
    metadata(value) {
      this.catalog = value?.verified && Array.isArray(value.tracks) ? value.tracks.filter(t => /^spotify:track:[a-zA-Z0-9]{22}$/.test(t.uri) && t.duration > 0) : [];
      this.verified = this.catalog.length > 0 && this.catalog.length === value?.tracks?.length;
    }
    observe(data, now) {
      const old = this.sample;
      const track = typeof data.playingURI === 'string' ? data.playingURI : '';
      const duration = Number.isFinite(data.duration) && data.duration > 0 ? data.duration : 0;
      const position = Number.isFinite(data.position) && data.position >= 0 ? Math.min(data.position, duration || data.position) : 0;
      const same = !!track && track === old.track;
      const advancing = same && position > old.position + 50;
      if (!same) { this.advanced = false; this.endArmed = false; this.repeatConsumed = false; }
      if (advancing) { this.lastAdvance = now; this.advanced = true; }
      const metadata = this.catalog.find(t => t.uri === track);
      const mismatch = metadata && duration > 0 && Math.abs(duration - metadata.duration) > 2500;
      // Short content is NOT proof of logout. Do not restart it, skip it or open login.
      const short = duration > 0 && duration <= 35000 && !metadata;
      const music = /^spotify:track:[a-zA-Z0-9]{22}$/.test(track);
      const ambiguous = !music || short || mismatch;
      // Remember real music progress, not an inferred login state. A short
      // interstitial must finish and return to that music before we may resume.
      if (ambiguous && duration > 0) {
        this.resumePending = null;
        if (this.lastMusic && now-this.lastMusic.at < 180000)
          this.interruption = {...this.lastMusic, seen:now};
      } else if (this.interruption) {
        const gap=this.interruption;
        const returned=track===gap.track && Math.abs(duration-gap.duration)<2500;
        if (returned && data.isPaused && !data.isBuffering && position===0 && now-gap.seen<15000)
          this.resumePending={at:now,track,duration};
        if (!data.isBuffering) this.interruption=null;
      }
      if (!data.isPaused || data.isBuffering || (this.resumePending &&
          (track!==this.resumePending.track || Math.abs(duration-this.resumePending.duration)>2500 || position!==0))) this.resumePending=null;
      if (!ambiguous && advancing && !data.isPaused && !data.isBuffering)
        this.lastMusic={track,duration,at:now};
      // Count a repeat only after Spotify reports a reset AND new progress.
      // Ads invalidate the attempt; never seek/restart an ambiguous media item.
      if (this.repeatPending) {
        if (ambiguous) this.repeatPending = null;
        else {
          if (!data.isPaused && (track !== this.repeatPending.track || position < duration-1500)) this.repeatPending.reset = true;
          if (this.repeatPending.reset && advancing && !data.isPaused && !data.isBuffering) {
            this.repeatCount++; this.repeatPending=null; this.repeatAttempts=0;
            this.watchAttempts=0; this.watchAt=0;
          }
        }
      }
      // Spotify also signals a natural end as paused + position=0. Accept this
      // ONLY after fresh, advancing near-end music of the same duration/URI.
      const resetAtEnd = same && this.advanced && !ambiguous && data.isPaused && !data.isBuffering
        && position === 0 && old.position >= old.duration-1500 && old.duration>0
        && Math.abs(old.duration-duration)<2500 && now-old.updated<=4000;
      const pendingReset = same && this.endArmed && old.state==='ended' && old.position===0 && position===0
        && data.isPaused && !data.isBuffering && !ambiguous && Math.abs(old.duration-duration)<2500;
      if (same && position+2000<old.position && !resetAtEnd) {this.endArmed=false;this.repeatConsumed=false;this.advanced=false;}
      if (old.state==='ended' && !data.isPaused && position<duration-1500) {this.endArmed=false;this.repeatConsumed=false;}
      let state = data.isBuffering ? 'buffering' : data.isPaused ? 'paused' : 'loading';
      if (!data.isPaused && !data.isBuffering && (advancing || (same && this.advanced && now-this.lastAdvance < 5000))) state = 'playing';
      if (!data.isPaused && !data.isBuffering && same && this.advanced && now-this.lastAdvance >= 5000) state = 'stalled';
      if (ambiguous && duration > 0) state = 'unverified_media';
      const atEnd = duration > 0 && position >= duration - 350;
      if ((!ambiguous && this.advanced && !data.isBuffering && atEnd) || resetAtEnd) this.endArmed = true;
      if (data.isBuffering || ambiguous || !duration) this.endArmed = false;
      if (this.endArmed && (atEnd || resetAtEnd || pendingReset) && data.isPaused) state = 'ended';
      const reason = mismatch ? 'Medya süresi parça bilgisiyle uyuşmuyor; reklam/önizleme olabilir. Müdahale edilmiyor.'
        : short ? 'Kısa medya: reklam, önizleme veya kısa parça olabilir. Oturum hatası varsayılmadı.'
        : state === 'stalled' ? 'Spotify konumu ilerlemiyor'
        : state === 'buffering' ? 'Spotify medya yüklüyor' : '';
      this.sample = {state, playing:state === 'playing', track, position, duration, updated:now, reason};
      return this.sample;
    }
    repeatAction(now) {
      if (this.repeatPending && now-this.repeatPending.at >= 5000 && this.sample.state==='ended') this.repeatConsumed=false;
      if (this.mode === 'off' || this.repeatConsumed || this.sample.state !== 'ended' || now-this.sample.updated > 5000) return null;
      if (this.repeatAttempts >= 3) return null;
      const current = this.sample.track;
      const reserve=action=>{this.repeatConsumed=true;this.repeatAttempts++;this.repeatPending={at:now,track:current,reset:false};return action;};
      if (this.mode === 'track') return reserve({type:'restart'});
      if (this.uri.startsWith('spotify:track:') && current === this.uri) return reserve({type:'restart'});
      // Never mistake an intermediate song for the end of a playlist. Duplicated
      // final URIs are ambiguous; leave Spotify's natural queue alone in that case.
      if (!this.verified || this.catalog.at(-1).uri !== current || this.catalog.filter(t=>t.uri===current).length !== 1) return null;
      return reserve({type:'load',uri:this.uri});
    }
    resumeAction(now) {
      const pending=this.resumePending;
      this.resumePending=null; // At most one attempt for each observed transition.
      if (!pending || this.mode==='off' || this.sample.state!=='paused' ||
          now-pending.at>5000 || now-this.sample.updated>5000) return null;
      return {type:'resume'};
    }
    watchdogAction(now) {
      // No new Spotify event is required to follow up a lost repeat command.
      // Without fresh media evidence never seek, reload or restart: resume only.
      const p=this.repeatPending;
      if (!p || this.mode==='off' || this.watchAttempts>=3 ||
          now-p.at<6000 || now-this.watchAt<6000 ||
          !['ended','loading','stalled'].includes(this.sample.state)) return null;
      this.watchAttempts++; this.watchAt=now;
      return {type:'resume',attempt:this.watchAttempts};
    }
    view(now) {
      const s = {...this.sample};
      if (s.updated && now-s.updated > 8000) {s.playing=false;s.state='unknown';s.reason='Spotify’dan güncel oynatma verisi gelmiyor';}
      // Display-only interpolation. Never sent back as a Spotify observation.
      if (s.playing) s.position = Math.min(s.duration, s.position + Math.min(2000,Math.max(0,now-s.updated)));
      return s;
    }
  }
  function uriFrom(value) {
    if (/^spotify:(track|playlist):[a-zA-Z0-9]{22}$/.test(value)) return value;
    try { const url=new URL(value); if(url.protocol!=='https:' || url.hostname!=='open.spotify.com') return null;
      const m=url.pathname.match(/^\/(?:intl-[a-z-]+\/)?(track|playlist)\/([a-zA-Z0-9]{22})\/?$/);
      return m ? `spotify:${m[1]}:${m[2]}` : null;
    } catch {return null;}
  }
  class ReconnectTracker {
    constructor(saved={}) {
      if(!saved || typeof saved!=='object')saved={};
      this.count=Number.isInteger(saved.count)?Math.max(0,Math.min(3,saved.count)):0;
      this.track=typeof saved.track==='string'?saved.track:'';
      this.lastCompleted=Number(saved.lastCompleted)||0;
      this.cooldownUntil=Number(saved.cooldownUntil)||0;
      this.known=saved.known && typeof saved.known==='object'?saved.known:{};
      this.run=null;this.last=null;this.pending=false;this.healthy=false;
    }
    snapshot(){return {count:this.count,track:this.track,lastCompleted:this.lastCompleted,cooldownUntil:this.cooldownUntil,known:this.known};}
    observe(data,now,expected=0){
      const uri=data.playingURI,d=Number(data.duration),p=Number(data.position);
      if(!/^spotify:track:[a-zA-Z0-9]{22}$/.test(uri||'') || !Number.isFinite(d) || !Number.isFinite(p)) {
        this.run=null;this.last=null;this.pending=false;return;
      }
      const previous=this.last;
      const advances=previous && previous.uri===uri && p>previous.p+50 && now-previous.at<5000;
      // Learn only an advancing full-length music sample, never a short ad.
      if(d>35000 && p>20000 && advances && !data.isPaused && !data.isBuffering){
        this.known[uri]=d;this.known=Object.fromEntries(Object.entries(this.known).slice(-50));
        this.count=0;this.pending=false;this.run=null;this.healthy=true;
      }
      const full=expected>35000?expected:Number(this.known[uri]);
      const candidate=d>=15000 && d<=17000 && full>35000 && Math.abs(full-d)>2500;
      if(this.track!==uri || now-this.lastCompleted>600000){this.count=0;this.track=uri;}
      if(!candidate || data.isBuffering){this.run=null;this.pending=false;}
      else {
        const reset=previous && previous.uri===uri && previous.p>8000 && p<1500;
        const completedReset=reset && data.isPaused===true && previous.p>=d-1500 &&
          Math.abs(previous.d-d)<500 && now-previous.at<4000;
        if(completedReset && this.run?.progressed && !this.run.counted &&
            (!this.lastCompleted || now-this.lastCompleted>=12000)) {
          this.run.counted=true;this.count=Math.min(3,this.count+1);this.lastCompleted=now;
        }
        if(p<1500 && !data.isPaused && (!this.run || reset || (this.run.counted && previous?.paused)))this.run={progressed:false,counted:false};
        if(this.run && advances && !data.isPaused)this.run.progressed=true;
        if(this.run?.progressed && !this.run.counted && p>=d-500 &&
            (!this.lastCompleted || now-this.lastCompleted>=12000)){
          this.run.counted=true;this.count=Math.min(3,this.count+1);this.lastCompleted=now;
        }
        this.pending=this.count>=3 && data.isPaused===true && (p>=d-500 || completedReset);
      }
      this.last={uri,p,d,at:now,paused:!!data.isPaused};
    }
    take(now){
      if(!this.pending || now<this.cooldownUntil)return false;
      this.pending=false;this.count=0;this.run=null;this.healthy=false;
      this.cooldownUntil=now+30*60*1000;
      return true;
    }
  }
  const exports = {Playback,uriFrom,ReconnectTracker};
  if(typeof module==='object' && module.exports) module.exports=exports; else root.SpotLiteCore=exports;
})(globalThis);
