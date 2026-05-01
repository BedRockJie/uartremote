use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use std::{
    collections::HashMap,
    io::{ErrorKind, Read, Write},
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{async_runtime::JoinHandle, AppHandle, Emitter, State};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{broadcast, mpsc, Mutex as AsyncMutex},
    time,
};
use tokio_tungstenite::{accept_async, connect_async, tungstenite::Message};
use uart_remote_core::{
    list_serial_ports as core_list_serial_ports, open_serial_port, ClientFrame, CoreError,
    SerialConfig, SerialPortInfo, ServerFrame, Status, TokenAuth, WriterLease,
};
use uuid::Uuid;

#[derive(Default)]
struct DesktopState {
    server: Mutex<Option<ServerHandle>>,
    remote: Mutex<Option<RemoteClient>>,
}

struct ServerHandle {
    bind: String,
    task: JoinHandle<()>,
}

struct RemoteClient {
    url: String,
    port: String,
    sender: mpsc::Sender<ClientFrame>,
    task: JoinHandle<()>,
}

#[derive(Debug, Serialize)]
struct AppStatus {
    app_name: &'static str,
    version: &'static str,
    core_ready: bool,
    server_embedded: bool,
}

#[derive(Debug, Serialize)]
struct RuntimeStatus {
    local_server: Option<String>,
    remote_client: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum DesktopEvent {
    Status { message: String },
    SerialData { text: String, hex: String },
    Error { message: String },
}

#[derive(Clone)]
struct ServerState {
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
    fn open(config: SerialConfig) -> Result<Arc<Self>, String> {
        let port = open_serial_port(&config).map_err(|error| error.to_string())?;
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

#[tauri::command]
fn app_status() -> AppStatus {
    AppStatus {
        app_name: "UartRemote",
        version: env!("CARGO_PKG_VERSION"),
        core_ready: true,
        server_embedded: true,
    }
}

#[tauri::command]
fn runtime_status(state: State<'_, DesktopState>) -> Result<RuntimeStatus, String> {
    let local_server = state
        .server
        .lock()
        .map_err(|_| "server state mutex poisoned".to_string())?
        .as_ref()
        .map(|server| server.bind.clone());
    let remote_client = state
        .remote
        .lock()
        .map_err(|_| "remote state mutex poisoned".to_string())?
        .as_ref()
        .map(|remote| format!("{} -> {}", remote.url, remote.port));

    Ok(RuntimeStatus {
        local_server,
        remote_client,
    })
}

#[tauri::command]
fn list_serial_ports() -> Result<Vec<SerialPortInfo>, String> {
    core_list_serial_ports().map_err(|error| error.to_string())
}

#[tauri::command]
fn default_serial_config() -> SerialConfig {
    SerialConfig::default()
}

#[tauri::command]
fn verify_token(expected: String, provided: String) -> bool {
    TokenAuth::new(expected).verify(&provided).is_ok()
}

#[tauri::command]
async fn start_local_server(
    state: State<'_, DesktopState>,
    bind: String,
    token: String,
) -> Result<(), String> {
    {
        let server = state
            .server
            .lock()
            .map_err(|_| "server state mutex poisoned".to_string())?;
        if server.is_some() {
            return Err("local server is already running".to_string());
        }
    }

    let addr: SocketAddr = bind
        .parse()
        .map_err(|error| format!("invalid bind address: {error}"))?;
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|error| format!("failed to bind {bind}: {error}"))?;
    let server_state = ServerState {
        auth: TokenAuth::new(token),
        hubs: Arc::new(AsyncMutex::new(HashMap::new())),
    };

    let task = tauri::async_runtime::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                break;
            };
            let server_state = server_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = handle_server_connection(stream, peer, server_state).await {
                    eprintln!("local server connection {peer} ended: {error}");
                }
            });
        }
    });

    *state
        .server
        .lock()
        .map_err(|_| "server state mutex poisoned".to_string())? =
        Some(ServerHandle { bind, task });
    Ok(())
}

