import net from "node:net";
import os from "node:os";
import path from "node:path";

function getIpcPath(name: string): string {
  if (process.platform === "win32") {
    return `\\\\.\\pipe\\${name}`;
  }

  return path.join(os.tmpdir(), `${name}.sock`);
}

const socketPath = getIpcPath("ozmux-extension-host");

const server = net.createServer((socket) => {
  socket.on("data", (data) => {
    console.log("received:", data.toString());
    socket.write(`echo: ${data}`);
  });

  socket.on("end", () => {
    console.log("client disconnected");
  });
});

server.listen(socketPath, () => {
  console.log(`listening on ${socketPath}`);
});
