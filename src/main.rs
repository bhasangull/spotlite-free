#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::{
    collections::HashSet,
    env,
    ffi::{c_void, OsStr, OsString},
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Command, ExitCode},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const PLAYER_URL: &str = "https://open.spotify.com/";
const TOKENS_CSS: &str = include_str!("../tokens.css");
mod auth;
mod desktop;
mod diagnostics;
mod credentials;
mod login_assist;
mod ipv6;
const CONTROL_HTML: &str = include_str!("ui/control.html");
const LITE_HTML: &str = include_str!("ui/player.html");

#[derive(Default)]
struct PlaybackCommand {
    version: u64,
    acknowledged: u64,
    compact: bool,
    client: String,
    url: String,
    loop_mode: String,
    login: bool,
    playing: bool,
    starting: bool,
    track: String,
    position_ms: u64,
    duration_ms: u64,
    updated_ms: u64,
    last_reason: String,
    state: String,
    received_ms: u64,
    loop_only: bool,
    health: serde_json::Value,
    started_ms: u64,
    last_advance_ms: u64,
    events: Vec<serde_json::Value>,
    recovery_ms: u64,
    reconnect_state: String,
    reconnect_ms: u64,
}

impl PlaybackCommand {
    fn event(&mut self,kind:&str,now:u64){
        self.events.push(serde_json::json!({"at":now,"kind":kind}));
        if self.events.len()>20 {self.events.remove(0);}
    }
    fn set_loop(&mut self, mode: String) {
        // An unconsumed Play/Login intent must survive a later mode change.
        self.loop_only = !self.login && (self.loop_only || self.acknowledged == self.version);
        self.version = self.version.wrapping_add(1).max(1);
        self.loop_mode = mode;
    }
    fn acknowledge(&mut self, version: u64) {
        if version == self.version {
            self.acknowledged = version;
            self.login = false;
        }
    }
}

struct ProcessFileLock { _file: fs::File }

fn acquire_process_file_lock(path: PathBuf, timeout: Duration) -> std::io::Result<ProcessFileLock> {
    let started = std::time::Instant::now();
    while started.elapsed() < timeout {
        let mut options=fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)] {use std::os::windows::fs::OpenOptionsExt;options.share_mode(0);}
        match options.open(&path) {
            Ok(file)=>return Ok(ProcessFileLock{_file:file}),
            Err(error) if error.raw_os_error()==Some(32)=>thread::sleep(Duration::from_millis(50)),
            Err(error)=>return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "Başka bir hesap penceresi hazırlanıyor.",
    ))
}

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
    ipv6::ensure_pool(&app_data).map_err(|e|format!("IPv6 havuzu açılamadı: {e}"))?;
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
                    let _ = existing.write_all(b"POST /focus HTTP/1.1\r\nHost: 127.0.0.1:43116\r\nX-SpotLite-Request: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
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
    let account_ipv6=if controller_mode || full_mode {None}else{
        ipv6::read_assignment(&app_data,instance).map_err(|e|format!("Hesap IPv6 kaydı okunamadı: {e}"))?
    };
    if controller_mode {
        cleanup_stale_shared_runtime(&app_data);
        let auth_app_data = app_data.clone();
        thread::spawn(move || serve_auth_callback(auth_app_data));
        let refresh_app_data = app_data.clone();
        thread::spawn(move || refresh_tokens_loop(refresh_app_data));
    }

    let profile_name = if full_mode {
        format!("ProfileFull{instance}")
    } else if account_ipv6.is_some() {
        format!("IPv6Browser{instance}")
    } else {
        "PlaybackBrowser".to_owned()
    };
    let cache_name = if full_mode {
        format!("CacheFull{instance}")
    } else if account_ipv6.is_some() {
        format!("IPv6Cache{instance}")
    } else {
        "PlaybackCache".to_owned()
    };
    let profile = app_data.join(profile_name);
    let cache = app_data.join(cache_name);
    if account_ipv6.is_some() {
        fs::create_dir_all(profile.join("Default")).map_err(|e|format!("IPv6 hesap profili oluşturulamadı: {e}"))?;
    } else if !full_mode {
        prepare_shared_account_profile(&app_data, instance.max(1))
            .map_err(|error| format!("Hesap profili hazırlanamadı: {error}"))?;
        if controller_mode {
            prepare_shared_control_profile(&app_data)
                .map_err(|error| format!("Kontrol paneli profili hazırlanamadı: {error}"))?;
        }
    }
    fs::create_dir_all(&profile).map_err(|error| format!("Profil oluşturulamadı: {error}"))?;
    fs::create_dir_all(&cache).map_err(|error| format!("Önbellek oluşturulamadı: {error}"))?;
    if let Some(source)=account_ipv6 {
        ipv6::start_proxy(source,instance).map_err(|e|format!("Hesap {instance} IPv6 geçidi başlatılamadı: {e}"))?;
    }

    let client=auth::random_hex().map_err(|e|e.to_string())?;
    let target_url = if full_mode {
        PLAYER_URL.to_owned()
    } else if controller_mode {
        "http://127.0.0.1:43116/".to_owned()
    } else if login_on_start {
        format!("http://127.0.0.1:{main_port}/?client={client}&login=1")
    } else {
        format!("http://127.0.0.1:{main_port}/?client={client}")
    };

    let mut command = Command::new(edge);
    command
        // Edge'in Spotify tarafından desteklenen, DRM özellikli motorunu tek site
        // penceresi olarak kullanır. Şifreli medyaya müdahale edilmez.
        .arg(format!("--app={target_url}"))
        .arg(format!("--user-data-dir={}", profile.display()))
        .args((!full_mode).then(|| {
            format!(
                "--profile-directory={}",
                if controller_mode {
                    "ControlPanel".to_owned()
                } else if account_ipv6.is_some() {
                    "Default".to_owned()
                } else {
                    account_profile_directory(instance)
                }
            )
        }))
        .arg(format!("--disk-cache-dir={}", cache.display()))
        .arg("--disk-cache-size=268435456")
        .arg("--media-cache-size=134217728")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-default-apps")
        .arg("--disable-extensions")
        .arg("--remote-debugging-port=0")
        .arg("--remote-debugging-address=127.0.0.1")
        .arg("--disable-sync")
        .arg("--disable-popup-blocking")
        // Spotify Embed'in EME/DRM ve oturum davranışını bozabilecek deneysel
        // StoragePartitioning/çerez bayraklarını özellikle kullanma.
        // Widevine/EME tam akış için GPU hızlandırmasını kapatma; kapalı
        // mod bazı Edge profillerinde parçayı kısa preview'e düşürüyor.
        .args(desktop::browser_arguments())
        .arg("--disable-background-mode")
        .arg(if controller_mode {
            "--window-size=960,700"
        } else {
            "--window-size=560,650"
        });
    if account_ipv6.is_some() {
        command.arg(format!("--proxy-server=socks5://127.0.0.1:{}",ipv6::proxy_port(instance)));
        command.arg("--disable-quic");
        command.arg("--dns-prefetch-disable");
    }

    if background && !login_on_start && !controller_mode {
        command.arg("--window-position=-20000,-20000");
    }

    // Tanılama için URL veya güvenlik davranışını değiştirmeyen ek Edge
    // seçenekleri verilebilir: spotlite-free.exe --edge-arg=--enable-logging
    for argument in env::args_os().skip(1) {
        if let Some(value) = edge_argument(argument) {
            if account_ipv6.is_some(){return Err("IPv6 hesabında ek Edge ağ parametreleri kullanılamaz.".into());}
            command.arg(value);
        }
    }

    let launch_lock = if !full_mode {
        Some(
            acquire_process_file_lock(
                app_data.join(if account_ipv6.is_some(){format!("IPv6Browser{instance}.launch.lock")}else{"PlaybackBrowser.launch.lock".to_owned()}),
                Duration::from_secs(30),
            )
            .map_err(|error| format!("Hesap penceresi sırası alınamadı: {error}"))?,
        )
    } else {
        None
    };
    let existing_windows = (!full_mode).then(snapshot_edge_windows);
    let browser_identity_path=app_data.join(if account_ipv6.is_some(){format!("IPv6Browser{instance}.process.json")}else{"PlaybackBrowser.process.json".to_owned()});
    let previous_browser=fs::read(&browser_identity_path).ok().and_then(|s|serde_json::from_slice::<serde_json::Value>(&s).ok())
        .and_then(|v|{let pid=u32::try_from(v["pid"].as_u64()?).ok()?;let born=v["born"].as_u64()?;
            (desktop::process_birth(pid)==Some(born)).then_some(pid)});
    let child = command
        .spawn()
        .map_err(|error| format!("Spotify penceresi başlatılamadı: {error}"))?;
    let edge_pid = child.id();
    let browser_pid=previous_browser.unwrap_or_else(||if controller_mode || account_ipv6.is_some() {edge_pid}else{find_account_window(0).map(desktop::process).unwrap_or(edge_pid)});
    let account_window = if let Some(existing) = existing_windows.as_ref() {
        wait_for_new_edge_window(existing, Duration::from_secs(12),browser_pid)
    } else {
        None
    };
    if !full_mode && account_window.is_none() {
        return Err(if controller_mode {
            "Kontrol paneli penceresi hazırlanamadı.".to_owned()
        } else {
            format!("Hesap {instance} penceresi hazırlanamadı.")
        });
    }
    if let Some(window) = account_window {
        let pid=desktop::process(window);
        if let Some(born)=desktop::process_birth(pid) {let _=auth::atomic_write(&browser_identity_path,serde_json::json!({"pid":pid,"born":born}).to_string().as_bytes());}
        let _ = fs::write(account_window_path(&app_data, instance), window.to_string());
        let _ = fs::write(account_window_path(&app_data, instance).with_extension("pid"),desktop::process(window).to_string());
    }
    drop(launch_lock);
    if background && !login_on_start {if let Some(window)=account_window {desktop::background(window,true);}}
    let _ = replace_window;

    if let Some(listener) = listener {
        let command_state = Arc::new(Mutex::new(PlaybackCommand{client,..Default::default()}));
        serve_lite_player(
            listener,
            instance,
            edge_pid,
            controller_pid,
            app_data,
            child,
            account_window,
            command_state,
        )
        .map_err(|error| format!("Hafif oynatıcı bağlantısı kesildi: {error}"))?;
    }

    Ok(())
}

