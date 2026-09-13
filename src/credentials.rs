use std::{io::{self, Read}, path::{Path, PathBuf}};
use serde::{Deserialize, Serialize};
#[derive(Deserialize, Serialize)]
pub struct Credentials { pub email: String, pub password: String }
pub fn path(root: &Path, account: u8) -> PathBuf { root.join(format!("Credentials{account}.json")) }
pub fn read_body(reader: &mut impl Read, request: &[u8]) -> io::Result<Vec<u8>> {
    let invalid = || io::Error::other("Geçersiz istek gövdesi");
    let end = request.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(invalid)? + 4;
    let headers = std::str::from_utf8(&request[..end]).map_err(|_| invalid())?;
    let mut length = None;
    for line in headers.lines().skip(1) {
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("transfer-encoding") { return Err(invalid()); }
            if key.eq_ignore_ascii_case("content-length") {
                if length.is_some() { return Err(invalid()); }
                length = Some(value.trim().parse::<usize>().map_err(|_| invalid())?);
            }
        }
    }
    let length = length.filter(|n| *n <= 8192).ok_or_else(invalid)?;
    let mut body = request[end..].to_vec();
    if body.len() > length { return Err(invalid()); }
    let received = body.len(); body.resize(length, 0);
    reader.read_exact(&mut body[received..])?;
    Ok(body)
}
pub fn save(root: &Path, account: u8, body: &[u8]) -> io::Result<()> {
    let mut value: Credentials = serde_json::from_slice(body).map_err(|_| io::Error::other("Geçersiz giriş bilgisi"))?;
    value.email = value.email.trim().to_owned();
    if value.email.is_empty() || value.email.len() > 320 || value.password.is_empty() || value.password.len() > 4096 {
        return Err(io::Error::other("E-posta ve şifre gerekli"));
    }
    crate::auth::atomic_write(&path(root, account), &serde_json::to_vec(&value)?)
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn fragmented_body() {
        assert_eq!(read_body(&mut &b"cdef"[..], b"POST / HTTP/1.1\r\nContent-Length: 6\r\n\r\nab").unwrap(), b"abcdef");
    }
    #[test] fn rejects_ambiguous_body() {
        for headers in ["Content-Length: 9000", "Content-Length: 1\r\nContent-Length: 1", "Transfer-Encoding: chunked"] {
            assert!(read_body(&mut &b""[..], format!("POST / HTTP/1.1\r\n{headers}\r\n\r\n").as_bytes()).is_err());
        }
    }
}
