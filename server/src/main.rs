use anyhow::{Context, Result};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use log::{error, info, warn};
use std::{
    collections::HashMap,
    io::{ErrorKind, Read, Write},
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{broadcast, mpsc, Mutex as AsyncMutex},
    time,
};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use uart_remote_core::{
    list_serial_ports, open_serial_port, ClientFrame, CoreError, SerialConfig, ServerFrame, Status,
    TokenAuth, WriterLease,
};
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(author, version, about = "Expose local serial ports over WebSocket")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:9001")]
    bind: SocketAddr,

    #[arg(long, env = "UART_REMOTE_TOKEN")]
    token: String,
}

#[derive(Clone)]
struct AppState {
    auth: TokenAuth,
    hubs: Arc<AsyncMutex<HashMap<String, Arc<SerialHub>>>>,
}

struct SerialHub {
    config: SerialConfig,
    writer: AsyncMutex<WriterLease>,
    port: Arc<Mutex<Box<dyn serialport::SerialPort>>>,
    events: broadcast::Sender<ServerFrame>,
}

impl SerialHub {
    fn open(config: SerialConfig) -> Result<Arc<Self>> {
        let port = open_serial_port(&config)
            .with_context(|| format!("failed to open serial port {}", config.port))?;
        let (events, _) = broadcast::channel(1024);
        let hub = Arc::new(Self {
            config: config.clone(),
            writer: AsyncMutex::new(WriterLease::default()),
            port: Arc::new(Mutex::new(port)),
            events,
        });

        hub.spawn_reader();
        let _ = hub.events.send(ServerFrame::Status {
            status: Status::SerialOpened { port: config.port },
        });
        Ok(hub)
    }

    fn subscribe(&self) -> broadcast::Receiver<ServerFrame> {
        self.events.subscribe()
    }

    async fn claim_writer(&self, client_id: &str) -> ServerFrame {
        let mut writer = self.writer.lock().await;
        match writer.claim(client_id) {
            Ok(()) => {
                let frame = ServerFrame::Status {
                    status: Status::WriterClaimed {
                        client_id: client_id.to_string(),
                    },
                };
                let _ = self.events.send(frame.clone());
                frame
            }
            Err(CoreError::WriterAlreadyClaimed(owner)) => ServerFrame::Status {
                status: Status::ReadOnly { owner: Some(owner) },
            },
            Err(error) => ServerFrame::Error {
                message: error.to_string(),
            },
        }
    }

    async fn release_writer(&self, client_id: &str) -> ServerFrame {
        let mut writer = self.writer.lock().await;
        match writer.release(client_id) {
            Ok(true) => {
                let frame = ServerFrame::Status {
                    status: Status::WriterReleased,
                };
                let _ = self.events.send(frame.clone());
                frame
            }
            Ok(false) => ServerFrame::Status {
                status: Status::ReadOnly { owner: None },
            },
            Err(error) => ServerFrame::Error {
                message: error.to_string(),
            },
        }
    }

    async fn cleanup_client(&self, client_id: &str) {
        let mut writer = self.writer.lock().await;
        if writer.release_if_owner(client_id) {
            let _ = self.events.send(ServerFrame::Status {
                status: Status::WriterReleased,
            });
        }
    }

    async fn write_serial(&self, client_id: &str, data: &[u8]) -> ServerFrame {
        if !self.writer.lock().await.can_write(client_id) {
            return ServerFrame::Error {
                message: "client does not own writer permission".to_string(),
            };
        }

        let write_result = {
            let mut port = self.port.lock().expect("serial mutex poisoned");
            port.write_all(data).and_then(|_| port.flush())
        };

        match write_result {
            Ok(()) => ServerFrame::Status {
                status: Status::WriterClaimed {
                    client_id: client_id.to_string(),
                },
            },
            Err(error) => {
                let frame = ServerFrame::Status {
                    status: Status::Error {
                        message: format!("serial write failed: {error}"),
                    },
                };
                let _ = self.events.send(frame.clone());
                frame
            }
        }
    }

    fn spawn_reader(self: &Arc<Self>) {
        let port = Arc::clone(&self.port);
        let sender = self.events.clone();
        let config = self.config.clone();

        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            loop {
                let read_result = {
                    let mut port = port.lock().expect("serial mutex poisoned");
                    port.read(&mut buffer)
                };

                match read_result {
                    Ok(0) => continue,
                    Ok(count) => {
                        let _ = sender.send(ServerFrame::serial_data(
                            config.port.clone(),
                            &buffer[..count],
                        ));
                    }
                    Err(error) if error.kind() == ErrorKind::TimedOut => continue,
                    Err(error) => {
                        let _ = sender.send(ServerFrame::Status {
                            status: Status::Error {
                                message: format!("serial read failed on {}: {error}", config.port),
                            },
                        });
                        break;
                    }
                }
            }

            let _ = sender.send(ServerFrame::Status {
                status: Status::SerialClosed { port: config.port },
            });
        });
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();
    let state = AppState {
        auth: TokenAuth::new(args.token),
        hubs: Arc::new(AsyncMutex::new(HashMap::new())),
    };

    let listener = TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("failed to bind {}", args.bind))?;
    info!("listening on ws://{}", args.bind);

    loop {
        let (stream, peer) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, peer, state).await {
                warn!("connection {peer} ended: {error:#}");
            }
        });
    }
}

