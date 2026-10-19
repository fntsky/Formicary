use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;

use serde_json::Value;

const MAX_REQUEST_BODY: usize = 2 * 1024 * 1024;

pub struct Request {
    pub method: String,
    pub target: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

pub struct Response {
    pub status: String,
    pub content_type: String,
    pub body: Vec<u8>,
    pub headers: Vec<String>,
    pub content_length: Option<usize>,
}

impl Response {
    pub fn new(status: impl Into<String>, content_type: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            status: status.into(),
            content_type: content_type.into(),
            body,
            headers: Vec::new(),
            content_length: None,
        }
    }

    pub fn json(status: &'static str, body: Value) -> Self {
        Self::new(
            status,
            "application/json; charset=utf-8",
            serde_json::to_vec(&body).expect("JSON value serialization should succeed"),
        )
    }
}

pub fn read_request(stream: &TcpStream) -> io::Result<Option<Request>> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(None);
    }

    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or_default().to_owned();
    let target = request_parts.next().unwrap_or_default().to_owned();
    let path = target.split('?').next().unwrap_or_default().to_owned();
    let mut headers = HashMap::new();
    let mut content_length = 0;
    let mut content_length_seen = false;
    let mut transfer_encoding = None;

    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_owned();
            if name == "content-length" {
                if content_length_seen {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "duplicate content length",
                    ));
                }
                content_length = value.parse::<usize>().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid content length")
                })?;
                content_length_seen = true;
            }
            if name == "transfer-encoding" {
                if transfer_encoding.is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "duplicate transfer encoding",
                    ));
                }
                transfer_encoding = Some(value.to_ascii_lowercase());
            }
            headers.insert(name, value);
        }
    }

    let body = match transfer_encoding.as_deref() {
        Some("chunked") if !content_length_seen => read_chunked_body(&mut reader)?,
        Some(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported or conflicting transfer encoding",
            ));
        }
        None => {
            if content_length > MAX_REQUEST_BODY {
                return Err(io::Error::from(io::ErrorKind::FileTooLarge));
            }
            let mut body = vec![0; content_length];
            reader.read_exact(&mut body)?;
            body
        }
    };
    Ok(Some(Request {
        method,
        target,
        path,
        headers,
        body,
    }))
}

fn read_chunked_body(reader: &mut impl BufRead) -> io::Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let size = line
            .trim_end_matches(['\r', '\n'])
            .split(';')
            .next()
            .unwrap_or_default();
        let size = usize::from_str_radix(size, 16)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid chunk size"))?;
        if size == 0 {
            loop {
                let mut trailer = String::new();
                if reader.read_line(&mut trailer)? == 0 || trailer == "\r\n" || trailer == "\n" {
                    return Ok(body);
                }
            }
        }
        if body.len().saturating_add(size) > MAX_REQUEST_BODY {
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
                "invalid chunk terminator",
            ));
        }
    }
}

pub fn write_response(
    stream: &mut TcpStream,
    response: &Response,
    head_only: bool,
) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        response.content_type,
        response.content_length.unwrap_or(response.body.len())
    )?;
    for header in &response.headers {
        write!(stream, "{header}\r\n")?;
    }
    write!(stream, "\r\n")?;
    if !head_only {
        stream.write_all(&response.body)?;
    }
    Ok(())
}
