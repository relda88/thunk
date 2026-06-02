use crate::runtime::investigation::tool_surface::ToolSurface;

pub struct PromptPhysicsConfig {
    pub enabled: bool,
    pub thunk_md: Option<String>,
}

impl Default for PromptPhysicsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            thunk_md: None,
        }
    }
}

pub fn primacy_anchor_block(config: &PromptPhysicsConfig) -> Option<String> {
    if !config.enabled {
        return None;
    }
    let content = config.thunk_md.as_deref()?;
    Some(format!("[project rules]\n{content}\n[/project rules]\n"))
}

pub fn periodic_refresh_message(config: &PromptPhysicsConfig) -> Option<String> {
    if !config.enabled {
        return None;
    }
    Some(
        "You are thunk. The runtime owns control flow. Emit tool calls in exact wire format only."
            .to_string(),
    )
}

pub fn recency_field_message(config: &PromptPhysicsConfig, surface: ToolSurface) -> Option<String> {
    if !config.enabled {
        return None;
    }
    let mut tools = String::new();
    for name in surface.allowed_tool_names() {
        if !tools.is_empty() {
            tools.push_str(", ");
        }
        tools.push_str(name);
    }
    if tools.is_empty() {
        tools.push_str("none");
    }
    Some(format!(
        "[thunk: current context]\nSurface: {}\nTools: {}\nRuntime owns control flow. Emit wire format only.\n[/thunk: current context]",
        surface.as_str(),
        tools,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primacy_anchor_none_when_disabled() {
        let config = PromptPhysicsConfig {
            enabled: false,
            thunk_md: Some("x".to_string()),
        };
        assert!(primacy_anchor_block(&config).is_none());
    }

    #[test]
    fn primacy_anchor_none_when_no_thunk_md() {
        let config = PromptPhysicsConfig {
            enabled: true,
            thunk_md: None,
        };
        assert!(primacy_anchor_block(&config).is_none());
    }

    #[test]
    fn periodic_refresh_none_when_disabled() {
        let config = PromptPhysicsConfig {
            enabled: false,
            thunk_md: None,
        };
        assert!(periodic_refresh_message(&config).is_none());
    }

    #[test]
    fn periodic_refresh_some_when_enabled() {
        let config = PromptPhysicsConfig {
            enabled: true,
            thunk_md: None,
        };
        let result = periodic_refresh_message(&config).unwrap();
        assert!(result.contains("runtime owns control flow"));
    }

    #[test]
    fn primacy_anchor_wraps_content() {
        let config = PromptPhysicsConfig {
            enabled: true,
            thunk_md: Some("# Rules\nBe concise.".to_string()),
        };
        let result = primacy_anchor_block(&config).unwrap();
        assert!(result.contains("[project rules]"));
        assert!(result.contains("[/project rules]"));
        assert!(result.contains("# Rules\nBe concise."));
    }

    #[test]
    fn recency_field_none_when_disabled() {
        let config = PromptPhysicsConfig {
            enabled: false,
            thunk_md: None,
        };
        assert!(recency_field_message(&config, ToolSurface::RetrievalFirst).is_none());
    }

    #[test]
    fn recency_field_contains_surface_name() {
        let config = PromptPhysicsConfig {
            enabled: true,
            thunk_md: None,
        };
        let result = recency_field_message(&config, ToolSurface::RetrievalFirst).unwrap();
        assert!(result.contains("RetrievalFirst"));
    }

    #[test]
    fn recency_field_contains_tools() {
        let config = PromptPhysicsConfig {
            enabled: true,
            thunk_md: None,
        };
        let result = recency_field_message(&config, ToolSurface::RetrievalFirst).unwrap();
        assert!(result.contains("search_code"));
    }

    #[test]
    fn recency_field_has_delimiters() {
        let config = PromptPhysicsConfig {
            enabled: true,
            thunk_md: None,
        };
        let result = recency_field_message(&config, ToolSurface::RetrievalFirst).unwrap();
        assert!(result.contains("[thunk: current context]"));
        assert!(result.contains("[/thunk: current context]"));
    }

    #[test]
    fn recency_field_has_invariant_line() {
        let config = PromptPhysicsConfig {
            enabled: true,
            thunk_md: None,
        };
        let result = recency_field_message(&config, ToolSurface::RetrievalFirst).unwrap();
        assert!(result.contains("Runtime owns control flow"));
    }

    #[test]
    fn recency_field_answer_only_renders_none_tools() {
        let config = PromptPhysicsConfig {
            enabled: true,
            thunk_md: None,
        };
        let result = recency_field_message(&config, ToolSurface::AnswerOnly).unwrap();
        assert!(result.contains("Tools: none"));
    }
}
