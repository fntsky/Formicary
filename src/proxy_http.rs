use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::http::{Request, Response};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const IO_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_HEADERS: usize = 64 * 1024;
const MAX_RESPONSE_BODY: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
struct Upstream {
    host: String,
    port: u16,
    authority: String,
    base_path: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ProxyMount {
    pub(crate) route: String,
    upstream: Upstream,
    process: Arc<RunningProcess>,
}

#[derive(Debug)]
struct RunningProcess {
    child: Mutex<Child>,
}

impl ProxyMount {
    pub(crate) fn start(
        working_directory: &Path,
        script: &str,
        route: String,
        upstream: &str,
    ) -> io::Result<Self> {
        if script.trim().is_empty() {
            return Err(invalid_input("proxyhttp script must not be empty"));
        }
        let upstream = parse_upstream(upstream)?;
        let mut command = command_for(script);
        configure_process_group(&mut command);
        let child = command
            .current_dir(working_directory)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;

        Ok(Self {
            route,
            upstream,
            process: Arc::new(RunningProcess {
                child: Mutex::new(child),
            }),
        })
    }

    pub(crate) fn matches(&self, path: &str) -> bool {
        path == self.route
            || path
                .strip_prefix(&self.route)
                .is_some_and(|suffix| suffix.starts_with('/'))
    }

    pub(crate) fn forward(&self, request: &Request) -> Response {
        match self.try_forward(request) {
            Ok(response) => response,
            Err(error) => {
                let (status, message) = if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) {
                    ("504 Gateway Timeout", "Upstream request timed out")
                } else {
                    ("502 Bad Gateway", "Unable to reach upstream service")
                };
                eprintln!(
                    "proxyhttp {} -> {} failed: {error}",
                    self.route, self.upstream.authority
                );
                Response::new(
                    status,
                    "text/plain; charset=utf-8",
                    format!("{message}\n").into_bytes(),
                )
            }
        }
    }

    fn try_forward(&self, request: &Request) -> io::Result<Response> {
        if self
            .process
            .child
            .lock()
            .map_err(|_| io::Error::other("proxy process state is unavailable"))?
            .try_wait()?
            .is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "proxy service process has exited",
            ));
        }

        let target = self.upstream_target(&request.target)?;
        let mut stream = self.upstream.connect()?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;

        write!(
            stream,
            "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\n",
            request.method,
            target,
            self.upstream.authority,
            request.body.len()
        )?;
        let connection_headers = connection_header_tokens(&request.headers);
        for (name, value) in &request.headers {
            if is_hop_by_hop(name)
                || connection_headers.contains(name)
                || !valid_header_value(value)
            {
                continue;
            }
            write!(stream, "{name}: {value}\r\n")?;
        }
        write!(stream, "\r\n")?;
        stream.write_all(&request.body)?;
        stream.flush()?;

        read_upstream_response(stream, request.method == "HEAD")
    }

    fn upstream_target(&self, original_target: &str) -> io::Result<String> {
        let (path, query) = original_target
            .split_once('?')
            .map_or((original_target, ""), |(path, query)| (path, query));
        if !path.starts_with('/') || path.contains(['\r', '\n', '#']) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "request target is not a valid origin-form path",
            ));
        }
        let suffix = path.strip_prefix(&self.route).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "request does not match proxy route",
            )
        })?;
        if !suffix.is_empty() && !suffix.starts_with('/') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "request does not match proxy route boundary",
            ));
        }
        let suffix = if suffix.is_empty() { "/" } else { suffix };
        let base = self.upstream.base_path.trim_end_matches('/');
        let forwarded_path = format!("{base}{suffix}");
        if query.is_empty() {
            Ok(forwarded_path)
        } else {
            Ok(format!("{forwarded_path}?{query}"))
        }
    }
}

pub(crate) fn validate_upstream(upstream: &str) -> io::Result<()> {
    parse_upstream(upstream).map(|_| ())
}

