import { writeFile } from "node:fs/promises";
import { ReadTool, settings } from "@oh-my-pi/pi-coding-agent";

// Run the installed host's actual Read tool without making a model request.
export default function readSkillProbe(api) {
    api.on("session_start", async (_event, ctx) => {
        const results = [];
        const read = new ReadTool({
            cwd: ctx.cwd,
            hasUI: false,
            settings,
            getSessionFile: () => ctx.sessionManager.getSessionFile(),
        });
        for (const path of ["skill://taskix-manager", "skill://taskix-manager/references/commands.md"]) {
            try {
                results.push({ path, result: await read.execute("skill-probe", { path }) });
            } catch (error) {
                results.push({ path, error: String(error) });
            }
        }
        await writeFile(process.env.TASKIX_TEST_READ_RESULTS, JSON.stringify(results));
    });
}
