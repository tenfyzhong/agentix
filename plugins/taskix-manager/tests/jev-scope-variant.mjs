// Research-only request transformation. The live runner still uses production
// answer validation, confidence gates and revision checks without modification.
export function scopeReplayRequest(request) {
    const result = structuredClone(request);
    result.state.candidate_scope = {
        complete: true,
        eligible_statuses: ["ACTIVE", "PENDING_REVIEW"],
        excluded_statuses: ["COMPLETED", "CANCELLED"],
    };
    result.questions.route.instructions += " The eligible candidate list is complete. Only listed ACTIVE or PENDING_REVIEW Jobs can own this request. COMPLETED or CANCELLED Jobs cannot be resumed: later work needs a new Job even if the conversation continues their topic. Distinguish no eligible matching Job from an unresolved referent; choose uncertain when the request itself cannot be understood from the dialogue.";
    result.questions.route.criteria.new_job = "The current message does not belong to any listed eligible Job. This includes independent work, standalone conversation, or later work about an already completed or cancelled delivery. Sharing tools or a repository is not the same delivery.";
    return result;
}
