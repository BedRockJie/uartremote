import { invoke } from "@tauri-apps/api/core";
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

const app = document.querySelector<HTMLDivElement>("#app");

if (!app) {
  throw new Error("missing #app root");
}

app.innerHTML = `
  <main class="shell">
    <header class="topbar">
      <div>
        <h1>UartRemote</h1>
        <p>远程串口桌面端框架</p>
      </div>
      <button id="refreshPorts" type="button">刷新串口</button>
    </header>

    <section class="grid">
      <section class="panel">
        <h2>应用状态</h2>
        <dl id="statusList" class="kv"></dl>
      </section>

      <section class="panel">
        <h2>本机串口</h2>
        <div id="portsBox" class="list muted">未加载</div>
      </section>

      <section class="panel">
        <h2>默认串口配置</h2>
        <dl id="configList" class="kv"></dl>
      </section>

      <section class="panel">
        <h2>后端命令测试</h2>
        <label class="field">
          <span>Token</span>
          <input id="tokenInput" type="password" value="dev-token" />
        </label>
        <button id="verifyToken" type="button">调用 Token 校验</button>
        <pre id="commandLog" class="log">等待操作</pre>
      </section>
    </section>
  </main>
`;

const refreshButton = document.querySelector<HTMLButtonElement>("#refreshPorts");
const verifyButton = document.querySelector<HTMLButtonElement>("#verifyToken");
const tokenInput = document.querySelector<HTMLInputElement>("#tokenInput");
const statusList = document.querySelector<HTMLElement>("#statusList");
const configList = document.querySelector<HTMLElement>("#configList");
const portsBox = document.querySelector<HTMLElement>("#portsBox");
const commandLog = document.querySelector<HTMLElement>("#commandLog");

function requireElement<T extends Element>(value: T | null, name: string): T {
  if (!value) {
    throw new Error(`missing ${name}`);
  }
  return value;
}

const ui = {
  refreshButton: requireElement(refreshButton, "#refreshPorts"),
  verifyButton: requireElement(verifyButton, "#verifyToken"),
  tokenInput: requireElement(tokenInput, "#tokenInput"),
  statusList: requireElement(statusList, "#statusList"),
  configList: requireElement(configList, "#configList"),
  portsBox: requireElement(portsBox, "#portsBox"),
  commandLog: requireElement(commandLog, "#commandLog"),
};

function renderKv(target: HTMLElement, rows: Record<string, string | number | boolean>): void {
  target.innerHTML = Object.entries(rows)
    .map(([key, value]) => `<dt>${key}</dt><dd>${String(value)}</dd>`)
    .join("");
}

function writeLog(message: string): void {
  ui.commandLog.textContent = message;
}

async function loadStatus(): Promise<void> {
  const status = await invoke<AppStatus>("app_status");
  renderKv(ui.statusList, {
    名称: status.app_name,
    版本: status.version,
    Core可用: status.core_ready,
    内嵌Server: status.server_embedded,
  });
}

async function loadDefaultConfig(): Promise<void> {
  const config = await invoke<SerialConfig>("default_serial_config");
  renderKv(ui.configList, {
    端口: config.port || "未选择",
    波特率: config.baud_rate,
    数据位: config.data_bits,
    停止位: config.stop_bits,
    校验位: config.parity,
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

async function verifyToken(): Promise<void> {
  try {
    const ok = await invoke<boolean>("verify_token", {
      expected: "dev-token",
      provided: ui.tokenInput.value,
    });
    writeLog(ok ? "Token 校验通过" : "Token 校验失败");
  } catch (error) {
    writeLog(`命令失败: ${String(error)}`);
  }
}

ui.refreshButton.addEventListener("click", () => {
  void loadPorts();
});

ui.verifyButton.addEventListener("click", () => {
  void verifyToken();
});

void bootstrap();

async function bootstrap(): Promise<void> {
  await Promise.all([loadStatus(), loadDefaultConfig(), loadPorts()]);
}
