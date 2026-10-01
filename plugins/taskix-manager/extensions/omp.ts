import { registerExtension } from "../runtime.mjs";

export default function taskManager(api) {
    registerExtension(api, "omp");
}
