use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;

use crate::core::error::{AppError, Result};

/// Tools that user-defined commands are permitted to invoke.
/// Mutating tools are excluded by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowedCommandTool {
    ReadFile,
    SearchCode,
}

impl AllowedCommandTool {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "read_file" => Some(Self::ReadFile),
            "search_code" => Some(Self::SearchCode),
            _ => None,
        }
    }

    fn required_arg_key(self) -> &'static str {
        match self {
            Self::ReadFile => "path",
            Self::SearchCode => "query",
        }
    }
}

/// A validated user-defined command loaded from config.
#[derive(Debug, Clone)]
pub struct CustomCommandDef {
    pub tool: AllowedCommandTool,
    /// Argument value template. Contains `{input}` exactly once.
    pub template: String,
}

/// Raw deserialization target for a single `[commands.<name>]` entry.
#[derive(Debug, Deserialize)]
struct RawCustomCommand {
    tool: String,
    args: HashMap<String, String>,
}

impl<'de> Deserialize<'de> for CustomCommandDef {
    fn deserialize<D>(d: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawCustomCommand::deserialize(d)?;

        let tool = AllowedCommandTool::from_str(&raw.tool).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "unknown tool '{}': allowed values are 'read_file', 'search_code'",
                raw.tool
            ))
        })?;

        let key = tool.required_arg_key();

        if raw.args.len() != 1 {
            return Err(serde::de::Error::custom(format!(
                "expected exactly one arg key '{}', found {} keys",
                key,
                raw.args.len()
            )));
        }

        let template = raw.args.get(key).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "missing required arg key '{}' for tool '{}'",
                key, raw.tool
            ))
        })?;

        let count = template.matches("{input}").count();
        if count != 1 {
            return Err(serde::de::Error::custom(format!(
                "template must contain '{{input}}' exactly once, found {count} occurrence(s)"
            )));
        }

        Ok(CustomCommandDef {
            tool,
            template: template.clone(),
        })
    }
}

/// Built-in command names that custom commands must not shadow.
const BUILTIN_COMMAND_NAMES: &[&str] = &[
    "help", "quit", "exit", "clear", "approve", "reject", "last", "anchors", "history", "read",
    "search",
];

fn validate_command_names(commands: &HashMap<String, CustomCommandDef>) -> Result<()> {
    for name in commands.keys() {
        if name.is_empty() {
            return Err(AppError::Config(
                "custom command name cannot be empty".to_string(),
            ));
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(AppError::Config(format!(
                "custom command name '{name}' must contain only lowercase letters, digits, and underscores"
            )));
        }
        if BUILTIN_COMMAND_NAMES.contains(&name.as_str()) {
            return Err(AppError::Config(format!(
                "custom command name '{name}' conflicts with a built-in command"
            )));
        }
    }
    Ok(())
}

fn default_two() -> u32 {
    2
}

fn default_lsp_extensions() -> Vec<String> {
    vec!["rs".into()]
}

/// Per-project settings that customize runtime behavior for a specific codebase.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    pub test_command: Option<String>,
    /// Shell command to run after an approved mutation.
    /// None = disabled. Examples:
    ///   "cargo check"     (Rust)
    ///   "ruff check ."    (Python)
    ///   "tsc --noEmit"    (TypeScript)
    pub verify_command: Option<String>,
    /// Maximum number of self-correction attempts after a verify command failure.
    /// Each attempt injects a correction prompt, gets a new edit from the model,
    /// and presents it for user approval. 0 = corrections disabled.
    #[serde(default = "default_two")]
    pub max_correction_attempts: u32,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            test_command: None,
            verify_command: None,
            max_correction_attempts: 2,
        }
    }
}

/// LSP provider configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LspConfig {
    /// Must be explicitly set to true to activate LSP. Defaults to false so existing
    /// users see zero behavior change.
    pub enabled: bool,
    /// Absolute path to a rust-analyzer binary. When absent, the runtime probes PATH
    /// and common install locations.
    pub rust_analyzer_path: Option<PathBuf>,
    /// Milliseconds to wait for a single LSP query response before returning a timeout
    /// error. The session is kept alive on timeout — only a crash clears it.
    pub timeout_ms: u64,
    /// Milliseconds to wait for the first `publishDiagnostics` notification after server
    /// startup. This absorbs initial indexing time. Timeout here is not an error — the
    /// session proceeds and per-query retries handle residual not-ready responses.
    pub startup_timeout_ms: u64,
    /// File extensions the LSP server handles. Pre-check and diagnostics are skipped
    /// for files with extensions not in this list. Defaults to ["rs"] for rust-analyzer.
    #[serde(default = "default_lsp_extensions")]
    pub extensions: Vec<String>,
}

