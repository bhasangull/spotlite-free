//! Bounded login helper. Only the target containing the player's unpredictable
//! nonce can be selected; no searching Spotify tabs or crossing account profiles.
use std::{fs,io::{self,Read,Write},net::TcpStream,path::Path,sync::{Arc,Mutex},time::{Duration,Instant}};
use serde_json::{json,Value};
use tungstenite::{WebSocket,Message};
use crate::PlaybackCommand;
const SCRIPT:&str=include_str!("ui/login-assist.js");
// Only enumerated codes may reach disk: never URLs, DOM text, exceptions,
// credentials, OAuth responses or browser target identifiers.
fn safe_code(code:&str)->&str {
    match code {
        "panel_request"|"command_queued"|"auth_start"|"client_rejected"|"broker_failed"|"broker_ready"|
        "bind_start"|"invalid_client"|"debug_file_failed"|"debug_port_invalid"|"targets_failed"|
        "targets_invalid"|"target_missing"|"target_ambiguous"|"socket_url_invalid"|"socket_connect_failed"|
        "socket_setup_failed"|"socket_handshake_failed"|"bound"|"already_active"|"worker_started"|
        "player_verified_or_returned"|"probe_failed"|"callback"|"returned"|"outside"|"waiting"|"email"|"password"|
        "continue"|"finish"|"captcha_required"|"verification_required"|"credentials_rejected"|
        "credentials_required"|"action_sent"|"action_failed"|"action_done"|"deadline"=>code,
        _=>"unknown_code"
    }
}
pub fn trace(root:&Path,n:u8,code:&str) {
    let line=format!("unix_ms={} account={} pid={} code={}\n",crate::now_ms(),n,std::process::id(),safe_code(code));
    let _=crate::diagnostics::append(root,"login-diagnostics.log",&line);
}
fn error()->io::Error {io::Error::other("Giriş yardımcısı kullanılamıyor")}
fn targets(port:u16)->io::Result<Value> {
    let mut stream=TcpStream::connect_timeout(&([127,0,0,1],port).into(),Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    write!(stream,"GET /json/list HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")?;
    read_targets_response(&mut stream)
}
fn read_targets_response(reader:&mut impl Read)->io::Result<Value> {
    // DevTools may keep HTTP connections alive even with Connection: close.
    // Read the declared body, not EOF (which previously always timed out).
    let bytes=crate::read_headers(reader)?;
    let end=bytes.windows(4).position(|w|w==b"\r\n\r\n").ok_or_else(error)?+4;
    let headers=std::str::from_utf8(&bytes[..end]).map_err(|_|error())?;
    if headers.lines().next().and_then(|s|s.split_whitespace().nth(1))!=Some("200"){return Err(error());}
    let mut length=None;
    for line in headers.lines().skip(1) {
        if let Some((name,value))=line.split_once(':') {
            if name.eq_ignore_ascii_case("transfer-encoding"){return Err(error());}
            if name.eq_ignore_ascii_case("content-length") {
                if length.is_some(){return Err(error());}
                length=Some(value.trim().parse::<usize>().map_err(|_|error())?);
            }
        }
    }
    let length=length.filter(|n|*n<=1024*1024).ok_or_else(error)?;
    let mut body=bytes[end..].to_vec();if body.len()>length{return Err(error());}
    let received=body.len();body.resize(length,0);reader.read_exact(&mut body[received..])?;
    serde_json::from_slice(&body).map_err(|_|error())
}
pub fn bind(root:&Path,n:u8,client:&str)->io::Result<WebSocket<TcpStream>> {
    trace(root,n,"bind_start");
    let fail=|code|{trace(root,n,code);error()};
    if client.len()!=64 {return Err(fail("invalid_client"));}
    let browser=if crate::ipv6::assignment_path(root,n).exists(){crate::ipv6::browser_path(root,n)}else{root.join("PlaybackBrowser")};
    let active=fs::read_to_string(browser.join("DevToolsActivePort")).map_err(|_|fail("debug_file_failed"))?;
    let port=active.lines().next().and_then(|s|s.parse::<u16>().ok()).filter(|p|*p>0).ok_or_else(||fail("debug_port_invalid"))?;
    let list=targets(port).map_err(|_|fail("targets_failed"))?;
    let prefix=format!("http://127.0.0.1:{}/?",43116+u16::from(n));
    let matches:Vec<_>=list.as_array().ok_or_else(||fail("targets_invalid"))?.iter().filter(|t| {
        t["type"]=="page" && t["url"].as_str().is_some_and(|url|url.starts_with(&prefix)&&crate::query_value(url,"client").as_deref()==Some(client))
    }).collect();
    if matches.len()!=1 {return Err(fail(if matches.is_empty(){"target_missing"}else{"target_ambiguous"}));}
    let url=matches[0]["webSocketDebuggerUrl"].as_str().ok_or_else(||fail("socket_url_invalid"))?;
    if !url.starts_with(&format!("ws://127.0.0.1:{port}/devtools/page/")) {return Err(fail("socket_url_invalid"));}
    let stream=TcpStream::connect_timeout(&([127,0,0,1],port).into(),Duration::from_secs(2)).map_err(|_|fail("socket_connect_failed"))?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).map_err(|_|fail("socket_setup_failed"))?;stream.set_write_timeout(Some(Duration::from_secs(3))).map_err(|_|fail("socket_setup_failed"))?;
    let ws=tungstenite::client(url,stream).map(|(ws,_)|ws).map_err(|_|fail("socket_handshake_failed"))?;
    trace(root,n,"bound");Ok(ws)
}
fn evaluate(ws:&mut WebSocket<TcpStream>,id:u64,expression:String)->io::Result<String> {
    ws.send(Message::Text(json!({"id":id,"method":"Runtime.evaluate","params":{"expression":expression,"returnByValue":true,"awaitPromise":true,"timeout":2000}}).to_string().into())).map_err(|_|error())?;
    for _ in 0..100 {
        let message=ws.read().map_err(|_|error())?;
        if let Message::Text(text)=message {
            let value:Value=serde_json::from_str(&text).map_err(|_|error())?;
            if value["id"]==id {return value["result"]["result"]["value"].as_str().map(str::to_owned).ok_or_else(error);}
        }
    }
    Err(error())
}
fn status(state:&Arc<Mutex<PlaybackCommand>>,name:&str) {
    if let Ok(mut c)=state.lock(){if c.reconnect_state!=name {c.reconnect_state=name.into();c.reconnect_ms=crate::now_ms();c.event(&format!("login-{name}"),crate::now_ms());}}
}
pub fn start(mut ws:WebSocket<TcpStream>,root:std::path::PathBuf,n:u8,state:Arc<Mutex<PlaybackCommand>>) {
    static ACTIVE:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
    if ACTIVE.swap(true,std::sync::atomic::Ordering::SeqCst){trace(&root,n,"already_active");return;}
    std::thread::spawn(move|| {
        struct Release;
        impl Drop for Release {fn drop(&mut self){ACTIVE.store(false,std::sync::atomic::Ordering::SeqCst);}}
        let _release=Release;
        trace(&root,n,"worker_started");
        let return_origin=json!(format!("http://127.0.0.1:{}",43116+u16::from(n)));
        let credentials=fs::read(crate::credentials::path(&root,n)).ok().and_then(|b|serde_json::from_slice::<Value>(&b).ok());
        let started=Instant::now();let mut id=0;let mut used=std::collections::HashSet::new();let mut seen=false;let mut last_probe=String::new();
        status(&state,"connecting");
        while started.elapsed()<Duration::from_secs(90) {
            std::thread::sleep(Duration::from_millis(1000));id+=1;
            // OAuth may return before the first poll. The player marks that
            // independently; never overwrite its later playback verification.
            if state.lock().is_ok_and(|c|c.reconnect_state=="verifying"||c.reconnect_state.is_empty()){trace(&root,n,"player_verified_or_returned");return;}
            let probe=evaluate(&mut ws,id,format!("({SCRIPT})(null,'probe',{return_origin})"));
            let Ok(kind)=probe else {trace(&root,n,"probe_failed");status(&state,"interaction_required");return;};
            if kind!=last_probe {trace(&root,n,&kind);last_probe=kind.clone();}
            if kind=="callback" {continue;}
            if kind=="returned" {if seen {status(&state,"verifying");return;}continue;}
            if kind=="outside" {status(&state,"interaction_required");return;}
            seen=true;
            if ["captcha_required","verification_required","credentials_rejected"].contains(&kind.as_str()) {status(&state,&kind);return;}
            if ["email","password","continue","finish"].contains(&kind.as_str()) && !used.contains(&kind) {
                let needs_credentials=kind=="email"||kind=="password";
                if needs_credentials && credentials.is_none(){trace(&root,n,"credentials_required");status(&state,"credentials_required");return;}
                used.insert(kind.clone());id+=1;
                let payload=if !needs_credentials {Value::Null}else{credentials.clone().unwrap_or(Value::Null)};
                // Origin/path checks are executed in the SAME expression as fill,
                // protecting navigation races. No credential values are logged.
                trace(&root,n,"action_sent");
                let result=evaluate(&mut ws,id,format!("({SCRIPT})({payload},{},{return_origin})",json!(kind)));
                if !result.is_ok_and(|s|s=="continued"||s=="email_submitted"||s=="password_submitted") {trace(&root,n,"action_failed");status(&state,"interaction_required");return;}
                trace(&root,n,"action_done");
            }
        }
        trace(&root,n,"deadline");status(&state,"interaction_required");
    });
}
#[cfg(test)] mod tests {
    #[test] fn devtools_response_does_not_wait_for_connection_close() {
        struct OpenConnection(std::io::Cursor<Vec<u8>>);
        impl std::io::Read for OpenConnection {
            fn read(&mut self,b:&mut [u8])->std::io::Result<usize> {
                if self.0.position()==self.0.get_ref().len() as u64 {return Err(std::io::Error::new(std::io::ErrorKind::TimedOut,"connection still open"));}
                let max=b.len().min(7);std::io::Read::read(&mut self.0,&mut b[..max])
            }
        }
        let input=b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[]".to_vec();
        assert_eq!(super::read_targets_response(&mut OpenConnection(std::io::Cursor::new(input))).unwrap(),serde_json::json!([]));
    }
    #[test] fn devtools_response_rejects_bad_framing() {
        for input in ["HTTP/1.1 403 Forbidden\r\nContent-Length: 2\r\n\r\n[]","HTTP/1.1 200 OK\r\nContent-Length: 99999999\r\n\r\n","HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\n[]"] {
            assert!(super::read_targets_response(&mut std::io::Cursor::new(input.as_bytes())).is_err());
        }
    }
    #[test] fn diagnostic_codes_cannot_include_secrets() {
        for value in ["test@example.invalid","https://accounts.spotify.com/?token=secret","password\ncode=bound", "{\"password\":\"secret\"}"] {
            assert_eq!(super::safe_code(value),"unknown_code");
        }
        assert_eq!(super::safe_code("target_missing"),"target_missing");
    }
}
