use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;

use async_channel::Sender;
use codex_protocol::plan_tool::UpdatePlanArgs;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentActivity;
use codex_protocol::protocol::SubAgentActivityKind;
use codex_protocol::protocol::SubAgentStatus;
use codex_protocol::protocol::SubAgentUiItem;
use codex_protocol::protocol::SubAgentsUpdateEvent;
use codex_protocol::protocol::Submission;
use codex_protocol::user_input::UserInput;
use codex_utils_string::take_bytes_at_char_boundary;
use indexmap::IndexMap;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::AuthManager;
use crate::codex::Codex;
use crate::codex::CodexSpawnOk;
use crate::config::Config;
use crate::features::Feature;
use crate::openai_models::models_manager::ModelsManager;
use crate::project_doc::read_project_docs;
use crate::skills::SkillsManager;
use crate::subagents::agents_md::SubAgentTemplate;

pub(crate) mod agents_md;

#[derive(Debug, Clone)]
pub(crate) struct SubAgentSummary {
    pub(crate) id: String,
    pub(crate) template: String,
    pub(crate) status: SubAgentStatus,
    pub(crate) title: String,
    pub(crate) tool_uses: usize,
    pub(crate) total_tokens: Option<i64>,
    pub(crate) last_activity: Option<SubAgentActivity>,
}

#[derive(Debug, Clone)]
pub(crate) struct SubAgentPoll {
    pub(crate) id: String,
    pub(crate) template: String,
    pub(crate) status: SubAgentStatus,
    pub(crate) title: String,
    pub(crate) tool_uses: usize,
    pub(crate) total_tokens: Option<i64>,
    pub(crate) last_activity: Option<SubAgentActivity>,
    pub(crate) drained_messages: Vec<String>,
    pub(crate) drained_plan_suggestions: Vec<UpdatePlanArgs>,
    pub(crate) result: Option<String>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug)]
struct SubAgentState {
    template: String,
    title: String,
    status: SubAgentStatus,
    tool_uses: usize,
    total_tokens: Option<i64>,
    last_activity: Option<SubAgentActivity>,
    transcript_tail: std::collections::VecDeque<String>,
    transcript_truncated: bool,
    drained_messages: Vec<String>,
    drained_plan_suggestions: Vec<UpdatePlanArgs>,
    result: Option<String>,
    warnings: Vec<String>,
    cancel: CancellationToken,
    tx_sub: Option<Sender<Submission>>,
}

