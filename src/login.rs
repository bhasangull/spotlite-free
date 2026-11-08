//! A short-lived, interactive Spotify authorization code + PKCE flow.
//! Tokens stay in memory; persistent login is a separate future feature.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream},
    time::{Duration, Instant},
};

const REDIRECT: &str = "http://127.0.0.1:8989/login";
const AUTH_URL: &str = "https://accounts.spotify.com/authorize";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";

/// The application never prints or writes this value to disk.
pub struct AccessToken(String);
impl AccessToken {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn random_urlsafe(length: usize) -> Result<String, Box<dyn Error>> {
    let mut bytes = vec![0; length];
    getrandom::fill(&mut bytes).map_err(|_| "Rastgele değer oluşturulamadı")?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn authorization_url(
    client_id: &str,
    verifier: &str,
    state: &str,
) -> Result<reqwest::Url, Box<dyn Error>> {
    if client_id.len() != 32 || !client_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Spotify Client ID geçersiz".into());
    }
    let mut url = reqwest::Url::parse(AUTH_URL)?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", REDIRECT)
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", &challenge(verifier))
        .append_pair("state", state)
        .append_pair("scope", "playlist-read-private playlist-read-collaborative");
    Ok(url)
}

/// Reads only the Spotify OAuth callback request target. State mismatches are
/// rejected without accepting the code; unrelated requests cannot finish login.
#[derive(Debug, PartialEq, Eq)]
enum Callback {
    Granted(String),
    Denied,
}

fn callback_code(target: &str, expected_state: &str) -> Option<Callback> {
    if !target.starts_with("/login?") {
        return None;
    }
    let url = reqwest::Url::parse(&format!("http://127.0.0.1:8989{target}")).ok()?;
    if url.path() != "/login" {
        return None;
    }
    let pairs: Vec<_> = url.query_pairs().collect();
    let states: Vec<_> = pairs.iter().filter(|(key, _)| key == "state").collect();
    let codes: Vec<_> = pairs.iter().filter(|(key, _)| key == "code").collect();
    let errors: Vec<_> = pairs.iter().filter(|(key, _)| key == "error").collect();
    if states.len() != 1 || states[0].1 != expected_state {
        return None;
    }
    if codes.len() == 1 && errors.is_empty() && !codes[0].1.is_empty() {
        Some(Callback::Granted(codes[0].1.to_string()))
    } else if errors.len() == 1 && codes.is_empty() && !errors[0].1.is_empty() {
        Some(Callback::Denied)
    } else {
        None
    }
}

fn handle_callback(mut stream: TcpStream, state: &str) -> Option<Callback> {
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .ok()?;
    let mut first = String::new();
    let reader = BufReader::new((&stream).take(8192));
    reader.take(8192).read_line(&mut first).ok()?;
    let mut parts = first.split_whitespace();
    let code = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("GET"), Some(target), Some("HTTP/1.1" | "HTTP/1.0"), None) => {
            callback_code(target, state)
        }
        _ => None,
    };
    let (status, text) = match &code {
        Some(Callback::Granted(_)) => (
            "200 OK",
            "Spotify authorization received. You may return to SpotLite.",
        ),
        Some(Callback::Denied) => (
            "200 OK",
            "Spotify authorization was declined. You may return to SpotLite.",
        ),
        None => (
            "400 Bad Request",
            "Authorization was not accepted. Return to SpotLite and try again.",
        ),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{text}",
        text.len()
    );
    code
}

/// Opens Spotify's official approval page and returns an in-memory access token.
pub fn authorize(client_id: &str) -> Result<AccessToken, Box<dyn Error>> {
    let verifier = random_urlsafe(48)?;
    let state = random_urlsafe(24)?;
    let url = authorization_url(client_id, &verifier, &state)?;
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 8989))?;
    listener.set_nonblocking(true)?;
    webbrowser::open(url.as_str()).map_err(|_| "Spotify giriş sayfası açılamadı")?;
    let deadline = Instant::now() + Duration::from_secs(180);
    let code = loop {
        if Instant::now() >= deadline {
            return Err("Spotify girişi zaman aşımına uğradı".into());
        }
        match listener.accept() {
            Ok((stream, peer)) if peer.ip().is_loopback() => {
                match handle_callback(stream, &state) {
                    Some(Callback::Granted(code)) => break code,
                    Some(Callback::Denied) => {
                        return Err("Spotify yetkilendirmesi reddedildi".into());
                    }
                    None => {}
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(error) => return Err(error.into()),
        }
    };
    drop(listener);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = client
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client_id),
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .map_err(|_| "Spotify bağlantısı kurulamadı")?;
    if !response.status().is_success() {
        return Err("Spotify girişi tamamlanamadı".into());
    }
    let mut data = Vec::new();
    response.take(1024 * 1024 + 1).read_to_end(&mut data)?;
    if data.len() > 1024 * 1024 {
        return Err("Spotify giriş yanıtı çok büyük".into());
    }
    let value: serde_json::Value = serde_json::from_slice(&data)?;
    let access = value
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("Spotify erişim yanıtı eksik")?;
    Ok(AccessToken(access.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_pkce_challenge_matches_reference_vector() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn callback_rejects_wrong_state_duplicate_fields_and_wrong_path() {
        assert_eq!(
            callback_code("/login?code=ok&state=expected", "expected"),
            Some(Callback::Granted("ok".into()))
        );
        assert_eq!(
            callback_code("/login?code=ok&state=wrong", "expected"),
            None
        );
        assert_eq!(
            callback_code("/login?code=ok&state=expected&state=wrong", "expected"),
            None
        );
        assert_eq!(
            callback_code("/other?code=ok&state=expected", "expected"),
            None
        );
        assert_eq!(
            callback_code("/login?error=access_denied&state=expected", "expected"),
            Some(Callback::Denied)
        );
        assert_eq!(
            callback_code("/login?error=access_denied&state=wrong", "expected"),
            None
        );
    }

    #[test]
    fn authorization_is_pkce_and_rejects_invalid_client_id() {
        let url =
            authorization_url("0123456789abcdef0123456789abcdef", "verifier", "state").unwrap();
        assert!(
            url.query_pairs()
                .any(|(k, v)| k == "code_challenge_method" && v == "S256")
        );
        assert!(authorization_url("invalid", "verifier", "state").is_err());
    }
}
