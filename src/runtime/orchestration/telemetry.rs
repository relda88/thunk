use crate::llm::backend::BackendTimingStage;
use crate::tools::ToolInput;

use super::super::investigation::investigation::InvestigationState;
use super::super::trace::{trace_runtime_decision, RUNTIME_TRACE_ENV};
use super::super::types::{Activity, RuntimeEvent};
use super::tool_round::SearchBudget;

#[derive(Clone, Copy)]
pub(crate) enum GenerationRoundLabel {
    Initial,
    PostTool,
    PostEvidenceRetry,
    CorrectionRetry,
}

impl GenerationRoundLabel {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::PostTool => "post-tool",
            Self::PostEvidenceRetry => "post-evidence-retry",
            Self::CorrectionRetry => "correction-retry",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum GenerationRoundCause {
    Initial,
    ToolResults,
    Recovery,
    SearchRetry,
    PostEvidenceToolCallRejected,
    AnswerPhaseToolCallRejected,
    SearchBudgetClosedCorrection,
    EditRepairCorrection,
    FabricationCorrection,
    MalformedBlockCorrection,
    ReadRequestToolRequired,
    SearchBeforeAnsweringCorrection,
    ReadBeforeAnsweringCorrection,
}

impl GenerationRoundCause {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::ToolResults => "tool-results",
            Self::Recovery => "recovery",
            Self::SearchRetry => "search-retry",
            Self::PostEvidenceToolCallRejected => "post_evidence_tool_call_rejected",
            Self::AnswerPhaseToolCallRejected => "answer_phase_tool_call_rejected",
            Self::SearchBudgetClosedCorrection => "search_budget_closed_correction",
            Self::EditRepairCorrection => "edit_repair_correction",
            Self::FabricationCorrection => "fabrication_correction",
            Self::MalformedBlockCorrection => "malformed_block_correction",
            Self::ReadRequestToolRequired => "read_request_tool_required",
            Self::SearchBeforeAnsweringCorrection => "search_before_answering",
            Self::ReadBeforeAnsweringCorrection => "read_before_answering",
        }
    }
}

pub(crate) struct TurnPerformance {
    enabled: bool,
    turn_start: Option<std::time::Instant>,
    rounds: usize,
    round_labels: Vec<GenerationRoundLabel>,
    round_causes: Vec<GenerationRoundCause>,
    prompt_sizes: Vec<usize>,
    ctx_ms: u64,
    tokenize_ms: u64,
    prefill_ms: u64,
    generation_ms: u64,
    model_load_ms: u64,
    tool_ms: u64,
    tokens_prompt: u64,
    tokens_completion: u64,
    context_window_tokens: Option<u32>,
}

impl TurnPerformance {
    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn new(context_window_tokens: Option<u32>) -> Self {
        let enabled = std::env::var_os(RUNTIME_TRACE_ENV).is_some();
        Self {
            enabled,
            turn_start: enabled.then(std::time::Instant::now),
            rounds: 0,
            round_labels: Vec::new(),
            round_causes: Vec::new(),
            prompt_sizes: Vec::new(),
            ctx_ms: 0,
            tokenize_ms: 0,
            prefill_ms: 0,
            generation_ms: 0,
            model_load_ms: 0,
            tool_ms: 0,
            tokens_prompt: 0,
            tokens_completion: 0,
            context_window_tokens,
        }
    }

    /// Test-only constructor that always enables tracing without reading the env var.
    /// Avoids races from parallel tests mutating RUNTIME_TRACE_ENV.
    #[cfg(test)]
    fn new_enabled(context_window_tokens: Option<u32>) -> Self {
        Self {
            enabled: true,
            turn_start: Some(std::time::Instant::now()),
            rounds: 0,
            round_labels: Vec::new(),
            round_causes: Vec::new(),
            prompt_sizes: Vec::new(),
            ctx_ms: 0,
            tokenize_ms: 0,
            prefill_ms: 0,
            generation_ms: 0,
            model_load_ms: 0,
            tool_ms: 0,
            tokens_prompt: 0,
            tokens_completion: 0,
            context_window_tokens,
        }
    }

    pub(crate) fn start_round(
        &mut self,
        label: GenerationRoundLabel,
        cause: GenerationRoundCause,
        prompt_chars: usize,
        on_event: &mut dyn FnMut(RuntimeEvent),
    ) {
        if !self.enabled {
            return;
        }

        self.rounds += 1;
        self.round_labels.push(label);
        self.round_causes.push(cause);
        self.prompt_sizes.push(prompt_chars);
        on_event(RuntimeEvent::RuntimeTrace(format!(
            "[runtime:perf] round={} label={} cause={} prompt_chars={}",
            self.rounds,
            label.as_str(),
            cause.as_str(),
            prompt_chars
        )));
    }

