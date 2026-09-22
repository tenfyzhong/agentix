import { runHook } from "../runtime.mjs";
import { reportHookError } from "./error-log.mjs";

let event;
try {
    let input = "";
    for await (const chunk of process.stdin) input += chunk;
    event = JSON.parse(input);
    const result = await runHook(event);
    process.stdout.write(`${JSON.stringify(result)}\n`);
} catch (error) {
    process.exitCode = 1;
    await reportHookError(error, event);
}
