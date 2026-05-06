import { parentPort, workerData } from "worker_threads";

export class ActivityHost {
  static async create(viewPath: string): Promise<ActivityHost> {
    if (!parentPort)
      throw new Error(
        "parentPort was not found. This code may not be running inside a worker thread.",
      );
    parentPort.postMessage({
      type: "createActivity",
      pane: workerData.pane,
    });
    parentPort.once("");
    parentPort.on("message", (m) => {});
  }
}