async fn handle_connection(stream: TcpStream, peer: SocketAddr, state: AppState) -> Result<()> {
    let ws = accept_async(stream)
        .await
        .with_context(|| format!("websocket handshake failed for {peer}"))?;
    let client_id = Uuid::new_v4().to_string();
    let (mut ws_tx, mut ws_rx) = ws.split();
    let (out_tx, mut out_rx) = mpsc::channel::<ServerFrame>(256);

    tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            match serde_json::to_string(&frame) {
                Ok(text) => {
                    if ws_tx.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                Err(error) => error!("failed to encode server frame: {error}"),
            }
        }
    });

    let auth_frame = time::timeout(Duration::from_secs(10), ws_rx.next())
        .await
        .context("auth timeout")?
        .context("client disconnected before auth")??;
    let auth_frame = parse_client_frame(auth_frame)?;
    match auth_frame {
        ClientFrame::Auth { token } => {
            if let Err(error) = state.auth.verify(&token) {
                send_frame(
                    &out_tx,
                    ServerFrame::AuthFailed {
                        reason: error.to_string(),
                    },
                )
                .await;
                return Ok(());
            }
        }
        _ => {
            send_frame(
                &out_tx,
                ServerFrame::AuthFailed {
                    reason: "first frame must be auth".to_string(),
                },
            )
            .await;
            return Ok(());
        }
    }

    send_frame(&out_tx, ServerFrame::AuthOk).await;
    send_frame(
        &out_tx,
        ServerFrame::Ports {
            ports: list_serial_ports().unwrap_or_else(|error| {
                warn!("failed to list serial ports: {error}");
                Vec::new()
            }),
        },
    )
    .await;
    send_frame(
        &out_tx,
        ServerFrame::Status {
            status: Status::Connected {
                client_id: client_id.clone(),
            },
        },
    )
    .await;

    let mut current_hub: Option<Arc<SerialHub>> = None;
    let mut hub_events: Option<broadcast::Receiver<ServerFrame>> = None;

    loop {
        tokio::select! {
            message = ws_rx.next() => {
                let Some(message) = message else {
                    break;
                };
                let frame = parse_client_frame(message?)?;
                match frame {
                    ClientFrame::Auth { .. } => {
                        send_frame(&out_tx, ServerFrame::Error { message: "already authenticated".to_string() }).await;
                    }
                    ClientFrame::Open { config } => {
                        let hub = get_or_open_hub(&state, config.clone()).await?;
                        hub_events = Some(hub.subscribe());
                        current_hub = Some(Arc::clone(&hub));
                        send_frame(&out_tx, ServerFrame::Opened { config }).await;
                    }
                    ClientFrame::ClaimWriter => {
                        if let Some(hub) = &current_hub {
                            send_frame(&out_tx, hub.claim_writer(&client_id).await).await;
                        } else {
                            send_frame(&out_tx, ServerFrame::Error { message: "open a serial port before claiming writer".to_string() }).await;
                        }
                    }
                    ClientFrame::ReleaseWriter => {
                        if let Some(hub) = &current_hub {
                            send_frame(&out_tx, hub.release_writer(&client_id).await).await;
                        }
                    }
                    ClientFrame::SerialData { data } => {
                        if let Some(hub) = &current_hub {
                            match base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data) {
                                Ok(bytes) => send_frame(&out_tx, hub.write_serial(&client_id, &bytes).await).await,
                                Err(error) => send_frame(&out_tx, ServerFrame::Error { message: format!("invalid serial_data payload: {error}") }).await,
                            }
                        } else {
                            send_frame(&out_tx, ServerFrame::Error { message: "open a serial port before writing".to_string() }).await;
                        }
                    }
                    ClientFrame::Ping => send_frame(&out_tx, ServerFrame::Pong).await,
                }
            }
            event = recv_hub_event(&mut hub_events), if hub_events.is_some() => {
                match event {
                    Ok(frame) => send_frame(&out_tx, frame).await,
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        send_frame(&out_tx, ServerFrame::Status {
                            status: Status::Error {
                                message: format!("client skipped {skipped} serial events"),
                            },
                        }).await;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    if let Some(hub) = &current_hub {
        hub.cleanup_client(&client_id).await;
    }
    info!("client {client_id} disconnected from {peer}");
    Ok(())
}

async fn recv_hub_event(
    receiver: &mut Option<broadcast::Receiver<ServerFrame>>,
) -> std::result::Result<ServerFrame, broadcast::error::RecvError> {
    receiver.as_mut().expect("receiver exists").recv().await
}

async fn get_or_open_hub(state: &AppState, config: SerialConfig) -> Result<Arc<SerialHub>> {
    let key = config.service_key();
    let mut hubs = state.hubs.lock().await;
    if let Some(hub) = hubs.get(&key) {
        return Ok(Arc::clone(hub));
    }

    let hub = SerialHub::open(config)?;
    hubs.insert(key, Arc::clone(&hub));
    Ok(hub)
}

fn parse_client_frame(message: Message) -> Result<ClientFrame> {
    match message {
        Message::Text(text) => serde_json::from_str(&text).context("invalid client frame"),
        Message::Binary(bytes) => {
            serde_json::from_slice(&bytes).context("invalid binary client frame")
        }
        Message::Ping(_) | Message::Pong(_) => Ok(ClientFrame::Ping),
        Message::Close(_) => anyhow::bail!("client closed connection"),
        Message::Frame(_) => anyhow::bail!("unexpected raw websocket frame"),
    }
}

async fn send_frame(sender: &mpsc::Sender<ServerFrame>, frame: ServerFrame) {
    if sender.send(frame).await.is_err() {
        warn!("failed to queue server frame for disconnected client");
    }
}