    pub(crate) fn record_backend_timing(&mut self, stage: BackendTimingStage, elapsed_ms: u64) {
        if !self.enabled {
            return;
        }

        match stage {
            BackendTimingStage::CtxCreate => self.ctx_ms += elapsed_ms,
            BackendTimingStage::Tokenize => self.tokenize_ms += elapsed_ms,
            BackendTimingStage::PrefillDone => self.prefill_ms += elapsed_ms,
            BackendTimingStage::GenerationDone => self.generation_ms += elapsed_ms,
            BackendTimingStage::ModelLoad => self.model_load_ms += elapsed_ms,
            BackendTimingStage::PrefillStart => {}
        }
    }

    pub(crate) fn record_tool_elapsed(&mut self, elapsed_ms: u64) {
        if !self.enabled {
            return;
        }
        self.tool_ms += elapsed_ms;
    }

    pub(crate) fn record_token_counts(&mut self, prompt: u32, completion: u32) {
        if !self.enabled {
            return;
        }
        self.tokens_prompt += u64::from(prompt);
        self.tokens_completion += u64::from(completion);
    }

    pub(crate) fn emit_summary(&self, on_event: &mut dyn FnMut(RuntimeEvent)) {
        if !self.enabled {
            return;
        }

        let round_labels = if self.round_labels.is_empty() {
            "none".to_string()
        } else {
            self.round_labels
                .iter()
                .map(|label| label.as_str())
                .collect::<Vec<_>>()
                .join(",")
        };
        let causes = if self.round_causes.is_empty() {
            "none".to_string()
        } else {
            self.round_causes
                .iter()
                .map(|cause| cause.as_str())
                .collect::<Vec<_>>()
                .join(",")
        };
        let prompt_sizes = if self.prompt_sizes.is_empty() {
            "none".to_string()
        } else {
            self.prompt_sizes
                .iter()
                .map(|size| size.to_string())
                .collect::<Vec<_>>()
                .join(",")
        };

        let model_ms = self.ctx_ms + self.tokenize_ms + self.prefill_ms + self.generation_ms;
        let total_turn_ms = self
            .turn_start
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0);

        let mut line = format!(
            "[runtime:perf] rounds={} round_labels={} causes={} prompt_sizes={} prefill_ms={} generation_ms={} ctx_ms={} tokenize_ms={} model_load_ms={} tool_ms={} model_ms={} total_turn_ms={} tokens_prompt={} tokens_completion={}",
            self.rounds,
            round_labels,
            causes,
            prompt_sizes,
            self.prefill_ms,
            self.generation_ms,
            self.ctx_ms,
            self.tokenize_ms,
            self.model_load_ms,
            self.tool_ms,
            model_ms,
            total_turn_ms,
            self.tokens_prompt,
            self.tokens_completion,
        );
        if let Some(ctx) = self.context_window_tokens {
            if ctx > 0 {
                let pct = self.tokens_prompt * 100 / u64::from(ctx);
                line.push_str(&format!(" context_used_pct={pct}"));
            }
        }
        on_event(RuntimeEvent::RuntimeTrace(line));
    }
}

pub(crate) fn trace_insufficient_evidence_terminal(
    reason: &str,
    tool_rounds: usize,
    search_budget: &SearchBudget,
    investigation: &InvestigationState,
    on_event: &mut dyn FnMut(RuntimeEvent),
) {
    trace_runtime_decision(
        on_event,
        "terminal_insufficient_evidence",
        &[
            ("reason", reason.to_string()),
            ("rounds", tool_rounds.to_string()),
            ("search_calls", search_budget.calls.to_string()),
            (
                "search_produced_results",
                investigation.search_produced_results().to_string(),
            ),
            ("files_read", investigation.files_read_count().to_string()),
            (
                "candidate_reads",
                investigation.candidate_reads_count().to_string(),
            ),
            ("evidence_ready", investigation.evidence_ready().to_string()),
        ],
    );
}

pub(crate) fn infer_post_tool_round_cause(results: &str) -> GenerationRoundCause {
    if results.contains("=== tool_result: search_code ===") && results.contains("No matches found.")
    {
        GenerationRoundCause::SearchRetry
    } else if results.contains("This is a usage lookup")
        || results.contains("This is a config lookup")
        || results.contains("This is an initialization lookup")
        || results.contains("This is a creation lookup")
        || results.contains("This is a registration lookup")
        || results.contains("This is a load lookup")
        || results.contains("This is a save lookup")
        || results.contains("The file just read contained only import matches")
        || results.contains("The file just read is a lockfile")
    {
        GenerationRoundCause::Recovery
    } else {
        GenerationRoundCause::ToolResults
    }
}

