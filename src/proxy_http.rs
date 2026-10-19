use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::http::{Request, Response};
use crate::service::shell_runner::{ShellControl, ShellExecution, ShellLogEntry, ShellRunner};

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
    execution: Arc<ShellExecution>,
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
        let execution =
            ShellRunner::start(working_directory, script, None, ShellControl::default())?;

        Ok(Self {
            route,
            upstream,
            execution: Arc::new(execution),
        })
    }

    /// Returns this proxy's shared shell handle for logs, input, and termination.
    /// The service lives until the final proxy or shell handle is released.
    pub(crate) fn shell(&self) -> Arc<ShellExecution> {
        Arc::clone(&self.execution)
    }

    /// Returns captured stdout, stderr, and timestamped output entries.
    /// Logs remain readable after the service exits while this proxy is retained.
    pub(crate) fn logs(&self) -> Vec<ShellLogEntry> {
        self.execution.logs()
    }

    pub(crate) fn matches(&self, path: &str) -> bool {
        path == self.route
            || path
                .strip_prefix(&self.route)
                .is_some_and(|suffix| suffix.starts_with('/'))
    }

    pub(crate) fn forward(
        &self,
        request: &Request,
        authenticated_username: Option<&str>,
    ) -> Response {
        match self.try_forward(request, authenticated_username) {
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

    fn try_forward(
        &self,
        request: &Request,
        authenticated_username: Option<&str>,
    ) -> io::Result<Response> {
        if self.shell().is_finished() {
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
        if let Some(username) = authenticated_username.filter(|value| !value.contains(['\r', '\n']))
        {
            write!(stream, "X-Formicary-User: {username}\r\n")?;
        }
        let connection_headers = connection_header_tokens(&request.headers);
        for (name, value) in &request.headers {
            if name == "x-formicary-user"
                || is_hop_by_hop(name)
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

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::service::shell_runner::ShellLogStream;
    fn output(
        entries: &[crate::service::shell_runner::ShellLogEntry],
        stream: crate::service::shell_runner::ShellLogStream,
    ) -> Vec<u8> {
        entries
            .iter()
            .filter(|entry| entry.stream == stream)
            .flat_map(|entry| entry.bytes.iter().copied())
            .collect()
    }

    use std::thread;
    use std::time::Instant;

    /// 验证代理持有 shell 执行句柄，并可读取实时输出、发送输入和终止服务。
    /// Verifies that a proxy owns its shell execution and supports live logs, input, and termination.
    #[test]
    fn proxy_owns_controllable_shell_execution() {
        let mount = ProxyMount::start(
            Path::new("."),
            "printf ready; exec cat",
            "/test".into(),
            "http://127.0.0.1:1",
        )
        .unwrap();
        let clone = mount.clone();
        let deadline = Instant::now() + Duration::from_secs(3);
        while output(&mount.logs(), ShellLogStream::Stdout) != b"ready" {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        clone.shell().send_line("hello").unwrap();
        while output(&mount.logs(), ShellLogStream::Stdout) != b"readyhello\n" {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        clone.shell().terminate();
        while !mount.execution.is_finished() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!mount.logs().is_empty());
    }

    /// 验证代理克隆共享进程，最后一个持有者释放后会终止并回收服务进程。
    /// Verifies that proxy clones share a process and releasing the final owner terminates and reaps it.
    #[test]
    fn final_proxy_owner_stops_service() {
        let mount = ProxyMount::start(
            Path::new("."),
            "printf '%s' $$; exec sleep 30",
            "/test".into(),
            "http://127.0.0.1:1",
        )
        .unwrap();
        let clone = mount.clone();
        let deadline = Instant::now() + Duration::from_secs(3);
        let pid = loop {
            if let Ok(pid) = String::from_utf8_lossy(&output(&mount.logs(), ShellLogStream::Stdout))
                .parse::<i32>()
            {
                break pid;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        };
        drop(mount);
        assert!(!clone.execution.is_finished());
        drop(clone);
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }
}
