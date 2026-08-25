use serde::{Deserialize, Serialize};

pub const WIRE_VERSION: u16 = 1;

#[derive(Debug, Deserialize, Serialize)]
pub struct RequestFrame {
    pub version: u16,
    pub method: String,
    #[serde(with = "serde_bytes")]
    pub body: Vec<u8>,
}

impl RequestFrame {
    pub fn new(method: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            version: WIRE_VERSION,
            method: method.into(),
            body,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub enum ResponseFrame {
    Service(#[serde(with = "serde_bytes")] Vec<u8>),
    Transport(String),
}
