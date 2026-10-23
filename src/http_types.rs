use std::collections::HashMap;

use serde_json::Value;

/// Request data passed from Axum's HTTP layer to the existing application handlers.
pub struct Request {
    pub method: String,
    pub target: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

/// Response data returned by the application handlers and translated into an Axum response.
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
