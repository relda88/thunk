#![cfg(feature = "tree-sitter-parsing")]

use std::path::Path;
use std::time::{Duration, Instant};

use crate::storage::index::types::{ExtractedSymbol, SymbolConfidence, SymbolKind};

const PARSE_TIMEOUT: Duration = Duration::from_millis(500);

/// Parse `source` with tree-sitter using the language inferred from `path`'s extension.
/// Returns `None` if the language is unsupported, the parse times out, or the parse fails.
/// Caller falls back to the regex extractor on `None`.
pub(crate) fn parse_file(
    path: &Path,
    source: &str,
    project_root: &Path,
) -> Option<Vec<ExtractedSymbol>> {
    let ext = path.extension()?.to_str()?;
    let ext_lower = ext.to_ascii_lowercase();
    let ext = ext_lower.as_str();

    let lang: tree_sitter::Language = match ext {
        "rs" => tree_sitter_rust::LANGUAGE.into(),
        "py" => tree_sitter_python::LANGUAGE.into(),
        "js" | "jsx" => tree_sitter_javascript::LANGUAGE.into(),
        "ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        _ => return None,
    };

    let start = Instant::now();
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang).ok()?;
    let tree = parser.parse(source, None)?;
    if start.elapsed() > PARSE_TIMEOUT {
        return None;
    }

    let rel_path = path.strip_prefix(project_root).ok()?;
    let file_path = rel_path.to_string_lossy().replace('\\', "/");

    let mut symbols = Vec::new();
    let root = tree.root_node();
    visit_node(root, source, &file_path, ext, &mut symbols);

    Some(symbols)
}

fn visit_node(
    node: tree_sitter::Node<'_>,
    source: &str,
    file_path: &str,
    ext: &str,
    symbols: &mut Vec<ExtractedSymbol>,
) {
    if let Some(sym) = extract_symbol(&node, source, file_path, ext) {
        symbols.push(sym);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit_node(child, source, file_path, ext, symbols);
    }
}

fn extract_symbol(
    node: &tree_sitter::Node<'_>,
    source: &str,
    file_path: &str,
    ext: &str,
) -> Option<ExtractedSymbol> {
    let (kind, name_field) = match (ext, node.kind()) {
        // Rust
        ("rs", "function_item") => (SymbolKind::Function, "name"),
        ("rs", "struct_item") => (SymbolKind::Struct, "name"),
        ("rs", "enum_item") => (SymbolKind::Enum, "name"),
        ("rs", "trait_item") => (SymbolKind::Trait, "name"),
        ("rs", "type_alias") => (SymbolKind::TypeAlias, "name"),
        ("rs", "const_item") => (SymbolKind::Constant, "name"),
        ("rs", "static_item") => (SymbolKind::Static, "name"),
        ("rs", "impl_item") => (SymbolKind::Impl, "type"),
        // Python
        ("py", "function_definition") => (SymbolKind::Function, "name"),
        ("py", "class_definition") => (SymbolKind::Class, "name"),
        // JS / JSX
        ("js" | "jsx", "function_declaration") => (SymbolKind::Function, "name"),
        ("js" | "jsx", "class_declaration") => (SymbolKind::Class, "name"),
        ("js" | "jsx", "method_definition") => (SymbolKind::Function, "name"),
        // TS / TSX
        ("ts" | "tsx", "function_declaration") => (SymbolKind::Function, "name"),
        ("ts" | "tsx", "class_declaration") => (SymbolKind::Class, "name"),
        ("ts" | "tsx", "method_definition") => (SymbolKind::Function, "name"),
        ("ts" | "tsx", "interface_declaration") => (SymbolKind::Interface, "name"),
        ("ts" | "tsx", "type_alias_declaration") => (SymbolKind::TypeAlias, "name"),
        _ => return None,
    };

    let name_node = node.child_by_field_name(name_field)?;
    let name = name_node.utf8_text(source.as_bytes()).ok()?.to_string();
    if name.is_empty() {
        return None;
    }

    let pos = node.start_position();
    let line = pos.row + 1;
    let col = pos.column + 1;

    // First line of node source trimmed to 120 chars.
    let node_text = &source[node.start_byte()..node.end_byte().min(source.len())];
    let first_line = node_text.lines().next().unwrap_or("").trim();
    let signature = if first_line.len() > 120 {
        first_line[..120].to_string()
    } else {
        first_line.to_string()
    };

    let parent_scope = find_parent_scope(*node, source, ext);

    Some(ExtractedSymbol {
        name,
        kind,
        file_path: file_path.to_string(),
        line,
        col,
        signature,
        confidence: SymbolConfidence::High,
        parent_scope,
    })
}

