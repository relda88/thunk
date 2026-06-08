/// Returns true if the line (after stripping leading whitespace) looks like a symbol definition.
/// Coverage: Rust, Python, Go, TypeScript, JavaScript.
/// C/C++ patterns are excluded — too many false positives without a type parser.
/// No regex, no scoring — prefix matching only.
pub(crate) fn looks_like_definition(line: &str) -> bool {
    let t = line.trim_start();
    // Rust
    t.starts_with("pub enum ")
        || t.starts_with("pub struct ")
        || t.starts_with("pub fn ")
        || t.starts_with("pub type ")
        || t.starts_with("pub trait ")
        || t.starts_with("pub const ")
        || t.starts_with("pub static ")
        || t.starts_with("enum ")
        || t.starts_with("struct ")
        || t.starts_with("fn ")
        || t.starts_with("type ")
        || t.starts_with("const ")
        || t.starts_with("trait ")
        || t.starts_with("impl ")
        // Python / TypeScript / JavaScript (shared keywords)
        || t.starts_with("class ")
        // Python
        || t.starts_with("def ")
        // Go
        || t.starts_with("func ")
        // TypeScript / JavaScript
        || t.starts_with("function ")
        || t.starts_with("interface ")
}

/// Returns true if the line defines exactly `symbol` as its top-level symbol.
/// Trims leading whitespace, strips the definition keyword prefix,
/// extracts the first identifier after that prefix, and compares it exactly to `symbol`.
/// Does NOT match type annotations in function parameters.
pub(crate) fn is_exact_symbol_definition(line: &str, symbol: &str) -> bool {
    let line = line.trim_start();
    let def_prefixes = [
        "pub struct ",
        "pub const ",
        "pub static ",
        "pub enum ",
        "pub fn ",
        "pub type ",
        "pub trait ",
        "function ",
        "interface ",
        "struct ",
        "enum ",
        "class ",
        "impl ",
        "const ",
        "trait ",
        "def ",
        "func ",
        "type ",
        "fn ",
    ];
    for prefix in def_prefixes {
        if let Some(after_prefix) = line.strip_prefix(prefix) {
            let first_ident = after_prefix
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .next();
            if let Some(ident) = first_ident {
                return ident == symbol;
            }
            return false;
        }
    }
    false
}

