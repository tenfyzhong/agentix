// Construct complete provider distributions so protocol additions cannot leave
// stale hard-coded probability keys in transport and CLI integration fixtures.
export function choiceAnswers(request, choices) {
    return { answers: Object.fromEntries(Object.entries(request.questions).map(([id, question]) => {
        const choice = choices[id] ?? (id === "review_policy" ? "required" : "unrelated");
        return [id, { type: "choice", choice, confidence: 1,
            probabilities: Object.fromEntries(Object.keys(question.criteria).map(key => [key, Number(key === choice)])) }];
    })) };
}
