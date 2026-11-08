//! Local integration with Spotify Soloist. Account setup is a later stage.

use serde_json::{json, Value};
use std::{
    fs, io,
    net::{Ipv4Addr, SocketAddrV4, TcpStream},
    path::Path,
    time::{Duration, Instant},
};
use tungstenite::{client, Message};

#[derive(Debug, PartialEq, Eq)]
pub struct PlaybackStatus {
    pub logged_in: bool,
    pub status: Option<String>,
    pub title: Option<String>,
}

/// Play a local MP3 on the default audio device. The caller owns the decision
/// to interrupt any other player; the prototype does not stop Spotify.
pub fn play_local_mp3(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mp3"))
    {
        return Err("Bir MP3 dosyası seçin".into());
    }
    let file = fs::File::open(path)?;
    let audio = rodio::Decoder::try_from(file)?;
    let stream = rodio::OutputStreamBuilder::open_default_stream()?;
    let sink = rodio::Sink::connect_new(stream.mixer());
    sink.append(audio);
    sink.sleep_until_end();
    Ok(())
}

#[derive(Debug)]
pub enum Command<'a> {
    Play(&'a str),
    Pause,
    RepeatTrack(bool),
    RepeatContext(bool),
}

fn payload(command: &Command<'_>) -> Result<Value, &'static str> {
    Ok(match command {
        Command::Play(uri) => {
            let mut parts = uri.split(':');
            let valid = parts.next() == Some("spotify")
                && matches!(
                    parts.next(),
                    Some("track" | "album" | "playlist" | "episode")
                )
                && parts.next().is_some_and(|id| {
                    id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric())
                })
                && parts.next().is_none();
            if !valid {
                return Err("Geçersiz Spotify URI");
            }
            json!({"type":"command", "command":"play", "uri":uri})
        }
        Command::Pause => json!({"type":"command", "command":"pause"}),
        Command::RepeatTrack(enabled) => {
            json!({"type":"command", "command":"set_repeat_track", "enabled":enabled})
        }
        Command::RepeatContext(enabled) => {
            json!({"type":"command", "command":"set_repeat_context", "enabled":enabled})
        }
    })
}

fn connect_local(
    endpoint: SocketAddrV4,
) -> Result<tungstenite::WebSocket<TcpStream>, Box<dyn std::error::Error>> {
    if !endpoint.ip().is_loopback() {
        return Err("Yalnızca yerel bağlantı destekleniyor".into());
    }
    let timeout = Duration::from_secs(5);
    let stream = TcpStream::connect_timeout(&endpoint.into(), timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let (socket, _) = client(format!("ws://{endpoint}/"), stream)?;
    Ok(socket)
}

fn read_before(
    socket: &mut tungstenite::WebSocket<TcpStream>,
    deadline: Instant,
) -> Result<Message, Box<dyn std::error::Error>> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or("Soloist yanıtı zaman aşımına uğradı")?;
    socket.get_mut().set_read_timeout(Some(remaining))?;
    Ok(socket.read()?)
}

/// Acknowledgement means accepted for dispatch, not confirmed playback.
pub fn send_command(
    endpoint: SocketAddrV4,
    command: Command<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let data = payload(&command)?;
    let name = data["command"].as_str().ok_or("Komut eksik")?.to_owned();
    let mut socket = connect_local(endpoint)?;
    socket.send(Message::Text(data.to_string().into()))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    for _ in 0..32 {
        let Message::Text(text) = read_before(&mut socket, deadline)? else {
            continue;
        };
        let event: Value = serde_json::from_str(text.as_str())?;
        if event.get("type").and_then(Value::as_str) == Some("error") {
            return Err("Soloist komutu reddetti".into());
        }
        if event.get("type").and_then(Value::as_str) == Some("command_result")
            && event.get("command").and_then(Value::as_str) == Some(name.as_str())
        {
            return Ok(());
        }
    }
    Err("Komut onayı alınamadı".into())
}

/// Soloist writes this endpoint in its data directory when its WebSocket API
/// is enabled. Only loopback is accepted; never connect to a remote address.
pub fn read_endpoint(directory: &Path) -> io::Result<SocketAddrV4> {
    let address = fs::read_to_string(directory.join("ws.addr"))?;
    if address.trim() != "127.0.0.1" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Soloist adresi yerel değil",
        ));
    }
    let port: u16 = fs::read_to_string(directory.join("ws.port"))?
        .trim()
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Geçersiz port"))?;
    if port == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Geçersiz port"));
    }
    Ok(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
}