#[tauri::command]
fn stop_local_server(state: State<'_, DesktopState>) -> Result<(), String> {
    let mut server = state
        .server
        .lock()
        .map_err(|_| "server state mutex poisoned".to_string())?;
    let Some(server) = server.take() else {
        return Ok(());
    };
    server.task.abort();
    Ok(())
}

#[tauri::command]
async fn connect_remote_serial(
    app: AppHandle,
    state: State<'_, DesktopState>,
    url: String,
    token: String,
    config: SerialConfig,
    claim_writer: bool,
) -> Result<(), String> {
    {
        let remote = state
            .remote
            .lock()
            .map_err(|_| "remote state mutex poisoned".to_string())?;
        if remote.is_some() {
            return Err("remote client is already connected".to_string());
        }
    }

    let (ws, _) = connect_async(&url)
        .await
        .map_err(|error| format!("failed to connect to {url}: {error}"))?;
    let (mut ws_tx, mut ws_rx) = ws.split();
    let (out_tx, mut out_rx) = mpsc::channel::<ClientFrame>(256);

    let task_app = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        let mut client_id: Option<String> = None;
        loop {
            tokio::select! {
                outgoing = out_rx.recv() => {
                    let Some(frame) = outgoing else {
                        break;
                    };
                    match serde_json::to_string(&frame) {
                        Ok(text) => {
                            if let Err(error) = ws_tx.send(Message::Text(text.into())).await {
                                emit_remote(&task_app, DesktopEvent::Error { message: format!("remote send failed: {error}") });
                                break;
                            }
                        }
                        Err(error) => emit_remote(&task_app, DesktopEvent::Error { message: format!("encode frame failed: {error}") }),
                    }
                }
                incoming = ws_rx.next() => {
                    let Some(message) = incoming else {
                        emit_remote(&task_app, DesktopEvent::Status { message: "remote disconnected".to_string() });
                        break;
                    };
                    match message {
                        Ok(message) => match parse_server_frame(message) {
                            Ok(frame) => handle_remote_frame(&task_app, frame, &mut client_id),
                            Err(error) => emit_remote(&task_app, DesktopEvent::Error { message: error }),
                        },
                        Err(error) => {
                            emit_remote(&task_app, DesktopEvent::Error { message: format!("remote receive failed: {error}") });
                            break;
                        }
                    }
                }
            }
        }
    });

    send_client_frame(&out_tx, ClientFrame::Auth { token }).await?;
    send_client_frame(
        &out_tx,
        ClientFrame::Open {
            config: config.clone(),
        },
    )
    .await?;
    if claim_writer {
        send_client_frame(&out_tx, ClientFrame::ClaimWriter).await?;
    }

    *state
        .remote
        .lock()
        .map_err(|_| "remote state mutex poisoned".to_string())? = Some(RemoteClient {
        url,
        port: config.port,
        sender: out_tx,
        task,
    });
    Ok(())
}

#[tauri::command]
fn disconnect_remote_serial(state: State<'_, DesktopState>) -> Result<(), String> {
    let mut remote = state
        .remote
        .lock()
        .map_err(|_| "remote state mutex poisoned".to_string())?;
    let Some(remote) = remote.take() else {
        return Ok(());
    };
    remote.task.abort();
    Ok(())
}

#[tauri::command]
async fn claim_remote_writer(state: State<'_, DesktopState>) -> Result<(), String> {
    send_remote(&state, ClientFrame::ClaimWriter).await
}

#[tauri::command]
async fn release_remote_writer(state: State<'_, DesktopState>) -> Result<(), String> {
    send_remote(&state, ClientFrame::ReleaseWriter).await
}

#[tauri::command]
async fn send_remote_serial_text(
    state: State<'_, DesktopState>,
    text: String,
) -> Result<(), String> {
    send_remote(&state, ClientFrame::serial_data(text.as_bytes())).await
}

async fn send_remote(state: &State<'_, DesktopState>, frame: ClientFrame) -> Result<(), String> {
    let sender = {
        let remote = state
            .remote
            .lock()
            .map_err(|_| "remote state mutex poisoned".to_string())?;
        remote
            .as_ref()
            .map(|remote| remote.sender.clone())
            .ok_or_else(|| "remote client is not connected".to_string())?
    };
    send_client_frame(&sender, frame).await
}