impl Default for LspConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            rust_analyzer_path: None,
            timeout_ms: 5000,
            startup_timeout_ms: 30000,
            extensions: default_lsp_extensions(),
        }
    }
}

/// Prompt physics injection settings.
/// Enabled by default — set `[prompt_physics]\nenabled = false` to opt out.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PromptPhysicsSettings {
    pub enabled: bool,
}

impl Default for PromptPhysicsSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RetrievalConfig {
    pub embedding_model: Option<String>,
    pub vector_weight: f32,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            embedding_model: None,
            vector_weight: 0.3,
        }
    }
}

/// Web fetch configuration. Enabled by default; set `[web_fetch]\nenabled = false` to disable.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WebFetchConfig {
    pub enabled: bool,
}

impl Default for WebFetchConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Investigation depth for retrieval — how aggressively the runtime follows import chains.
/// `shallow`: stop after the first useful candidate read.
/// `normal`: current behavior (up to two candidate reads).
/// `deep`: after candidate exhaustion, follow import/definition edges for up to `hop_limit`
///         additional hops, bounded by `max_total_reads`. Call-site following is not
///         available (no callers index); only import chains and definition-site edges are used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InvestigationDepth {
    Shallow,
    #[default]
    Normal,
    Deep,
}

impl InvestigationDepth {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shallow => "shallow",
            Self::Normal => "normal",
            Self::Deep => "deep",
        }
    }
}

/// Investigation behavior settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct InvestigationConfig {
    pub depth: InvestigationDepth,
    /// Maximum number of deepening hops in deep mode (import-chain following after
    /// initial candidate exhaustion). Only used when `depth = "deep"`.
    pub hop_limit: usize,
    /// Hard cap on total reads per turn in deep mode (initial reads + deepening reads).
    /// Only used when `depth = "deep"`.
    pub max_total_reads: usize,
}

impl Default for InvestigationConfig {
    fn default() -> Self {
        Self {
            depth: InvestigationDepth::Normal,
            hop_limit: 3,
            max_total_reads: 10,
        }
    }
}

/// Main configuration struct for the application
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub app: AppConfig,
    pub ui: UiConfig,
    pub llm: LlmConfig,
    pub llama_cpp: LlamaCppConfig,
    pub openai: OpenAiConfig,
    pub ollama: OllamaConfig,
    pub openrouter: OpenRouterConfig,
    pub groq: GroqConfig,
    pub lsp: LspConfig,
    pub commands: HashMap<String, CustomCommandDef>,
    pub project: ProjectConfig,
    pub prompt_physics: PromptPhysicsSettings,
    pub web_fetch: WebFetchConfig,
    pub retrieval: RetrievalConfig,
    pub investigation: InvestigationConfig,
}

/// Application configuration for the app
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub name: String,
}

/// Default app config with the name set to "thunk"
impl Default for AppConfig {
    fn default() -> Self {
        Self {
            name: "thunk".to_string(),
        }
    }
}

/// UI configuration for the application
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub show_activity: bool,
}

/// Default UI config with activity display enabled
impl Default for UiConfig {
    fn default() -> Self {
        Self {
            show_activity: true,
        }
    }
}

/// Model provider selection for the application
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub provider: String,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: "mock".to_string(),
        }
    }
}

/// llama.cpp provider configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LlamaCppConfig {
    pub model_path: Option<PathBuf>,
    pub gpu_layers: u32,
    pub context_tokens: u32,
    pub batch_tokens: u32,
    pub max_tokens: usize,
    pub temperature: f32,
    pub show_native_logs: bool,
}

/// Default llama.cpp config with no model path and reasonable defaults for other parameters
impl Default for LlamaCppConfig {
    fn default() -> Self {
        Self {
            model_path: None,
            gpu_layers: 0,
            context_tokens: 2048,
            batch_tokens: 256,
            max_tokens: 512,
            temperature: 0.7,
            show_native_logs: false,
        }
    }
}

