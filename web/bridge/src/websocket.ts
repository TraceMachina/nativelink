import type { ServerWebSocket } from "bun";

const clients = new Set<ServerWebSocket<unknown>>();

export const startWebSocket = () => {
  console.log('\nWebSocket server is running on ws://localhost:8080\n');
  Bun.serve({
  port: 8080,
  fetch(req, server) {
    // Upgrade the request to a WebSocket
    // Here we can also do the websocket auth/token auth
    if (server.upgrade(req)) {
      return;
    }
    return new Response("Upgrade failed", { status: 500 });
  },
  websocket: {
    open(ws) {
      console.log('New client connected');
      clients.add(ws);
      ws.send("Hello Web Client")
    },
    message(ws, message) {
      console.log('Received message from web client:', message);
    },
    close(ws) {
      console.log('Web Client disconnected');
      clients.delete(ws);
    },
    drain(ws) {
      console.log('Ready to receive more data');
    },
  },
});}

export function broadcastProgress(progress: string) {
  // Convert the string to a Uint8Array
  // const buffer = new TextEncoder().encode(progress);
  // console.log(progress)
  // console.log("----------------------------------------------")
  const buffer = Buffer.from(progress)

  for (const ws of clients) {
      ws.send(new Uint8Array(buffer)); // Send the ArrayBufferView
  }
}