fn parse_upstream(value: &str) -> io::Result<Upstream> {
    let remainder = value
        .strip_prefix("http://")
        .ok_or_else(|| invalid_input("proxyhttp upstream must be an http:// URL"))?;
    let authority_end = remainder.find('/').unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    let base_path = &remainder[authority_end..];
    if authority.is_empty()
        || authority.contains(['@', '?', '#', '\\', '\r', '\n', ' '])
        || base_path.contains(['?', '#', '\\', '\r', '\n', ' '])
    {
        return Err(invalid_input("proxyhttp upstream URL is invalid"));
    }

    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let closing = bracketed
            .find(']')
            .ok_or_else(|| invalid_input("proxyhttp upstream has an invalid IPv6 host"))?;
        let host = bracketed[..closing].to_owned();
        let port_text = &bracketed[closing + 1..];
        let port = if port_text.is_empty() {
            80
        } else {
            port_text
                .strip_prefix(':')
                .ok_or_else(|| invalid_input("proxyhttp upstream authority is invalid"))?
                .parse::<u16>()
                .map_err(|_| invalid_input("proxyhttp upstream port is invalid"))?
        };
        (host, port)
    } else if let Some((host, port_text)) = authority.rsplit_once(':') {
        if host.is_empty() || host.contains(':') || port_text.is_empty() {
            return Err(invalid_input("IPv6 upstream hosts must use brackets"));
        }
        let port = port_text
            .parse::<u16>()
            .map_err(|_| invalid_input("proxyhttp upstream port is invalid"))?;
        (host.to_owned(), port)
    } else {
        (authority.to_owned(), 80)
    };
    if host.is_empty() || port == 0 {
        return Err(invalid_input("proxyhttp upstream host or port is invalid"));
    }

    Ok(Upstream {
        host,
        port,
        authority: authority.to_owned(),
        base_path: base_path.trim_end_matches('/').to_owned(),
    })
}

impl Upstream {
    fn connect(&self) -> io::Result<TcpStream> {
        let addresses = (self.host.as_str(), self.port).to_socket_addrs()?;
        let mut last_error = None;
        for address in addresses {
            match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
                Ok(stream) => return Ok(stream),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "upstream host resolved to no addresses",
            )
        }))
    }
}

fn read_upstream_response(stream: TcpStream, head_only: bool) -> io::Result<Response> {
    let mut reader = BufReader::new(stream);
    let (status, status_code, headers) = read_response_headers(&mut reader)?;
    let connection_headers = response_connection_header_tokens(&headers);
    let content_length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| {
            value.parse::<usize>().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid upstream content length",
                )
            })
        })
        .transpose()?;
    let transfer_encoding = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("transfer-encoding"))
        .map(|(_, value)| value.as_str());
    let chunked = match transfer_encoding {
        Some(value) if value.trim().eq_ignore_ascii_case("chunked") => true,
        Some(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported upstream transfer encoding",
            ));
        }
        None => false,
    };
    if status_code == 101 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "upstream protocol upgrades are not supported",
        ));
    }
    let no_body = head_only
        || (100..200).contains(&status_code)
        || status_code == 204
        || status_code == 205
        || status_code == 304;
    let body = if no_body {
        Vec::new()
    } else if chunked {
        read_chunked_response(&mut reader)?
    } else if let Some(length) = content_length {
        if length > MAX_RESPONSE_BODY {
            return Err(io::Error::from(io::ErrorKind::FileTooLarge));
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body)?;
        body
    } else {
        let mut body = Vec::new();
        reader
            .take((MAX_RESPONSE_BODY + 1) as u64)
            .read_to_end(&mut body)?;
        if body.len() > MAX_RESPONSE_BODY {
            return Err(io::Error::from(io::ErrorKind::FileTooLarge));
        }
        body
    };

    let content_type = headers
        .iter()
        .find(|(name, value)| {
            name.eq_ignore_ascii_case("content-type") && valid_header_value(value)
        })
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| "application/octet-stream".to_owned());
    let mut response = Response::new(status, content_type, body);
    if head_only {
        response.content_length = content_length;
    }
    for (name, value) in headers {
        let lower_name = name.to_ascii_lowercase();
        if matches!(
            lower_name.as_str(),
            "content-type"
                | "content-length"
                | "transfer-encoding"
                | "connection"
                | "keep-alive"
                | "trailer"
                | "upgrade"
                | "proxy-authenticate"
                | "proxy-authorization"
        ) || connection_headers.contains(&lower_name)
            || !valid_header_name(&name)
            || !valid_header_value(&value)
        {
            continue;
        }
        response.headers.push(format!("{name}: {value}"));
    }
    Ok(response)
}

