use super::super::conversation::LIVE_TRIM_THRESHOLD;
use crate::llm::backend::BackendCapabilities;

/// Policy values derived once from backend capabilities at construction time.
/// Both layers of capability-aware context management read from this struct.
pub(super) struct ContextPolicy {
    /// Message count threshold at which conversation trimming fires (Layer 2).
    pub(super) trim_threshold: usize,
    /// Maximum content lines per tool result block before it is capped (Layer 1).
    pub(super) tool_result_max_lines: usize,
}

impl ContextPolicy {
    pub(super) fn from_capabilities(caps: BackendCapabilities) -> Self {
        match caps.context_window_tokens {
            Some(t) if t >= 16_384 => Self {
                trim_threshold: LIVE_TRIM_THRESHOLD,
                tool_result_max_lines: 200,
            },
            Some(t) if t >= 8_192 => Self {
                trim_threshold: 30,
                tool_result_max_lines: 150,
            },
            Some(t) if t >= 4_096 => Self {
                trim_threshold: 20,
                tool_result_max_lines: 80,
            },
            Some(_) => Self {
                trim_threshold: 12,
                tool_result_max_lines: 40,
            },
            None => Self {
                trim_threshold: LIVE_TRIM_THRESHOLD,
                tool_result_max_lines: 200,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ContextPolicy;
    use crate::llm::backend::BackendCapabilities;

    #[test]
    fn context_policy_none_uses_defaults() {
        let policy = ContextPolicy::from_capabilities(BackendCapabilities {
            context_window_tokens: None,
            max_output_tokens: None,
        });
        assert_eq!(policy.trim_threshold, 40);
        assert_eq!(policy.tool_result_max_lines, 200);
    }

    #[test]
    fn context_policy_small_context_uses_tight_limits() {
        let policy = ContextPolicy::from_capabilities(BackendCapabilities {
            context_window_tokens: Some(2048),
            max_output_tokens: None,
        });
        assert_eq!(policy.trim_threshold, 12);
        assert_eq!(policy.tool_result_max_lines, 40);
    }

    #[test]
    fn context_policy_mid_context_uses_intermediate_limits() {
        let policy = ContextPolicy::from_capabilities(BackendCapabilities {
            context_window_tokens: Some(4096),
            max_output_tokens: None,
        });
        assert_eq!(policy.trim_threshold, 20);
        assert_eq!(policy.tool_result_max_lines, 80);
    }

    #[test]
    fn context_policy_large_context_uses_defaults() {
        let policy = ContextPolicy::from_capabilities(BackendCapabilities {
            context_window_tokens: Some(32768),
            max_output_tokens: None,
        });
        assert_eq!(policy.trim_threshold, 40);
        assert_eq!(policy.tool_result_max_lines, 200);
    }
}
