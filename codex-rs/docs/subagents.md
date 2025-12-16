# Sub-agents (experimental)

Codex can spawn asynchronous “sub-agents” as background workers. Sub-agents are configured per-repository via `AGENTS.md` and are invoked by the `subagents` tool.

## Configure in `AGENTS.md`

Add one or more fenced TOML blocks named `codex-subagents`:

```codex-subagents
[[agent]]
name = "tests"
instructions = "Focus on running the smallest set of tests to validate a change."
skills = ["rust-tests"]

[[agent]]
name = "refactor"
instructions = "Refactor code with minimal diff and keep behavior unchanged."
```

Fields:

- `name` (required): Template name used when spawning.
- `instructions` (optional): Extra guidance prepended to the sub-agent task.
- `skills` (optional): Skill preset names to inject (requires skills feature support).
- `model` (optional): Override model slug for this sub-agent; defaults to the parent session model.

When multiple `AGENTS.md` files are discovered (repo root → cwd), later definitions override earlier ones by `name`.

If no `codex-subagents` blocks are found, Codex provides a small set of built-in templates: `inspect`, `implement`, `tests`, `refactor`, `docs`.

## Tool API

The `sub_agents` feature is enabled by default. Disable it with `codex --disable sub_agents …` (or `-c features.sub_agents=false`).

Tool name: `subagents`

- Spawn: `{ "action": "spawn", "template": "tests", "task": "…" }`
- Poll: `{ "action": "poll", "id": "…" }`
- Cancel: `{ "action": "cancel", "id": "…" }`
- List: `{ "action": "list" }`

Plan updates:

Sub-agents may call `update_plan`; these are captured as `plan_suggestions` in `poll` results so the parent can confirm/apply them.
