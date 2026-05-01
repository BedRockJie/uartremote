import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./styles.css";

type SerialPortInfo = {
  port_name: string;
  port_type: string;
};

type SerialConfig = {
  port: string;
  baud_rate: number;
  data_bits: "five" | "six" | "seven" | "eight";
  stop_bits: "one" | "two";
  parity: "none" | "odd" | "even";
};

type AppStatus = {
  app_name: string;
  version: string;
  core_ready: boolean;
  server_embedded: boolean;
};

type RuntimeStatus = {
  local_server: string | null;
  remote_client: string | null;
};

type RemoteEvent =
  | { kind: "status"; message: string }
  | { kind: "serial_data"; text: string; hex: string }
  | { kind: "error"; message: string };

type SavedCommand = {
  id: string;
  name: string;
  payload: string;
};

const COMMANDS_KEY = "uartremote.savedCommands";

const app = document.querySelector<HTMLDivElement>("#app");

if (!app) {
  throw new Error("missing #app root");
}

app.innerHTML = `
  <main class="shell">
    <header class="topbar">
      <div>
        <h1>UartRemote</h1>
        <p>同一个桌面端支持共享本机串口，也支持连接远端串口服务</p>
      </div>
      <button id="refreshPorts" type="button">刷新串口</button>
    </header>

    <section class="grid">
      <section class="panel">
        <h2>运行状态</h2>
        <dl id="statusList" class="kv"></dl>
      </section>

      <section class="panel">
        <h2>本机串口</h2>
        <div id="portsBox" class="list muted">未加载</div>
      </section>

      <section class="panel">
        <h2>共享本机串口</h2>
        <label class="field">
          <span>监听地址</span>
          <input id="localBind" value="127.0.0.1:9001" />
        </label>
        <label class="field">
          <span>Token</span>
          <input id="localToken" type="password" value="dev-token" />
        </label>
        <div class="actions">
          <button id="startServer" type="button">启动共享</button>
          <button id="stopServer" type="button" class="secondary">停止共享</button>
        </div>
      </section>

      <section class="panel">
        <h2>连接远端串口</h2>
        <label class="field">
          <span>服务地址</span>
          <input id="remoteUrl" value="ws://127.0.0.1:9001" />
        </label>
        <label class="field">
          <span>Token</span>
          <input id="remoteToken" type="password" value="dev-token" />
        </label>
        <div class="row">
          <label class="field">
            <span>串口</span>
            <input id="remotePort" value="COM3" />
          </label>
          <label class="field">
            <span>波特率</span>
            <input id="remoteBaud" type="number" value="115200" />
          </label>
        </div>
        <label class="check">
          <input id="claimWriter" type="checkbox" checked />
          <span>连接后申请写权限</span>
        </label>
        <div class="actions">
          <button id="connectRemote" type="button">连接远端</button>
          <button id="disconnectRemote" type="button" class="secondary">断开</button>
          <button id="claimRemoteWriter" type="button" class="secondary">申请写权限</button>
          <button id="releaseRemoteWriter" type="button" class="secondary">释放写权限</button>
        </div>
      </section>

      <section class="panel wide">
        <h2>远端串口收发</h2>
        <div class="send-grid">
          <div>
            <label class="field">
              <span>发送文本</span>
              <textarea id="sendText" rows="4"></textarea>
            </label>
            <div class="actions">
              <button id="sendRemote" type="button">发送</button>
              <button id="saveCommand" type="button" class="secondary">保存为命令</button>
            </div>
          </div>
          <div>
            <label class="field">
              <span>命令名称</span>
              <input id="commandName" value="AT" />
            </label>
            <div id="savedCommands" class="command-list muted">暂无保存命令</div>
          </div>
        </div>
      </section>

      <section class="panel wide">
        <h2>串口输出</h2>
        <pre id="serialOutput" class="log serial-log">等待串口数据</pre>
      </section>

      <section class="panel wide">
        <h2>运行日志</h2>
        <pre id="appLog" class="log app-log">等待操作</pre>
      </section>
    </section>
  </main>
`;

