//! The only place of the application that opens network connections.
//!
//! Model downloads, local AI servers and cloud services all go through
//! [`Net`], so one module decides what leaves the computer and how a download
//! is verified.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};
use tungstenite::client::IntoClientRequest as _;
use tungstenite::stream::MaybeTlsStream;
use url::Url;

use pixelit_core::{CancelToken, Error, Result};

/// Largest accepted response that is kept in memory: 512 MiB.
const MAX_RESPONSE: u64 = 512 * 1024 * 1024;

/// How much of an error response is shown to the user.
const ERROR_TEXT: usize = 400;

/// Digits of a hexadecimal hash.
const HEX: &[u8; 16] = b"0123456789abcdef";

/// Redirects followed for one request.
const MAX_REDIRECTS: u32 = 8;

/// What a request that was kept out of the local network says about itself.
const INSIDE: &str = "the name points at the local network";

/// How PixelIt names itself to servers. Wikimedia asks every client for a
/// name with an address where its authors can be reached.
const USER_AGENT: &str = concat!(
    "PixelIt/",
    env!("CARGO_PKG_VERSION"),
    " (+",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);

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

/// How a page or a picture of the web is asked for.
#[derive(Clone, Copy, Debug)]
pub struct Ask<'a> {
    /// What the server is told PixelIt accepts.
    pub accept: &'a str,
    /// The page the address was found on; some servers hand a picture out
    /// only to a request that names it.
    pub referer: Option<&'a str>,
    /// Whether the address may point at this computer or its local network.
    pub local: bool,
    pub timeout: Option<Duration>,
}

#[derive(Clone)]
pub struct Net {
    agent: ureq::Agent,
    /// Follows no redirects by itself, so every address on the way is checked.
    direct: ureq::Agent,
    /// Like `direct`, and connects to nothing inside the local network.
    outside: ureq::Agent,
}

/// An open response whose body is read as it arrives.
pub struct Stream {
    pub status: u16,
    /// Media type without parameters, lower-cased.
    pub content_type: String,
    /// The address that answered, after redirects.
    pub url: String,
    /// Size of the body, when the server tells it.
    pub length: Option<u64>,
    headers: Vec<(String, String)>,
    reader: Box<dyn Read + Send + Sync + 'static>,
}

impl Stream {
    fn of(response: ureq::Response) -> Self {
        let headers = response
            .headers_names()
            .into_iter()
            .filter_map(|name| {
                let value = response.header(&name)?.to_owned();
                Some((name.to_ascii_lowercase(), value))
            })
            .collect();
        Self {
            status: response.status(),
            content_type: response.content_type().to_ascii_lowercase(),
            url: response.get_url().to_owned(),
            length: response
                .header("Content-Length")
                .and_then(|length| length.trim().parse().ok()),
            headers,
            reader: response.into_reader(),
        }
    }

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
                    .parse::<Ipv4Addr>()
                    .is_ok_and(|address| address.is_loopback())
        }
        None => false,
    }
}

/// Whether a network address belongs to this computer or its local network.
fn private_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            address.is_loopback()
                || address.is_private()
                || address.is_link_local()
                || address.is_unspecified()
                || address.is_broadcast()
                // Carrier-grade NAT, 100.64.0.0/10.
                || (address.octets()[0] == 100 && address.octets()[1] & 0xc0 == 64)
        }
        IpAddr::V6(address) => {
            let first = address.segments()[0];
            address.is_loopback()
                || address.is_unspecified()
                // Unique local (fc00::/7) and link-local (fe80::/10) addresses.
                || first & 0xfe00 == 0xfc00
                || first & 0xffc0 == 0xfe80
                || address
                    .to_ipv4_mapped()
                    .is_some_and(|mapped| private_ip(IpAddr::V4(mapped)))
        }
    }
}

