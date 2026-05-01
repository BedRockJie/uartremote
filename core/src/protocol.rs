use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataBits {
    Five,
    Six,
    Seven,
    Eight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StopBits {
    One,
    Two,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Parity {
    None,
    Odd,
    Even,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerialConfig {
    pub port: String,
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,
    #[serde(default = "default_data_bits")]
    pub data_bits: DataBits,
    #[serde(default = "default_stop_bits")]
    pub stop_bits: StopBits,
    #[serde(default = "default_parity")]
    pub parity: Parity,
}

impl SerialConfig {
    pub fn service_key(&self) -> String {
        format!(
            "{}:{}:{:?}:{:?}:{:?}",
            self.port, self.baud_rate, self.data_bits, self.stop_bits, self.parity
        )
    }
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            port: String::new(),
            baud_rate: default_baud_rate(),
            data_bits: default_data_bits(),
            stop_bits: default_stop_bits(),
            parity: default_parity(),
        }
    }
}

pub fn default_baud_rate() -> u32 {
    115_200
}

pub fn default_data_bits() -> DataBits {
    DataBits::Eight
}

pub fn default_stop_bits() -> StopBits {
    StopBits::One
}

pub fn default_parity() -> Parity {
    Parity::None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    Auth { token: String },
    Open { config: SerialConfig },
    ClaimWriter,
    ReleaseWriter,
    SerialData { data: String },
    Ping,
}

impl ClientFrame {
    pub fn serial_data(data: &[u8]) -> Self {
        Self::SerialData {
            data: BASE64.encode(data),
        }
    }

    pub fn decode_data(&self) -> Option<Vec<u8>> {
        match self {
            Self::SerialData { data } => BASE64.decode(data).ok(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    AuthOk,
    AuthFailed { reason: String },
    Ports { ports: Vec<SerialPortInfo> },
    Opened { config: SerialConfig },
    SerialData { port: String, data: String },
    Status { status: Status },
    Pong,
    Error { message: String },
}

impl ServerFrame {
    pub fn serial_data(port: impl Into<String>, data: &[u8]) -> Self {
        Self::SerialData {
            port: port.into(),
            data: BASE64.encode(data),
        }
    }

    pub fn decode_data(&self) -> Option<Vec<u8>> {
        match self {
            Self::SerialData { data, .. } => BASE64.decode(data).ok(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerialPortInfo {
    pub port_name: String,
    pub port_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Status {
    Connected { client_id: String },
    Disconnected { client_id: String },
    SerialOpened { port: String },
    SerialClosed { port: String },
    WriterClaimed { client_id: String },
    WriterReleased,
    ReadOnly { owner: Option<String> },
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::{ClientFrame, SerialConfig, ServerFrame};

    #[test]
    fn client_frame_round_trips() {
        let frame = ClientFrame::Open {
            config: SerialConfig {
                port: "COM3".to_string(),
                ..SerialConfig::default()
            },
        };

        let json = serde_json::to_string(&frame).unwrap();
        let decoded: ClientFrame = serde_json::from_str(&json).unwrap();

        match decoded {
            ClientFrame::Open { config } => assert_eq!(config.port, "COM3"),
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    #[test]
    fn serial_data_uses_base64_payloads() {
        let frame = ServerFrame::serial_data("COM1", b"\x00hello\xff");
        assert_eq!(frame.decode_data().unwrap(), b"\x00hello\xff");
    }
}
