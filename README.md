# UartRemote

## 中文

UartRemote 是一个 Rust 优先的远程串口系统。当前阶段提供可复用核心库、WebSocket 串口服务端、CLI 远程终端客户端，以及一个 TypeScript + Tauri 桌面端。桌面端是单一 APP，不拆成服务端版和客户端版：它既可以启动本机共享服务，也可以连接远端串口服务。

### 仓库架构

这是一个 Rust workspace，主要模块如下：

- `core`：共享核心库。包含协议帧、Token 认证、串口枚举/打开能力、串口配置结构、写权限状态机。CLI、server、Tauri 后端都应优先复用这里的能力。
- `server`：WebSocket 串口服务端。负责监听连接、校验 Token、打开本机串口、广播串口读取数据、管理单 client 独占写权限。
- `client`：CLI 远程串口客户端。负责连接 server、认证、打开远程串口、申请写权限、将终端输入写入远端串口，并把串口输出打印到终端。
- `desktop`：Tauri 桌面端。前端使用 TypeScript + Vite；后端位于 `desktop/src-tauri`，作为 workspace 成员复用 `core`。当前已打通应用状态、串口枚举、启动/停止本机共享服务、连接/断开远端串口、申请/释放写权限、发送文本和接收串口事件。

### 运行 CLI Server

```powershell
$env:UART_REMOTE_TOKEN="dev-token"
cargo run -p uart-remote-server -- --bind 127.0.0.1:9001
```

### 运行 CLI Client

```powershell
$env:UART_REMOTE_TOKEN="dev-token"
cargo run -p uart-remote-client -- --url ws://127.0.0.1:9001 --port COM3 --baud-rate 115200
```

默认串口参数是 `115200 8N1`。可以通过 `--data-bits`、`--stop-bits`、`--parity` 覆盖。

只读观察模式：

```powershell
cargo run -p uart-remote-client -- --url ws://127.0.0.1:9001 --token dev-token --port COM3 --read-only
```

### 运行桌面端

```powershell
cd desktop
npm install
npm run tauri:dev
```

桌面端包含两个基础工作区：

- 共享本机串口：输入监听地址和 Token，启动内嵌 WebSocket server，让其他客户端连接本机串口。
- 连接远端串口：输入远端 WebSocket 地址、Token、串口名和波特率，连接远端 server 后收发串口数据。
- 自定义串口命令：在发送区保存常用文本命令，之后可以一键发送；运行日志和串口输出分开展示。

只验证前端构建：

```powershell
cd desktop
npm run build
```

### 协议

WebSocket payload 使用 JSON。串口二进制数据在 `serial_data` 帧中使用 base64 编码。

Client frames：

- `auth`：静态 Token 认证。
- `open`：选择串口和串口参数。
- `claim_writer`：申请独占写权限。
- `release_writer`：释放独占写权限。
- `serial_data`：写入远端串口的数据。
- `ping`：应用层心跳。

Server frames：

- `auth_ok` / `auth_failed`：认证结果。
- `ports`：server 可见的本机串口列表。
- `opened`：串口打开成功。
- `serial_data`：从串口读到并广播给订阅者的数据。
- `status`：连接、串口、写权限和错误状态。
- `pong` / `error`：心跳响应和错误信息。

### 并发模型

每个打开的串口配置对应一个 server-side hub。多个 client 可以同时订阅读取数据；同一时间只有一个 client 可以持有写权限。持有写权限的 client 断开后，server 会自动释放写权限。

### 验证

```powershell
cargo check
cargo test
cd desktop
npm run build
```

## English

UartRemote is a Rust-first remote serial-port system. The current milestone provides a reusable core library, a WebSocket serial server, a CLI terminal client, and a TypeScript + Tauri desktop app. The desktop app is a single product rather than separate server/client apps: it can start a local sharing server and it can also connect to a remote serial server.

### Repository Architecture

This repository is a Rust workspace with these main modules:

- `core`: Shared library. It contains protocol frames, token authentication, serial port enumeration/open helpers, serial configuration types, and the writer-permission state machine. CLI tools, the server, and the Tauri backend should reuse this crate first.
- `server`: WebSocket serial server. It accepts client connections, validates tokens, opens local serial ports, broadcasts serial read data, and enforces single-client exclusive write permission.
- `client`: CLI remote serial client. It connects to the server, authenticates, opens a remote serial port, claims write permission, forwards terminal input to the remote serial port, and prints serial output to the terminal.
- `desktop`: Tauri desktop app. The frontend uses TypeScript + Vite. The backend lives in `desktop/src-tauri`, is a workspace member, and reuses `core`. It currently wires app status, serial port listing, local sharing server start/stop, remote serial connect/disconnect, writer claim/release, text sending, and serial receive events.

### Run The CLI Server

```powershell
$env:UART_REMOTE_TOKEN="dev-token"
cargo run -p uart-remote-server -- --bind 127.0.0.1:9001
```

### Run The CLI Client

```powershell
$env:UART_REMOTE_TOKEN="dev-token"
cargo run -p uart-remote-client -- --url ws://127.0.0.1:9001 --port COM3 --baud-rate 115200
```

The default serial format is `115200 8N1`. Override it with `--data-bits`, `--stop-bits`, and `--parity`.

Read-only observation mode:

```powershell
cargo run -p uart-remote-client -- --url ws://127.0.0.1:9001 --token dev-token --port COM3 --read-only
```

### Run The Desktop App

```powershell
cd desktop
npm install
npm run tauri:dev
```

The desktop app has two basic work areas:

- Share local serial ports: enter a bind address and token, then start the embedded WebSocket server so other clients can connect to local serial ports.
- Connect to a remote serial port: enter the remote WebSocket URL, token, serial port name, and baud rate, then connect and exchange serial data.
- Custom serial commands: save frequently used text commands in the send panel and send them with one click. Runtime logs and serial output are displayed separately.

Frontend-only build check:

```powershell
cd desktop
npm run build
```

### Protocol

WebSocket payloads are JSON. Binary serial data is base64-encoded inside `serial_data` frames.

Client frames:

- `auth`: static token authentication.
- `open`: select serial port and serial parameters.
- `claim_writer`: request exclusive write permission.
- `release_writer`: release exclusive write permission.
- `serial_data`: bytes to write to the remote serial port.
- `ping`: application-level heartbeat.

Server frames:

- `auth_ok` / `auth_failed`: authentication result.
- `ports`: serial ports visible to the server.
- `opened`: selected serial configuration was opened.
- `serial_data`: bytes read from the serial port and broadcast to subscribers.
- `status`: connection, serial, writer, and error state.
- `pong` / `error`: heartbeat response and error message.

### Concurrency Model

Each opened serial configuration has one server-side hub. Multiple clients may subscribe and receive broadcast read data. Only one client at a time may hold writer permission; writer ownership is released automatically when that client disconnects.

### Verification

```powershell
cargo check
cargo test
cd desktop
npm run build
```
