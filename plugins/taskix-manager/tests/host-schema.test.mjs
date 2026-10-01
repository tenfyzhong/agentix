import test from "node:test";
import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";

const validationModule = process.env.TASKIX_TEST_PI_VALIDATION_MODULE;
test("plain_tool_schema_is_accepted_by_the_installed_pi_validator", { skip: !validationModule }, async () => {
    const { validateToolArguments } = await import(pathToFileURL(validationModule).href);
    for (const host of ["pi", "omp"]) {
        const { default: install } = await import(`../extensions/${host}.ts`);
        const tools = [];
        install({ on() {}, registerTool: tool => tools.push(tool) });
        const tool = tools[0];
        assert.deepEqual(validateToolArguments(tool, { name: "taskix", arguments: { args: ["context", "--json"] } }),
            { args: ["context", "--json"] });
        for (const args of [{}, { args: {} }, { args: [{}] }]) {
            assert.throws(() => validateToolArguments(tool, { name: "taskix", arguments: args }), /Validation failed/);
        }
    }
});