/// Returns true if the line contains a declaration keyword anywhere on it.
/// Uses contains() — intentionally more permissive than looks_like_definition.
/// Rust-specific; designed for LSP go_to_definition seeding.
pub(crate) fn is_declaration_line(line: &str) -> bool {
    let t = line.trim();
    if t.starts_with("//") || t.starts_with("/*") || t.starts_with("use ") {
        return false;
    }
    t.contains("struct ")
        || t.contains("fn ")
        || t.contains("enum ")
        || t.contains("trait ")
        || t.contains("type ")
        || t.contains("impl ")
        || t.contains("const ")
        || t.contains("static ")
        || t.contains("macro_rules!")
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- looks_like_definition ---

    #[test]
    fn looks_like_definition_matches_rust_keywords() {
        assert!(looks_like_definition("pub enum TaskStatus {"));
        assert!(looks_like_definition("pub struct Config {"));
        assert!(looks_like_definition("pub fn run_turns("));
        assert!(looks_like_definition("pub type Result<T> ="));
        assert!(looks_like_definition("pub trait Backend {"));
        assert!(looks_like_definition("pub const MAX: usize = 10;"));
        assert!(looks_like_definition("pub static INSTANCE: Lazy<Foo>"));
        assert!(looks_like_definition("enum State {"));
        assert!(looks_like_definition("struct Inner {"));
        assert!(looks_like_definition("fn helper("));
        assert!(looks_like_definition("type Alias = u32;"));
        assert!(looks_like_definition("const CAP: usize = 50;"));
        assert!(looks_like_definition("trait Render {"));
        assert!(looks_like_definition("impl TaskStatus {"));
        // leading whitespace stripped
        assert!(looks_like_definition("    pub fn method("));
        assert!(looks_like_definition("\tfn indented("));
    }

    #[test]
    fn looks_like_definition_matches_other_languages() {
        assert!(looks_like_definition("def my_function(self):"));
        assert!(looks_like_definition("class MyService:"));
        assert!(looks_like_definition("func HandleRequest("));
        assert!(looks_like_definition("function onClick("));
        assert!(looks_like_definition("interface UserRepo {"));
        assert!(looks_like_definition(
            "class Component extends React.Component {"
        ));
        assert!(looks_like_definition("type Config = {"));
        assert!(looks_like_definition("const handler = ("));
    }

    #[test]
    fn looks_like_definition_rejects_usage_lines() {
        assert!(!looks_like_definition("let x = TaskStatus::Running;"));
        assert!(!looks_like_definition("task.execute();"));
        assert!(!looks_like_definition(
            "use crate::tools::types::SearchMatch;"
        ));
        assert!(!looks_like_definition("// pub fn commented_out("));
        assert!(!looks_like_definition("println!(\"fn not a definition\");"));
        assert!(!looks_like_definition("x.fn_call()"));
        assert!(!looks_like_definition("result = some_fn(a, b)"));
    }

    // --- is_declaration_line ---

    #[test]
    fn is_declaration_line_accepts_struct() {
        assert!(is_declaration_line(
            "pub(crate) struct InvestigationGraph {"
        ));
    }

    #[test]
    fn is_declaration_line_rejects_comment() {
        assert!(!is_declaration_line(
            "// InvestigationGraph — graph-shaped candidate tracker."
        ));
    }

    // --- is_exact_symbol_definition ---

    #[test]
    fn is_exact_symbol_definition_matches_exact_symbol() {
        assert!(is_exact_symbol_definition("class Task:", "Task"));
        assert!(is_exact_symbol_definition("class Task(Base):", "Task"));
        assert!(is_exact_symbol_definition("def Task(self):", "Task"));
        assert!(is_exact_symbol_definition("struct Task {", "Task"));
        assert!(is_exact_symbol_definition("pub struct Task {", "Task"));
        assert!(is_exact_symbol_definition("fn Task(", "Task"));
        assert!(is_exact_symbol_definition("pub fn Task(", "Task"));
        assert!(is_exact_symbol_definition("interface Task {", "Task"));
        assert!(is_exact_symbol_definition("func Task(", "Task"));
        assert!(is_exact_symbol_definition("type Task =", "Task"));
        assert!(is_exact_symbol_definition("enum Task {", "Task"));
    }

    #[test]
    fn is_exact_symbol_definition_rejects_prefix_symbols() {
        // "Task" must not match lines defining "TaskStatus", "TaskRunner", etc.
        assert!(!is_exact_symbol_definition("class TaskStatus:", "Task"));
        assert!(!is_exact_symbol_definition("class TaskRunner(", "Task"));
        assert!(!is_exact_symbol_definition(
            "def TaskFactory(self):",
            "Task"
        ));
        assert!(!is_exact_symbol_definition("struct TaskManager {", "Task"));
        assert!(!is_exact_symbol_definition("pub enum TaskState {", "Task"));
    }

    #[test]
    fn is_exact_symbol_definition_rejects_type_annotations() {
        // Type annotations in function signatures must not match as definitions.
        assert!(!is_exact_symbol_definition(
            "def _format_task(task: Task) -> str:",
            "Task"
        ));
        assert!(!is_exact_symbol_definition(
            "fn process_data(data: Task) -> Result",
            "Task"
        ));
        assert!(!is_exact_symbol_definition(
            "def create_instance(task: Task) -> None:",
            "Task"
        ));
    }

    #[test]
    fn is_exact_symbol_definition_rejects_non_definition_lines() {
        assert!(!is_exact_symbol_definition("x = Task()", "Task"));
        assert!(!is_exact_symbol_definition("let t = Task::new();", "Task"));
        assert!(!is_exact_symbol_definition(
            "return Task.from_dict(data)",
            "Task"
        ));
        assert!(!is_exact_symbol_definition(
            "from models import Task",
            "Task"
        ));
    }

    #[test]
    fn is_exact_symbol_definition_ascii_alphanumeric_boundary() {
        // A non-ASCII char immediately after the keyword prefix must not produce a match,
        // because is_ascii_alphanumeric() correctly rejects it as an identifier start.
        assert!(!is_exact_symbol_definition("struct \u{00e9}lite {", "lite"));
    }
}
