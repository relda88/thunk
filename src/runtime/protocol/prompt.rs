use std::path::Path;

use crate::runtime::mcp::McpTool;
use crate::tools::{DynamicToolSpec, ExecutionKind, RiskLevel, ToolSpec};

use super::super::project::{CargoContext, ProjectStructureEntryKind, ProjectStructureSnapshot};
use super::prompt_physics;
use super::prompt_physics::PromptPhysicsConfig;
use super::tool_codec;

/// Builds the ephemeral per-turn tool-surface hint injected before generation.
/// This is not persisted in conversation history.
pub(crate) fn render_tool_surface_hint<I, S>(surface_name: &str, allowed_tools: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut tools = String::new();
    for tool in allowed_tools {
        if !tools.is_empty() {
            tools.push_str(", ");
        }
        tools.push_str(tool.as_ref());
    }
    if tools.is_empty() {
        format!("Active tool surface: {surface_name}. No tools are available. Provide your final answer now.")
    } else {
        format!("Active tool surface: {surface_name}. Available this turn: {tools}.")
    }
}

pub(crate) fn render_project_snapshot_hint(snapshot: &ProjectStructureSnapshot) -> String {
    const IMPORTANT_FILE_CAP: usize = 4;
    const TOP_LEVEL_DIR_CAP: usize = 6;
    const TOP_LEVEL_FILE_CAP: usize = 6;
    const MAX_ITEM_CHARS: usize = 32;

    let top_level_dirs = snapshot
        .entries
        .iter()
        .filter(|entry| entry.depth == 1 && entry.kind == ProjectStructureEntryKind::Dir)
        .map(|entry| entry.path.as_str())
        .collect::<Vec<_>>();
    let top_level_files = snapshot
        .entries
        .iter()
        .filter(|entry| entry.depth == 1 && entry.kind == ProjectStructureEntryKind::File)
        .map(|entry| entry.path.as_str())
        .collect::<Vec<_>>();

    let (important_files, important_truncated) = render_capped_list(
        &snapshot.important_files,
        IMPORTANT_FILE_CAP,
        MAX_ITEM_CHARS,
    );
    let (dirs, dirs_truncated) =
        render_capped_list(&top_level_dirs, TOP_LEVEL_DIR_CAP, MAX_ITEM_CHARS);
    let (files, files_truncated) =
        render_capped_list(&top_level_files, TOP_LEVEL_FILE_CAP, MAX_ITEM_CHARS);
    let truncated = snapshot.truncated || important_truncated || dirs_truncated || files_truncated;

    format!(
        "[project snapshot]\nImportant files: {important_files}\nTop-level dirs: {dirs}\nTop-level files: {files}\nTruncated: {truncated}\n[/project snapshot]"
    )
}

fn render_capped_list<T>(items: &[T], cap: usize, max_item_chars: usize) -> (String, bool)
where
    T: AsRef<str>,
{
    if items.is_empty() {
        return ("none".to_string(), false);
    }

    let truncated = items.len() > cap;
    let rendered = items
        .iter()
        .take(cap)
        .map(|item| truncate_item(item.as_ref(), max_item_chars))
        .collect::<Vec<_>>()
        .join(", ");

    if truncated {
        (format!("{rendered}, ..."), true)
    } else {
        (rendered, false)
    }
}