/// OpenAI provider configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OpenAiConfig {
    pub model: String,
    pub base_url: String,
    pub max_tokens: usize,
    pub temperature: f32,
    /// Overrides the default context window size (128 000) used for the usage indicator.
    pub context_window_tokens: Option<u32>,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            base_url: "https://api.openai.com/v1".to_string(),
            max_tokens: 512,
            temperature: 0.2,
            context_window_tokens: None,
        }
    }
}

/// Ollama provider configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OllamaConfig {
    pub model: String,
    pub base_url: String,
    pub max_tokens: u32,
    pub temperature: f32,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            model: "gemma3:1b".to_string(),
            base_url: "http://localhost:11434".to_string(),
            max_tokens: 512,
            temperature: 0.2,
        }
    }
}

/// OpenRouter provider configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OpenRouterConfig {
    pub model: String,
    pub base_url: String,
    pub max_tokens: u32,
    pub temperature: f32,
    /// Overrides the default context window size (128 000) used for the usage indicator.
    pub context_window_tokens: Option<u32>,
}

impl Default for OpenRouterConfig {
    fn default() -> Self {
        Self {
            model: "anthropic/claude-3-haiku".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            max_tokens: 512,
            temperature: 0.2,
            context_window_tokens: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GroqConfig {
    pub model: String,
    pub base_url: String,
    pub max_tokens: u32,
    pub temperature: f32,
    /// Overrides the default context window size (131 072) used for the usage indicator.
    pub context_window_tokens: Option<u32>,
}

impl Default for GroqConfig {
    fn default() -> Self {
        Self {
            model: "qwen-qwq-32b".to_string(),
            base_url: "https://api.groq.com/openai/v1".to_string(),
            max_tokens: 512,
            temperature: 0.2,
            context_window_tokens: None,
        }
    }
}

/// Resolves relative paths in the config to absolute paths based on the provided root directory
impl Config {
    pub fn resolve_paths(mut self, root_dir: &Path) -> Self {
        self.llama_cpp.resolve_paths(root_dir);
        self
    }
}

/// Resolves relative paths in the llama.cpp config to absolute paths based on the provided root directory
impl LlamaCppConfig {
    fn resolve_paths(&mut self, root_dir: &Path) {
        if let Some(model_path) = self.model_path.as_mut() {
            if model_path.is_relative() {
                *model_path = root_dir.join(&*model_path);
            }
        }
    }
}

/// Loads the config from a TOML file at the specified path, or returns defaults if absent.
pub fn load(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Ok(Config::default());
    }

    let raw = fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok(Config::default());
    }

    let config: Config = toml::from_str(&raw)?;
    validate_command_names(&config.commands)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        validate_command_names, AllowedCommandTool, Config, CustomCommandDef, LlamaCppConfig,
    };

    fn parse_config(toml: &str) -> Config {
        toml::from_str(toml).expect("config parse failed")
    }

    fn parse_config_err(toml: &str) -> String {
        toml::from_str::<Config>(toml)
            .err()
            .expect("expected parse error")
            .to_string()
    }

    #[test]
    fn custom_search_command_parses_correctly() {
        let cfg = parse_config(
            r#"
            [commands.find_def]
            tool = "search_code"
            args = { query = "{input}" }
        "#,
        );
        let def = cfg.commands.get("find_def").expect("find_def missing");
        assert_eq!(def.tool, AllowedCommandTool::SearchCode);
        assert_eq!(def.template, "{input}");
    }

    #[test]
    fn custom_read_command_parses_correctly() {
        let cfg = parse_config(
            r#"
            [commands.show]
            tool = "read_file"
            args = { path = "src/{input}" }
        "#,
        );
        let def = cfg.commands.get("show").expect("show missing");
        assert_eq!(def.tool, AllowedCommandTool::ReadFile);
        assert_eq!(def.template, "src/{input}");
    }

    #[test]
    fn unknown_tool_is_rejected() {
        let err = parse_config_err(
            r#"
            [commands.bad]
            tool = "write_file"
            args = { path = "{input}" }
        "#,
        );
        assert!(err.contains("unknown tool"), "unexpected error: {err}");
    }

