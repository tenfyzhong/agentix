// Provider identity is independent of the wire protocol. No lifecycle writes here.
const enabled = value => /^(true|1)$/i.test(value?.trim() || "");
const text = value => value?.trim() || undefined;
export const validProvider = value => typeof value === "string" && /^[a-z][a-z0-9_-]{0,63}$/.test(value);
export const validProtocol = value => ["typesafe", "openai_decisions"].includes(value);
export function metricsEnabled(env = process.env) {
    return enabled(env.TASKIX_DECISION_METRICS_ENABLED ?? env.TASKIX_JEV_METRICS_ENABLED);
}
export function metricsDatabase(env = process.env) {
    return text(env.TASKIX_DECISION_METRICS_DB) ?? text(env.TASKIX_JEV_METRICS_DB);
}
export function decisionConfig(env = process.env) {
    const provider = text(env.TASKIX_DECISION_PROVIDER) ?? "typesafe";
    const protocol = text(env.TASKIX_DECISION_PROTOCOL) ?? ({ typesafe: "typesafe", openai: "openai_decisions" })[provider];
    if (!validProvider(provider) || !validProtocol(protocol)) return;
    const legacy = provider === "typesafe" && protocol === "typesafe";
    const officialOpenAI = provider === "openai" && protocol === "openai_decisions";
    if (!enabled(env.TASKIX_DECISION_ENABLED ?? (legacy ? env.TASKIX_JEV_ENABLED : undefined))) return;
    const value = name => text(env[`TASKIX_DECISION_${name}`]) ?? (legacy ? text(env[`TASKIX_JEV_${name}`]) : undefined);
    const url = value("URL") ?? (officialOpenAI ? "https://api.openai.com/v1/decisions" : undefined);
    const key = value("API_KEY") ?? (officialOpenAI ? text(env.OPENAI_API_KEY) : undefined);
    const model = value("MODEL") ?? (legacy ? "jev-latest" : officialOpenAI ? "gpt-6-luna" : undefined);
    if (!url || !key || !model || /[^\x20-\x7e]/.test(key)) return;
    try {
        const parsed = new URL(url);
        if (!["http:", "https:"].includes(parsed.protocol) || !parsed.hostname || parsed.username || parsed.password) return;
    } catch { return; }
    const threshold = Number(value("MIN_CONFIDENCE") ?? "0.65");
    if (!Number.isFinite(threshold) || threshold < .5 || threshold > 1) return;
    return { provider, protocol, url, key, model, threshold };
}

export function encodeDecision(config, state, questions) {
    if (config.protocol === "typesafe") return JSON.stringify({ model: config.model, state, questions });
    const entries = Object.entries(questions);
    if (!entries.length || entries.length > 400) throw Error("Invalid decision question count");
    return JSON.stringify({ model: config.model, input: [{ role: "user", content: [{ type: "input_json", data: state }] }],
        questions: entries.map(([name, question]) => {
            const criteria = Object.entries(question.criteria);
            if (question.type !== "choice" || criteria.length < 2 || criteria.length > 255) throw Error("Invalid decision choices");
            return { name, type: "choice", instructions: question.instructions,
                choices: criteria.map(([value, description]) => ({ value, description: typeof description === "string" ? description : JSON.stringify(description) })) };
        }) });
}

export function decodeDecision(config, reply) {
    if (config.protocol === "typesafe") return reply;
    if (!Array.isArray(reply?.answers)) throw Error("Invalid decision answers");
    const answers = Object.create(null);
    for (const answer of reply.answers) {
        if (typeof answer?.name !== "string" || Object.hasOwn(answers, answer.name)) throw Error("Invalid decision answer name");
        // Refusals remain invalid answers for the caller's normal confidence gate.
        if (answer.type !== "choice") { answers[answer.name] = { type: answer.type }; continue; }
        if (!Array.isArray(answer.probabilities)) throw Error("Invalid decision distribution");
        const probabilities = Object.create(null);
        for (const item of answer.probabilities) {
            if (typeof item?.value !== "string" || Object.hasOwn(probabilities, item.value)) throw Error("Invalid decision probability key");
            probabilities[item.value] = item.probability;
        }
        answers[answer.name] = { type: "choice", choice: answer.choice, confidence: answer.confidence, probabilities };
    }
    return { answers };
}