fn truncate_item(item: &str, max_chars: usize) -> String {
    let mut chars = item.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

pub fn build_system_prompt(
    app_name: &str,
    project_root: &Path,
    cargo_context: Option<&CargoContext>,
    specs: &[ToolSpec],
    include_mutation_tools: bool,
    prompt_physics: &PromptPhysicsConfig,
    dynamic_tools: &[McpTool],
    anchor_facts: &[crate::storage::memory::MemoryFact],
) -> String {
    let mut prompt = String::new();
    if let Some(anchor) = prompt_physics::primacy_anchor_block(prompt_physics) {
        prompt.push_str(&anchor);
        prompt.push('\n');
    }
    if !anchor_facts.is_empty() {
        prompt.push_str("## What I know about you\n");
        for fact in anchor_facts {
            prompt.push_str(&format!("- {}\n", fact.text));
        }
        prompt.push('\n');
    }
    prompt.push_str(&format!(
        "You are {app_name}, a local AI coding assistant.\n\
Project: {}\n\n\
Be concise, grounded, and practical. \
When the user asks about this project's code, investigate using the tools before responding — \
do not guess or ask the user for information the tools can find. \
When you show code, keep it focused on the user's request.",
        project_root.display()
    ));

    if let Some(ctx) = cargo_context {
        let ed = if ctx.edition.is_empty() {
            "unknown"
        } else {
            &ctx.edition
        };
        let pkg = if ctx.version.is_empty() {
            ctx.package_name.clone()
        } else {
            format!("{}@{}", ctx.package_name, ctx.version)
        };
        let mut line = format!("[cargo context] {pkg} edition={ed}");
        if !ctx.direct_deps.is_empty() {
            let deps: Vec<String> = ctx
                .direct_deps
                .iter()
                .map(|(n, v)| format!("{n}@{v}"))
                .collect();
            line.push_str(&format!(", deps={}", deps.join(",")));
        }
        if !ctx.workspace_members.is_empty() {
            let shown = ctx.workspace_members.join(",");
            line.push_str(&format!(", workspace=[{shown}]"));
        }
        line.push_str("\n\n");
        prompt.push_str("\n\n");
        prompt.push_str(&line);
    }

    let visible_specs: Vec<&ToolSpec> = specs
        .iter()
        .filter(|s| include_mutation_tools || s.execution_kind != ExecutionKind::RequiresApproval)
        .collect();

    if !visible_specs.is_empty() {
        let instructions = tool_codec::format_instructions();

        // Guard: every listed tool must appear in the protocol instructions.
        // A missing entry means the model is told a tool exists but not how to call it.
        for spec in &visible_specs {
            debug_assert!(
                instructions.contains(spec.name),
                "tool '{}' is registered but its call syntax is missing from format_instructions()",
                spec.name
            );
        }

        prompt.push_str("\n\nYou have access to the following tools:\n\n");
        for spec in &visible_specs {
            prompt.push_str(&format!("  {}: {}\n", spec.name, spec.description));
        }
        for tool in dynamic_tools {
            prompt.push_str(&format!("  {}: {}\n", tool.name, tool.description));
        }
        prompt.push('\n');
        prompt.push_str("For files or directories outside the project root, use mcp::filesystem tools — native tools are scoped to the project only.\n");
        prompt.push_str(instructions);
        let dynamic_specs: Vec<DynamicToolSpec> = dynamic_tools
            .iter()
            .map(|t| DynamicToolSpec {
                name: t.name.clone(),
                description: t.description.clone(),
                execution_kind: ExecutionKind::RequiresApproval,
                default_risk: Some(RiskLevel::Medium),
            })
            .collect();
        let dynamic_instructions = tool_codec::format_dynamic_instructions(&dynamic_specs);
        if !dynamic_instructions.is_empty() {
            prompt.push('\n');
            prompt.push_str(&dynamic_instructions);
        }
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::super::super::project::{
        ProjectStructureEntry, ProjectStructureEntryKind, ProjectStructureSnapshot,
    };
    use super::*;

    fn make_fact(text: &str) -> crate::storage::memory::MemoryFact {
        crate::storage::memory::MemoryFact {
            id: 1,
            text: text.to_string(),
            category: "test".to_string(),
            scope: None,
            salience: 1.0,
            embedding: None,
            model_name: None,
            source: crate::storage::memory::MemorySource::User,
            created_at: "2026-01-01".to_string(),
            updated_at: "2026-01-01".to_string(),
            last_recalled_at: None,
        }
    }

    fn stub_prompt(anchor_facts: &[crate::storage::memory::MemoryFact]) -> String {
        use std::path::Path;
        build_system_prompt(
            "thunk",
            Path::new("."),
            None,
            &[],
            false,
            &PromptPhysicsConfig::default(),
            &[],
            anchor_facts,
        )
    }

    #[test]
    fn anchor_facts_appear_in_system_prompt() {
        let facts = [make_fact("prefers Rust"), make_fact("lives in NYC")];
        let prompt = stub_prompt(&facts);
        assert!(
            prompt.contains("## What I know about you"),
            "section heading must appear: {prompt}"
        );
        assert!(prompt.contains("prefers Rust"), "first fact must appear");
        assert!(prompt.contains("lives in NYC"), "second fact must appear");
    }

    #[test]
    fn empty_anchor_facts_produces_no_section() {
        let prompt = stub_prompt(&[]);
        assert!(
            !prompt.contains("## What I know about you"),
            "section must be absent when no facts: {prompt}"
        );
    }

    #[test]
    fn project_snapshot_hint_is_compact_and_bounded() {
        let snapshot = ProjectStructureSnapshot {
            entries: vec![
                ProjectStructureEntry {
                    path: "docs".into(),
                    depth: 1,
                    kind: ProjectStructureEntryKind::Dir,
                    important: false,
                },
                ProjectStructureEntry {
                    path: "src".into(),
                    depth: 1,
                    kind: ProjectStructureEntryKind::Dir,
                    important: false,
                },
                ProjectStructureEntry {
                    path: "tests".into(),
                    depth: 1,
                    kind: ProjectStructureEntryKind::Dir,
                    important: false,
                },
                ProjectStructureEntry {
                    path: "Cargo.toml".into(),
                    depth: 1,
                    kind: ProjectStructureEntryKind::File,
                    important: true,
                },
                ProjectStructureEntry {
                    path: "README.md".into(),
                    depth: 1,
                    kind: ProjectStructureEntryKind::File,
                    important: true,
                },
                ProjectStructureEntry {
                    path: "config.toml".into(),
                    depth: 1,
                    kind: ProjectStructureEntryKind::File,
                    important: true,
                },
                ProjectStructureEntry {
                    path: "very-long-top-level-file-name-that-should-be-truncated.txt".into(),
                    depth: 1,
                    kind: ProjectStructureEntryKind::File,
                    important: false,
                },
            ],
            important_files: vec![
                "Cargo.toml".into(),
                "README.md".into(),
                "config.toml".into(),
                "package.json".into(),
                "pyproject.toml".into(),
            ],
            max_depth: 2,
            max_nodes: 40,
            truncated: false,
        };

        let hint = render_project_snapshot_hint(&snapshot);

        assert!(hint.starts_with("[project snapshot]\n"));
        assert!(hint.ends_with("\n[/project snapshot]"));
        assert!(
            hint.contains("Important files: Cargo.toml, README.md, config.toml, package.json, ...")
        );
        assert!(hint.contains("Top-level dirs: docs, src, tests"));
        assert!(hint.contains("Top-level files: Cargo.toml, README.md, config.toml"));
        assert!(hint.contains("very-long-top-level-file-name-th..."));
        assert!(hint.contains("Truncated: true"));
        assert_eq!(
            hint.lines().count(),
            6,
            "hint format must stay short: {hint}"
        );
        assert!(hint.len() <= 320, "hint must stay compact: {}", hint.len());
    }
}
