use async_trait::async_trait;
use codex_protocol::plan_tool::UpdatePlanArgs;
use codex_protocol::protocol::SubAgentActivity;
use codex_protocol::protocol::SubAgentStatus;
use serde::Deserialize;
use serde::Serialize;

use crate::function_tool::FunctionCallError;
use crate::subagents::agents_md::SubAgentTemplate;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct SubAgentsHandler;

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum SubAgentsArgs {
    Spawn {
        template: String,
        task: String,
    },
    Poll {
        id: String,
        #[serde(default)]
        include_messages: bool,
    },
    Cancel {
        id: String,
    },
    List,
    ListTemplates,
}

#[derive(Debug, Serialize)]
struct SpawnResponse {
    id: String,
}

#[derive(Debug, Serialize)]
struct ListResponse {
    agents: Vec<ListEntry>,
}

#[derive(Debug, Serialize)]
struct ListTemplatesResponse {
    templates: Vec<TemplateEntry>,
}

#[derive(Debug, Serialize)]
struct ListEntry {
    id: String,
    template: String,
    status: SubAgentStatus,
    title: String,
    tool_uses: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_activity: Option<SubAgentActivity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_tokens: Option<i64>,
}

#[derive(Debug, Serialize)]
struct TemplateEntry {
    name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    instructions: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    skills: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
}

#[derive(Debug, Serialize)]
struct PollResponse {
    id: String,
    template: String,
    status: SubAgentStatus,
    title: String,
    tool_uses: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_activity: Option<SubAgentActivity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_tokens: Option<i64>,
    messages: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    plan_suggestions: Vec<UpdatePlanArgs>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct CancelResponse {
    canceled: bool,
}

fn template_entry(template: SubAgentTemplate) -> TemplateEntry {
    TemplateEntry {
        name: template.name,
        instructions: template.instructions,
        skills: template.skills,
        model: template.model,
    }
}

#[async_trait]
impl ToolHandler for SubAgentsHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            payload,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "subagents handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: SubAgentsArgs = serde_json::from_str(&arguments).map_err(|e| {
            FunctionCallError::RespondToModel(format!("failed to parse function arguments: {e}"))
        })?;

        let content = match args {
            SubAgentsArgs::Spawn { template, task } => {
                let id = session
                    .services
                    .subagents
                    .spawn(
                        template,
                        task,
                        turn.client.get_model(),
                        turn.client.get_reasoning_effort(),
                        turn.client.get_reasoning_summary(),
                        session.clone_original_config().await,
                        session.services.auth_manager.clone(),
                        session.services.models_manager.clone(),
                        session.services.skills_manager.clone(),
                    )
                    .await
                    .map_err(|e| FunctionCallError::RespondToModel(e.to_string()))?;

                serde_json::to_string(&SpawnResponse { id }).unwrap_or_default()
            }
            SubAgentsArgs::Poll {
                id,
                include_messages,
            } => {
                let Some(poll) = session.services.subagents.poll(&id, include_messages).await
                else {
                    return Err(FunctionCallError::RespondToModel(format!(
                        "unknown sub-agent id: {id}"
                    )));
                };

                serde_json::to_string(&PollResponse {
                    id: poll.id,
                    template: poll.template,
                    status: poll.status,
                    title: poll.title,
                    tool_uses: poll.tool_uses,
                    last_activity: poll.last_activity,
                    total_tokens: poll.total_tokens,
                    messages: poll.drained_messages,
                    plan_suggestions: poll.drained_plan_suggestions,
                    result: poll.result,
                    warnings: poll.warnings,
                })
                .unwrap_or_default()
            }
            SubAgentsArgs::Cancel { id } => {
                let canceled = session.services.subagents.cancel(&id).await;
                serde_json::to_string(&CancelResponse { canceled }).unwrap_or_default()
            }
            SubAgentsArgs::List => {
                let agents = session
                    .services
                    .subagents
                    .list()
                    .await
                    .into_iter()
                    .map(|a| ListEntry {
                        id: a.id,
                        template: a.template,
                        status: a.status,
                        title: a.title,
                        tool_uses: a.tool_uses,
                        last_activity: a.last_activity,
                        total_tokens: a.total_tokens,
                    })
                    .collect();
                serde_json::to_string(&ListResponse { agents }).unwrap_or_default()
            }
            SubAgentsArgs::ListTemplates => {
                let config = session.clone_original_config().await;
                let templates = crate::subagents::agents_md::load_subagent_templates(&config)
                    .await
                    .map_err(|e| FunctionCallError::RespondToModel(e.to_string()))?
                    .into_iter()
                    .map(template_entry)
                    .collect::<Vec<_>>();
                serde_json::to_string(&ListTemplatesResponse { templates }).unwrap_or_default()
            }
        };

        Ok(ToolOutput::Function {
            content,
            content_items: None,
            success: Some(true),
        })
    }
}
