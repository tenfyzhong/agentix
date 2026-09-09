import { Type } from "@sinclair/typebox";
import { registerExtension } from "../runtime.mjs";

export default function taskManager(api) {
    registerExtension(
        api,
        "pi",
        undefined,
        undefined,
        Type.Object({ args: Type.Array(Type.String()) }),
    );
}