fn find_parent_scope(node: tree_sitter::Node<'_>, source: &str, ext: &str) -> Option<String> {
    let mut current = node.parent();
    while let Some(parent) = current {
        let scope = match (ext, parent.kind()) {
            ("rs", "impl_item") => parent
                .child_by_field_name("type")
                .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                .map(str::to_string),
            ("py", "class_definition") | ("js" | "jsx" | "ts" | "tsx", "class_declaration") => {
                parent
                    .child_by_field_name("name")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .map(str::to_string)
            }
            _ => None,
        };
        if scope.is_some() {
            return scope;
        }
        current = parent.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        PathBuf::from("/project")
    }

    fn parse(rel: &str, src: &str) -> Vec<ExtractedSymbol> {
        let path = PathBuf::from("/project").join(rel);
        parse_file(&path, src, &root()).unwrap_or_default()
    }

    #[test]
    fn parse_rust_function() {
        let syms = parse("src/lib.rs", "pub fn foo() {}");
        let sym = syms
            .iter()
            .find(|s| s.name == "foo")
            .expect("foo not found");
        assert!(matches!(sym.kind, SymbolKind::Function));
        assert_eq!(sym.line, 1);
        assert_eq!(sym.parent_scope, None);
        assert!(matches!(sym.confidence, SymbolConfidence::High));
    }

    #[test]
    fn parse_rust_impl_method() {
        let src = "impl Bar {\n    pub fn method(&self) {}\n}";
        let syms = parse("src/lib.rs", src);
        let method = syms
            .iter()
            .find(|s| s.name == "method")
            .expect("method not found");
        assert_eq!(method.parent_scope.as_deref(), Some("Bar"));
        assert!(matches!(method.kind, SymbolKind::Function));
    }

    #[test]
    fn parse_rust_struct() {
        let syms = parse("src/lib.rs", "pub struct Foo { x: i32 }");
        let sym = syms
            .iter()
            .find(|s| s.name == "Foo")
            .expect("Foo not found");
        assert!(matches!(sym.kind, SymbolKind::Struct));
        assert_eq!(sym.parent_scope, None);
    }

    #[test]
    fn parse_python_class() {
        let syms = parse("app/main.py", "class Foo:\n    pass\n");
        let sym = syms
            .iter()
            .find(|s| s.name == "Foo")
            .expect("Foo not found");
        assert!(matches!(sym.kind, SymbolKind::Class));
        assert_eq!(sym.parent_scope, None);
    }

    #[test]
    fn parse_python_method() {
        let src = "class Foo:\n    def bar(self):\n        pass\n";
        let syms = parse("app/main.py", src);
        let method = syms
            .iter()
            .find(|s| s.name == "bar")
            .expect("bar not found");
        assert_eq!(method.parent_scope.as_deref(), Some("Foo"));
        assert!(matches!(method.kind, SymbolKind::Function));
    }

    #[test]
    fn parse_unsupported_extension_returns_none() {
        let path = PathBuf::from("/project/config.yaml");
        let result = parse_file(&path, "key: value\n", &root());
        assert!(result.is_none(), "yaml must return None");
    }

    #[test]
    fn parse_typescript_interface() {
        let src = "interface IFoo { bar(): void; }";
        let syms = parse("src/types.ts", src);
        let sym = syms
            .iter()
            .find(|s| s.name == "IFoo")
            .expect("IFoo not found");
        assert!(matches!(sym.kind, SymbolKind::Interface));
    }

    #[test]
    fn parse_javascript_class() {
        let src = "class Widget {\n  render() {}\n}\n";
        let syms = parse("src/widget.js", src);
        let cls = syms
            .iter()
            .find(|s| s.name == "Widget")
            .expect("Widget not found");
        assert!(matches!(cls.kind, SymbolKind::Class));
        let method = syms
            .iter()
            .find(|s| s.name == "render")
            .expect("render not found");
        assert_eq!(method.parent_scope.as_deref(), Some("Widget"));
    }

    #[test]
    fn file_path_is_project_relative() {
        let syms = parse("src/lib.rs", "pub fn greet() {}");
        let sym = syms
            .iter()
            .find(|s| s.name == "greet")
            .expect("greet not found");
        assert_eq!(sym.file_path, "src/lib.rs");
    }
}
