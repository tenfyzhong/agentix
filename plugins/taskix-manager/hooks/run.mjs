import { runHook } from "../runtime.mjs";

try {
    let input = "";
    for await (const chunk of process.stdin) input += chunk;
    const result = await runHook(JSON.parse(input));
    process.stdout.write(`${JSON.stringify(result)}\n`);
} catch (error) {
    process.stderr.write(`Task board hook: ${error.message}\n`);
    process.exitCode = 1;
}