fn serve_auth_callback(root: PathBuf) { auth::serve(root); }
fn refresh_tokens_loop(root: PathBuf) { auth::refresh_loop(root); }

#[cfg(windows)]
#[repr(C)]
struct DataBlob { cb_data: u32, pb_data: *mut u8 }

#[cfg(windows)]
#[link(name = "crypt32")]
extern "system" { fn CryptProtectData(data_in: *const DataBlob, description: *const u16, entropy: *const DataBlob, reserved: *mut c_void, prompt: *mut c_void, flags: u32, data_out: *mut DataBlob) -> i32; fn CryptUnprotectData(data_in: *const DataBlob, description: *mut *mut u16, entropy: *const DataBlob, reserved: *mut c_void, prompt: *mut c_void, flags: u32, data_out: *mut DataBlob) -> i32; }

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" { fn LocalFree(memory: *mut c_void) -> *mut c_void; }

#[cfg(windows)]
fn dpapi_protect(input: &[u8]) -> Option<Vec<u8>> {
    unsafe {
        let source = DataBlob { cb_data: input.len() as u32, pb_data: input.as_ptr() as *mut u8 };
        let mut output = DataBlob { cb_data: 0, pb_data: std::ptr::null_mut() };
        if CryptProtectData(&source, std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), 1, &mut output) == 0 { return None; }
        let result = std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec();
        LocalFree(output.pb_data as *mut c_void);
        Some(result)
    }
}

#[cfg(windows)]
fn dpapi_unprotect(input: &[u8]) -> Option<Vec<u8>> {
    unsafe {
        let source = DataBlob { cb_data: input.len() as u32, pb_data: input.as_ptr() as *mut u8 };
        let mut output = DataBlob { cb_data: 0, pb_data: std::ptr::null_mut() };
        if CryptUnprotectData(&source, std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), 1, &mut output) == 0 { return None; }
        let result = std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec();
        LocalFree(output.pb_data as *mut c_void);
        Some(result)
    }
}

#[cfg(windows)]
fn hex_encode(bytes: &[u8]) -> String { bytes.iter().map(|byte| format!("{byte:02x}")).collect() }

#[cfg(windows)]
fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if !value.is_ascii() || value.len()%2!=0 {return None;}
    value.as_bytes().chunks_exact(2).map(|s| u8::from_str_radix(std::str::from_utf8(s).ok()?,16).ok()).collect()
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

fn local_request_allowed(request: &[u8], port: u16, mutation: bool) -> bool {
    let text=String::from_utf8_lossy(request);
    let mut lines=text.lines();
    let first=lines.next().unwrap_or_default();
    let method=first.split_whitespace().next().unwrap_or_default();
    let headers: Vec<_>=lines.filter_map(|s|s.split_once(':')).map(|(k,v)|(k.trim().to_ascii_lowercase(),v.trim())).collect();
    let header=|name:&str|headers.iter().find(|(k,_)|k==name).map(|(_,v)|*v);
    let host=format!("127.0.0.1:{port}");
    let localhost=format!("localhost:{port}");
    if !matches!(header("host"),Some(v) if v==host || v==localhost){return false;}
    if let Some(origin)=header("origin") {
        if origin!=format!("http://{host}") && origin!=format!("http://{localhost}") {return false;}
    }
    if mutation {method=="POST" && header("x-spotlite-request")==Some("1") && header("sec-fetch-site")!=Some("cross-site")}
    else {method=="GET"}
}

