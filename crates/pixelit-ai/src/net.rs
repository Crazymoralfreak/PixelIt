//! The only place of the application that opens network connections.
//!
//! Model downloads, local AI servers and cloud services all go through
//! [`Net`], so one module decides what leaves the computer and how a download
//! is verified.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};

use pixelit_core::{CancelToken, Error, Result};

/// Largest accepted response that is kept in memory: 512 MiB.
const MAX_RESPONSE: u64 = 512 * 1024 * 1024;

/// How much of an error response is shown to the user.
const ERROR_TEXT: usize = 400;

/// Digits of a hexadecimal hash.
const HEX: &[u8; 16] = b"0123456789abcdef";

/// Expected hash of a downloaded file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checksum {
    /// Hexadecimal SHA-256, as Hugging Face and GitHub publish it.
    Sha256(&'static str),
    /// Base64 SHA-512, as NuGet publishes it.
    Sha512(&'static str),
}

/// One part of a `multipart/form-data` request.
#[derive(Clone, Copy, Debug)]
pub struct Part<'a> {
    pub name: &'a str,
    /// Present for files, absent for plain fields.
    pub file_name: Option<&'a str>,
    pub mime: &'a str,
    pub data: &'a [u8],
}

impl<'a> Part<'a> {
    pub fn field(name: &'a str, value: &'a str) -> Self {
        Self {
            name,
            file_name: None,
            mime: "",
            data: value.as_bytes(),
        }
    }

    pub fn file(name: &'a str, file_name: &'a str, mime: &'a str, data: &'a [u8]) -> Self {
        Self {
            name,
            file_name: Some(file_name),
            mime,
            data,
        }
    }
}

#[derive(Clone)]
pub struct Net {
    agent: ureq::Agent,
}

/// An open response whose body is read as it arrives.
pub struct Stream {
    pub status: u16,
    /// Media type without parameters, lower-cased.
    pub content_type: String,
    headers: Vec<(String, String)>,
    reader: Box<dyn Read + Send + Sync + 'static>,
}

impl Stream {
    /// A response header by its case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn into_reader(self) -> Box<dyn Read + Send + Sync + 'static> {
        self.reader
    }
}

impl Default for Net {
    fn default() -> Self {
        Self::new()
    }
}

/// The host of an address, lower-cased, without port and credentials.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url
        .trim()
        .split_once("://")
        .map_or(url.trim(), |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if let Some(bracketed) = host.strip_prefix('[') {
        bracketed.split(']').next()?
    } else {
        host.rsplit_once(':').map_or(host, |(name, port)| {
            if port.chars().all(|digit| digit.is_ascii_digit()) {
                name
            } else {
                host
            }
        })
    };
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

/// Whether the address points at this computer.
pub fn is_local(url: &str) -> bool {
    match host_of(url) {
        Some(host) => {
            host == "localhost"
                || host == "::1"
                || host
                    .parse::<std::net::Ipv4Addr>()
                    .is_ok_and(|address| address.is_loopback())
        }
        None => false,
    }
}

fn is_http(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn failure(error: ureq::Error) -> Error {
    match error {
        ureq::Error::Status(status, response) => {
            let mut text = String::new();
            let _ = response.into_reader().take(4096).read_to_string(&mut text);
            let text: String = text.chars().take(ERROR_TEXT).collect();
            Error::new("httpStatus")
                .with("status", status)
                .detail(text.trim())
        }
        ureq::Error::Transport(transport) => Error::new("network").detail(transport),
    }
}

fn read_limited(response: ureq::Response) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| Error::new("network").detail(error))?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err(Error::new("responseTooLarge"));
    }
    Ok(bytes)
}

fn parse_json(bytes: &[u8]) -> Result<Value> {
    serde_json::from_slice(bytes).map_err(|error| Error::new("badResponse").detail(error))
}

fn part_path(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map_or_default(std::ffi::OsStr::to_os_string);
    name.push(".part");
    target.with_file_name(name)
}

/// Checks a file against the expected hash.
pub fn verify(path: &Path, checksum: &Checksum, cancel: &CancelToken) -> Result<bool> {
    let mut file = File::open(path)?;
    let mut buffer = vec![0u8; 1 << 20];
    let mut sha256 = Sha256::new();
    let mut sha512 = Sha512::new();
    loop {
        cancel.check()?;
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        match checksum {
            Checksum::Sha256(_) => sha256.update(&buffer[..read]),
            Checksum::Sha512(_) => sha512.update(&buffer[..read]),
        }
    }
    Ok(match checksum {
        Checksum::Sha256(expected) => {
            let digest = sha256.finalize();
            let mut actual = String::with_capacity(64);
            for byte in digest.as_slice() {
                actual.push(char::from(HEX[usize::from(byte >> 4)]));
                actual.push(char::from(HEX[usize::from(byte & 15)]));
            }
            actual.eq_ignore_ascii_case(expected)
        }
        Checksum::Sha512(expected) => {
            let digest = sha512.finalize();
            STANDARD.encode(digest.as_slice()) == *expected
        }
    })
}

