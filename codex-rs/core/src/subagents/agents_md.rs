use crate::config::Config;
use crate::project_doc::discover_project_doc_paths;
use serde::Deserialize;
use tokio::io::AsyncReadExt;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SubAgentTemplate {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) instructions: String,
    #[serde(default)]
    pub(crate) skills: Vec<String>,
    #[serde(default)]
    pub(crate) model: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct SubAgentsConfig {
    #[serde(default)]
    agent: Vec<SubAgentTemplate>,
}

fn builtin_subagent_templates() -> Vec<SubAgentTemplate> {
    vec![
        SubAgentTemplate {
            name: "inspect".to_string(),
            instructions: "Explore and understand the codebase by reading files and summarizing findings. Prefer commands that only read (e.g., git diff, rg/grep, ls, cat/sed). Do not make edits.".to_string(),
            skills: Vec::new(),
            model: None,
        },
        SubAgentTemplate {
            name: "implement".to_string(),
            instructions: "Make focused code changes with minimal diff. Apply repository conventions, run the smallest relevant tests/formatters, and report what changed and why.".to_string(),
            skills: Vec::new(),
            model: None,
        },
        SubAgentTemplate {
            name: "tests".to_string(),
            instructions: "Run the smallest set of tests to validate the change. Prefer fast, scoped commands (e.g., a single crate or a single test). Report commands run and failures clearly.".to_string(),
            skills: Vec::new(),
            model: None,
        },
        SubAgentTemplate {
            name: "refactor".to_string(),
            instructions:
                "Refactor with minimal diff and keep behavior unchanged. Prefer mechanical transformations and keep names/structure consistent with the file.".to_string(),
            skills: Vec::new(),
            model: None,
        },
        SubAgentTemplate {
            name: "docs".to_string(),
            instructions:
                "Update documentation to match the code changes. Keep docs concise and verify any commands/paths mentioned.".to_string(),
            skills: Vec::new(),
            model: None,
        },
    ]
}

pub(crate) async fn load_subagent_templates(
    config: &Config,
) -> anyhow::Result<Vec<SubAgentTemplate>> {
    let mut templates_by_name = std::collections::BTreeMap::<String, SubAgentTemplate>::new();
    for path in discover_project_doc_paths(config)? {
        let mut file = match tokio::fs::File::open(&path).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };

        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).await?;
        let text = String::from_utf8_lossy(&bytes);

        for block in extract_fenced_blocks(&text, "codex-subagents") {
            let parsed: SubAgentsConfig = toml::from_str(&block)?;
            for agent in parsed.agent {
                templates_by_name.insert(agent.name.clone(), agent);
            }
        }
    }

    if templates_by_name.is_empty() {
        for template in builtin_subagent_templates() {
            templates_by_name.insert(template.name.clone(), template);
        }
    }

    Ok(templates_by_name.into_values().collect())
}

fn extract_fenced_blocks(contents: &str, fence: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut buf = String::new();
    let opener = format!("```{fence}");

    for line in contents.lines() {
        if !in_block {
            if line.trim_start().starts_with(&opener) {
                in_block = true;
                buf.clear();
            }
            continue;
        }

        if line.trim_start().starts_with("```") {
            in_block = false;
            if !buf.trim().is_empty() {
                blocks.push(buf.clone());
            }
            continue;
        }

        buf.push_str(line);
        buf.push('\n');
    }

    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn extracts_named_fenced_blocks() {
        let contents = r#"
before
```codex-subagents
[[agent]]
name = "a"
```
middle
```codex-subagents
[[agent]]
name = "b"
```
after
"#;

        let blocks = extract_fenced_blocks(contents, "codex-subagents");
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].contains("name = \"a\""));
        assert!(blocks[1].contains("name = \"b\""));
    }

    #[test]
    fn ignores_other_fenced_blocks() {
        let contents = r#"
```toml
foo = "bar"
```
```codex-subagents
[[agent]]
name = "ok"
```
"#;
        let blocks = extract_fenced_blocks(contents, "codex-subagents");
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].contains("name = \"ok\""));
    }
}