/// Whether the address, as it is written, points at this computer or at its
/// local network. What a name resolves to is checked when the connection is
/// made, see [`outside_only`].
pub fn is_private(url: &str) -> bool {
    let Some(host) = host_of(url) else {
        return false;
    };
    // Names of this computer and of the devices beside it.
    if matches!(host.rsplit('.').next(), Some("localhost" | "local")) {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(private_ip)
}

/// Resolves a name and keeps only the addresses outside the local network,
/// so a public name that points inside is not a way in.
fn outside_only(netloc: &str) -> io::Result<Vec<SocketAddr>> {
    let found: Vec<SocketAddr> = netloc
        .to_socket_addrs()?
        .filter(|address| !private_ip(address.ip()))
        .collect();
    if found.is_empty() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, INSIDE));
    }
    Ok(found)
}

/// The error of a request that was not allowed to reach the local network.
fn refused(error: ureq::Error, address: &str) -> Error {
    let inside = matches!(&error, ureq::Error::Transport(transport)
        if transport.kind() == ureq::ErrorKind::Dns && transport.to_string().contains(INSIDE));
    if inside {
        Error::new("localAddress").with("url", address)
    } else {
        failure(error)
    }
}

fn is_http(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// The hexadecimal form of a hash.
fn hex(digest: &[u8]) -> String {
    let mut text = String::with_capacity(digest.len() * 2);
    for byte in digest {
        text.push(char::from(HEX[usize::from(byte >> 4)]));
        text.push(char::from(HEX[usize::from(byte & 15)]));
    }
    text
}

/// A short name for an address, safe in a file name: the start of its SHA-256.
pub fn fingerprint(text: &str) -> String {
    let mut name = hex(Sha256::digest(text.as_bytes()).as_slice());
    name.truncate(32);
    name
}

/// Reads at most `limit` bytes of a body. A longer body is an error, unless
/// `cut` says the start of it is enough.
pub fn read_body(
    reader: &mut dyn Read,
    limit: u64,
    cut: bool,
    cancel: &CancelToken,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        cancel.check()?;
        let read = reader
            .read(&mut buffer)
            .map_err(|error| Error::new("network").detail(error))?;
        if read == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() as u64 > limit {
            if cut {
                bytes.truncate(limit as usize);
                return Ok(bytes);
            }
            return Err(Error::new("responseTooLarge"));
        }
    }
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

/// Checks a file against a hexadecimal SHA-256.
pub fn verify_sha256(path: &Path, expected: &str, cancel: &CancelToken) -> Result<bool> {
    let mut file = File::open(path)?;
    let mut buffer = vec![0u8; 1 << 20];
    let mut sha256 = Sha256::new();
    loop {
        cancel.check()?;
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        sha256.update(&buffer[..read]);
    }
    Ok(hex(sha256.finalize().as_slice()).eq_ignore_ascii_case(expected))
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
            hex(sha256.finalize().as_slice()).eq_ignore_ascii_case(expected)
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
        let connector = match native_tls::TlsConnector::new() {
            Ok(connector) => Some(Arc::new(connector)),
            Err(error) => {
                tracing::warn!("TLS is unavailable: {error}");
                None
            }
        };
        let builder = |redirects: u32| {
            let mut builder = ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(15))
                .timeout_read(Duration::from_secs(90))
                .timeout_write(Duration::from_secs(90))
                .redirects(redirects)
                .user_agent(USER_AGENT);
            if let Some(connector) = &connector {
                builder = builder.tls_connector(Arc::clone(connector));
            }
            builder
        };
        Self {
            agent: builder(MAX_REDIRECTS).build(),
            direct: builder(0).build(),
            outside: builder(0).resolver(outside_only).build(),
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
        Ok(Stream::of(response))
    }

    /// Asks for a page or a picture and hands the response over unread, so
    /// the caller decides how much of it to take. Redirects are followed one
    /// by one: with `ask.local` off, none of them may lead to this computer
    /// or its local network.
    pub fn open_get(&self, url: &str, ask: &Ask<'_>) -> Result<Stream> {
        let mut address = url.trim().to_owned();
        for _ in 0..=MAX_REDIRECTS {
            if !is_http(&address) {
                return Err(Error::new("badUrl").with("url", address));
            }
            if !ask.local && is_private(&address) {
                return Err(Error::new("localAddress").with("url", address));
            }
            let agent = if ask.local {
                &self.direct
            } else {
                &self.outside
            };
            let mut request = agent.request("GET", &address).set("Accept", ask.accept);
            if let Some(referer) = ask.referer.filter(|referer| is_http(referer)) {
                request = request.set("Referer", referer.trim());
            }
            if let Some(timeout) = ask.timeout {
                request = request.timeout(timeout);
            }
            let response = request.call().map_err(|error| refused(error, &address))?;
            if !(300..400).contains(&response.status()) {
                let mut stream = Stream::of(response);
                stream.url = address;
                return Ok(stream);
            }
            let next = response
                .header("Location")
                .and_then(|location| Url::parse(&address).ok()?.join(location.trim()).ok());
            match next {
                Some(next) => address = next.into(),
                None => {
                    return Err(Error::new("badResponse").detail("a redirect without an address"))
                }
            }
        }
        Err(Error::new("network").detail("too many redirects"))
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
        let sha = match checksum {
            Checksum::Sha256(sha) => Some(*sha),
            Checksum::Sha512(_) => None,
        };
        if let Some(sha) = sha {
            return self.download_sha256(url, target, Some(bytes), Some(sha), progress, cancel);
        }
        self.receive(url, target, bytes, true, progress, cancel)?;
        if !verify(target, checksum, cancel)? {
            let _ = fs::remove_file(target);
            return Err(Error::new("checksumMismatch").with("url", url));
        }
        Ok(())
    }

    /// Downloads a file of a known size and, when `sha256` is set, a known hash.
    /// `bytes` of `None` accepts any size up to 20 GiB.
    pub fn download_sha256(
        &self,
        url: &str,
        target: &Path,
        bytes: Option<u64>,
        sha256: Option<&str>,
        progress: &mut dyn FnMut(u64),
        cancel: &CancelToken,
    ) -> Result<()> {
        let limit = bytes.unwrap_or(20 * 1024 * 1024 * 1024);
        self.receive(url, target, limit, bytes.is_some(), progress, cancel)?;
        if let Some(expected) = bytes {
            let have = fs::metadata(target).map_or(0, |meta| meta.len());
            if have != expected {
                let _ = fs::remove_file(target);
                return Err(Error::new("checksumMismatch").with("url", url));
            }
        }
        if let Some(sha) = sha256.filter(|sha| !sha.is_empty()) {
            if !verify_sha256(target, sha, cancel)? {
                let _ = fs::remove_file(target);
                return Err(Error::new("checksumMismatch").with("url", url));
            }
        }
        Ok(())
    }

    /// Downloads up to `bytes`. When `exact` is set, a short connection is an
    /// error and the next call continues the same partial file.
    fn receive(
        &self,
        url: &str,
        target: &Path,
        bytes: u64,
        exact: bool,
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
        if exact && have < bytes {
            // The connection dropped; the next attempt continues from here.
            return Err(Error::new("network").detail("connection closed early"));
        }
        if target.exists() {
            fs::remove_file(target)?;
        }
        fs::rename(&part, target)?;
        Ok(())
    }

    /// Opens a WebSocket. Used for the live progress of ComfyUI; the address
    /// is the `ws` form of a server the caller has already been allowed to use.
    pub fn connect_socket(&self, url: &str, key: Option<&str>) -> Result<Socket> {
        if !url.starts_with("ws://") && !url.starts_with("wss://") {
            return Err(Error::new("badUrl").with("url", url));
        }
        let mut request = url
            .into_client_request()
            .map_err(|error| Error::new("badUrl").with("url", url).detail(error))?;
        if let Some(key) = key.filter(|key| !key.is_empty()) {
            let value = tungstenite::http::HeaderValue::from_str(&format!("Bearer {key}"))
                .map_err(|error| Error::new("badKey").detail(error))?;
            request
                .headers_mut()
                .insert(tungstenite::http::header::AUTHORIZATION, value);
        }
        let (inner, _) =
            tungstenite::connect(request).map_err(|error| Error::new("network").detail(error))?;
        if let Some(tcp) = tcp_of(inner.get_ref()) {
            let _ = tcp.set_read_timeout(Some(Duration::from_millis(400)));
        }
        Ok(Socket { inner })
    }
}