const ui = {
  refreshButton: must<HTMLButtonElement>("#refreshPorts"),
  statusList: must<HTMLElement>("#statusList"),
  portsBox: must<HTMLElement>("#portsBox"),
  localBind: must<HTMLInputElement>("#localBind"),
  localToken: must<HTMLInputElement>("#localToken"),
  startServer: must<HTMLButtonElement>("#startServer"),
  stopServer: must<HTMLButtonElement>("#stopServer"),
  remoteUrl: must<HTMLInputElement>("#remoteUrl"),
  remoteToken: must<HTMLInputElement>("#remoteToken"),
  remotePort: must<HTMLInputElement>("#remotePort"),
  remoteBaud: must<HTMLInputElement>("#remoteBaud"),
  claimWriter: must<HTMLInputElement>("#claimWriter"),
  connectRemote: must<HTMLButtonElement>("#connectRemote"),
  disconnectRemote: must<HTMLButtonElement>("#disconnectRemote"),
  claimRemoteWriter: must<HTMLButtonElement>("#claimRemoteWriter"),
  releaseRemoteWriter: must<HTMLButtonElement>("#releaseRemoteWriter"),
  sendText: must<HTMLTextAreaElement>("#sendText"),
  sendRemote: must<HTMLButtonElement>("#sendRemote"),
  saveCommand: must<HTMLButtonElement>("#saveCommand"),
  commandName: must<HTMLInputElement>("#commandName"),
  savedCommands: must<HTMLElement>("#savedCommands"),
  serialOutput: must<HTMLElement>("#serialOutput"),
  appLog: must<HTMLElement>("#appLog"),
};

function must<T extends Element>(selector: string): T {
  const element = document.querySelector<T>(selector);
  if (!element) {
    throw new Error(`missing ${selector}`);
  }
  return element;
}

function renderKv(target: HTMLElement, rows: Record<string, string | number | boolean>): void {
  target.innerHTML = Object.entries(rows)
    .map(([key, value]) => `<dt>${key}</dt><dd>${String(value)}</dd>`)
    .join("");
}

function appendAppLog(message: string): void {
  appendToPre(ui.appLog, message, "等待操作");
}

function appendSerialOutput(message: string): void {
  appendToPre(ui.serialOutput, message, "等待串口数据");
}

function appendToPre(target: HTMLElement, message: string, placeholder: string): void {
  const current = target.textContent === placeholder ? "" : target.textContent ?? "";
  target.textContent = `${current}${message}\n`;
  target.scrollTop = target.scrollHeight;
}

function remoteConfig(): SerialConfig {
  return {
    port: ui.remotePort.value.trim(),
    baud_rate: Number(ui.remoteBaud.value),
    data_bits: "eight",
    stop_bits: "one",
    parity: "none",
  };
}

async function loadStatus(): Promise<void> {
  const [status, runtime] = await Promise.all([
    invoke<AppStatus>("app_status"),
    invoke<RuntimeStatus>("runtime_status"),
  ]);
  renderKv(ui.statusList, {
    名称: status.app_name,
    版本: status.version,
    Core可用: status.core_ready,
    内嵌Server: status.server_embedded,
    本机共享: runtime.local_server ?? "未启动",
    远端连接: runtime.remote_client ?? "未连接",
  });
}

async function loadPorts(): Promise<void> {
  ui.portsBox.textContent = "加载中...";
  try {
    const ports = await invoke<SerialPortInfo[]>("list_serial_ports");
    if (ports.length === 0) {
      ui.portsBox.innerHTML = `<p class="muted">未发现串口设备</p>`;
      return;
    }

    ui.portsBox.innerHTML = ports
      .map(
        (port) => `
          <article class="port">
            <strong>${port.port_name}</strong>
            <span>${port.port_type}</span>
          </article>
        `,
      )
      .join("");
  } catch (error) {
    ui.portsBox.innerHTML = `<p class="error">${String(error)}</p>`;
  }
}

async function startServer(): Promise<void> {
  await invoke("start_local_server", {
    bind: ui.localBind.value.trim(),
    token: ui.localToken.value,
  });
  appendAppLog(`[local] server started at ${ui.localBind.value.trim()}`);
  await loadStatus();
}

async function stopServer(): Promise<void> {
  await invoke("stop_local_server");
  appendAppLog("[local] server stopped");
  await loadStatus();
}

async function connectRemote(): Promise<void> {
  await invoke("connect_remote_serial", {
    url: ui.remoteUrl.value.trim(),
    token: ui.remoteToken.value,
    config: remoteConfig(),
    claimWriter: ui.claimWriter.checked,
  });
  appendAppLog(`[remote] connecting ${ui.remoteUrl.value.trim()}`);
  await loadStatus();
}

async function disconnectRemote(): Promise<void> {
  await invoke("disconnect_remote_serial");
  appendAppLog("[remote] disconnected");
  await loadStatus();
}

