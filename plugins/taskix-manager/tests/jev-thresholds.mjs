// Offline sensitivity analysis of recorded native scores. This is not semantic
// accuracy and does not establish that later live revision/assignment guards pass.
export function thresholdProjection(results, threshold) {
    if (!Number.isFinite(threshold) || threshold < .5 || threshold > 1) throw new RangeError("Invalid threshold");
    const accepted = results.filter(result => {
        if (!result.accepted && result.decision?.reason !== "uncertain_or_conflicting") return false;
        const answers = result.answers || [];
        const questions = new Set(answers.map(a => a.question));
        if (!questions.has("intent") || !questions.has("route") || questions.size !== answers.length) return false;
        return answers.every(a => a.valid === 1 && a.choice && a.choice !== "uncertain" &&
            Number.isFinite(a.confidence) && a.confidence >= threshold && a.confidence <= 1 &&
            Number.isFinite(a.probability) && a.probability >= threshold && a.probability <= 1 &&
            Number.isFinite(a.margin) && a.margin >= .2);
    });
    return { threshold, total: results.length, accepted: accepted.length,
        accepted_percent: results.length ? 100 * accepted.length / results.length : 0,
        newly_accepted_ids: accepted.filter(r => !r.accepted).map(r => r.id) };
}