fn read_response_headers<R: BufRead>(
    reader: &mut R,
) -> io::Result<(String, u16, Vec<(String, String)>)> {
    let mut consumed = 0;
    loop {
        let mut status_line = String::new();
        let count = reader.read_line(&mut status_line)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "empty upstream response",
            ));
        }
        consumed += count;
        if consumed > MAX_RESPONSE_HEADERS {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        let mut parts = status_line.trim_end_matches(['\r', '\n']).splitn(3, ' ');
        let version = parts.next().unwrap_or_default();
        let code = parts
            .next()
            .unwrap_or_default()
            .parse::<u16>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid upstream status"))?;
        if !matches!(version, "HTTP/1.0" | "HTTP/1.1") || !(100..=599).contains(&code) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid upstream status line",
            ));
        }
        let reason = parts.next().unwrap_or_else(|| default_reason(code));
        if !valid_header_value(reason) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid upstream reason phrase",
            ));
        }
        let status = format!("{code} {reason}");
        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            let count = reader.read_line(&mut line)?;
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete upstream headers",
                ));
            }
            consumed += count;
            if consumed > MAX_RESPONSE_HEADERS {
                return Err(io::Error::from(io::ErrorKind::InvalidData));
            }
            if line == "\r\n" || line == "\n" {
                break;
            }
            let (name, value) = line
                .trim_end_matches(['\r', '\n'])
                .split_once(':')
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "malformed upstream response header",
                    )
                })?;
            headers.push((name.trim().to_owned(), value.trim().to_owned()));
        }
        if (100..200).contains(&code) && code != 101 {
            continue;
        }
        return Ok((status, code, headers));
    }
}

fn read_chunked_response(reader: &mut impl BufRead) -> io::Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "incomplete chunked response",
            ));
        }
        let size_text = line
            .trim_end_matches(['\r', '\n'])
            .split(';')
            .next()
            .unwrap_or_default();
        let size = usize::from_str_radix(size_text, 16).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "invalid upstream chunk size")
        })?;
        if size == 0 {
            loop {
                let mut trailer = String::new();
                if reader.read_line(&mut trailer)? == 0 || trailer == "\r\n" || trailer == "\n" {
                    return Ok(body);
                }
            }
        }
        if body.len().saturating_add(size) > MAX_RESPONSE_BODY {
            return Err(io::Error::from(io::ErrorKind::FileTooLarge));
        }
        let start = body.len();
        body.resize(start + size, 0);
        reader.read_exact(&mut body[start..])?;
        let mut terminator = [0; 2];
        reader.read_exact(&mut terminator)?;
        if terminator != *b"\r\n" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid upstream chunk terminator",
            ));
        }
    }
}

fn connection_header_tokens(headers: &std::collections::HashMap<String, String>) -> Vec<String> {
    headers
        .get("connection")
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(|name| name.trim().to_ascii_lowercase())
        .collect()
}

fn response_connection_header_tokens(headers: &[(String, String)]) -> Vec<String> {
    headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, value)| value.split(','))
        .map(|name| name.trim().to_ascii_lowercase())
        .collect()
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "host"
            | "content-length"
            | "expect"
    )
}

fn valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn valid_header_value(value: &str) -> bool {
    !value
        .chars()
        .any(|character| character.is_control() && character != '\t')
}

fn default_reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Upstream Response",
    }
}

fn command_for(script: &str) -> Command {
    #[cfg(windows)]
    let command = {
        let mut command = Command::new("cmd.exe");
        command.args(["/C", script]);
        command
    };
    #[cfg(not(windows))]
    let command = {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    };
    command
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

impl Drop for RunningProcess {
    fn drop(&mut self) {
        let Ok(child) = self.child.get_mut() else {
            return;
        };
        stop_process_tree(child);
    }
}

fn stop_process_tree(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGTERM);
    }
    #[cfg(windows)]
    {
        let pid = child.id().to_string();
        let _ = Command::new("taskkill")
            .args(["/PID", &pid, "/T", "/F"])
            .status();
    }

    let deadline = Instant::now() + Duration::from_millis(400);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => break,
        }
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
