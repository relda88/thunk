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

#[allow(dead_code)]
pub fn periodic_refresh_message(_config: &PromptPhysicsConfig) -> Option<String> {
    None
}

#[allow(dead_code)]
pub fn recency_field_message(_config: &PromptPhysicsConfig) -> Option<String> {
    None
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
}
