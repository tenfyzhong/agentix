use agentix_task::{InboxStatus, JobStatus, TaskPhase, TaskStatus};

pub(super) const fn job_status_label(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Active => "Active",
        JobStatus::PendingReview => "Pending review",
        JobStatus::Completed => "Completed",
        JobStatus::Cancelled => "Cancelled",
    }
}

pub(super) const fn inbox_status_label(status: InboxStatus) -> &'static str {
    match status {
        InboxStatus::Todo => task_status_label(TaskStatus::Todo),
        InboxStatus::Active => job_status_label(JobStatus::Active),
        InboxStatus::PendingReview => job_status_label(JobStatus::PendingReview),
        InboxStatus::Completed => job_status_label(JobStatus::Completed),
        InboxStatus::Cancelled => job_status_label(JobStatus::Cancelled),
    }
}

pub(super) const fn task_status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Todo => "Todo",
        TaskStatus::InProgress => "In progress",
        TaskStatus::Blocked => "Blocked",
        TaskStatus::WaitingUser => "Waiting user",
        TaskStatus::Done => "Done",
        TaskStatus::Failed => "Failed",
        TaskStatus::Cancelled => job_status_label(JobStatus::Cancelled),
    }
}

pub(super) const fn task_phase_label(phase: TaskPhase) -> &'static str {
    match phase {
        TaskPhase::Planning => "Planning",
        TaskPhase::Executing => "Executing",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_preserves_words(raw: impl std::fmt::Display, label: &str) {
        assert_eq!(
            label.replace(' ', "_").to_ascii_uppercase(),
            raw.to_string()
        );
        assert!(label.starts_with(|c: char| c.is_ascii_uppercase()));
        assert!(
            label
                .chars()
                .skip(1)
                .all(|c| c.is_ascii_lowercase() || c == ' ')
        );
    }

    #[test]
    fn job_labels_preserve_original_words_with_sentence_case() {
        for status in JobStatus::ALL {
            assert_preserves_words(status, job_status_label(status));
        }
    }

    #[test]
    fn inbox_labels_preserve_original_words_with_sentence_case() {
        for status in [
            InboxStatus::Todo,
            InboxStatus::Active,
            InboxStatus::PendingReview,
            InboxStatus::Completed,
            InboxStatus::Cancelled,
        ] {
            assert_preserves_words(status, inbox_status_label(status));
        }
    }

    #[test]
    fn task_labels_preserve_original_words_with_sentence_case() {
        for status in TaskStatus::ALL {
            assert_preserves_words(status, task_status_label(status));
        }
        for phase in [TaskPhase::Planning, TaskPhase::Executing] {
            assert_preserves_words(phase, task_phase_label(phase));
        }
    }
}
