use anyhow::{Context, Result};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use std::io::Write;
use tokio::{
    io::{self, AsyncReadExt},
    sync::{mpsc, watch},
};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uart_remote_core::{
    ClientFrame, DataBits, Parity, SerialConfig, ServerFrame, Status, StopBits,
};

#[derive(Debug, Parser)]
#[command(author, version, about = "Bridge a terminal to a remote serial port")]
struct Args {
    #[arg(long, default_value = "ws://127.0.0.1:9001")]
    url: String,

    #[arg(long, env = "UART_REMOTE_TOKEN")]
    token: String,

    #[arg(long)]
    port: String,

    #[arg(long, default_value_t = 115_200)]
    baud_rate: u32,

    #[arg(long, value_enum, default_value_t = CliDataBits::Eight)]
    data_bits: CliDataBits,

    #[arg(long, value_enum, default_value_t = CliStopBits::One)]
    stop_bits: CliStopBits,

    #[arg(long, value_enum, default_value_t = CliParity::None)]
    parity: CliParity,

    #[arg(long, help = "Do not request remote write permission")]
    read_only: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum CliDataBits {
    Five,
    Six,
    Seven,
    Eight,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum CliStopBits {
    One,
    Two,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum CliParity {
    None,
    Odd,
    Even,
}

impl From<CliDataBits> for DataBits {
    fn from(value: CliDataBits) -> Self {
        match value {
            CliDataBits::Five => Self::Five,
            CliDataBits::Six => Self::Six,
            CliDataBits::Seven => Self::Seven,
            CliDataBits::Eight => Self::Eight,
        }
    }
}

impl From<CliStopBits> for StopBits {
    fn from(value: CliStopBits) -> Self {
        match value {
            CliStopBits::One => Self::One,
            CliStopBits::Two => Self::Two,
        }
    }
}

impl From<CliParity> for Parity {
    fn from(value: CliParity) -> Self {
        match value {
            CliParity::None => Self::None,
            CliParity::Odd => Self::Odd,
            CliParity::Even => Self::Even,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();
    let config = SerialConfig {
        port: args.port,
        baud_rate: args.baud_rate,
        data_bits: args.data_bits.into(),
        stop_bits: args.stop_bits.into(),
        parity: args.parity.into(),
    };

    let (ws, _) = connect_async(&args.url)
        .await
        .with_context(|| format!("failed to connect to {}", args.url))?;
    let (mut ws_tx, mut ws_rx) = ws.split();
    let (out_tx, mut out_rx) = mpsc::channel::<ClientFrame>(256);

    tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            match serde_json::to_string(&frame) {
                Ok(text) => {
                    if ws_tx.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                Err(error) => eprintln!("failed to encode client frame: {error}"),
            }
        }
    });

    send_frame(&out_tx, ClientFrame::Auth { token: args.token }).await?;
    send_frame(
        &out_tx,
        ClientFrame::Open {
            config: config.clone(),
        },
    )
    .await?;
    if !args.read_only {
        send_frame(&out_tx, ClientFrame::ClaimWriter).await?;
    }

    let mut client_id = None;
    let (writer_tx, mut writer_rx) = watch::channel(false);
    let input_tx = out_tx.clone();
    tokio::spawn(async move {
        let mut stdin = io::stdin();
        let mut buffer = [0u8; 1024];
        loop {
            while !*writer_rx.borrow() {
                if writer_rx.changed().await.is_err() {
                    return;
                }
            }

            match stdin.read(&mut buffer).await {
                Ok(0) => break,
                Ok(count) => {
                    if input_tx
                        .send(ClientFrame::serial_data(&buffer[..count]))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(error) => {
                    eprintln!("stdin read failed: {error}");
                    break;
                }
            }
        }
    });

    while let Some(message) = ws_rx.next().await {
        let message = message.context("websocket receive failed")?;
        let frame = parse_server_frame(message)?;
        handle_server_frame(frame, &mut client_id, &writer_tx)?;
    }

    Ok(())
}

fn handle_server_frame(
    frame: ServerFrame,
    client_id: &mut Option<String>,
    writer_tx: &watch::Sender<bool>,
) -> Result<()> {
    match frame {
        ServerFrame::AuthOk => eprintln!("authenticated"),
        ServerFrame::AuthFailed { reason } => anyhow::bail!("authentication failed: {reason}"),
        ServerFrame::Ports { ports } => {
            let names = ports
                .into_iter()
                .map(|port| port.port_name)
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!("server ports: {names}");
        }
        ServerFrame::Opened { config } => eprintln!(
            "opened remote serial port {} at {} baud",
            config.port, config.baud_rate
        ),
        ServerFrame::SerialData { data, .. } => {
            let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data)
                .context("invalid serial data from server")?;
            std::io::stdout().write_all(&bytes)?;
            std::io::stdout().flush()?;
        }
        ServerFrame::Status { status } => {
            match &status {
                Status::Connected { client_id: id } => *client_id = Some(id.clone()),
                Status::WriterClaimed { client_id: owner } => {
                    let owns_writer = client_id.as_deref() == Some(owner.as_str());
                    let _ = writer_tx.send(owns_writer);
                }
                Status::WriterReleased | Status::ReadOnly { .. } => {
                    let _ = writer_tx.send(false);
                }
                _ => {}
            }
            print_status(status);
        }
        ServerFrame::Pong => {}
        ServerFrame::Error { message } => eprintln!("server error: {message}"),
    }

    Ok(())
}

fn print_status(status: Status) {
    match status {
        Status::Connected { client_id } => eprintln!("connected as {client_id}"),
        Status::Disconnected { client_id } => eprintln!("client disconnected: {client_id}"),
        Status::SerialOpened { port } => eprintln!("serial opened: {port}"),
        Status::SerialClosed { port } => eprintln!("serial closed: {port}"),
        Status::WriterClaimed { client_id } => eprintln!("writer claimed by {client_id}"),
        Status::WriterReleased => eprintln!("writer released"),
        Status::ReadOnly { owner } => {
            if let Some(owner) = owner {
                eprintln!("read-only mode; writer owned by {owner}");
            } else {
                eprintln!("read-only mode");
            }
        }
        Status::Error { message } => eprintln!("status error: {message}"),
    }
}

fn parse_server_frame(message: Message) -> Result<ServerFrame> {
    match message {
        Message::Text(text) => serde_json::from_str(&text).context("invalid server frame"),
        Message::Binary(bytes) => {
            serde_json::from_slice(&bytes).context("invalid binary server frame")
        }
        Message::Ping(_) | Message::Pong(_) => Ok(ServerFrame::Pong),
        Message::Close(_) => anyhow::bail!("server closed connection"),
        Message::Frame(_) => anyhow::bail!("unexpected raw websocket frame"),
    }
}

async fn send_frame(sender: &mpsc::Sender<ClientFrame>, frame: ClientFrame) -> Result<()> {
    sender
        .send(frame)
        .await
        .context("websocket sender task stopped")
}
