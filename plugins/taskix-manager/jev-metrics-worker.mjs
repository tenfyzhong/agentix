import { parentPort, workerData } from "node:worker_threads";
import { writeMetric } from "./jev-metrics.mjs";

parentPort.postMessage(await writeMetric(workerData));