fn mutation_path(path:&str)->bool {
    if path=="/session" {return false;}
    !matches!(path.split('?').next().unwrap_or_default(), "/" | "/tokens.css" | "/ui.css" | "/player.js" | "/control.js" | "/playback-core.js" | "/accounts" | "/status" | "/command" | "/auth-state" | "/catalog" | "/favicon.ico" | "/heartbeat")
}
fn player_request_matches(request:&[u8],client:&str)->bool {
    !client.is_empty() && String::from_utf8_lossy(request).lines().filter_map(|l|l.split_once(':'))
        .any(|(key,value)|key.eq_ignore_ascii_case("x-spotlite-player") && value.trim()==client)
}

fn read_headers(reader:&mut impl Read)->std::io::Result<Vec<u8>> {
    let mut bytes=Vec::with_capacity(1024);
    let mut chunk=[0u8;1024];
    while bytes.len()<8192 {
        let limit=chunk.len().min(8192-bytes.len());
        let size=reader.read(&mut chunk[..limit])?;
        if size==0 {return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof,"Eksik HTTP başlığı"));}
        bytes.extend_from_slice(&chunk[..size]);
        if bytes.windows(4).any(|w|w==b"\r\n\r\n"){return Ok(bytes);}
    }
    Err(std::io::Error::new(std::io::ErrorKind::InvalidData,"HTTP başlığı çok büyük"))
}

fn send_local_request(port: u16, path: &str) -> std::io::Result<String> {
    let mut stream = TcpStream::connect_timeout(&std::net::SocketAddr::from(([127,0,0,1],port)),Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(if port==5588 {55}else{3})))?;
    let method=if port==5588 || mutation_path(path) {"POST"}else{"GET"};
    stream.write_all(format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-SpotLite-Request: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes())?;
    let mut response=String::new();stream.read_to_string(&mut response)?;Ok(response)
}
fn response_body(response:&str)->&str {response.split_once("\r\n\r\n").map(|(_,b)|b).unwrap_or("{}")}

fn now_ms()->u64 {SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64}
fn apply_sample(command:&mut PlaybackCommand,path:&str,now:u64)->bool {
    let number=|key|query_value(path,key).and_then(|s|s.parse::<u64>().ok()).unwrap_or(0);
    let timestamp=number("updated");
    if number("version")!=command.version || timestamp<command.updated_ms || timestamp>now+5000 {return false;}
    let state=query_value(path,"state").unwrap_or_else(||"unknown".into());
    if !["playing","loading","paused","ended","buffering","stalled","unknown","unverified_media"].contains(&state.as_str()){return false;}
    let track=query_value(path,"track").unwrap_or_default();
    let duration=number("duration");
    if command.started_ms==0 {command.started_ms=now;}
    if state=="playing" && track==command.track && number("position")>command.position_ms+50 {
        command.last_advance_ms=now;
        if command.recovery_ms>0 {command.event("recovered",now);command.recovery_ms=0;}
    }
    if state!=command.state && matches!(state.as_str(),"stalled"|"buffering"|"unverified_media") {command.event(&state,now);}
    // A zero-duration packet can preserve the displayed timeline, but must not
    // preserve playing=true. Track changes invalidate the previous timeline.
    if duration>0 || track!=command.track {
        command.position_ms=number("position").min(duration);
        command.duration_ms=duration;
    }
    command.playing=state=="playing" && query_value(path,"playing").as_deref()==Some("1");
    command.starting=state=="loading";
    command.track=track;command.updated_ms=timestamp;command.received_ms=now;
    command.state=state;command.last_reason=query_value(path,"reason").unwrap_or_default();true
}
fn runtime_snapshot(command:&PlaybackCommand,now:u64)->serde_json::Value {
    let fresh=command.updated_ms>0 && now.saturating_sub(command.updated_ms)<=8000 && now.saturating_sub(command.received_ms)<=8000;
    serde_json::json!({"playing":fresh&&command.playing,"starting":command.starting&&now.saturating_sub(command.received_ms)<20000,
        "track":command.track,"position":command.position_ms,"duration":command.duration_ms,"updated":command.updated_ms,
        "started":command.started_ms,"lastAdvance":command.last_advance_ms,"events":command.events,
        "recovering":command.recovery_ms>0 && now.saturating_sub(command.recovery_ms)<10000,
        "reconnect":if command.reconnect_state=="connecting" && now.saturating_sub(command.reconnect_ms)>20000 {"interaction_required"} else {command.reconnect_state.as_str()},
        "state":if fresh{command.state.as_str()}else{"unknown"},"lastReason":if fresh{command.last_reason.as_str()}else{"Spotify’dan güncel oynatma verisi gelmiyor"}})
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
    let root=env::var_os("LOCALAPPDATA").map(PathBuf::from).ok_or_else(||std::io::Error::other("LOCALAPPDATA eksik"))?.join("SpotLiteFree");
    let _lock=acquire_process_file_lock(root.join(format!("Account{instance}.start.guard")),Duration::from_secs(30))?;
    if !account_server_running(instance) {start_account_process(instance, controller_pid, false, true)?;}
    // A bound port is not a ready HTTP server: the helper binds before launching
    // Edge. Wait for a response instead of losing the first bulk-start command.
    let deadline=std::time::Instant::now()+Duration::from_secs(25);
    while std::time::Instant::now()<deadline {
        if send_local_request(43116+u16::from(instance),"/status").is_ok_and(|s|s.starts_with("HTTP/1.1 200")){return Ok(());}
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
    send_local_request(port,"/park-self")?;
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
    account_window: Option<isize>,
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
                        account_window,
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
            if account_window.is_none() {
                return Ok(());
            }
        }
        if let Some(window) = account_window {
            if !window_exists(window) {
                return Ok(());
            }
            if controller_pid.is_some_and(|pid| !process_exists(pid)) {
                close_window(window);
                return Ok(());
            }
        }
    }
}