/// Builds a `multipart/form-data` body; returns the content type and the body.
pub fn multipart(parts: &[Part<'_>]) -> (String, Vec<u8>) {
    let boundary = format!(
        "----pixelit{:016x}{:016x}",
        fastrand::u64(..),
        fastrand::u64(..)
    );
    let mut body = Vec::new();
    for part in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match part.file_name {
            Some(file_name) => {
                let file_name = file_name.replace(['"', '\r', '\n'], "_");
                body.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{}\"; filename=\"{file_name}\"\r\nContent-Type: {}\r\n\r\n",
                        part.name, part.mime
                    )
                    .as_bytes(),
                );
            }
            None => body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{}\"\r\n\r\n",
                    part.name
                )
                .as_bytes(),
            ),
        }
        body.extend_from_slice(part.data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

impl Net {
    pub fn new() -> Self {
        let mut builder = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(90))
            .timeout_write(Duration::from_secs(90))
            .redirects(8)
            .user_agent(concat!("PixelIt/", env!("CARGO_PKG_VERSION")));
        match native_tls::TlsConnector::new() {
            Ok(connector) => builder = builder.tls_connector(Arc::new(connector)),
            Err(error) => tracing::warn!("TLS is unavailable: {error}"),
        }
        Self {
            agent: builder.build(),
        }
    }

    fn request(
        &self,
        method: &str,
        url: &str,
        key: Option<&str>,
        timeout: Option<Duration>,
    ) -> Result<ureq::Request> {
        if !is_http(url) {
            return Err(Error::new("badUrl").with("url", url));
        }
        let mut request = self.agent.request(method, url.trim());
        if let Some(key) = key.filter(|key| !key.is_empty()) {
            request = request.set("Authorization", &format!("Bearer {key}"));
        }
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        Ok(request)
    }

    /// Downloads a response into memory.
    pub fn get(&self, url: &str, key: Option<&str>, timeout: Option<Duration>) -> Result<Vec<u8>> {
        let response = self
            .request("GET", url, key, timeout)?
            .call()
            .map_err(failure)?;
        read_limited(response)
    }

    pub fn get_json(
        &self,
        url: &str,
        key: Option<&str>,
        timeout: Option<Duration>,
    ) -> Result<Value> {
        parse_json(&self.get(url, key, timeout)?)
    }

    /// Sends JSON and returns the raw response.
    pub fn post(
        &self,
        url: &str,
        key: Option<&str>,
        body: &Value,
        timeout: Option<Duration>,
    ) -> Result<Vec<u8>> {
        let response = self
            .request("POST", url, key, timeout)?
            .send_json(body)
            .map_err(failure)?;
        read_limited(response)
    }

    pub fn post_json(
        &self,
        url: &str,
        key: Option<&str>,
        body: &Value,
        timeout: Option<Duration>,
    ) -> Result<Value> {
        let bytes = self.post(url, key, body, timeout)?;
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        parse_json(&bytes)
    }

    /// Sends JSON with extra headers and hands the response over unread, for
    /// protocols that answer with a stream of events.
    pub fn open(
        &self,
        url: &str,
        key: Option<&str>,
        headers: &[(&str, &str)],
        body: &Value,
        timeout: Option<Duration>,
    ) -> Result<Stream> {
        let mut request = self.request("POST", url, key, timeout)?;
        for (name, value) in headers {
            request = request.set(name, value);
        }
        let response = request.send_json(body).map_err(failure)?;
        let headers = response
            .headers_names()
            .into_iter()
            .filter_map(|name| {
                let value = response.header(&name)?.to_owned();
                Some((name.to_ascii_lowercase(), value))
            })
            .collect();
        Ok(Stream {
            status: response.status(),
            content_type: response.content_type().to_ascii_lowercase(),
            headers,
            reader: response.into_reader(),
        })
    }

    /// Sends a form with files and returns the JSON response.
    pub fn post_form(
        &self,
        url: &str,
        key: Option<&str>,
        parts: &[Part<'_>],
        timeout: Option<Duration>,
    ) -> Result<Value> {
        let (content_type, body) = multipart(parts);
        let response = self
            .request("POST", url, key, timeout)?
            .set("Content-Type", &content_type)
            .send_bytes(&body)
            .map_err(failure)?;
        parse_json(&read_limited(response)?)
    }

    /// Downloads a file, continuing an interrupted download, and accepts it
    /// only when its size and hash match the catalogue.
    pub fn download(
        &self,
        url: &str,
        target: &Path,
        bytes: u64,
        checksum: &Checksum,
        progress: &mut dyn FnMut(u64),
        cancel: &CancelToken,
    ) -> Result<()> {
        if let Some(folder) = target.parent() {
            fs::create_dir_all(folder)?;
        }
        let part = part_path(target);
        let mut have = fs::metadata(&part).map_or(0, |meta| meta.len());
        if have > bytes {
            fs::remove_file(&part)?;
            have = 0;
        }
        if have < bytes {
            let mut request = self.request("GET", url, None, None)?;
            if have > 0 {
                request = request.set("Range", &format!("bytes={have}-"));
            }
            let response = request.call().map_err(failure)?;
            let resumed = have > 0 && response.status() == 206;
            if !resumed {
                have = 0;
            }
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .append(resumed)
                .truncate(!resumed)
                .open(&part)?;
            let mut reader = response.into_reader();
            let mut buffer = vec![0u8; 1 << 16];
            progress(have);
            loop {
                cancel.check()?;
                let read = reader
                    .read(&mut buffer)
                    .map_err(|error| Error::new("network").detail(error))?;
                if read == 0 {
                    break;
                }
                have += read as u64;
                if have > bytes {
                    drop(file);
                    let _ = fs::remove_file(&part);
                    return Err(Error::new("checksumMismatch").with("url", url));
                }
                file.write_all(&buffer[..read])?;
                progress(have);
            }
            file.flush()?;
        }
        if have < bytes {
            // The connection dropped; the next attempt continues from here.
            return Err(Error::new("network").detail("connection closed early"));
        }
        if !verify(&part, checksum, cancel)? {
            let _ = fs::remove_file(&part);
            return Err(Error::new("checksumMismatch").with("url", url));
        }
        if target.exists() {
            fs::remove_file(target)?;
        }
        fs::rename(&part, target)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_extracted() {
        assert_eq!(
            host_of("http://127.0.0.1:11434/v1").as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(
            host_of("https://API.openai.com/v1/").as_deref(),
            Some("api.openai.com")
        );
        assert_eq!(
            host_of("http://user:pass@Example.org:8080/x").as_deref(),
            Some("example.org")
        );
        assert_eq!(host_of("http://[::1]:8188/").as_deref(), Some("::1"));
        assert_eq!(host_of("localhost:1234").as_deref(), Some("localhost"));
        assert_eq!(host_of(""), None);
    }

    #[test]
    fn only_loopback_is_local() {
        assert!(is_local("http://localhost:11434"));
        assert!(is_local("http://127.0.0.1:8188"));
        assert!(is_local("http://127.8.9.1"));
        assert!(is_local("http://[::1]:7860"));
        assert!(!is_local("http://192.168.1.10:11434"));
        assert!(!is_local("https://api.openai.com/v1"));
        assert!(!is_local("http://localhost.example.com"));
    }

    #[test]
    fn hashes_are_checked() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("file.bin");
        fs::write(&path, b"abc").unwrap();
        let cancel = CancelToken::new();
        let sha256 =
            Checksum::Sha256("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert!(verify(&path, &sha256, &cancel).unwrap());
        let sha512 = Checksum::Sha512(
            "3a81oZNherrMQXNJriBBMRLm+k6JqX6iCp7u5ktV05ohkpkqJ0/BqDa6PCOj/uu9RU1EI2Q86A4qmslPpUyknw==",
        );
        assert!(verify(&path, &sha512, &cancel).unwrap());
        let wrong =
            Checksum::Sha256("0000000000000000000000000000000000000000000000000000000000000000");
        assert!(!verify(&path, &wrong, &cancel).unwrap());
    }

    #[test]
    fn forms_are_built() {
        let (content_type, body) = multipart(&[
            Part::field("prompt", "a cat"),
            Part::file("image", "in\"put.png", "image/png", &[1, 2, 3]),
        ]);
        let boundary = content_type.split("boundary=").nth(1).unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("name=\"prompt\"\r\n\r\na cat\r\n"));
        assert!(text.contains("filename=\"in_put.png\""));
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));
    }

    #[test]
    fn addresses_must_be_http() {
        let net = Net::new();
        let error = net.get("file:///etc/passwd", None, None).unwrap_err();
        assert_eq!(error.code, "badUrl");
    }

    #[test]
    fn part_files_sit_beside_the_target() {
        assert_eq!(
            part_path(Path::new("models/u2net/u2net.onnx")),
            Path::new("models/u2net/u2net.onnx.part")
        );
    }
}