pub(crate) fn short_tool_name(tool_name: &str) -> &str {
    match tool_name {
        "read_file" => "read",
        "list_dir" => "list",
        "search_code" => "search",
        "edit_file" => "edit",
        "write_file" => "write",
        "shell" => "shell",
        "git_status" | "git_diff" | "git_log" => "git",
        other => other,
    }
}

pub(crate) fn tool_input_activity(input: Option<&ToolInput>) -> Activity {
    let (tool, detail) = match input {
        Some(ToolInput::ReadFile { path }) => ("read".to_string(), Some(path.clone())),
        Some(ToolInput::ListDir { path }) => ("list".to_string(), Some(path.clone())),
        Some(ToolInput::SearchCode { query, .. }) => ("search".to_string(), Some(query.clone())),
        Some(ToolInput::EditFile { path, .. }) => ("edit".to_string(), Some(path.clone())),
        Some(ToolInput::WriteFile { path, .. }) => ("write".to_string(), Some(path.clone())),
        Some(ToolInput::Shell { command }) => ("shell".to_string(), Some(command.clone())),
        Some(
            ToolInput::GitStatus | ToolInput::GitDiff | ToolInput::GitLog | ToolInput::GitBranch,
        ) => ("git".to_string(), None),
        None => ("tool".to_string(), None),
    };
    Activity::ExecutingTools { tool, detail }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::backend::BackendTimingStage;

    #[test]
    fn perf_summary_includes_cold_start_and_tool_fields() {
        let mut perf = TurnPerformance::new_enabled(None);

        perf.record_backend_timing(BackendTimingStage::ModelLoad, 4200);
        perf.record_backend_timing(BackendTimingStage::CtxCreate, 50);
        perf.record_backend_timing(BackendTimingStage::Tokenize, 20);
        perf.record_backend_timing(BackendTimingStage::PrefillDone, 1000);
        perf.record_backend_timing(BackendTimingStage::GenerationDone, 800);
        perf.record_tool_elapsed(300);
        perf.record_tool_elapsed(150);

        let mut lines = Vec::new();
        perf.emit_summary(&mut |e| {
            if let RuntimeEvent::RuntimeTrace(line) = e {
                lines.push(line);
            }
        });

        assert_eq!(lines.len(), 1, "expect exactly one summary line");
        let summary = &lines[0];
        assert!(
            summary.contains("model_load_ms=4200"),
            "cold-start field missing: {summary}"
        );
        assert!(
            summary.contains("tool_ms=450"),
            "tool aggregation field missing: {summary}"
        );
        // model_ms = ctx_ms(50) + tokenize_ms(20) + prefill_ms(1000) + generation_ms(800) = 1870
        assert!(
            summary.contains("model_ms=1870"),
            "model-side aggregate missing: {summary}"
        );
        assert!(
            summary.contains("total_turn_ms="),
            "wall-clock turn time missing: {summary}"
        );
    }

    #[test]
    fn perf_token_counts_accumulate_across_rounds() {
        let mut perf = TurnPerformance::new_enabled(None);

        perf.record_token_counts(100, 50);
        perf.record_token_counts(200, 75);

        assert_eq!(perf.tokens_prompt, 300);
        assert_eq!(perf.tokens_completion, 125);
    }

    #[test]
    fn perf_summary_includes_token_fields_when_available() {
        let mut perf = TurnPerformance::new_enabled(None);

        perf.record_token_counts(512, 128);

        let mut lines = Vec::new();
        perf.emit_summary(&mut |e| {
            if let RuntimeEvent::RuntimeTrace(line) = e {
                lines.push(line);
            }
        });

        assert_eq!(lines.len(), 1, "expect exactly one summary line");
        let summary = &lines[0];
        assert!(
            summary.contains("tokens_prompt=512"),
            "tokens_prompt missing: {summary}"
        );
        assert!(
            summary.contains("tokens_completion=128"),
            "tokens_completion missing: {summary}"
        );
        assert!(
            !summary.contains("context_used_pct"),
            "context_used_pct must be absent when context_window_tokens is None: {summary}"
        );
    }

    #[test]
    fn perf_summary_omits_context_used_pct_when_context_window_unknown() {
        let mut perf = TurnPerformance::new_enabled(None);

        perf.record_token_counts(1000, 200);

        let mut lines = Vec::new();
        perf.emit_summary(&mut |e| {
            if let RuntimeEvent::RuntimeTrace(line) = e {
                lines.push(line);
            }
        });

        let summary = &lines[0];
        assert!(
            !summary.contains("context_used_pct"),
            "context_used_pct must not appear when context_window_tokens is None: {summary}"
        );
    }
}
