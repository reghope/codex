use crate::status::format_tokens_compact;
use codex_core::protocol::SubAgentActivityKind;
use codex_core::protocol::SubAgentStatus;
use codex_core::protocol::SubAgentUiItem;
use codex_core::protocol::SubAgentsUpdateEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use crate::render::renderable::Renderable;

pub(crate) struct SubAgentsPane<'a> {
    pub(crate) update: &'a SubAgentsUpdateEvent,
    pub(crate) expanded: bool,
    pub(crate) background_mode: bool,
}

impl SubAgentsPane<'_> {
    fn lines(&self) -> Vec<Line<'static>> {
        if self.update.running_count == 0 {
            return Vec::new();
        }

        subagents_tree_lines(self.update, self.expanded, self.background_mode)
    }
}

impl Renderable for SubAgentsPane<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(Text::from(self.lines())).render(area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        self.lines().len().try_into().unwrap_or(u16::MAX)
    }
}

fn subagents_tree_lines(
    update: &SubAgentsUpdateEvent,
    show_transcripts: bool,
    background_mode: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let bg_badge = if background_mode {
        "bg:on".cyan().bold()
    } else {
        "bg:off".dim()
    };
    lines.push(Line::from(vec![
        "Running ".into(),
        update.running_count.to_string().bold(),
        " Task agents… ".into(),
        "(".dim(),
        "ctrl+o".dim(),
        if show_transcripts {
            " to collapse transcripts".dim()
        } else {
            " to expand transcripts".dim()
        },
        " · ".dim(),
        "ctrl+b".dim(),
        " ".dim(),
        bg_badge,
        ")".dim(),
    ]));

    for (idx, agent) in update.agents.iter().enumerate() {
        let is_last = idx + 1 == update.agents.len();
        lines.extend(subagent_lines(agent, is_last, show_transcripts));
    }

    lines
}

fn subagent_lines(
    agent: &SubAgentUiItem,
    is_last: bool,
    show_transcripts: bool,
) -> Vec<Line<'static>> {
    let branch = if is_last { "└─ " } else { "├─ " };
    let title = match agent.status {
        SubAgentStatus::Running => Span::from(agent.title.clone()),
        SubAgentStatus::Completed | SubAgentStatus::Canceled => {
            Span::from(agent.title.clone()).dim()
        }
        SubAgentStatus::Failed => Span::from(agent.title.clone()).red(),
    };

    let mut header = Line::from(vec![branch.dim(), title]);
    header.push_span(" · ".dim());
    header.push_span(format!("{} tool uses", agent.tool_uses).dim());
    header.push_span(" · ".dim());
    let total_tokens = agent
        .total_tokens
        .map_or_else(|| "?".to_string(), format_tokens_compact);
    header.push_span(format!("{total_tokens} tokens").dim());

    let pipe = if is_last { "   " } else { "│  " };
    let (kind, label) = if let Some(activity) = agent.last_activity.as_ref() {
        let kind = match activity.kind {
            SubAgentActivityKind::Bash => "Bash",
            SubAgentActivityKind::Read => "Read",
            SubAgentActivityKind::Mcp => "MCP",
            SubAgentActivityKind::WebSearch => "WebSearch",
            SubAgentActivityKind::ApplyPatch => "ApplyPatch",
            SubAgentActivityKind::Other => "Activity",
        };
        (kind, activity.label.clone())
    } else {
        let label = match agent.status {
            SubAgentStatus::Running => "Starting…",
            SubAgentStatus::Completed => "Completed",
            SubAgentStatus::Failed => "Failed",
            SubAgentStatus::Canceled => "Canceled",
        };
        ("Activity", label.to_string())
    };

    let mut lines = vec![
        header,
        Line::from(vec![
            pipe.dim(),
            "⎿  ".dim(),
            format!("{kind}: ").dim(),
            label.into(),
        ]),
    ];

    if show_transcripts && !agent.transcript.is_empty() {
        for line in &agent.transcript {
            lines.push(Line::from(vec![
                pipe.dim(),
                "   ".dim(),
                line.clone().dim(),
            ]));
        }
        if agent.transcript_truncated {
            lines.push(Line::from(vec![
                pipe.dim(),
                "   ".dim(),
                "(older transcript truncated)".dim(),
            ]));
        }
    }

    lines
}