async fn send_client_frame(
    sender: &mpsc::Sender<ClientFrame>,
    frame: ClientFrame,
) -> Result<(), String> {
    sender
        .send(frame)
        .await
        .map_err(|_| "remote client task stopped".to_string())
}

async fn handle_server_connection(
    stream: TcpStream,
    _peer: SocketAddr,
    state: ServerState,
) -> Result<(), String> {
    let ws = accept_async(stream)
        .await
        .map_err(|error| format!("websocket handshake failed: {error}"))?;
    let client_id = Uuid::new_v4().to_string();
    let (mut ws_tx, mut ws_rx) = ws.split();
    let (out_tx, mut out_rx) = mpsc::channel::<ServerFrame>(256);

    tauri::async_runtime::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if let Ok(text) = serde_json::to_string(&frame) {
                if ws_tx.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    let auth_message = time::timeout(Duration::from_secs(10), ws_rx.next())
        .await
        .map_err(|_| "auth timeout".to_string())?
        .ok_or_else(|| "client disconnected before auth".to_string())?
        .map_err(|error| error.to_string())?;
    match parse_client_frame(auth_message)? {
        ClientFrame::Auth { token } => {
            if let Err(error) = state.auth.verify(&token) {
                send_server_frame(
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
            send_server_frame(
                &out_tx,
                ServerFrame::AuthFailed {
                    reason: "first frame must be auth".to_string(),
                },
            )
            .await;
            return Ok(());
        }
    }

    send_server_frame(&out_tx, ServerFrame::AuthOk).await;
    send_server_frame(
        &out_tx,
        ServerFrame::Ports {
            ports: core_list_serial_ports().unwrap_or_default(),
        },
    )
    .await;
    send_server_frame(
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
                match parse_client_frame(message.map_err(|error| error.to_string())?)? {
                    ClientFrame::Auth { .. } => {
                        send_server_frame(&out_tx, ServerFrame::Error { message: "already authenticated".to_string() }).await;
                    }
                    ClientFrame::Open { config } => {
                        let hub = get_or_open_hub(&state, config.clone()).await?;
                        hub_events = Some(hub.subscribe());
                        current_hub = Some(Arc::clone(&hub));
                        send_server_frame(&out_tx, ServerFrame::Opened { config }).await;
                    }
                    ClientFrame::ClaimWriter => {
                        if let Some(hub) = &current_hub {
                            send_server_frame(&out_tx, hub.claim_writer(&client_id).await).await;
                        } else {
                            send_server_frame(&out_tx, ServerFrame::Error { message: "open a serial port before claiming writer".to_string() }).await;
                        }
                    }
                    ClientFrame::ReleaseWriter => {
                        if let Some(hub) = &current_hub {
                            send_server_frame(&out_tx, hub.release_writer(&client_id).await).await;
                        }
                    }
                    ClientFrame::SerialData { data } => {
                        if let Some(hub) = &current_hub {
                            match BASE64.decode(data) {
                                Ok(bytes) => send_server_frame(&out_tx, hub.write_serial(&client_id, &bytes).await).await,
                                Err(error) => send_server_frame(&out_tx, ServerFrame::Error { message: format!("invalid serial_data payload: {error}") }).await,
                            }
                        } else {
                            send_server_frame(&out_tx, ServerFrame::Error { message: "open a serial port before writing".to_string() }).await;
                        }
                    }
                    ClientFrame::Ping => send_server_frame(&out_tx, ServerFrame::Pong).await,
                }
            }
            event = recv_hub_event(&mut hub_events), if hub_events.is_some() => {
                match event {
                    Ok(frame) => send_server_frame(&out_tx, frame).await,
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        send_server_frame(&out_tx, ServerFrame::Status {
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
    Ok(())
}

async fn recv_hub_event(
    receiver: &mut Option<broadcast::Receiver<ServerFrame>>,
) -> Result<ServerFrame, broadcast::error::RecvError> {
    receiver.as_mut().expect("receiver exists").recv().await
}

async fn get_or_open_hub(
    state: &ServerState,
    config: SerialConfig,
) -> Result<Arc<SerialHub>, String> {
    let key = config.service_key();
    let mut hubs = state.hubs.lock().await;
    if let Some(hub) = hubs.get(&key) {
        return Ok(Arc::clone(hub));
    }

    let hub = SerialHub::open(config)?;
    hubs.insert(key, Arc::clone(&hub));
    Ok(hub)
}

async fn send_server_frame(sender: &mpsc::Sender<ServerFrame>, frame: ServerFrame) {
    let _ = sender.send(frame).await;
}

fn parse_client_frame(message: Message) -> Result<ClientFrame, String> {
    match message {
        Message::Text(text) => serde_json::from_str(&text).map_err(|error| error.to_string()),
        Message::Binary(bytes) => serde_json::from_slice(&bytes).map_err(|error| error.to_string()),
        Message::Ping(_) | Message::Pong(_) => Ok(ClientFrame::Ping),
        Message::Close(_) => Err("client closed connection".to_string()),
        Message::Frame(_) => Err("unexpected raw websocket frame".to_string()),
    }
}

fn parse_server_frame(message: Message) -> Result<ServerFrame, String> {
    match message {
        Message::Text(text) => serde_json::from_str(&text).map_err(|error| error.to_string()),
        Message::Binary(bytes) => serde_json::from_slice(&bytes).map_err(|error| error.to_string()),
        Message::Ping(_) | Message::Pong(_) => Ok(ServerFrame::Pong),
        Message::Close(_) => Err("server closed connection".to_string()),
        Message::Frame(_) => Err("unexpected raw websocket frame".to_string()),
    }
}

fn handle_remote_frame(app: &AppHandle, frame: ServerFrame, client_id: &mut Option<String>) {
    match frame {
        ServerFrame::AuthOk => emit_remote(
            app,
            DesktopEvent::Status {
                message: "authenticated".to_string(),
            },
        ),
        ServerFrame::AuthFailed { reason } => emit_remote(
            app,
            DesktopEvent::Error {
                message: format!("authentication failed: {reason}"),
            },
        ),
        ServerFrame::Ports { ports } => emit_remote(
            app,
            DesktopEvent::Status {
                message: format!("server ports: {}", ports.len()),
            },
        ),
        ServerFrame::Opened { config } => emit_remote(
            app,
            DesktopEvent::Status {
                message: format!("opened {} at {} baud", config.port, config.baud_rate),
            },
        ),
        ServerFrame::SerialData { data, .. } => match BASE64.decode(data) {
            Ok(bytes) => emit_remote(
                app,
                DesktopEvent::SerialData {
                    text: String::from_utf8_lossy(&bytes).to_string(),
                    hex: bytes
                        .iter()
                        .map(|byte| format!("{byte:02X}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                },
            ),
            Err(error) => emit_remote(
                app,
                DesktopEvent::Error {
                    message: format!("invalid serial data: {error}"),
                },
            ),
        },
        ServerFrame::Status { status } => {
            if let Status::Connected { client_id: id } = &status {
                *client_id = Some(id.clone());
            }
            emit_remote(
                app,
                DesktopEvent::Status {
                    message: format!("{status:?}"),
                },
            );
        }
        ServerFrame::Pong => {}
        ServerFrame::Error { message } => emit_remote(app, DesktopEvent::Error { message }),
    }
}

fn emit_remote(app: &AppHandle, event: DesktopEvent) {
    let _ = app.emit("remote-serial-event", event);
}

fn main() {
    tauri::Builder::default()
        .manage(DesktopState::default())
        .invoke_handler(tauri::generate_handler![
            app_status,
            runtime_status,
            default_serial_config,
            verify_token,
            list_serial_ports,
            start_local_server,
            stop_local_server,
            connect_remote_serial,
            disconnect_remote_serial,
            claim_remote_writer,
            release_remote_writer,
            send_remote_serial_text
        ])
        .run(tauri::generate_context!())
        .expect("failed to run UartRemote desktop app");
}