/// Observe a single snapshot; this sends no playback commands.
pub fn read_status(endpoint: SocketAddrV4) -> Result<PlaybackStatus, Box<dyn std::error::Error>> {
    let mut socket = connect_local(endpoint)?;
    let mut logged_in = None;
    let mut playback = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    // A bounded number of messages prevents an event stream from keeping a
    // one-shot status request alive indefinitely.
    for _ in 0..32 {
        let message = read_before(&mut socket, deadline)?;
        let Message::Text(text) = message else {
            continue;
        };
        let event: Value = serde_json::from_str(text.as_str())?;
        match event.get("type").and_then(Value::as_str) {
            Some("auth_state") => {
                logged_in = Some(
                    event
                        .get("logged_in")
                        .and_then(Value::as_bool)
                        .ok_or("Eksik giriş durumu")?,
                );
                if logged_in == Some(false) {
                    break;
                }
            }
            Some("playback_state") => playback = Some(event),
            _ => {}
        }
        if logged_in == Some(true) && playback.is_some() {
            break;
        }
    }
    let logged_in = logged_in.ok_or("Soloist giriş durumu alınamadı")?;
    if logged_in && playback.is_none() {
        return Err("Soloist oynatma durumu alınamadı".into());
    }
    let status = playback
        .as_ref()
        .and_then(|v| v.get("status"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let title = playback
        .as_ref()
        .and_then(|v| v.pointer("/item/decorations/identity/name"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(PlaybackStatus {
        logged_in,
        status,
        title,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, thread};

    #[test]
    fn receives_real_websocket_events_without_playback_commands() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = match listener.local_addr().unwrap() {
            std::net::SocketAddr::V4(a) => a,
            _ => unreachable!(),
        };
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            ws.send(Message::Text(
                r#"{"type":"auth_state","logged_in":true}"#.into(),
            ))
            .unwrap();
            ws.send(Message::Text(r#"{"type":"playback_state","status":"playing","item":{"decorations":{"identity":{"name":"Test"}}}}"#.into())).unwrap();
        });
        let state = read_status(address).unwrap();
        assert_eq!(
            state,
            PlaybackStatus {
                logged_in: true,
                status: Some("playing".into()),
                title: Some("Test".into())
            }
        );
        server.join().unwrap();
    }

    #[test]
    fn refuses_non_loopback_before_opening_a_socket() {
        let result = read_status(SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 1), 1234));
        assert!(result.is_err());
    }

    #[test]
    fn signed_out_does_not_wait_for_playback() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = match listener.local_addr().unwrap() {
            std::net::SocketAddr::V4(a) => a,
            _ => unreachable!(),
        };
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            ws.send(Message::Text(
                r#"{"type":"auth_state","logged_in":false}"#.into(),
            ))
            .unwrap();
        });
        assert_eq!(
            read_status(address).unwrap(),
            PlaybackStatus {
                logged_in: false,
                status: None,
                title: None
            }
        );
        server.join().unwrap();
    }

    #[test]
    fn play_command_waits_for_matching_acknowledgement() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = match listener.local_addr().unwrap() {
            std::net::SocketAddr::V4(a) => a,
            _ => unreachable!(),
        };
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            ws.send(Message::Text(
                r#"{"type":"auth_state","logged_in":true}"#.into(),
            ))
            .unwrap();
            let message = ws.read().unwrap().into_text().unwrap();
            let value: Value = serde_json::from_str(&message).unwrap();
            assert_eq!(value["command"], "play");
            assert_eq!(value["uri"], "spotify:track:7GqvrVU8R6tBv9DAN1USaG");
            ws.send(Message::Text(
                r#"{"type":"command_result","command":"play"}"#.into(),
            ))
            .unwrap();
        });
        send_command(
            address,
            Command::Play("spotify:track:7GqvrVU8R6tBv9DAN1USaG"),
        )
        .unwrap();
        server.join().unwrap();
    }

    #[test]
    fn invalid_uri_is_rejected_without_a_connection() {
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1);
        assert!(
            send_command(address, Command::Play("spotify:track:not-valid"))
                .unwrap_err()
                .to_string()
                .contains("URI")
        );
    }

    #[test]
    fn repeat_commands_encode_the_requested_mode() {
        assert_eq!(
            payload(&Command::RepeatTrack(true)).unwrap(),
            json!({"type":"command", "command":"set_repeat_track", "enabled":true})
        );
        assert_eq!(
            payload(&Command::RepeatContext(false)).unwrap(),
            json!({"type":"command", "command":"set_repeat_context", "enabled":false})
        );
    }

    #[test]
    fn a_server_rejection_is_not_a_success() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = match listener.local_addr().unwrap() {
            std::net::SocketAddr::V4(a) => a,
            _ => unreachable!(),
        };
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let _ = ws.read().unwrap();
            ws.send(Message::Text(
                r#"{"type":"error","message":"command requires authentication"}"#.into(),
            ))
            .unwrap();
        });
        assert!(send_command(address, Command::Pause).is_err());
        server.join().unwrap();
    }

    #[test]
    fn local_player_rejects_non_mp3_and_missing_files() {
        assert!(play_local_mp3(Path::new("not-a-song.txt"))
            .unwrap_err()
            .to_string()
            .contains("MP3"));
        assert!(play_local_mp3(Path::new("missing-test-file.mp3")).is_err());
    }
}