    #[test]
    fn wrong_arg_key_is_rejected() {
        let err = parse_config_err(
            r#"
            [commands.bad]
            tool = "search_code"
            args = { path = "{input}" }
        "#,
        );
        assert!(
            err.contains("missing required arg key"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn extra_arg_key_is_rejected() {
        let err = parse_config_err(
            r#"
            [commands.bad]
            tool = "search_code"
            args = { query = "{input}", extra = "value" }
        "#,
        );
        assert!(
            err.contains("exactly one arg key"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn missing_input_placeholder_is_rejected() {
        let err = parse_config_err(
            r#"
            [commands.bad]
            tool = "search_code"
            args = { query = "hardcoded" }
        "#,
        );
        assert!(err.contains("exactly once"), "unexpected error: {err}");
    }

    #[test]
    fn duplicate_input_placeholder_is_rejected() {
        let err = parse_config_err(
            r#"
            [commands.bad]
            tool = "search_code"
            args = { query = "{input}{input}" }
        "#,
        );
        assert!(err.contains("exactly once"), "unexpected error: {err}");
    }

    #[test]
    fn invalid_name_chars_are_rejected() {
        use std::collections::HashMap;
        let mut commands = HashMap::new();
        commands.insert(
            "bad-name".to_string(),
            CustomCommandDef {
                tool: AllowedCommandTool::SearchCode,
                template: "{input}".to_string(),
            },
        );
        let err = validate_command_names(&commands).unwrap_err();
        assert!(err.to_string().contains("lowercase letters"), "{err}");
    }

    #[test]
    fn builtin_name_collision_is_rejected() {
        use std::collections::HashMap;
        let mut commands = HashMap::new();
        commands.insert(
            "search".to_string(),
            CustomCommandDef {
                tool: AllowedCommandTool::SearchCode,
                template: "{input}".to_string(),
            },
        );
        let err = validate_command_names(&commands).unwrap_err();
        assert!(
            err.to_string().contains("conflicts with a built-in"),
            "{err}"
        );
    }

    #[test]
    fn empty_commands_map_is_valid() {
        let cfg = parse_config("[app]\nname = \"thunk\"");
        assert!(cfg.commands.is_empty());
    }

    #[test]
    fn lsp_config_defaults() {
        let cfg = parse_config("[lsp]");
        assert!(!cfg.lsp.enabled);
        assert_eq!(cfg.lsp.timeout_ms, 5000);
        assert_eq!(cfg.lsp.startup_timeout_ms, 30000);
        assert!(cfg.lsp.rust_analyzer_path.is_none());
    }

    #[test]
    fn project_test_command_deserializes_correctly() {
        let cfg = parse_config(
            r#"
            [project]
            test_command = "cargo test"
        "#,
        );
        assert_eq!(cfg.project.test_command.as_deref(), Some("cargo test"));
    }

    #[test]
    fn ollama_config_deserializes_with_default_base_url() {
        let cfg = parse_config(
            r#"
            [ollama]
            model = "llama3:8b"
        "#,
        );
        assert_eq!(cfg.ollama.model, "llama3:8b");
        assert_eq!(cfg.ollama.base_url, "http://localhost:11434");
        assert_eq!(cfg.ollama.max_tokens, 512);
    }

    #[test]
    fn openrouter_config_deserializes_with_default_base_url() {
        let cfg = parse_config(
            r#"
            [openrouter]
            model = "openai/gpt-4o"
        "#,
        );
        assert_eq!(cfg.openrouter.model, "openai/gpt-4o");
        assert_eq!(cfg.openrouter.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(cfg.openrouter.max_tokens, 512);
    }

    #[test]
    fn resolves_relative_llama_model_paths_from_project_root() {
        let mut config = Config::default();
        config.llama_cpp = LlamaCppConfig {
            model_path: Some("data/models/model.gguf".into()),
            gpu_layers: 0,
            context_tokens: 2048,
            batch_tokens: 256,
            max_tokens: 128,
            temperature: 0.5,
            show_native_logs: false,
        };

        let resolved = config.resolve_paths(Path::new("/tmp/project"));
        assert_eq!(
            resolved.llama_cpp.model_path.as_deref(),
            Some(Path::new("/tmp/project/data/models/model.gguf"))
        );
    }
}
