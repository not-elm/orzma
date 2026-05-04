import { workerData } from "node:worker_threads";

try {
  const { scriptPath } = workerData;
  console.log("scriptPath", scriptPath);
  await import(scriptPath);
} catch (e) {
  console.error("worker error", e);
  process.exit(1);
}