/// The `ws` address of a ComfyUI server.
pub fn websocket_of(http_base: &str, client: &str) -> String {
    let base = http_base.trim().trim_end_matches('/');
    let base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_owned()
    };
    format!("{base}/ws?clientId={client}")
}

/// One message from a WebSocket.
pub enum SocketMessage {
    Text(String),
    Binary(Vec<u8>),
}

/// An open WebSocket.
pub struct Socket {
    inner: tungstenite::WebSocket<MaybeTlsStream<TcpStream>>,
}

fn tcp_of(stream: &MaybeTlsStream<TcpStream>) -> Option<&TcpStream> {
    match stream {
        MaybeTlsStream::Plain(tcp) => Some(tcp),
        MaybeTlsStream::NativeTls(tls) => Some(tls.get_ref()),
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

impl Socket {
    /// The next message, or nothing when the wait ran out.
    pub fn recv(&mut self) -> Result<Option<SocketMessage>> {
        match self.inner.read() {
            Ok(tungstenite::Message::Text(text)) => Ok(Some(SocketMessage::Text(text.to_string()))),
            Ok(tungstenite::Message::Binary(bytes)) => {
                Ok(Some(SocketMessage::Binary(bytes.to_vec())))
            }
            Ok(tungstenite::Message::Ping(payload)) => {
                let _ = self.inner.send(tungstenite::Message::Pong(payload));
                Ok(None)
            }
            Ok(tungstenite::Message::Close(_)) => {
                Err(Error::new("network").detail("socket closed"))
            }
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(error))
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::TimedOut =>
            {
                Ok(None)
            }
            Err(error) => Err(Error::new("network").detail(error)),
        }
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
    fn the_local_network_is_private() {
        for address in [
            "http://localhost/x",
            "http://printer.local/",
            "http://app.localhost:3000",
            "http://127.0.0.1:8080",
            "http://10.1.2.3/",
            "http://172.20.0.1/",
            "http://192.168.1.1/reboot",
            "http://169.254.169.254/latest",
            "http://0.0.0.0/",
            "http://[::1]/",
            "http://[fd00::1]/",
            "http://[fe80::1]/",
            "http://[::ffff:192.168.1.1]/",
            "http://100.64.0.1/",
        ] {
            assert!(is_private(address), "{address}");
        }
        for address in [
            "https://www.pinterest.com/pin/1/",
            "https://8.8.8.8/",
            "https://localhost.example.com/",
            "https://[2001:4860:4860::8888]/",
            "",
        ] {
            assert!(!is_private(address), "{address}");
        }
    }

    #[test]
    fn local_addresses_are_refused_before_any_request() {
        let net = Net::new();
        let ask = Ask {
            accept: "*/*",
            referer: None,
            local: false,
            timeout: None,
        };
        let refused = net.open_get("http://192.168.0.1/", &ask).err().unwrap();
        assert_eq!(refused.code, "localAddress");
        let bad = net.open_get("ftp://example.org/a.png", &ask).err().unwrap();
        assert_eq!(bad.code, "badUrl");
        assert!(outside_only("127.0.0.1:80").is_err());
        assert_eq!(outside_only("8.8.8.8:443").unwrap().len(), 1);
    }

    #[test]
    fn bodies_are_read_up_to_the_limit() {
        let cancel = CancelToken::new();
        let data = vec![7u8; 100_000];
        let whole = read_body(&mut data.as_slice(), 200_000, false, &cancel).unwrap();
        assert_eq!(whole.len(), 100_000);
        let cut = read_body(&mut data.as_slice(), 1000, true, &cancel).unwrap();
        assert_eq!(cut.len(), 1000);
        let refused = read_body(&mut data.as_slice(), 1000, false, &cancel).unwrap_err();
        assert_eq!(refused.code, "responseTooLarge");
    }

    #[test]
    fn fingerprints_are_short_and_stable() {
        let name = fingerprint("https://example.org/a.png");
        assert_eq!(name.len(), 32);
        assert_eq!(name, fingerprint("https://example.org/a.png"));
        assert_ne!(name, fingerprint("https://example.org/b.png"));
        assert!(name.bytes().all(|byte| byte.is_ascii_hexdigit()));
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
