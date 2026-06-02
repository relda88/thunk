mod tool_detector;
/// tool_codec owns the complete wire protocol between the model and the tool layer.
///
/// Responsibilities:
///   - Parse model output text into typed ToolInput values (inbound)
///   - Format ToolOutput values into conversation text for the model (outbound)
///   - Describe the wire format to the model via format_instructions()
///
/// When the protocol format changes, only this module changes.
/// engine.rs and prompt.rs are unaffected.
mod tool_parser;
mod tool_renderer;

pub(crate) use tool_detector::is_tool_call_message;
pub use tool_detector::{
    contains_edit_attempt, contains_fabricated_exchange, contains_malformed_block,
    detected_malformed_mutation_tool,
};
pub use tool_parser::parse_all_tool_inputs;
pub(crate) use tool_renderer::render_output;
pub use tool_renderer::{
    format_instructions, format_tool_error, format_tool_result,
    format_tool_result_definition_ordered, render_compact_summary,
};