impl SubAgentState {
    fn new(template: String, title: String) -> Self {
        Self {
            template,
            title,
            status: SubAgentStatus::Running,
            tool_uses: 0,
            total_tokens: None,
            last_activity: None,
            transcript_tail: std::collections::VecDeque::new(),
            transcript_truncated: false,
            drained_messages: Vec::new(),
            drained_plan_suggestions: Vec::new(),
            result: None,
            warnings: Vec::new(),
            cancel: CancellationToken::new(),
            tx_sub: None,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct SubAgentsManager {
    inner: Arc<Mutex<IndexMap<String, SubAgentState>>>,
    tx_event: Arc<Mutex<Option<Sender<Event>>>>,
    last_emitted_hash: Arc<Mutex<Option<u64>>>,
}

impl SubAgentsManager {
    pub(crate) async fn set_event_sender(&self, tx_event: Sender<Event>) {
        let mut guard = self.tx_event.lock().await;
        *guard = Some(tx_event);
    }

    pub(crate) async fn list(&self) -> Vec<SubAgentSummary> {
        let guard = self.inner.lock().await;
        guard
            .iter()
            .map(|(id, state)| SubAgentSummary {
                id: id.clone(),
                template: state.template.clone(),
                status: state.status,
                title: state.title.clone(),
                tool_uses: state.tool_uses,
                total_tokens: state.total_tokens,
                last_activity: state.last_activity.clone(),
            })
            .collect()
    }

    pub(crate) async fn cancel(&self, id: &str) -> bool {
        let (cancel, tx_sub) = {
            let mut guard = self.inner.lock().await;
            let Some(state) = guard.get_mut(id) else {
                return false;
            };
            state.status = SubAgentStatus::Canceled;
            state.cancel.cancel();
            (state.cancel.clone(), state.tx_sub.clone())
        };

        if cancel.is_cancelled()
            && let Some(tx_sub) = tx_sub
        {
            let _ = tx_sub
                .send(Submission {
                    id: uuid::Uuid::new_v4().to_string(),
                    op: Op::Interrupt,
                })
                .await;
        }

        self.emit_update_if_changed().await;
        true
    }

    pub(crate) async fn poll(&self, id: &str, include_messages: bool) -> Option<SubAgentPoll> {
        let mut guard = self.inner.lock().await;
        let state = guard.get_mut(id)?;
        let drained_messages = if include_messages {
            std::mem::take(&mut state.drained_messages)
        } else {
            Vec::new()
        };
        Some(SubAgentPoll {
            id: id.to_string(),
            template: state.template.clone(),
            status: state.status,
            title: state.title.clone(),
            tool_uses: state.tool_uses,
            total_tokens: state.total_tokens,
            last_activity: state.last_activity.clone(),
            drained_messages,
            drained_plan_suggestions: std::mem::take(&mut state.drained_plan_suggestions),
            result: state.result.clone(),
            warnings: state.warnings.clone(),
        })
    }

    pub(crate) async fn spawn(
        &self,
        template_name: String,
        task: String,
        default_model: String,
        default_effort: Option<codex_protocol::openai_models::ReasoningEffort>,
        default_summary: codex_protocol::config_types::ReasoningSummary,
        mut parent_config: Config,
        auth_manager: Arc<AuthManager>,
        models_manager: Arc<ModelsManager>,
        skills_manager: Arc<SkillsManager>,
    ) -> anyhow::Result<String> {
        let id = uuid::Uuid::new_v4().to_string();

        let (template, project_doc) = {
            let docs = read_project_docs(&parent_config).await?;
            let templates = agents_md::load_subagent_templates(&parent_config).await?;
            let Some(template) = templates.into_iter().find(|t| t.name == template_name) else {
                anyhow::bail!("unknown sub-agent template: {template_name}");
            };
            (template, docs)
        };

        let title = title_from_task(&task).unwrap_or_else(|| template.name.clone());
        let state = SubAgentState::new(template.name.clone(), title);
        let cancel = state.cancel.clone();
        {
            let mut guard = self.inner.lock().await;
            guard.insert(id.clone(), state);
        }

        self.emit_update_if_changed().await;

        // Ensure the sub-agent sees the same project instructions text.
        parent_config.user_instructions = project_doc;
        parent_config.features.disable(Feature::SubAgents);
        if !template.skills.is_empty() {
            parent_config.features.enable(Feature::Skills);
        }

        let this = self.clone();
        let id_for_task = id.clone();
        tokio::spawn(async move {
            this.run_subagent(
                id_for_task,
                template,
                task,
                default_model,
                default_effort,
                default_summary,
                parent_config,
                auth_manager,
                models_manager,
                skills_manager,
                cancel,
            )
            .await;
        });

        Ok(id)
    }

    async fn run_subagent(
        &self,
        id: String,
        template: SubAgentTemplate,
        task: String,
        default_model: String,
        default_effort: Option<codex_protocol::openai_models::ReasoningEffort>,
        default_summary: codex_protocol::config_types::ReasoningSummary,
        config: Config,
        auth_manager: Arc<AuthManager>,
        models_manager: Arc<ModelsManager>,
        skills_manager: Arc<SkillsManager>,
        cancel: CancellationToken,
    ) {
        let mut warnings = Vec::new();

        let CodexSpawnOk {
            codex,
            conversation_id: _,
        } = match Codex::spawn(
            config.clone(),
            auth_manager,
            models_manager,
            skills_manager.clone(),
            codex_protocol::protocol::InitialHistory::New,
            SessionSource::Exec,
        )
        .await
        {
            Ok(ok) => ok,
            Err(err) => {
                self.fail(&id, vec![format!("failed to spawn sub-agent: {err:#}")])
                    .await;
                return;
            }
        };

        {
            let mut guard = self.inner.lock().await;
            if let Some(state) = guard.get_mut(&id) {
                state.tx_sub = Some(codex.tx_sub.clone());
            }
        }

        // Drain SessionConfigured.
        let _ = codex.next_event().await;

        if cancel.is_cancelled() {
            self.fail(&id, vec!["sub-agent was canceled before start".to_string()])
                .await;
            return;
        }

        let mut items = Vec::new();

        if !template.instructions.trim().is_empty() {
            items.push(UserInput::Text {
                text: template.instructions.clone(),
            });
        }

        // Turn the template skills into explicit skill mentions so the existing skills injection path is reused.
        if !template.skills.is_empty() {
            let outcome = skills_manager.skills_for_cwd(&config.cwd);
            for skill_name in &template.skills {
                match outcome.skills.iter().find(|s| s.name == *skill_name) {
                    Some(skill) => {
                        items.push(UserInput::Skill {
                            name: skill.name.clone(),
                            path: skill.path.clone(),
                        });
                    }
                    None => {
                        warnings.push(format!("unknown skill preset: {skill_name}"));
                    }
                }
            }
        }

        items.push(UserInput::Text {
            text: format!("{task}\n"),
        });

        let submit_id = match codex
            .submit(Op::UserTurn {
                items,
                cwd: config.cwd.clone(),
                approval_policy: config.approval_policy,
                sandbox_policy: config.sandbox_policy.clone(),
                model: template.model.unwrap_or(default_model),
                effort: default_effort,
                summary: default_summary,
                final_output_json_schema: None,
            })
            .await
        {
            Ok(id) => id,
            Err(err) => {
                self.fail(
                    &id,
                    vec![format!("failed to submit sub-agent task: {err:#}")],
                )
                .await;
                return;
            }
        };

        self.append_warnings(&id, warnings).await;

        loop {
            if cancel.is_cancelled() {
                let _ = codex.submit(Op::Interrupt).await;
            }

            let event = match codex.next_event().await {
                Ok(e) => e,
                Err(err) => {
                    self.fail(&id, vec![format!("sub-agent event stream failed: {err:#}")])
                        .await;
                    return;
                }
            };

            if cancel.is_cancelled() {
                self.set_status(&id, SubAgentStatus::Canceled).await;
                self.emit_update_if_changed().await;
                return;
            }

            match event.msg {
                codex_protocol::protocol::EventMsg::AgentMessage(m) => {
                    self.append_message(&id, m.message).await;
                }
                codex_protocol::protocol::EventMsg::PlanUpdate(args) => {
                    self.append_plan_suggestion(&id, args).await;
                }
                codex_protocol::protocol::EventMsg::ExecCommandBegin(ev) => {
                    self.bump_tool_use(
                        &id,
                        SubAgentActivity {
                            kind: SubAgentActivityKind::Bash,
                            label: format_exec_label(&ev.command),
                        },
                    )
                    .await;
                }
                codex_protocol::protocol::EventMsg::ReadFileToolCall(ev) => {
                    self.bump_tool_use(
                        &id,
                        SubAgentActivity {
                            kind: SubAgentActivityKind::Read,
                            label: ev.path.display().to_string(),
                        },
                    )
                    .await;
                }
                codex_protocol::protocol::EventMsg::McpToolCallBegin(ev) => {
                    self.bump_tool_use(
                        &id,
                        SubAgentActivity {
                            kind: SubAgentActivityKind::Mcp,
                            label: format!("{}::{}", ev.invocation.server, ev.invocation.tool),
                        },
                    )
                    .await;
                }
                codex_protocol::protocol::EventMsg::WebSearchBegin(_) => {
                    self.bump_tool_use(
                        &id,
                        SubAgentActivity {
                            kind: SubAgentActivityKind::WebSearch,
                            label: "web_search".to_string(),
                        },
                    )
                    .await;
                }
                codex_protocol::protocol::EventMsg::PatchApplyBegin(_) => {
                    self.bump_tool_use(
                        &id,
                        SubAgentActivity {
                            kind: SubAgentActivityKind::ApplyPatch,
                            label: "apply_patch".to_string(),
                        },
                    )
                    .await;
                }
                codex_protocol::protocol::EventMsg::TokenCount(ev) => {
                    self.set_total_tokens(
                        &id,
                        ev.info
                            .as_ref()
                            .map(|i| i.total_token_usage.blended_total()),
                    )
                    .await;
                }
                codex_protocol::protocol::EventMsg::TaskComplete(done) if event.id == submit_id => {
                    self.complete(&id, done.last_agent_message).await;
                    self.emit_update_if_changed().await;
                    return;
                }
                codex_protocol::protocol::EventMsg::Error(err) if event.id == submit_id => {
                    self.fail(&id, vec![err.message]).await;
                    self.emit_update_if_changed().await;
                    return;
                }
                _ => {}
            }
        }
    }

    async fn append_message(&self, id: &str, msg: String) {
        let mut guard = self.inner.lock().await;
        if let Some(state) = guard.get_mut(id) {
            append_transcript_tail(state, &msg);
            state.drained_messages.push(msg);
        }
    }

    async fn append_plan_suggestion(&self, id: &str, args: UpdatePlanArgs) {
        let mut guard = self.inner.lock().await;
        if let Some(state) = guard.get_mut(id) {
            state.drained_plan_suggestions.push(args);
        }
    }

    async fn bump_tool_use(&self, id: &str, activity: SubAgentActivity) {
        let mut guard = self.inner.lock().await;
        if let Some(state) = guard.get_mut(id) {
            state.tool_uses = state.tool_uses.saturating_add(1);
            state.last_activity = Some(activity);
        }
        drop(guard);
        self.emit_update_if_changed().await;
    }

    async fn set_total_tokens(&self, id: &str, total_tokens: Option<i64>) {
        let mut guard = self.inner.lock().await;
        if let Some(state) = guard.get_mut(id) {
            state.total_tokens = total_tokens;
        }
        drop(guard);
        self.emit_update_if_changed().await;
    }

    async fn append_warnings(&self, id: &str, warnings: Vec<String>) {
        if warnings.is_empty() {
            return;
        }
        let mut guard = self.inner.lock().await;
        if let Some(state) = guard.get_mut(id) {
            state.warnings.extend(warnings);
        }
    }

    async fn complete(&self, id: &str, result: Option<String>) {
        let mut guard = self.inner.lock().await;
        if let Some(state) = guard.get_mut(id) {
            state.status = SubAgentStatus::Completed;
            state.result = result;
        }
    }

    async fn fail(&self, id: &str, errors: Vec<String>) {
        let mut guard = self.inner.lock().await;
        if let Some(state) = guard.get_mut(id) {
            state.status = SubAgentStatus::Failed;
            state.warnings.extend(errors);
        }
    }

    async fn set_status(&self, id: &str, status: SubAgentStatus) {
        let mut guard = self.inner.lock().await;
        if let Some(state) = guard.get_mut(id) {
            state.status = status;
        }
    }

    async fn emit_update_if_changed(&self) {
        let tx_event = { self.tx_event.lock().await.clone() };
        let Some(tx_event) = tx_event else {
            return;
        };

        let (created_count, running_count, agents) = {
            let guard = self.inner.lock().await;
            let created_count = guard.len();
            let running_count = guard
                .values()
                .filter(|agent| agent.status == SubAgentStatus::Running)
                .count();
            let agents = guard
                .iter()
                .map(|(id, state)| SubAgentUiItem {
                    id: id.clone(),
                    template: state.template.clone(),
                    title: state.title.clone(),
                    status: state.status,
                    tool_uses: state.tool_uses,
                    total_tokens: state.total_tokens,
                    last_activity: state.last_activity.clone(),
                    transcript: state.transcript_tail.iter().cloned().collect(),
                    transcript_truncated: state.transcript_truncated,
                })
                .collect::<Vec<_>>();
            (created_count, running_count, agents)
        };

        let update = SubAgentsUpdateEvent {
            created_count,
            running_count,
            agents,
        };

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        update.created_count.hash(&mut hasher);
        update.running_count.hash(&mut hasher);
        for agent in &update.agents {
            agent.id.hash(&mut hasher);
            agent.template.hash(&mut hasher);
            agent.title.hash(&mut hasher);
            agent.status.hash(&mut hasher);
            agent.tool_uses.hash(&mut hasher);
            agent.total_tokens.hash(&mut hasher);
            agent.last_activity.hash(&mut hasher);
            agent.transcript.hash(&mut hasher);
            agent.transcript_truncated.hash(&mut hasher);
        }
        let hash = hasher.finish();

        {
            let mut guard = self.last_emitted_hash.lock().await;
            if *guard == Some(hash) {
                return;
            }
            *guard = Some(hash);
        }

        let _ = tx_event
            .send(Event {
                id: uuid::Uuid::new_v4().to_string(),
                msg: EventMsg::SubAgentsUpdate(update),
            })
            .await;
    }
}

fn title_from_task(task: &str) -> Option<String> {
    task.lines().find_map(|line| {
        let title = line.trim();
        if title.is_empty() {
            None
        } else {
            Some(title.to_string())
        }
    })
}

fn format_exec_label(command: &[String]) -> String {
    if command.len() >= 3 && command[0] == "bash" && command[1] == "-lc" {
        return command[2].clone();
    }
    command.join(" ")
}

const SUBAGENT_TRANSCRIPT_MAX_LINES: usize = 30;
const SUBAGENT_TRANSCRIPT_MAX_LINE_BYTES: usize = 300;

fn append_transcript_tail(state: &mut SubAgentState, msg: &str) {
    for raw_line in msg.lines() {
        let line = raw_line.trim_end();
        if line.is_empty() {
            continue;
        }

        let clipped = take_bytes_at_char_boundary(line, SUBAGENT_TRANSCRIPT_MAX_LINE_BYTES);
        let clipped = if clipped.len() < line.len() {
            format!("{clipped}…")
        } else {
            clipped.to_string()
        };

        state.transcript_tail.push_back(clipped);
        while state.transcript_tail.len() > SUBAGENT_TRANSCRIPT_MAX_LINES {
            state.transcript_tail.pop_front();
            state.transcript_truncated = true;
        }
    }
}
