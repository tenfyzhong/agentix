use super::{Cli, Reason, Service, mutate, resolve_project, response};
use agentix_task::{IssueFilter, IssueStatus};
use anyhow::Result;
use clap::{Args, Subcommand};
use serde_json::{Value, json};

#[derive(Args)]
pub struct IssueFields {
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub requirement: Option<String>,
    #[arg(long)]
    pub acceptance: Option<String>,
    #[arg(long)]
    pub notes: Option<String>,
    #[arg(long, conflicts_with = "unlink_milestone")]
    pub milestone: Option<String>,
    #[arg(long)]
    pub unlink_milestone: bool,
}

impl IssueFields {
    fn apply(&self, request: &mut Value) {
        for (key, value) in [
            ("title", &self.title),
            ("name", &self.name),
            ("requirement", &self.requirement),
            ("acceptance", &self.acceptance),
            ("notes", &self.notes),
            ("milestone", &self.milestone),
        ] {
            if let Some(value) = value {
                request[key] = json!(value);
            }
        }
        if self.unlink_milestone {
            request["milestone"] = Value::Null;
        }
    }
}

#[derive(Subcommand)]
pub enum IssueCommand {
    /// Create a project deliverable using the shared Issue document template.
    Create {
        #[command(flatten)]
        fields: IssueFields,
    },
    /// Edit requirement, acceptance criteria, notes, or Milestone association.
    Update {
        id: String,
        #[command(flatten)]
        fields: IssueFields,
    },
    /// List Issues with predicates applied before pagination.
    List {
        #[arg(long, value_parser = ["TODO", "ACTIVE", "PENDING_REVIEW", "COMPLETED", "CANCELLED", "ALL"])]
        status: Option<String>,
        #[arg(long)]
        milestone: Option<String>,
        #[arg(long)]
        archived: bool,
        #[arg(long)]
        after: Option<String>,
        #[arg(long, default_value_t = 32)]
        limit: i64,
    },
    /// Show a requirement, acceptance criteria, Milestone, and associated Jobs.
    Show { id: String },
    /// Mark a TODO Issue active without creating execution work.
    Start { id: String },
    /// Submit a deliverable after every associated Job is terminal.
    Submit { id: String },
    /// Record human acceptance of an Issue awaiting review.
    Approve { id: String },
    /// Return an Issue to ACTIVE with a human rejection reason.
    Reject(Reason),
    /// Cancel an Issue and revoke all related execution leases.
    Cancel { id: String },
    /// Reopen a terminal Issue while preserving all historical Jobs.
    Reopen { id: String },
    /// Archive a completed or cancelled Issue document.
    Archive { id: String },
    /// Restore an archived Issue without changing its lifecycle.
    Unarchive { id: String },
}

#[derive(Args)]
pub struct MilestoneFields {
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    goal: Option<String>,
    #[arg(long)]
    order: Option<i64>,
    #[arg(long, conflicts_with = "clear_due_date")]
    due_date: Option<String>,
    #[arg(long)]
    clear_due_date: bool,
}

impl MilestoneFields {
    fn apply(&self, request: &mut Value) {
        for (key, value) in [
            ("title", &self.title),
            ("name", &self.name),
            ("goal", &self.goal),
            ("due_date", &self.due_date),
        ] {
            if let Some(value) = value {
                request[key] = json!(value);
            }
        }
        if let Some(order) = self.order {
            request["order"] = json!(order);
        }
        if self.clear_due_date {
            request["due_date"] = Value::Null;
        }
    }
}

#[derive(Subcommand)]
pub enum MilestoneCommand {
    /// Create a basic Project Milestone.
    Create {
        #[command(flatten)]
        fields: MilestoneFields,
    },
    /// Edit a Milestone's title, goal, order, or optional due date.
    Update {
        id: String,
        #[command(flatten)]
        fields: MilestoneFields,
    },
    /// List Milestones in Project order.
    List,
    /// Show a Milestone and Issue acceptance counts.
    Show { id: String },
    /// Delete a Milestone after unlinking or reassigning its Issues.
    Delete { id: String },
}

pub async fn issue(cli: &Cli, service: &Service, command: &IssueCommand) -> Result<Value> {
    let request = match command {
        IssueCommand::Create { fields } => {
            let mut request =
                json!({"command":"issue.create","project":resolve_project(cli,service).await?});
            fields.apply(&mut request);
            request
        }
        IssueCommand::Update { id, fields } => {
            let mut request = json!({"command":"issue.update","issue":id});
            fields.apply(&mut request);
            request
        }
        IssueCommand::List {
            status,
            milestone,
            archived,
            after,
            limit,
        } => {
            let filter = IssueFilter {
                status: status
                    .as_deref()
                    .filter(|status| *status != "ALL")
                    .map(|status| serde_json::from_value::<IssueStatus>(json!(status)))
                    .transpose()?,
                milestone_id: milestone.clone(),
                archived: Some(*archived),
            };
            return Ok(response(
                service
                    .store()
                    .issue_page(cli.project.as_deref(), &filter, after.as_deref(), *limit)
                    .await?,
            ));
        }
        IssueCommand::Show { id } => return Ok(response(service.store().issue_detail(id).await?)),
        IssueCommand::Start { id } => json!({"command":"issue.start","issue":id}),
        IssueCommand::Submit { id } => json!({"command":"issue.submit","issue":id}),
        IssueCommand::Approve { id } => json!({"command":"issue.approve","issue":id}),
        IssueCommand::Reject(reason) => {
            json!({"command":"issue.reject","issue":reason.id,"reason":reason.reason})
        }
        IssueCommand::Cancel { id } => json!({"command":"issue.cancel","issue":id}),
        IssueCommand::Reopen { id } => json!({"command":"issue.reopen","issue":id}),
        IssueCommand::Archive { id } => json!({"command":"issue.archive","issue":id}),
        IssueCommand::Unarchive { id } => json!({"command":"issue.unarchive","issue":id}),
    };
    mutate(cli, service, request).await
}

pub async fn milestone(cli: &Cli, service: &Service, command: &MilestoneCommand) -> Result<Value> {
    let request = match command {
        MilestoneCommand::Create { fields } => {
            let mut request =
                json!({"command":"milestone.create","project":resolve_project(cli,service).await?});
            fields.apply(&mut request);
            request
        }
        MilestoneCommand::Update { id, fields } => {
            let mut request = json!({"command":"milestone.update","milestone":id});
            fields.apply(&mut request);
            request
        }
        MilestoneCommand::List => {
            return Ok(response(json!(
                service.store().milestones(cli.project.as_deref()).await?
            )));
        }
        MilestoneCommand::Show { id } => {
            return Ok(response(service.store().milestone_detail(id).await?));
        }
        MilestoneCommand::Delete { id } => json!({"command":"milestone.delete","milestone":id}),
    };
    mutate(cli, service, request).await
}
