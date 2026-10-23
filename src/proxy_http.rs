use std::io::{self, Read};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use reqwest::blocking::{Client, Response as UpstreamResponse};
use reqwest::header::{
    CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue,
};
use reqwest::redirect::Policy;

use crate::http_types::{Request, Response};
use crate::service::shell_runner::{ShellControl, ShellExecution, ShellLogEntry, ShellRunner};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const IO_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BODY: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
struct Upstream {
    authority: String,
    base_path: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ProxyMount {
    pub(crate) route: String,
    upstream: Upstream,
    client: Arc<OnceLock<Result<Client, String>>>,
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
            client: Arc::new(OnceLock::new()),
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
        let url = format!("http://{}{target}", self.upstream.authority);
        let client = self
            .client
            .get_or_init(build_client)
            .as_ref()
            .map_err(|error| io::Error::other(error.clone()))?;
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let mut builder = client.request(method, url).header(CONNECTION, "close");

        if let Some(username) = authenticated_username.filter(|value| !value.contains(['\r', '\n']))
        {
            builder = builder.header("X-Formicary-User", username);
        }
        let connection_headers = connection_header_tokens(&request.headers);
        for (name, value) in &request.headers {
            if name == "x-formicary-user"
                || is_hop_by_hop(name)
                || connection_headers.contains(name)
            {
                continue;
            }
            let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) else {
                continue;
            };
            builder = builder.header(name, value);
        }

        let response = builder
            .body(request.body.clone())
            .send()
            .map_err(map_reqwest_error)?;
        read_upstream_response(response, request.method == "HEAD")
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

    let parsed = reqwest::Url::parse(value)
        .map_err(|_| invalid_input("proxyhttp upstream URL is invalid"))?;
    if parsed.scheme() != "http"
        || parsed.host_str().is_none_or(str::is_empty)
        || parsed.port_or_known_default().is_none_or(|port| port == 0)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(invalid_input("proxyhttp upstream URL is invalid"));
    }

    Ok(Upstream {
        authority: authority.to_owned(),
        base_path: base_path.trim_end_matches('/').to_owned(),
    })
}

fn read_upstream_response(
    upstream_response: UpstreamResponse,
    head_only: bool,
) -> io::Result<Response> {
    let upstream_status = upstream_response.status();
    let status_code = upstream_status.as_u16();
    if status_code == 101 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "upstream protocol upgrades are not supported",
        ));
    }
    let reason = upstream_status
        .canonical_reason()
        .unwrap_or("Upstream Response");
    let status = format!("{} {reason}", upstream_status.as_u16());

    let headers = upstream_response.headers().clone();
    let connection_headers = response_connection_header_tokens(&headers);
    let content_length = headers
        .get(CONTENT_LENGTH)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid upstream content length",
                    )
                })?
                .parse::<usize>()
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid upstream content length",
                    )
                })
        })
        .transpose()?;
    let no_body = head_only
        || (100..200).contains(&status_code)
        || status_code == 204
        || status_code == 205
        || status_code == 304;
    let body = if no_body {
        Vec::new()
    } else {
        if content_length.is_some_and(|length| length > MAX_RESPONSE_BODY) {
            return Err(io::Error::from(io::ErrorKind::FileTooLarge));
        }
        let mut body = Vec::new();
        upstream_response
            .take((MAX_RESPONSE_BODY + 1) as u64)
            .read_to_end(&mut body)?;
        if body.len() > MAX_RESPONSE_BODY {
            return Err(io::Error::from(io::ErrorKind::FileTooLarge));
        }
        body
    };

    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();
    let mut response = Response::new(status, content_type, body);
    if head_only {
        response.content_length = content_length;
    }
    for (name, value) in &headers {
        let lower_name = name.as_str();
        if matches!(
            lower_name,
            "content-type"
                | "content-length"
                | "transfer-encoding"
                | "connection"
                | "keep-alive"
                | "trailer"
                | "upgrade"
                | "proxy-authenticate"
                | "proxy-authorization"
        ) || connection_headers.iter().any(|header| header == lower_name)
        {
            continue;
        }
        if let Ok(value) = value.to_str() {
            response.headers.push(format!("{name}: {value}"));
        }
    }
    Ok(response)
}

fn connection_header_tokens(headers: &std::collections::HashMap<String, String>) -> Vec<String> {
    headers
        .get("connection")
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(|name| name.trim().to_ascii_lowercase())
        .collect()
}

fn response_connection_header_tokens(headers: &HeaderMap) -> Vec<String> {
    headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
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

fn map_reqwest_error(error: reqwest::Error) -> io::Error {
    let kind = if error.is_timeout() {
        io::ErrorKind::TimedOut
    } else {
        io::ErrorKind::Other
    };
    io::Error::new(kind, error)
}

fn build_client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(IO_TIMEOUT)
        .redirect(Policy::none())
        .no_proxy()
        .build()
        .map_err(|error| error.to_string())
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
