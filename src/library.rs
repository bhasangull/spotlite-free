//! Spotify Web API library access. The caller owns authentication and tokens.

use serde_json::Value;
use std::{error::Error, fmt, io::Read, time::Duration};

#[derive(Debug, PartialEq, Eq)]
pub struct Playlist {
    pub name: String,
    pub uri: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PlaylistPage {
    pub playlists: Vec<Playlist>,
    pub next_offset: Option<u32>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LibraryError {
    SignInRequired,
    PermissionDenied,
    RateLimited,
    ServiceUnavailable,
    InvalidResponse,
    Network,
}

impl fmt::Display for LibraryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::SignInRequired => "Spotify girişi gerekli",
            Self::PermissionDenied => "Spotify kitaplık erişimine izin vermedi",
            Self::RateLimited => "Spotify istek sınırına ulaşıldı, sonra tekrar deneyin",
            Self::ServiceUnavailable => "Spotify hizmetine şu anda ulaşılamıyor",
            Self::InvalidResponse => "Spotify yanıtı okunamadı",
            Self::Network => "Spotify bağlantısı kurulamadı",
        })
    }
}
impl Error for LibraryError {}

/// Fetch one page. Never follows a response URL or logs credentials.
pub fn playlists(token: &str, offset: u32) -> Result<PlaylistPage, LibraryError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| LibraryError::Network)?;
    fetch(
        &client,
        "https://api.spotify.com/v1/me/playlists",
        token,
        offset,
    )
}

fn fetch(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    token: &str,
    offset: u32,
) -> Result<PlaylistPage, LibraryError> {
    if token.trim().is_empty() {
        return Err(LibraryError::SignInRequired);
    }
    let response = client
        .get(endpoint)
        .bearer_auth(token)
        .query(&[("limit", 50_u32), ("offset", offset)])
        .send()
        .map_err(|_| LibraryError::Network)?;
    match response.status().as_u16() {
        200 => {}
        401 => return Err(LibraryError::SignInRequired),
        403 => return Err(LibraryError::PermissionDenied),
        429 => return Err(LibraryError::RateLimited),
        500..=599 => return Err(LibraryError::ServiceUnavailable),
        _ => return Err(LibraryError::InvalidResponse),
    }
    const MAX_RESPONSE: u64 = 2 * 1024 * 1024;
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| LibraryError::Network)?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err(LibraryError::InvalidResponse);
    }
    let body: Value = serde_json::from_slice(&bytes).map_err(|_| LibraryError::InvalidResponse)?;
    parse_page(&body, offset)
}

fn parse_page(body: &Value, offset: u32) -> Result<PlaylistPage, LibraryError> {
    let items = body
        .get("items")
        .and_then(Value::as_array)
        .ok_or(LibraryError::InvalidResponse)?;
    let mut playlists = Vec::with_capacity(items.len());
    for item in items {
        let Some(uri) = item
            .get("uri")
            .and_then(Value::as_str)
            .filter(|u| u.starts_with("spotify:playlist:"))
        else {
            continue;
        };
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        playlists.push(Playlist {
            name: name.to_owned(),
            uri: uri.to_owned(),
        });
    }
    // Spotify's next URL is never fetched; only the numeric offset advances.
    let next_offset = if body.get("next").is_some_and(Value::is_string) && !items.is_empty() {
        offset.checked_add(items.len() as u32)
    } else {
        None
    };
    Ok(PlaylistPage {
        playlists,
        next_offset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    fn mock(status: &str, body: &str) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let worker = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buf = [0; 8192];
            let length = socket.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..length]).to_ascii_lowercase();
            assert!(request.contains("authorization: bearer test-token"));
            assert!(request.contains("offset=50"));
            socket.write_all(response.as_bytes()).unwrap();
        });
        (format!("http://{address}/playlists"), worker)
    }

    #[test]
    fn parses_page_and_skips_null_entries() {
        let (url, worker) = mock(
            "200 OK",
            r#"{"items":[null,{"name":"Mix","uri":"spotify:playlist:abc"}],"next":"https://api.spotify.com/v1/next"}"#,
        );
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        let page = fetch(&client, &url, "test-token", 50).unwrap();
        assert_eq!(page.playlists.len(), 1);
        assert_eq!(page.playlists[0].name, "Mix");
        assert_eq!(page.next_offset, Some(52));
        worker.join().unwrap();
    }

    #[test]
    fn rate_limit_is_not_reported_as_bad_login() {
        let (url, worker) = mock("429 Too Many Requests", "{}");
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        assert_eq!(
            fetch(&client, &url, "test-token", 50),
            Err(LibraryError::RateLimited)
        );
        worker.join().unwrap();
    }

    #[test]
    fn malformed_response_is_not_silently_an_empty_library() {
        assert_eq!(
            parse_page(&serde_json::json!({"items": null}), 0),
            Err(LibraryError::InvalidResponse)
        );
    }
}
