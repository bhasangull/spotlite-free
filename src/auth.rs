//! OAuth/profile data only. Browser cookies and Spotify media authority are not
//! inferred from API token status. Secrets never leave this broker over HTTP.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fs, io::{self, Write}, net::{TcpListener, TcpStream},
    path::{Path, PathBuf}, process::{Command, Stdio}, sync::{Arc, Mutex, OnceLock},
    thread, time::{Duration, Instant}};
use crate::{percent_encode, query_value, request_path, unix_now};
#[cfg(not(windows))] use std::io::Read;

const CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";
const REDIRECT: &str = "http://127.0.0.1:5588/login";
#[derive(Clone, Default, Serialize, Deserialize)]
struct Token {
    refresh_token: String,
    #[serde(default, skip_serializing)] access_token: String,
    #[serde(default)] expires_at: u64,
}
#[derive(Default)]
struct Account { token: Option<Token>, retry_at: u64, failures: u32 }
type AccountLock = Arc<Mutex<Account>>;
static ACCOUNTS: OnceLock<Mutex<HashMap<u8, AccountLock>>> = OnceLock::new();
struct Pending { account: u8, verifier: String, client: String, created: Instant }
static PENDING: OnceLock<Mutex<HashMap<String, Pending>>> = OnceLock::new();
fn account_lock(n: u8) -> AccountLock {
    Arc::clone(ACCOUNTS.get_or_init(Default::default).lock().unwrap()
        .entry(n).or_insert_with(|| Arc::new(Mutex::new(Account::default()))))
}

pub(crate) fn random_hex() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    #[cfg(windows)] {
        #[link(name="bcrypt")]
        extern "system" { fn BCryptGenRandom(handle: *mut std::ffi::c_void, out: *mut u8, size: u32, flags: u32) -> i32; }
        if unsafe { BCryptGenRandom(std::ptr::null_mut(), bytes.as_mut_ptr(), 32, 2) } != 0 {
            return Err(io::Error::other("Güvenli rastgele sayı üretilemedi"));
        }
    }
    #[cfg(not(windows))] fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn base64url(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new(); let mut bits = 0u32; let mut count = 0;
    for &byte in bytes { bits = (bits << 8) | u32::from(byte); count += 8;
        while count >= 6 { count -= 6; out.push(TABLE[((bits >> count) & 63) as usize] as char); }
    }
    if count > 0 { out.push(TABLE[((bits << (6-count)) & 63) as usize] as char); }
    out
}

// OS-owned handle lock: released on crash, and no empty PID-file race.
pub struct DiskLock { _file: fs::File }
pub fn disk_lock(root: &Path, n: u8) -> io::Result<DiskLock> {
    let path = root.join(format!("SpotifyToken{n}.guard"));
    let start = Instant::now();
    loop {
        let mut options = fs::OpenOptions::new(); options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)] { use std::os::windows::fs::OpenOptionsExt; options.share_mode(0); }
        match options.open(&path) {
            Ok(file) => return Ok(DiskLock { _file: file }),
            Err(e) if start.elapsed() >= Duration::from_secs(20) => return Err(e),
            Err(_) => thread::sleep(Duration::from_millis(40)),
        }
    }
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension(format!("tmp-{}", random_hex()?));
    let result = (|| {
        let mut file = fs::OpenOptions::new().create_new(true).write(true).open(&temp)?;
        file.write_all(bytes)?; file.sync_all()?; drop(file);
        #[cfg(windows)] {
            use std::os::windows::ffi::OsStrExt;
            #[link(name="kernel32")]
            extern "system" { fn MoveFileExW(from: *const u16, to: *const u16, flags:u32)->i32; }
            let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            if unsafe { MoveFileExW(from.as_ptr(),to.as_ptr(),1|8) } == 0 { return Err(io::Error::last_os_error()); }
        }
        #[cfg(not(windows))] fs::rename(&temp,path)?;
        Ok(())
    })();
    if result.is_err() { let _ = fs::remove_file(temp); }
    result
}
fn encode_record(token:&Token)->io::Result<Vec<u8>> {
    let json=serde_json::to_vec(token)?;
    #[cfg(windows)] { let cipher=crate::dpapi_protect(&json).ok_or_else(||io::Error::other("DPAPI şifreleme başarısız"))?;
        Ok(format!("dpapi1:{}",crate::hex_encode(&cipher)).into_bytes()) }
    #[cfg(not(windows))] { let _=json; Err(io::Error::other("Windows DPAPI gerekli")) }
}
fn decode_record(raw:&str)->io::Result<Token> {
    if let Some(hex)=raw.strip_prefix("dpapi1:") {
        #[cfg(windows)] { let plain=crate::hex_decode(hex).and_then(|x|crate::dpapi_unprotect(&x))
            .ok_or_else(||io::Error::other("Token bu Windows hesabıyla çözülemedi"))?;
            return serde_json::from_slice(&plain).map_err(io::Error::other); }
        #[cfg(not(windows))] return Err(io::Error::other("Windows DPAPI gerekli"));
    }
    // Read legacy JSON once. It is rewritten encrypted before any network call.
    serde_json::from_str(raw).map_err(io::Error::other)
}
fn token_path(root:&Path,n:u8)->PathBuf {root.join(format!("SpotifyToken{n}.json"))}
fn write_token(root:&Path,n:u8,t:&Token)->io::Result<()> {atomic_write(&token_path(root,n),&encode_record(t)?)}
fn status(root:&Path,n:u8,state:&str,message:&str) {
    let _=atomic_write(&root.join(format!("OAuthState{n}.json")),json!({"state":state,"message":message,"updated":unix_now()}).to_string().as_bytes());
}
pub fn state(root:&Path,n:u8)->Value {
    let mut value=fs::read(root.join(format!("OAuthState{n}.json"))).ok()
        .and_then(|b|serde_json::from_slice::<Value>(&b).ok())
        .unwrap_or_else(||json!({"state":"unknown","message":"Hesap bilgisi henüz doğrulanmadı"}));
    value["identity"]=Value::Null;
    value
}
fn log(root:&Path,n:u8,reason:&str,http:u16) {
    let _=crate::diagnostics::append(root,"auth-diagnostics.log",&format!("unix={} | hesap={n} | neden={reason} | http={http}\n",unix_now()));
}

