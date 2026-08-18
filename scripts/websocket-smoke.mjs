import WebSocket from "ws";

const baseUrl = process.env.MONITOR_BASE_URL;
const adminToken = process.env.MONITOR_ADMIN_TOKEN;
const agentToken = process.env.MONITOR_AGENT_TOKEN;

if (!baseUrl || !adminToken || !agentToken) {
  throw new Error("MONITOR_BASE_URL, MONITOR_ADMIN_TOKEN and MONITOR_AGENT_TOKEN are required");
}

function websocketUrl(path) {
  const url = new URL(path, baseUrl);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  return url.toString();
}

function openSocket(path, token) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(websocketUrl(path), {
      headers: { Authorization: `Bearer ${token}` },
    });
    const timer = setTimeout(() => {
      socket.terminate();
      reject(new Error(`${path} WebSocket handshake timed out`));
    }, 5_000);
    socket.once("open", () => {
      clearTimeout(timer);
      resolve(socket);
    });
    socket.once("unexpected-response", (_request, response) => {
      clearTimeout(timer);
      reject(new Error(`${path} WebSocket returned HTTP ${response.statusCode}`));
    });
    socket.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
  });
}

function waitForMessage(socket, expected) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`WebSocket did not receive ${expected}`)), 5_000);
    socket.once("message", (data) => {
      clearTimeout(timer);
      const message = data.toString();
      if (message !== expected) reject(new Error(`Expected ${expected}, received ${message}`));
      else resolve();
    });
    socket.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
  });
}

function closeSocket(socket) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      socket.terminate();
      reject(new Error("WebSocket close timed out"));
    }, 5_000);
    socket.once("close", () => {
      clearTimeout(timer);
      resolve();
    });
    socket.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    socket.close();
  });
}

const dashboard = await openSocket("/api/ws", adminToken);
const pong = waitForMessage(dashboard, "pong");
dashboard.send("ping");
await pong;
await closeSocket(dashboard);

const agent = await openSocket("/api/agent/live", agentToken);
await closeSocket(agent);