fn respond(
    stream: &mut TcpStream,
    instance: u8,
    edge_pid: u32,
    controller_pid: Option<u32>,
    app_data: &std::path::Path,
    account_window: Option<isize>,
    command_state: &Arc<Mutex<PlaybackCommand>>,
) -> std::io::Result<()> {
    let controller_host_pid = std::process::id();
    let request = read_headers(stream)?;
    let no_content = request.starts_with(b"GET /favicon.ico ")
        || request.starts_with(b"GET /heartbeat ");
    let path = request_path(&request).unwrap_or_default();
    let focus = path == "/focus" || path.starts_with("/focus?");
    let port=43116+u16::from(instance);
    if !local_request_allowed(&request,port,mutation_path(&path)) {
        stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 9\r\nConnection: close\r\n\r\nForbidden")?;
        return Ok(());
    }
    let renderer_route=matches!(path.split('?').next().unwrap_or_default(),"/command"|"/command-ack"|"/player-health"|"/player-status");
    if instance>0 && renderer_route {
        let valid=command_state.lock().map(|c|player_request_matches(&request,&c.client)).unwrap_or(false);
        if !valid {stream.write_all(b"HTTP/1.1 410 Gone\r\nContent-Length: 7\r\nConnection: close\r\n\r\nRetired")?;return Ok(());}
    }
    if instance>0 && path.starts_with("/player-event?") {
        let valid=command_state.lock().map(|c|player_request_matches(&request,&c.client)).unwrap_or(false);
        if !valid{return respond_text(stream,"retired");}
        let event=query_value(&path,"event").unwrap_or_default();
        if ["repeat-resume-1","repeat-resume-2","repeat-resume-3","repeat-resume-error","repeat-last-recovery-attempt","reconnect-start","reconnect-failed","reconnect-returned","reconnect-verified"].contains(&event.as_str()) {
            let line=format!("unix={} | hesap={} | event={}\n",now_ms()/1000,instance,event);
            let _=diagnostics::append(app_data,"playback-diagnostics.log",&line);
            if let Ok(mut c)=command_state.lock(){
                if event.starts_with("reconnect-") {
                    c.reconnect_state=match event.as_str(){"reconnect-start"=>"connecting","reconnect-failed"=>"failed","reconnect-returned"=>"verifying",_=>""}.into();
                    c.reconnect_ms=now_ms();
                }else{c.recovery_ms=now_ms();}
                c.event(&event,now_ms());
            }
            if event=="reconnect-start" {if let Some(window)=account_window {desktop::background(window,false);}}
        }
        return respond_text(stream,"ok");
    }
    if instance==0 && path.starts_with("/credentials?") {
        let _guard=acquire_process_file_lock(app_data.join("accounts.allocate.lock"),Duration::from_secs(5))?;
        let n=query_value(&path,"instance").and_then(|s|s.parse::<u8>().ok()).filter(|n|(1..=9).contains(n)&&account_exists(app_data,*n));
        let Some(n)=n else {return respond_text(stream,"error");};
        if query_value(&path,"action").as_deref()==Some("status") {
            let value=fs::read(credentials::path(app_data,n)).ok().and_then(|b|serde_json::from_slice::<credentials::Credentials>(&b).ok());
            return respond_json(stream,&serde_json::json!({"saved":value.is_some(),"email":value.map(|v|v.email).unwrap_or_default()}).to_string());
        }
        let ok=credentials::read_body(stream,&request).and_then(|b|credentials::save(app_data,n,&b)).is_ok();
        return respond_text(stream,if ok {"ok"} else {"error"});
    }
    if instance==0 && path=="/session" {
        static BOOT:std::sync::OnceLock<u64>=std::sync::OnceLock::new();
        return respond_json(stream,&serde_json::json!({"boot":BOOT.get_or_init(now_ms)}).to_string());
    }
    let asset=match path.as_str(){
        "/ui.css"=>Some((include_str!("ui/ui.css"),"text/css")),
        "/player.js"=>Some((include_str!("ui/player.js"),"text/javascript")),
        "/control.js"=>Some((include_str!("ui/control.js"),"text/javascript")),
        "/playback-core.js"=>Some((include_str!("ui/playback-core.js"),"text/javascript")),
        _=>None
    };
    if let Some((body,kind))=asset{return respond_with_type(stream,body,kind);}
    if instance>0 && path=="/auth-state" {return respond_json(stream,&auth::state(app_data,instance).to_string());}
    if instance>0 && path=="/park-self" {if let Some(window)=account_window{desktop::background(window,true);}return respond_text(stream,"ok");}
    if instance>0 && path.starts_with("/player-health?") {
        if let Ok(mut c)=command_state.lock(){c.health=serde_json::json!({"visibility":query_value(&path,"visibility"),"focused":query_value(&path,"focused").as_deref()==Some("1"),"ready":query_value(&path,"ready").as_deref()==Some("1"),"iframeLoading":query_value(&path,"loading"),"repeats":query_value(&path,"repeats").and_then(|s|s.parse::<u64>().ok()).unwrap_or(0),"updated":now_ms()});}
        return respond_text(stream,"ok");
    }
    if instance==0 && path.starts_with("/control-loop?") {
        let n=query_value(&path,"instance").and_then(|s|s.parse::<u8>().ok()).filter(|n|(1..=9).contains(n)&&account_exists(app_data,*n));
        let ok=n.is_some_and(|n|send_local_request(43116+u16::from(n),&format!("/set-loop?loop={}",percent_encode(&query_value(&path,"loop").unwrap_or_default()))).is_ok());
        return respond_text(stream,if ok{"ok"}else{"error"});
    }
    if instance>0 && path.starts_with("/set-loop?") {
        let mode=query_value(&path,"loop").unwrap_or_default();
        if !["off","playlist","track"].contains(&mode.as_str()){return respond_text(stream,"error");}
        if let Ok(mut c)=command_state.lock(){c.set_loop(mode);}
        return respond_text(stream,"ok");
    }
    if instance>0 && path.starts_with("/command-ack?") {
        let version=query_value(&path,"version").and_then(|v|v.parse::<u64>().ok()).unwrap_or(0);
        if let Ok(mut c)=command_state.lock(){c.acknowledge(version);}
        return respond_text(stream,"ok");
    }
    if instance>0 && path.starts_with("/set-layout?") {
        let compact=query_value(&path,"compact").as_deref()==Some("1");
        if let Ok(mut c)=command_state.lock(){c.compact=compact;}
        if let Some(window)=account_window{desktop::compact(window,compact);}
        return respond_text(stream,"ok");
    }
    if instance>0 && (path=="/auth-start" || path.starts_with("/catalog?")) {
        let helper=if path=="/auth-start" {
            login_assist::trace(app_data,instance,"auth_start");
            let client=command_state.lock().map(|c|c.client.clone()).unwrap_or_default();
            if !player_request_matches(&request,&client){login_assist::trace(app_data,instance,"client_rejected");return respond_text(stream,"retired");}
            Some(login_assist::bind(app_data,instance,&client))
        }else{None};
        let broker_path=if path=="/auth-start" {let client=command_state.lock().map(|c|c.client.clone()).unwrap_or_default();format!("/begin?instance={instance}&client={client}")}else{format!("/catalog?instance={instance}&uri={}",percent_encode(&query_value(&path,"uri").unwrap_or_default()))};
        let response=send_local_request(5588,&broker_path);
        let body=response.as_ref().map(|s|response_body(s).to_owned()).unwrap_or_else(|_|serde_json::json!({"error":"Hesap servisine ulaşılamadı; kontrol panelini açık tutun."}).to_string());
        if serde_json::from_str::<serde_json::Value>(&body).ok().is_some_and(|v|v["url"].is_string()) {
            login_assist::trace(app_data,instance,"broker_ready");
            if let Some(helper)=helper {match helper {
                Ok(ws)=>login_assist::start(ws,app_data.to_path_buf(),instance,Arc::clone(command_state)),
                Err(_)=>{if let Ok(mut c)=command_state.lock(){c.reconnect_state="helper_unavailable".into();c.reconnect_ms=now_ms();}}
            }}
        }else if path=="/auth-start" {login_assist::trace(app_data,instance,"broker_failed");}
        return respond_json(stream,&body);
    }
    if instance==0 && path.starts_with("/open-account?") {
        let n=query_value(&path,"instance").and_then(|s|s.parse::<u8>().ok()).filter(|n|(1..=9).contains(n)&&account_exists(app_data,*n));
        let ok=n.is_some_and(|n|ensure_account_running(n,controller_host_pid).and_then(|_|send_local_request(43116+u16::from(n),"/focus").map(|_|())).is_ok());
        return respond_text(stream,if ok{"ok"}else{"error"});
    }
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
            .and_then(|number| control_account(number, controller_host_pid, &url, &loop_mode));
        respond_text(stream, if result.is_ok() { "ok" } else { "error" })?;
    } else if instance == 0 && path.starts_with("/control-all?") {
        let url = query_value(&path, "url").unwrap_or_default();
        let loop_mode = query_value(&path, "loop").unwrap_or_else(|| "playlist".to_owned());
        let mut failures = 0_u8;
        for selected in (1_u8..=9).filter(|number| account_exists(app_data, *number)) {
            if control_account(selected, controller_host_pid, &url, &loop_mode).is_err() {
                failures += 1;
            }
        }
        respond_text(stream, if failures == 0 { "ok" } else { "error" })?;
    } else if instance == 0 && path.starts_with("/login-account?") {
        let result = if let Some(selected) =
            query_value(&path, "instance").and_then(|value| value.parse::<u8>().ok())
        {
            if (1..=9).contains(&selected) && account_exists(app_data, selected) {
                login_assist::trace(app_data,selected,"panel_request");
                let port = 43116_u16 + u16::from(selected);
                if account_server_running(selected) {
                    send_local_request(
                        port,
                        &format!("/request-login?controller={controller_host_pid}"),
                    )
                    .map(|_| ())
                } else {
                    start_account_process(selected, controller_host_pid, true, false)
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
    } else if instance==0 && path == "/add-account" {
        let _lock=acquire_process_file_lock(app_data.join("accounts.allocate.lock"),Duration::from_secs(5))?;
        // Reserve synchronously, before spawning. Profile existence prevents
        // creating another account in an already occupied slot.
        let selected=(1..=9).find(|n|!account_exists(app_data,*n) && !app_data.join(format!("AccountDeleted{n}.txt")).exists());
        if let Some(n)=selected {
            if ipv6::assign_new(app_data,n).is_err(){return respond_text(stream,"ipv6_unavailable");}
            auth::atomic_write(&account_spotify_name_path(app_data,n),format!("Hesap {n}").as_bytes())?;
            let result=start_account_process(n,controller_host_pid,true,false);
            respond_text(stream,if result.is_ok(){"opened"}else{"error"})?;
        }else{respond_text(stream,"full")?;}
    } else if instance > 0 && path.starts_with("/player-status?") {
        if let Ok(mut command)=command_state.lock() {
            let previous=command.state.clone();
            let accepted=apply_sample(&mut command,&path,now_ms());
            let state=command.state.clone();
            if accepted && !state.is_empty() && state!=previous {
                let line=format!("unix={} | hesap={} | state={} | position={} | duration={}\n",unix_now(),instance,state,command.position_ms,command.duration_ms);
                let _=diagnostics::append(app_data,"playback-diagnostics.log",&line);
            }
        }
        respond_text(stream,"saved")?;
    } else if instance > 0 && path == "/status" {
        let body=command_state.lock().map(|c|{let mut v=runtime_snapshot(&c,now_ms());v["health"]=c.health.clone();v["window"]=account_window.map(desktop::state).unwrap_or_default();v["silentConfigured"]=true.into();v.to_string()}).unwrap_or_else(|_|"{}".into());
        respond_json(stream,&body)?;
    } else if instance > 0 && path.starts_with("/set-command?") {
        let url = query_value(&path, "url").unwrap_or_default();
        let loop_mode = query_value(&path, "loop").unwrap_or_else(|| "playlist".to_owned());
        if let Ok(mut command) = command_state.lock() {
            command.version = command.version.wrapping_add(1).max(1);
            command.url = url;
            command.loop_mode = loop_mode;
            command.login = false;
            command.loop_only = false;
            command.playing = false;
            command.starting = true;
            command.track.clear();
            command.position_ms = 0;
            command.duration_ms = 0;
            command.updated_ms = 0;
            command.received_ms = now_ms();
            command.state = "loading".into();
            command.last_reason = "Spotify oynatması bekleniyor".into();
        }
        respond_text(stream, "queued")?;
    } else if instance > 0 && path.starts_with("/command?") {
        let after = query_value(&path, "after")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default();
        let body = if let Ok(command) = command_state.lock() {
            let is_new = command.version > after;
            let body = format!(
                "{{\"version\":{},\"url\":\"{}\",\"loop\":\"{}\",\"login\":{},\"loopOnly\":{},\"compact\":{}}}",
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
                is_new && command.login,
                is_new && command.loop_only,
                command.compact
            );
            body
        } else {
            "{\"version\":0,\"url\":\"\",\"loop\":\"\",\"login\":false}".to_owned()
        };
        respond_json(stream, &body)?;
    } else if instance > 0 && path.starts_with("/request-login?") {
        login_assist::trace(app_data,instance,"command_queued");
        if let Ok(mut command) = command_state.lock() {
            command.version = command.version.wrapping_add(1).max(1);
            command.login = true;
            command.loop_only = false;
        }
        let panel_pid = query_value(&path, "controller")
            .and_then(|value| value.parse::<u32>().ok())
            .or(controller_pid);
        thread::spawn(move || {
            if let Some(window) = account_window {
                desktop::background(window,false);
                focus_window(window);
                return;
            }
            for _ in 0..100 {
                if focus_spotify_login_window_from(edge_pid, panel_pid) {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        });
        respond_text(stream, "queued")?;
    } else if path.starts_with("/delete-account?") {
        if let Some(selected) =
            query_value(&path, "instance").and_then(|value| value.parse::<u8>().ok())
        {
            if (1..=9).contains(&selected) {
                if selected == instance {
                    let _ = respond_text(stream, "deleted");
                    delete_account_data(app_data, selected, account_window);
                    std::process::exit(0);
                }
                let port = 43116_u16 + u16::from(selected);
                if let Ok(mut target) = TcpStream::connect(("127.0.0.1", port)) {
                    target.write_all(format!("POST /shutdown-self HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-SpotLite-Request: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes())?;
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
        delete_account_data(app_data, instance, account_window);
        std::process::exit(0);
    } else if path == "/shutdown-self" {
        let _ = respond_text(stream, "stopped");
        if let Some(window) = account_window {
            close_window(window);
        }
        std::process::exit(0);
    } else if focus {
        let previous_pid = query_value(&path, "from").and_then(|value| value.parse().ok());
        if let Some(window) = account_window {
            desktop::background(window,false);
            focus_window(window);
            if let Some(previous_pid) = previous_pid {
                hide_process_window(previous_pid);
            }
        } else {
            focus_process_window_from(edge_pid, previous_pid);
        }
        respond_text(stream, "focused")?;
    } else if no_content {
        stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")?;

    } else if instance == 0 {
        respond_html(stream, CONTROL_HTML)?;
    } else {
        let client=command_state.lock().map(|c|c.client.clone()).unwrap_or_default();
        if query_value(&path,"client").as_deref()!=Some(client.as_str()) {return respond_html(stream,"<!doctype html><meta charset=utf-8><p>Bu oynatıcı penceresi artık etkin değil. Kontrol panelindeki Aç düğmesini kullanın.</p>");}
        let html = LITE_HTML.replace("{{INSTANCE}}", &instance.to_string()).replace("{{CLIENT}}",&client);
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

fn account_profile_directory(instance: u8) -> String {
    if instance == 1 {
        "Default".to_owned()
    } else {
        format!("Account{instance}")
    }
}

fn shared_playback_profile_path(app_data: &std::path::Path) -> PathBuf {
    app_data.join("PlaybackBrowser")
}

fn legacy_account_profile_path(app_data: &std::path::Path, instance: u8) -> PathBuf {
    app_data.join(if instance == 1 {
        "ProfileLite".to_owned()
    } else {
        format!("ProfileLite{instance}")
    })
}

fn account_profile_path(app_data: &std::path::Path, instance: u8) -> PathBuf {
    if ipv6::assignment_path(app_data,instance).exists(){ipv6::browser_path(app_data,instance)}
    else{shared_playback_profile_path(app_data).join(account_profile_directory(instance))}
}

fn copy_directory(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&source_path, &destination_path)?;
        } else {
            fs::copy(source_path, destination_path)?;
        }
    }
    Ok(())
}

fn prepare_shared_account_profile(app_data: &std::path::Path, instance:u8)->std::io::Result<()> {
    let shared=shared_playback_profile_path(app_data);
    let _lock=acquire_process_file_lock(app_data.join("PlaybackBrowser.migration.lock"),Duration::from_secs(30))?;
    // Marker absence is NOT permission to erase an existing browser profile.
    if !shared.is_dir() {
        let legacy=legacy_account_profile_path(app_data,1);
        if legacy.is_dir(){copy_directory(&legacy,&shared)?;}else{fs::create_dir_all(&shared)?;}
    }
    fs::create_dir_all(account_profile_path(app_data,instance))?;
    auth::atomic_write(&app_data.join("PlaybackBrowser.ready"),b"ok")
}

fn prepare_shared_control_profile(app_data: &std::path::Path) -> std::io::Result<()> {
    let target = shared_playback_profile_path(app_data).join("ControlPanel");
    if target.is_dir() {
        return Ok(());
    }
    let legacy = app_data.join("ControlPanel").join("Default");
    if legacy.is_dir() {
        copy_directory(&legacy, &target)
    } else {
        fs::create_dir_all(target)
    }
}

fn cleanup_stale_shared_runtime(app_data: &std::path::Path) {
    // Only close our stale windows gracefully; never force-kill shared Edge.
    for n in 1..=9 {
        if !account_server_running(n) {
            if let Some(window)=find_account_window(n){close_window(window);}
            let _=fs::remove_file(account_window_path(app_data,n));
        }
    }
}
fn stop_stale_account_browser(instance:u8) {
    if !account_server_running(instance) {
        if let Some(window)=find_account_window(instance) {close_window(window);thread::sleep(Duration::from_millis(300));}
    }
}

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

fn account_login_marker_path(app_data: &std::path::Path, instance: u8) -> PathBuf {
    app_data.join(format!("SharedLogin{instance}.ok"))
}

fn account_token_path(app_data: &std::path::Path, instance: u8) -> PathBuf {
    app_data.join(format!("SpotifyToken{instance}.json"))
}

fn account_refresh_lock_path(app_data: &std::path::Path, instance: u8) -> PathBuf {
    app_data.join(format!("SpotifyToken{instance}.refresh.lock"))
}

fn account_reauth_path(app_data: &std::path::Path, instance: u8) -> PathBuf {
    app_data.join(format!("SpotifyReauth{instance}.required"))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn account_window_path(app_data: &std::path::Path, instance: u8) -> PathBuf {
    app_data.join(format!("AccountWindow{instance}.txt"))
}

fn account_exists(app_data: &std::path::Path, instance: u8) -> bool {
    if app_data.join(format!("AccountDeleted{instance}.txt")).is_file(){return false;}
    account_profile_path(app_data, instance).is_dir()
        || legacy_account_profile_path(app_data, instance).is_dir()
        || ipv6::assignment_path(app_data, instance).is_file()
        || account_custom_name_path(app_data, instance).is_file()
        || account_spotify_name_path(app_data, instance).is_file()
}

fn account_display_name(_root:&std::path::Path,n:u8)->String {
    format!("Hesap {n}")
}
fn json_escape(value:&str)->String {let s=serde_json::to_string(value).unwrap();s[1..s.len()-1].to_owned()}
#[cfg(test)]
fn json_string_field(body:&str,key:&str)->Option<String> {serde_json::from_str::<serde_json::Value>(body).ok()?[key].as_str().map(str::to_owned)}
fn accounts_json(root:&std::path::Path,current:u8)->String {
    serde_json::Value::Array((1..=9).filter(|n|account_exists(root,*n)).map(|n|{
        let running=current==n||account_server_running(n);
        let runtime=if running && current==0 {serde_json::from_str(&account_runtime_json(n)).unwrap_or_default()}else{serde_json::json!({"playing":false,"updated":0})};
        serde_json::json!({"instance":n,"name":account_display_name(root,n),"ipv6":ipv6::read_assignment(root,n).ok().flatten().map(|v|v.to_string()),"running":running,"runtime":runtime,"auth":auth::state(root,n)})
    }).collect()).to_string()
}

fn delete_account_files(app_data: &std::path::Path, instance: u8) {
    let result=delete_account_files_locked(app_data,instance);
    if let Err(error)=result {
        let _=diagnostics::append(app_data,"playback-diagnostics.log",&format!("account-delete-pending | account={instance} | code={:?}\n",error.raw_os_error()));
    }
}
fn delete_account_files_locked(app_data: &std::path::Path, instance: u8)->std::io::Result<()> {
    if !(1..=9).contains(&instance){return Err(std::io::Error::other("Geçersiz hesap"));}
    let _allocate=acquire_process_file_lock(app_data.join("accounts.allocate.lock"),Duration::from_secs(5))?;
    let marker=app_data.join(format!("AccountDeleted{instance}.txt"));
    auth::atomic_write(&marker,b"deleting")?;
    auth::forget(app_data,instance,|| {
    let dirs=[account_profile_path(app_data,instance),legacy_account_profile_path(app_data,instance),account_cache_path(app_data,instance),ipv6::cache_path(app_data,instance)];
    let files=[ipv6::assignment_path(app_data,instance),app_data.join(format!("IPv6Browser{instance}.process.json")),credentials::path(app_data,instance),account_custom_name_path(app_data,instance),account_spotify_name_path(app_data,instance),account_login_marker_path(app_data,instance),
        account_token_path(app_data,instance),account_reauth_path(app_data,instance),account_window_path(app_data,instance),
        account_window_path(app_data,instance).with_extension("pid"),app_data.join(format!("AccountIdentity{instance}.json")),app_data.join(format!("OAuthState{instance}.json"))];
    for _ in 0..16 {
        for path in &dirs {let _=fs::remove_dir_all(path);}
        for path in &files {let _=fs::remove_file(path);}
        if !dirs.iter().chain(files.iter()).any(|p|p.exists()) {
            fs::remove_file(&marker)?;
            return Ok(());
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(std::io::Error::other("Profil kullanımda; hesap silindi olarak işaretlendi, dosyalar korundu"))
    })
}

fn delete_account_data(app_data: &std::path::Path, instance: u8, window: Option<isize>) {
    if let Some(window) = window {
        close_window(window);
    } else if let Some(window) = find_account_window(instance) {
        close_window(window);
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

#[cfg(target_os = "windows")]
fn find_account_window(instance: u8) -> Option<isize> {
    let app_data = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)?
        .join("SpotLiteFree");
    let window = fs::read_to_string(account_window_path(&app_data, instance))
        .ok()?
        .trim()
        .parse::<isize>()
        .ok()?;
    let pid=fs::read_to_string(account_window_path(&app_data,instance).with_extension("pid")).ok()?.trim().parse::<u32>().ok()?;
    (window_exists(window)&&desktop::process(window)==pid).then_some(window)
}

#[cfg(not(target_os = "windows"))]
fn find_account_window(_instance: u8) -> Option<isize> {
    None
}

#[cfg(target_os = "windows")]
fn snapshot_edge_windows() -> HashSet<isize> {
    use std::ffi::c_void;

    unsafe extern "system" fn visit(window: *mut c_void, data: isize) -> i32 {
        unsafe extern "system" {
            fn GetClassNameW(window: *mut c_void, name: *mut u16, length: i32) -> i32;
            fn IsWindowVisible(window: *mut c_void) -> i32;
        }
        if unsafe { IsWindowVisible(window) } == 0 {
            return 1;
        }
        let mut class = [0_u16; 64];
        let read = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
        if read > 0 && String::from_utf16_lossy(&class[..read as usize]) == "Chrome_WidgetWin_1" {
            let windows = unsafe { &mut *(data as *mut HashSet<isize>) };
            windows.insert(window as isize);
        }
        1
    }

    unsafe extern "system" {
        fn EnumWindows(
            callback: unsafe extern "system" fn(*mut c_void, isize) -> i32,
            data: isize,
        ) -> i32;
    }
    let mut windows = HashSet::new();
    unsafe {
        EnumWindows(visit, (&mut windows as *mut HashSet<isize>) as isize);
    }
    windows
}

#[cfg(not(target_os = "windows"))]
fn snapshot_edge_windows() -> HashSet<isize> {
    HashSet::new()
}

fn wait_for_new_edge_window(existing: &HashSet<isize>, timeout: Duration, browser_pid:u32) -> Option<isize> {
    let started = std::time::Instant::now();
    while started.elapsed() < timeout {
        if let Some(window) = snapshot_edge_windows().difference(existing).copied().find(|w|desktop::process(*w)==browser_pid) {
            return Some(window);
        }
        thread::sleep(Duration::from_millis(40));
    }
    None
}

#[cfg(target_os = "windows")]
fn window_exists(window: isize) -> bool {
    unsafe extern "system" {
        fn IsWindow(window: *mut std::ffi::c_void) -> i32;
    }
    unsafe { IsWindow(window as *mut _) != 0 }
}

#[cfg(not(target_os = "windows"))]
fn window_exists(_window: isize) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn focus_window(window: isize) {
    unsafe extern "system" {
        fn ShowWindow(window: *mut std::ffi::c_void, command: i32) -> i32;
        fn SetForegroundWindow(window: *mut std::ffi::c_void) -> i32;
    }
    unsafe {
        ShowWindow(window as *mut _, 5);
        SetForegroundWindow(window as *mut _);
    }
}

#[cfg(not(target_os = "windows"))]
fn focus_window(_window: isize) {}

#[cfg(target_os = "windows")]
fn hide_window(window: isize) {
    unsafe extern "system" {
        fn SetWindowPos(
            window: *mut std::ffi::c_void,
            insert_after: *mut std::ffi::c_void,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> i32;
    }
    // ShowWindow(SW_HIDE) veya ekran dışı pencere Chromium renderer
    // zamanlayıcılarını kısabilir. Pencereyi 1x1 piksele küçültüp görünür
    // durumda tutarak kullanıcıya göstermeden playback_update akışını canlı
    // tutuyoruz.
    unsafe {
        SetWindowPos(
            window as *mut _,
            std::ptr::null_mut(),
            0,
            0,
            1,
            1,
            0x0004 | 0x0010,
        );
    }
}

#[cfg(target_os = "windows")]
fn park_window(window: isize) {
    unsafe extern "system" {
        fn SetWindowPos(
            window: *mut std::ffi::c_void,
            insert_after: *mut std::ffi::c_void,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> i32;
    }
    unsafe {
        SetWindowPos(
            window as *mut _,
            std::ptr::null_mut(),
            -32000,
            -32000,
            400,
            352,
            0x0004 | 0x0010,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn park_window(_window: isize) {}

#[cfg(not(target_os = "windows"))]
fn hide_window(_window: isize) {}

#[cfg(target_os = "windows")]
fn close_window(window: isize) {
    unsafe extern "system" {
        fn PostMessageW(
            window: *mut std::ffi::c_void,
            message: u32,
            wparam: usize,
            lparam: isize,
        ) -> i32;
    }
    unsafe {
        PostMessageW(window as *mut _, 0x0010, 0, 0);
    }
}

#[cfg(not(target_os = "windows"))]
fn close_window(_window: isize) {}

#[cfg(target_os = "windows")]
fn process_exists(process_id: u32) -> bool {
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> *mut std::ffi::c_void;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    let handle = unsafe { OpenProcess(0x1000, 0, process_id) };
    if handle.is_null() {
        false
    } else {
        unsafe { CloseHandle(handle) };
        true
    }
}

#[cfg(not(target_os = "windows"))]
fn process_exists(_process_id: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::{edge_argument, json_escape, json_string_field, percent_decode};
    use std::ffi::OsString;
    #[test]
    fn previous_player_process_cannot_publish_to_reused_account_port() {
        assert!(!super::player_request_matches(b"POST /player-status HTTP/1.1\r\nX-SpotLite-Player: old\r\n\r\n","new"));
        assert!(!super::player_request_matches(b"POST /player-status HTTP/1.1\r\n\r\n","new"));
        assert!(super::player_request_matches(b"POST /player-status HTTP/1.1\r\nx-spotlite-player: new\r\n\r\n","new"));
    }

    #[test]
    fn loop_change_preserves_pending_start_and_login() {
        let mut c=super::PlaybackCommand{version:1,url:"track".into(),..Default::default()};
        c.set_loop("track".into());assert!(!c.loop_only);assert_eq!(c.url,"track");
        c.acknowledge(1);assert_eq!(c.acknowledged,0);
        c.acknowledge(2);c.set_loop("off".into());assert!(c.loop_only);
        c.login=true;c.loop_only=false;c.set_loop("playlist".into());assert!(c.login);assert!(!c.loop_only);
    }
    #[test]
    fn fragmented_headers_are_read_completely_and_bounded() {
        struct Fragment(std::io::Cursor<Vec<u8>>);
        impl std::io::Read for Fragment {fn read(&mut self,b:&mut [u8])->std::io::Result<usize>{std::io::Read::read(&mut self.0,&mut b[..1])}}
        let input=b"GET /status HTTP/1.1\r\nHost: 127.0.0.1:43117\r\n\r\n";
        assert_eq!(super::read_headers(&mut Fragment(std::io::Cursor::new(input.to_vec()))).unwrap(),input);
        assert!(super::read_headers(&mut std::io::Cursor::new(vec![b'x';9000])).is_err());
        assert!(super::read_headers(&mut std::io::Cursor::new(b"GET /".to_vec())).is_err());
    }
    #[test]
    fn deletion_cleans_identity_and_oauth_state_before_slot_reuse() {
        let root=std::env::temp_dir().join(format!("spotlite-delete-test-{}-{}",std::process::id(),super::now_ms()));
        std::fs::create_dir(&root).unwrap();
        for name in ["AccountIdentity8.json","OAuthState8.json","SpotifyName8.txt"] {std::fs::write(root.join(name),b"test").unwrap();}
        super::delete_account_files_locked(&root,8).unwrap();
        assert!(!root.join("AccountIdentity8.json").exists());assert!(!root.join("OAuthState8.json").exists());
        assert!(!root.join("AccountDeleted8.txt").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn health_never_turns_stale_playback_into_playing() {
        let c=super::PlaybackCommand{playing:true,updated_ms:1000,received_ms:1000,state:"playing".into(),health:serde_json::json!({"ready":true,"updated":50000}),..Default::default()};
        assert_eq!(super::runtime_snapshot(&c,50000)["playing"],false);
    }
    #[test]
    fn old_command_telemetry_cannot_overwrite_current_playback() {
        let mut c=super::PlaybackCommand{version:3,..Default::default()};
        assert!(!super::apply_sample(&mut c,"/player-status?version=2&updated=1000&state=playing&playing=1",1000));
        assert!(!c.playing);
    }
    #[test]
    fn pause_with_zero_duration_stops_playing_without_erasing_timeline() {
        let mut c=super::PlaybackCommand{version:1,playing:true,track:"track".into(),position_ms:25000,duration_ms:151265,..Default::default()};
        assert!(super::apply_sample(&mut c,"/player-status?version=1&updated=1000&state=paused&track=track&position=0&duration=0&playing=0",1000));
        assert!(!c.playing);assert_eq!(c.position_ms,25000);assert_eq!(c.updated_ms,1000);
    }
    #[test]
    fn control_routes_reject_cross_site_and_get_mutations() {
        assert!(!super::local_request_allowed(b"GET /park-self HTTP/1.1\r\nHost: 127.0.0.1:43117\r\n\r\n",43117,true));
        assert!(!super::local_request_allowed(b"POST /park-self HTTP/1.1\r\nHost: 127.0.0.1:43117\r\nX-SpotLite-Request: 1\r\nOrigin: https://example.org\r\n\r\n",43117,true));
        assert!(super::local_request_allowed(b"POST /park-self HTTP/1.1\r\nHost: 127.0.0.1:43117\r\nX-SpotLite-Request: 1\r\n\r\n",43117,true));
    }

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

    #[test]
    fn reads_spotify_profile_fields_with_json_spacing() {
        let profile =
            r#"{"id": "abc123", "display_name": "Test Kullanıcı", "email": "test@example.com"}"#;
        assert_eq!(json_string_field(profile, "id").as_deref(), Some("abc123"));
        assert_eq!(
            json_string_field(profile, "display_name").as_deref(),
            Some("Test Kullanıcı")
        );
        assert_eq!(
            json_string_field(profile, "email").as_deref(),
            Some("test@example.com")
        );
    }
}