struct Response { status:u16, body:Value }
fn config_quote(v:&str)->String {v.replace('\\',"\\\\").replace('"',"\\\"").replace('\r',"\\r").replace('\n',"\\n")}
fn http(root:&Path,n:u8,url:&str,form:Option<&[(&str,&str)]>,bearer:Option<&str>)->io::Result<Response> {
    let mut config=format!("url = \"{}\"\nsilent\nshow-error\nconnect-timeout = 8\nmax-time = 20\nwrite-out = \"\\n%{{http_code}}\"\n",config_quote(url));
    if let Some(source)=crate::ipv6::read_assignment(root,n)? {
        config.push_str(&format!("ipv6\ninterface = \"{}\"\nproxy = \"\"\nnoproxy = \"*\"\n",source));
    }
    if let Some(token)=bearer {config.push_str(&format!("header = \"Authorization: Bearer {}\"\n",config_quote(token)));}
    if let Some(form)=form {
        let body=form.iter().map(|(k,v)|format!("{}={}",percent_encode(k),percent_encode(v))).collect::<Vec<_>>().join("&");
        config.push_str(&format!("request = \"POST\"\nheader = \"Content-Type: application/x-www-form-urlencoded\"\ndata = \"{}\"\n",config_quote(&body)));
    }
    let mut command=Command::new("curl.exe");
    #[cfg(windows)] {use std::os::windows::process::CommandExt;command.creation_flags(0x08000000);}
    let mut child=command.args(["--config","-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    child.stdin.take().ok_or_else(||io::Error::other("HTTP stdin"))?.write_all(config.as_bytes())?;
    let output=child.wait_with_output()?;
    if !output.status.success(){return Err(io::Error::other("Spotify ağ isteği tamamlanamadı"));}
    let text=String::from_utf8(output.stdout).map_err(io::Error::other)?;
    let (body,status)=text.rsplit_once('\n').ok_or_else(||io::Error::other("HTTP yanıtı eksik"))?;
    Ok(Response{status:status.trim().parse().map_err(io::Error::other)?,body:serde_json::from_str(body).unwrap_or(Value::Null)})
}
fn refresh_locked(root:&Path,n:u8,account:&mut Account)->io::Result<String> {
    if !crate::account_exists(root,n) {account.token=None;return Err(io::Error::other("Hesap silinmiş"));}
    if unix_now()<account.retry_at {return Err(io::Error::other("Yenileme daha sonra tekrar denenecek"));}
    if account.token.is_none() {
        let raw=fs::read_to_string(token_path(root,n))?;
        let t=decode_record(&raw).inspect_err(|_|{status(root,n,"storage_error","Kayıtlı anahtar okunamadı; veri korundu");log(root,n,"token-decrypt-error",0);})?;
        write_token(root,n,&t)?;account.token=Some(t);
    }
    let old=account.token.as_ref().unwrap();
    if old.expires_at>unix_now()+300 && !old.access_token.is_empty(){return Ok(old.access_token.clone());}
    let r=http(root,n,"https://accounts.spotify.com/api/token",Some(&[("client_id",CLIENT_ID),("grant_type","refresh_token"),("refresh_token",&old.refresh_token)]),None)?;
    if r.status==400 && r.body["error"]=="invalid_grant" {
        fs::remove_file(token_path(root,n))?; account.token=None;
        status(root,n,"reauth_required","Hesap bilgisi için yeniden yetkilendirme gerekli; Spotify oynatıcısı açık kalabilir");
        log(root,n,"refresh-invalid-grant",r.status);return Err(io::Error::other("API yetkisi yenilenmeli"));
    }
    if r.status!=200 {
        let config_error=r.body["error"]=="invalid_client";
        account.retry_at=unix_now()+if config_error {3600}else{30u64.saturating_mul(1<<account.failures.min(6))};
        account.failures+=1;
        status(root,n,if config_error {"configuration_error"} else {"retrying"},if config_error {"Spotify uygulama kaydı geçersiz; yeniden giriş bunu düzeltmez"}else{"Spotify API bağlantısı bekleniyor; oynatma etkilenmez"});
        log(root,n,if config_error {"refresh-invalid-client"}else{"refresh-http-error"},r.status);
        return Err(io::Error::other(format!("API HTTP {}",r.status)));
    }
    let next=rotated_token(old,&r.body)?;
    // Keep rotated token in memory even if the disk fails; never retry old token.
    account.token=Some(next.clone());
    write_token(root,n,&next)?;
    account.failures=0; account.retry_at=0;log(root,n,"refresh-success",200);
    status(root,n,"ready","API yetkilendirmesi hazır; oynatma oturumu ayrıca doğrulanır.");
    Ok(next.access_token)
}
fn rotated_token(old:&Token,body:&Value)->io::Result<Token> {
    let access=body["access_token"].as_str().filter(|s|!s.is_empty()).ok_or_else(||io::Error::other("Access token eksik"))?;
    let expires=body["expires_in"].as_u64().filter(|n|*n>0).ok_or_else(||io::Error::other("Token süresi eksik"))?;
    Ok(Token{refresh_token:body["refresh_token"].as_str().filter(|s|!s.is_empty()).unwrap_or(&old.refresh_token).into(),access_token:access.into(),expires_at:unix_now()+expires})
}
fn access(root:&Path,n:u8)->io::Result<String> {
    let lock=account_lock(n);let mut account=lock.lock().map_err(|_|io::Error::other("Hesap kilidi"))?;
    let _disk=disk_lock(root,n)?;
    let result=refresh_locked(root,n,&mut account);
    if result.is_err() && account.retry_at<=unix_now() {account.retry_at=unix_now()+60;}
    result
}
pub fn refresh_loop(root:PathBuf) {
    for n in 1..=9 {let root=root.clone();thread::spawn(move||loop {
        if token_path(&root,n).is_file() {let _=access(&root,n);}
        thread::sleep(Duration::from_secs(30+n as u64));
    });}
}
pub fn forget(root:&Path,n:u8,cleanup:impl FnOnce()->io::Result<()>)->io::Result<()> {
    let lock=account_lock(n);let mut account=lock.lock().map_err(|_|io::Error::other("Hesap kilidi"))?;
    let _disk=disk_lock(root,n)?;
    *account=Account::default();
    PENDING.get_or_init(Default::default).lock().unwrap().retain(|_,p|p.account!=n);
    cleanup()
}

fn begin(n:u8,client:&str)->io::Result<String> {
    let verifier=random_hex()?;let state=random_hex()?;
    let challenge=base64url(&Sha256::digest(verifier.as_bytes()));
    let mut pending=PENDING.get_or_init(Default::default).lock().unwrap();
    pending.retain(|_,p|p.created.elapsed()<Duration::from_secs(600)&&p.account!=n);
    pending.insert(state.clone(),Pending{account:n,verifier,client:client.into(),created:Instant::now()});
    Ok(format!("https://accounts.spotify.com/authorize?client_id={CLIENT_ID}&response_type=code&show_dialog=false&redirect_uri={}&code_challenge_method=S256&code_challenge={challenge}&state={state}&scope=user-read-private%20user-read-email%20playlist-read-private",percent_encode(REDIRECT)))
}
fn consume(state:&str)->Option<Pending> {
    PENDING.get_or_init(Default::default).lock().ok()?.remove(state).filter(|p|p.created.elapsed()<Duration::from_secs(600))
}
fn complete(root:&Path,p:Pending,code:&str)->io::Result<()> {
    let lock=account_lock(p.account);let mut account=lock.lock().map_err(|_|io::Error::other("Hesap kilidi"))?;
    let _disk=disk_lock(root,p.account)?;
    if !crate::account_exists(root,p.account){return Err(io::Error::other("Hesap silinmiş"));}
    let r=http(root,p.account,"https://accounts.spotify.com/api/token",Some(&[("client_id",CLIENT_ID),("grant_type","authorization_code"),("code",code),("redirect_uri",REDIRECT),("code_verifier",&p.verifier)]),None)?;
    if r.status!=200 {log(root,p.account,"login-token-rejected",r.status);return Err(io::Error::other(format!("Spotify yetkilendirmesi reddedildi (HTTP {})",r.status)));}
    let token=rotated_token(&Token::default(),&r.body)?;
    if token.refresh_token.is_empty(){return Err(io::Error::other("Yenileme anahtarı eksik"));}
    account.token=Some(token.clone());account.retry_at=0;
    write_token(root,p.account,&token)?;
    status(root,p.account,"ready","Yetkilendirme tamamlandı.");
    Ok(())
}
fn catalog(root:&Path,n:u8,uri:&str)->io::Result<Value> {
    let parts:Vec<_>=uri.split(':').collect();
    if parts.len()!=3 || parts[0]!="spotify" || parts[2].len()!=22 || !parts[2].bytes().all(|b|b.is_ascii_alphanumeric()) {return Err(io::Error::other("Bağlantı geçersiz"));}
    let token=access(root,n)?;
    if parts[1]=="track" {
        let r=http(root,n,&format!("https://api.spotify.com/v1/tracks/{}",parts[2]),None,Some(&token))?;
        if r.status!=200{return Err(io::Error::other(format!("Parça bilgisi alınamadı (HTTP {})",r.status)));}
        return Ok(json!({"verified":true,"tracks":[{"uri":r.body["uri"],"duration":r.body["duration_ms"],"name":r.body["name"]}]}));
    }
    if parts[1]!="playlist"{return Err(io::Error::other("Playlist veya şarkı gerekli"));}
    let mut tracks=vec![];
    for page in 0..100 {
        let r=http(root,n,&format!("https://api.spotify.com/v1/playlists/{}/items?limit=50&offset={}",parts[2],page*50),None,Some(&token))?;
        if r.status!=200{return Err(io::Error::other(format!("Playlist içeriği doğrulanamadı (HTTP {}); doğal oynatma devam eder",r.status)));}
        let items=r.body["items"].as_array().ok_or_else(||io::Error::other("Playlist listesi eksik"))?;
        for item in items {let track=if item["item"].is_object(){&item["item"]}else{&item["track"]};
            if track["uri"].as_str().is_none() || track["duration_ms"].as_u64().is_none() {return Err(io::Error::other("Playlist'te doğrulanamayan içerik var"));}
            tracks.push(json!({"uri":track["uri"],"duration":track["duration_ms"],"name":track["name"]}));
        }
        if r.body["next"].is_null(){return Ok(json!({"verified":true,"tracks":tracks}));}
    }
    Err(io::Error::other("Playlist doğrulama sınırını aştı"))
}
fn answer(stream:&mut TcpStream,code:u16,body:&str,kind:&str)->io::Result<()> {
    write!(stream,"HTTP/1.1 {code} Result\r\nContent-Type: {kind}; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{body}",body.len())
}
pub fn serve(root:PathBuf) {
    let listener=match TcpListener::bind("127.0.0.1:5588") {Ok(l)=>l,Err(_)=>{log(&root,0,"callback-port-unavailable",0);return;}};
    for mut stream in listener.incoming().flatten() {let root=root.clone();thread::spawn(move||{
        let _=stream.set_read_timeout(Some(Duration::from_secs(2)));
        let Ok(bytes)=crate::read_headers(&mut stream) else{return;};let path=request_path(&bytes).unwrap_or_default();
        if path.starts_with("/login?") {
            let raw_state=query_value(&path,"state").unwrap_or_default();
            let Some(p)=consume(&raw_state) else {let _=answer(&mut stream,400,"Giriş isteğinin süresi doldu veya daha önce kullanıldı. Kontrol panelinden tekrar deneyin.","text/plain");return;};
            let n=p.account;
            let client=percent_encode(&p.client);
            let result=if let Some(code)=query_value(&path,"code") {complete(&root,p,&code)}else{Err(io::Error::other("Spotify girişi tamamlanmadı"))};
            let outcome=if result.is_ok(){"login-complete"}else{"login-failed"};
            let message=match result {Ok(())=>"Yetkilendirme tamamlandı. Oynatma ayrıca doğrulanacak.".to_string(),Err(e)=>{status(&root,n,"profile_unavailable",&e.to_string());e.to_string()}};
            let message=message.replace('&',"&amp;").replace('<',"&lt;").replace('>',"&gt;");
            let body=format!("<!doctype html><meta charset=utf-8><meta name=referrer content=no-referrer><title>Spotify giriş sonucu</title><style>body{{background:#111;color:white;font:16px Segoe UI;padding:32px}}a{{color:#1ed760}}</style><p>{message}</p><a href=\"http://127.0.0.1:{}/?client={client}&amp;{outcome}=1\">SpotLite'a dön</a><script>setTimeout(()=>location.replace('http://127.0.0.1:{}/?client={client}&{outcome}=1'),1800)</script>",43116+u16::from(n),43116+u16::from(n));
            let _=answer(&mut stream,200,&body,"text/html");return;
        }
        if !crate::local_request_allowed(&bytes,5588,true) {let _=answer(&mut stream,403,"Forbidden","text/plain");return;}
        let n=query_value(&path,"instance").and_then(|v|v.parse::<u8>().ok()).filter(|n|(1..=9).contains(n));
        let result=if let Some(n)=n {if path.starts_with("/begin?"){begin(n,&query_value(&path,"client").unwrap_or_default()).map(|url|json!({"url":url}))}
            else if path.starts_with("/catalog?"){catalog(&root,n,&query_value(&path,"uri").unwrap_or_default())}
            else {Err(io::Error::other("Bilinmeyen istek"))}}else{Err(io::Error::other("Hesap geçersiz"))};
        let (code,body)=match result {Ok(v)=>(200,v),Err(e)=>(400,json!({"error":e.to_string()}))};
        let _=answer(&mut stream,code,&body.to_string(),"application/json");
    });}
}

#[cfg(test)] mod tests {
    use super::*;
    fn folder()->PathBuf {let p=std::env::temp_dir().join(format!("spotlite-auth-test-{}",random_hex().unwrap()));fs::create_dir(&p).unwrap();p}
    #[test] fn pkce_challenge_rfc_vector(){assert_eq!(base64url(&Sha256::digest(b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")),"E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");}
    #[test] fn pending_state_is_single_use_and_keeps_verifier_private(){let url=begin(1,"test-client").unwrap();let state=query_value(&url,"state").unwrap();assert!(!url.contains("verifier"));assert_eq!(consume(&state).unwrap().client,"test-client");assert!(consume(&state).is_none());}
    #[test] fn rotation_preserves_refresh_if_absent(){let old=Token{refresh_token:"old".into(),..Default::default()};let t=rotated_token(&old,&json!({"access_token":"new","expires_in":3600})).unwrap();assert_eq!(t.refresh_token,"old");let t=rotated_token(&old,&json!({"access_token":"new","refresh_token":"rotated","expires_in":3600})).unwrap();assert_eq!(t.refresh_token,"rotated");}
    #[test] fn access_token_never_serialized(){let t=Token{access_token:"SECRET".into(),refresh_token:"REFRESH".into(),expires_at:12};assert!(!serde_json::to_string(&t).unwrap().contains("SECRET"));}
    #[test] fn encrypted_round_trip_and_corruption(){let t=Token{refresh_token:"sample".into(),..Default::default()};let raw=String::from_utf8(encode_record(&t).unwrap()).unwrap();assert!(!raw.contains("sample"));assert_eq!(decode_record(&raw).unwrap().refresh_token,"sample");assert!(decode_record("dpapi1:00").is_err());}
    #[test] fn replacement_is_complete_and_old_survives_failed_target(){let dir=folder();let p=dir.join("token");atomic_write(&p,b"old").unwrap();atomic_write(&p,b"new").unwrap();assert_eq!(fs::read(&p).unwrap(),b"new");let locked=fs::OpenOptions::new().read(true).open(&p).unwrap();drop(locked);fs::remove_file(p).unwrap();fs::remove_dir(dir).unwrap();}
}