async function runCommand(action: () => Promise<void>): Promise<void> {
  try {
    await action();
  } catch (error) {
    appendAppLog(`[error] ${String(error)}`);
  }
}

function loadSavedCommands(): SavedCommand[] {
  const raw = localStorage.getItem(COMMANDS_KEY);
  if (!raw) {
    return [];
  }

  try {
    const parsed = JSON.parse(raw) as SavedCommand[];
    return parsed.filter(
      (item) =>
        typeof item.id === "string" &&
        typeof item.name === "string" &&
        typeof item.payload === "string",
    );
  } catch {
    return [];
  }
}

function saveSavedCommands(commands: SavedCommand[]): void {
  localStorage.setItem(COMMANDS_KEY, JSON.stringify(commands));
}

function renderSavedCommands(): void {
  const commands = loadSavedCommands();
  if (commands.length === 0) {
    ui.savedCommands.innerHTML = `<p class="muted">暂无保存命令</p>`;
    return;
  }

  ui.savedCommands.innerHTML = commands
    .map(
      (command) => `
        <article class="saved-command">
          <button type="button" class="command-send" data-command-id="${command.id}">
            <strong>${escapeHtml(command.name)}</strong>
            <span>${escapeHtml(command.payload)}</span>
          </button>
          <button type="button" class="command-delete" data-command-id="${command.id}">删除</button>
        </article>
      `,
    )
    .join("");
}

function saveCurrentCommand(): void {
  const payload = ui.sendText.value;
  const name = ui.commandName.value.trim() || payload.slice(0, 20) || "未命名命令";
  if (!payload) {
    appendAppLog("[command] empty payload was not saved");
    return;
  }

  const commands = loadSavedCommands();
  commands.push({
    id: crypto.randomUUID(),
    name,
    payload,
  });
  saveSavedCommands(commands);
  renderSavedCommands();
  appendAppLog(`[command] saved ${name}`);
}

async function sendTextPayload(payload: string): Promise<void> {
  await invoke("send_remote_serial_text", { text: payload });
  appendAppLog(`[tx] ${payload}`);
}

function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}

ui.refreshButton.addEventListener("click", () => {
  void runCommand(loadPorts);
});

ui.startServer.addEventListener("click", () => {
  void runCommand(startServer);
});

ui.stopServer.addEventListener("click", () => {
  void runCommand(stopServer);
});

ui.connectRemote.addEventListener("click", () => {
  void runCommand(connectRemote);
});

ui.disconnectRemote.addEventListener("click", () => {
  void runCommand(disconnectRemote);
});

ui.claimRemoteWriter.addEventListener("click", () => {
  void runCommand(async () => {
    await invoke("claim_remote_writer");
  });
});

ui.releaseRemoteWriter.addEventListener("click", () => {
  void runCommand(async () => {
    await invoke("release_remote_writer");
  });
});

ui.sendRemote.addEventListener("click", () => {
  void runCommand(async () => {
    await sendTextPayload(ui.sendText.value);
  });
});

ui.saveCommand.addEventListener("click", () => {
  saveCurrentCommand();
});

ui.savedCommands.addEventListener("click", (event) => {
  const target = event.target;
  if (!(target instanceof HTMLElement)) {
    return;
  }

  const deleteButton = target.closest<HTMLButtonElement>(".command-delete");
  if (deleteButton) {
    const id = deleteButton.dataset.commandId;
    saveSavedCommands(loadSavedCommands().filter((command) => command.id !== id));
    renderSavedCommands();
    appendAppLog("[command] deleted");
    return;
  }

  const sendButton = target.closest<HTMLButtonElement>(".command-send");
  if (sendButton) {
    const id = sendButton.dataset.commandId;
    const command = loadSavedCommands().find((item) => item.id === id);
    if (command) {
      ui.sendText.value = command.payload;
      void runCommand(async () => {
        await sendTextPayload(command.payload);
      });
    }
  }
});

void bootstrap();

async function bootstrap(): Promise<void> {
  await listen<RemoteEvent>("remote-serial-event", (event) => {
    const payload = event.payload;
    if (payload.kind === "serial_data") {
      appendSerialOutput(`[rx] ${payload.text}`);
      appendSerialOutput(`[hex] ${payload.hex}`);
      return;
    }
    appendAppLog(`[${payload.kind}] ${payload.message}`);
  });
  renderSavedCommands();
  await Promise.all([loadStatus(), loadPorts()]);
}
