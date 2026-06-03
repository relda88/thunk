use super::abilities::AbilityContent;
use super::skills::SkillContent;
use crate::runtime::investigation::tool_surface::ToolSurface;

pub struct PromptPhysicsConfig {
    pub enabled: bool,
    pub thunk_md: Option<String>,
    pub active_ability: Option<AbilityContent>,
    pub active_skill: Option<SkillContent>,
}

impl Default for PromptPhysicsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            thunk_md: None,
            active_ability: None,
            active_skill: None,
        }
    }
}

pub fn primacy_anchor_block(config: &PromptPhysicsConfig) -> Option<String> {
    if !config.enabled {
        return None;
    }

    let mut parts: Vec<String> = Vec::new();

    if let Some(content) = &config.thunk_md {
        parts.push(format!("[project rules]\n{content}\n[/project rules]"));
    }

    if let Some(ability) = &config.active_ability {
        parts.push(format!(
            "[ability: {}]\n{}\n\n{}\n[/ability: {}]",
            ability.name, ability.invariants, ability.specification, ability.name,
        ));
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n") + "\n")
    }
}

pub fn periodic_refresh_message(config: &PromptPhysicsConfig) -> Option<String> {
    if !config.enabled {
        return None;
    }
    let anchor =
        "You are thunk. The runtime owns control flow. Emit tool calls in exact wire format only.";
    let skill_line = config
        .active_skill
        .as_ref()
        .map(|s| {
            let instructions = s
                .style_instructions
                .split(". ")
                .take(3)
                .collect::<Vec<_>>()
                .join(". ");
            format!("\nStyle: {instructions}")
        })
        .unwrap_or_default();
    Some(format!("{anchor}{skill_line}"))
}

