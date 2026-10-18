import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, rename, rm } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";

const [action, branch] = process.argv.slice(2);
const parent = join(process.env.XDG_CACHE_HOME || join(homedir(), ".cache"), "agentix");
const destination = join(parent, "omp-marketplace");
if (action === "remove") {
    await rm(destination, { recursive: true, force: true });
} else if (action === "install" && branch) {
    await mkdir(parent, { recursive: true });
    const stage = await mkdtemp(join(parent, "omp-install-"));
    try {
        // OMP has no marketplace ref option. Keep a remote-only checkout for its
        // relative plugin sources and subsequent marketplace operations.
        execFileSync("git", ["clone", "--depth", "1", "--single-branch", "--branch", branch,
            "--", "https://github.com/tenfyzhong/agentix.git", join(stage, "repository")],
        { stdio: "inherit", timeout: 180000 });
        await rm(destination, { recursive: true, force: true });
        await rename(join(stage, "repository"), destination);
        for (const args of [
            ["plugin", "marketplace", "add", destination],
            ["plugin", "install", "taskix-manager@agentix"],
            ["plugin", "install", "agentix-bridge@agentix"],
        ]) execFileSync(process.env.OMP || "omp", args, { stdio: "inherit", timeout: 180000 });
    } finally {
        await rm(stage, { recursive: true, force: true });
    }
} else {
    throw new Error("Usage: omp-plugin.mjs install BRANCH | remove");
}
