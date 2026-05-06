// import path from "path";
// import os from "node:os";
// import net from "node:net";
//
// export class OzmuxDaemonUdsClient {
//   private constructor(private readonly socket: net.Socket) {}
//
//   static async connect(): Promise<OzmuxDaemonUdsClient> {
//     return new Promise((resolve, reject) => {
//       const socketPath =
//         process.platform === "win32"
//           ? "\\\\.\\pipe\\ozmux-daemon.sock"
//           : path.join(os.tmpdir(), "ozmux-daemon.sock");
//
//       const socket = net.createConnection(socketPath, () => {
//         socket.removeAllListeners("error");
//         resolve(new OzmuxDaemonUdsClient(socket));
//       });
//       socket.once("error", (e) => {
//         reject(e);
//       });
//     });
//   }
// }