pub fn recency_field_message(config: &PromptPhysicsConfig, surface: ToolSurface) -> Option<String> {
    if !config.enabled {
        return None;
    }
    let tools = surface.allowed_tool_names().collect::<Vec<_>>().join(", ");
    let tools = if tools.is_empty() {
        "none".to_string()
    } else {
        tools
    };
    let ability_line = config
        .active_ability
        .as_ref()
        .map(|a| format!("\nAbility ({}): {}", a.name, a.reasoning_effect))
        .unwrap_or_default();
    Some(format!(
        "[thunk: current context]\n\
         Surface: {}\n\
         Tools: {}{}\n\
         Runtime owns control flow. Emit wire format only.\n\
         [/thunk: current context]\n",
        surface.as_str(),
        tools,
        ability_line,
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
            ..Default::default()
        };
        assert!(primacy_anchor_block(&config).is_none());
    }

    #[test]
    fn primacy_anchor_none_when_no_thunk_md() {
        let config = PromptPhysicsConfig {
            enabled: true,
            thunk_md: None,
            ..Default::default()
        };
        assert!(primacy_anchor_block(&config).is_none());
    }

    #[test]
    fn periodic_refresh_none_when_disabled() {
        let config = PromptPhysicsConfig {
            enabled: false,
            thunk_md: None,
            ..Default::default()
        };
        assert!(periodic_refresh_message(&config).is_none());
    }

    #[test]
    fn periodic_refresh_some_when_enabled() {
        let config = PromptPhysicsConfig {
            enabled: true,
            thunk_md: None,
            ..Default::default()
        };
        let result = periodic_refresh_message(&config).unwrap();
        assert!(result.contains("runtime owns control flow"));
    }

    #[test]
    fn primacy_anchor_wraps_content() {
        let config = PromptPhysicsConfig {
            enabled: true,
            thunk_md: Some("# Rules\nBe concise.".to_string()),
            ..Default::default()
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
            ..Default::default()
        };
        assert!(recency_field_message(&config, ToolSurface::RetrievalFirst).is_none());
    }

    #[test]
    fn recency_field_contains_surface_name() {
        let config = PromptPhysicsConfig {
            enabled: true,
            thunk_md: None,
            ..Default::default()
        };
        let result = recency_field_message(&config, ToolSurface::RetrievalFirst).unwrap();
        assert!(result.contains("RetrievalFirst"));
    }

    #[test]
    fn recency_field_contains_tools() {
        let config = PromptPhysicsConfig {
            enabled: true,
            thunk_md: None,
            ..Default::default()
        };
        let result = recency_field_message(&config, ToolSurface::RetrievalFirst).unwrap();
        assert!(result.contains("search_code"));
    }

    #[test]
    fn recency_field_has_delimiters() {
        let config = PromptPhysicsConfig {
            enabled: true,
            thunk_md: None,
            ..Default::default()
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
            ..Default::default()
        };
        let result = recency_field_message(&config, ToolSurface::RetrievalFirst).unwrap();
        assert!(result.contains("Runtime owns control flow"));
    }

    #[test]
    fn recency_field_answer_only_renders_none_tools() {
        let config = PromptPhysicsConfig {
            enabled: true,
            thunk_md: None,
            ..Default::default()
        };
        let result = recency_field_message(&config, ToolSurface::AnswerOnly).unwrap();
        assert!(result.contains("Tools: none"));
    }

    #[test]
    fn primacy_anchor_with_ability_only() {
        let config = PromptPhysicsConfig {
            enabled: true,
            thunk_md: None,
            active_ability: Some(AbilityContent {
                name: "debug".into(),
                invariants: "test invariants".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = primacy_anchor_block(&config).unwrap();
        assert!(result.contains("[ability: debug]"));
        assert!(result.contains("[/ability: debug]"));
        assert!(result.contains("test invariants"));
    }

    #[test]
    fn primacy_anchor_with_both_thunk_and_ability() {
        let config = PromptPhysicsConfig {
            enabled: true,
            thunk_md: Some("rules".to_string()),
            active_ability: Some(AbilityContent {
                name: "debug".into(),
                invariants: "test invariants".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = primacy_anchor_block(&config).unwrap();
        let rules_pos = result.find("[project rules]").unwrap();
        let ability_pos = result.find("[ability: debug]").unwrap();
        assert!(
            rules_pos < ability_pos,
            "[project rules] must appear before [ability: debug]"
        );
    }

    #[test]
    fn primacy_anchor_none_when_disabled_even_with_ability() {
        let config = PromptPhysicsConfig {
            enabled: false,
            thunk_md: None,
            active_ability: Some(AbilityContent {
                name: "debug".into(),
                invariants: "test invariants".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(primacy_anchor_block(&config).is_none());
    }

    #[test]
    fn recency_field_with_active_ability() {
        let config = PromptPhysicsConfig {
            enabled: true,
            thunk_md: None,
            active_ability: Some(AbilityContent {
                name: "debug".into(),
                reasoning_effect: "test effect".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = recency_field_message(&config, ToolSurface::RetrievalFirst).unwrap();
        assert!(result.contains("Ability (debug):"));
        assert!(result.contains("test effect"));
    }

    #[test]
    fn periodic_refresh_with_active_skill() {
        let config = PromptPhysicsConfig {
            enabled: true,
            active_skill: Some(SkillContent {
                name: "concise".into(),
                style_instructions: "Answer directly. No padding.".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = periodic_refresh_message(&config).unwrap();
        assert!(result.contains("Style:"));
        assert!(result.contains("Answer directly"));
    }

    #[test]
    fn periodic_refresh_skill_capped_at_three_sentences() {
        let config = PromptPhysicsConfig {
            enabled: true,
            active_skill: Some(SkillContent {
                name: "verbose".into(),
                style_instructions: "One. Two. Three. Four. Five.".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = periodic_refresh_message(&config).unwrap();
        assert!(result.contains("One"));
        assert!(result.contains("Two"));
        assert!(result.contains("Three"));
        assert!(
            !result.contains("Four"),
            "output must not include the fourth sentence"
        );
    }

    #[test]
    fn periodic_refresh_with_both_ability_and_skill() {
        let config = PromptPhysicsConfig {
            enabled: true,
            active_ability: Some(AbilityContent {
                name: "debug".into(),
                invariants: "test invariants".into(),
                ..Default::default()
            }),
            active_skill: Some(SkillContent {
                name: "concise".into(),
                style_instructions: "Answer directly.".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = periodic_refresh_message(&config).unwrap();
        assert!(
            result.contains("Style:"),
            "periodic refresh must carry skill"
        );
        assert!(
            !result.contains("[ability:"),
            "periodic refresh must not carry ability content"
        );
    }

    #[test]
    fn periodic_refresh_none_when_disabled_with_skill() {
        let config = PromptPhysicsConfig {
            enabled: false,
            active_skill: Some(SkillContent {
                name: "concise".into(),
                style_instructions: "Answer directly.".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(periodic_refresh_message(&config).is_none());
    }
}
