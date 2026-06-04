use std::fs;
use std::path::PathBuf;

use crate::dirs::DEFAULT_SKIP_DIRS;
use crate::runtime::project::ProjectRoot;

use super::types::{ExtractedSymbol, ImportEdge, SymbolConfidence, SymbolKind};

const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "py", "ts", "tsx", "js", "jsx", "go", "java", "c", "cpp", "h", "hpp",
];

pub(crate) fn extract_symbols(root: &ProjectRoot) -> Vec<ExtractedSymbol> {
    let mut symbols = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.path().to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let name = match entry.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue,
            };

            if path.is_dir() {
                if DEFAULT_SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                stack.push(path);
            } else {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase());
                let is_source = ext
                    .as_deref()
                    .map(|e| SOURCE_EXTENSIONS.contains(&e))
                    .unwrap_or(false);
                if !is_source {
                    continue;
                }

                let content = match fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                let rel = match path.strip_prefix(root.path()) {
                    Ok(r) => r.to_string_lossy().replace('\\', "/"),
                    Err(_) => continue,
                };

                #[cfg(feature = "tree-sitter-parsing")]
                if let Some(ts_symbols) =
                    super::tree_sitter_parser::parse_file(&path, &content, root.path())
                {
                    symbols.extend(ts_symbols);
                    continue;
                }

                extract_from_file(&content, &rel, &mut symbols);
            }
        }
    }

    symbols
}

pub(crate) fn extract_imports(root: &ProjectRoot) -> Vec<ImportEdge> {
    let mut edges = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.path().to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let name = match entry.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue,
            };

            if path.is_dir() {
                if DEFAULT_SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                stack.push(path);
            } else {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase());
                let is_source = ext
                    .as_deref()
                    .map(|e| SOURCE_EXTENSIONS.contains(&e))
                    .unwrap_or(false);
                if !is_source {
                    continue;
                }

                let content = match fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                let rel = match path.strip_prefix(root.path()) {
                    Ok(r) => r.to_string_lossy().replace('\\', "/"),
                    Err(_) => continue,
                };

                extract_imports_from_file(&content, &rel, &mut edges);
            }
        }
    }

    edges
}

fn extract_imports_from_file(content: &str, file_path: &str, out: &mut Vec<ImportEdge>) {
    for line in content.lines() {
        let trimmed = line.trim_start();

        // Python: `import foo.bar.baz`
        if trimmed.starts_with("import ") {
            let rest = &trimmed["import ".len()..];
            let module = rest
                .split(|c: char| c == ',' || c == ' ' || c == '#' || c == ';')
                .next()
                .unwrap_or("")
                .trim();
            if !module.is_empty() && !module.starts_with('.') {
                let path = module.replace('.', "/");
                if path.contains('/') {
                    out.push(ImportEdge {
                        from_file: file_path.to_string(),
                        to_file: format!("{path}.py"),
                    });
                }
            }
        // Python: `from foo.bar import Baz`
        } else if trimmed.starts_with("from ")
            && !trimmed.contains("from '")
            && !trimmed.contains("from \"")
        {
            let rest = &trimmed["from ".len()..];
            if let Some(module_part) = rest.split(" import").next() {
                let module = module_part.trim();
                if !module.is_empty() && !module.starts_with('.') {
                    let path = module.replace('.', "/");
                    if path.contains('/') {
                        out.push(ImportEdge {
                            from_file: file_path.to_string(),
                            to_file: format!("{path}.py"),
                        });
                    }
                }
            }
        // Rust: `use path::component;` — conservative: only produces candidates when
        // the first component is not a known stdlib/crate-relative prefix.
        // In practice all current Rust imports are crate-relative or external, so
        // this branch records no candidates. Kept for future extension.
        } else if trimmed.starts_with("use ") {
            let rest = &trimmed["use ".len()..];
            let component = rest
                .split("::")
                .next()
                .unwrap_or("")
                .trim_matches('{')
                .trim();
            match component {
                "std" | "core" | "alloc" | "crate" | "super" | "self" => {}
                _ => {
                    // External crate name — cannot map to a file path without manifest
                    // inspection; skip to avoid false positives.
                }
            }
        }

        // JS/TS: `import ... from './path'` or `import ... from "./path"`
        if trimmed.contains("from '") || trimmed.contains("from \"") {
            if let Some(path) = extract_js_import_path(trimmed) {
                if path.contains('/') && !path.starts_with("http") {
                    out.push(ImportEdge {
                        from_file: file_path.to_string(),
                        to_file: path,
                    });
                }
            }
        }
    }
}

fn extract_js_import_path(line: &str) -> Option<String> {
    for (quote_start, quote_end) in [("from '", '\''), ("from \"", '"')] {
        if let Some(pos) = line.rfind(quote_start) {
            let after = &line[pos + quote_start.len()..];
            if let Some(end) = after.find(quote_end) {
                let path = &after[..end];
                if !path.is_empty() {
                    return Some(path.to_string());
                }
            }
        }
    }
    None
}

fn extract_from_file(content: &str, file_path: &str, out: &mut Vec<ExtractedSymbol>) {
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        if let Some(sym) = classify_line(line, file_path, line_no) {
            out.push(sym);
        }
    }
}

// Prefix table: (prefix, kind, has_pub).
// Longer/more-specific prefixes must come first so "pub fn " matches before "fn ".
const PREFIXES: &[(&str, SymbolKind, bool)] = &[
    ("pub enum ", SymbolKind::Enum, true),
    ("pub struct ", SymbolKind::Struct, true),
    ("pub fn ", SymbolKind::Function, true),
    ("pub type ", SymbolKind::TypeAlias, true),
    ("pub trait ", SymbolKind::Trait, true),
    ("pub const ", SymbolKind::Constant, true),
    ("pub static ", SymbolKind::Static, true),
    ("enum ", SymbolKind::Enum, false),
    ("struct ", SymbolKind::Struct, false),
    ("fn ", SymbolKind::Function, false),
    ("type ", SymbolKind::TypeAlias, false),
    ("const ", SymbolKind::Constant, false),
    ("trait ", SymbolKind::Trait, false),
    ("impl ", SymbolKind::Impl, false),
    ("class ", SymbolKind::Class, false),
    ("def ", SymbolKind::Function, false),
    ("func ", SymbolKind::Function, false),
    ("function ", SymbolKind::Function, false),
    ("interface ", SymbolKind::Interface, false),
    ("static ", SymbolKind::Static, false),
    ("pub(crate) enum ", SymbolKind::Enum, true),
    ("pub(crate) struct ", SymbolKind::Struct, true),
    ("pub(crate) fn ", SymbolKind::Function, true),
    ("pub(crate) type ", SymbolKind::TypeAlias, true),
    ("pub(crate) trait ", SymbolKind::Trait, true),
    ("pub(crate) const ", SymbolKind::Constant, true),
    ("pub(crate) static ", SymbolKind::Static, true),
    ("pub(super) fn ", SymbolKind::Function, true),
];

fn classify_line(line: &str, file_path: &str, line_no: usize) -> Option<ExtractedSymbol> {
    let t = line.trim_start();
    let signature = t.to_string();

    for (prefix, kind, has_pub) in PREFIXES {
        let Some(rest) = t.strip_prefix(prefix) else {
            continue;
        };

        let (name, confidence) = if matches!(kind, SymbolKind::Impl) {
            // "impl Foo" or "impl Trait for Foo" — take the last token before '{' or '<'.
            let trimmed = rest
                .split(|c| c == '{' || c == '<')
                .next()
                .unwrap_or(rest)
                .trim();
            let name = trimmed.split_whitespace().last().unwrap_or("").to_string();
            if name.is_empty() {
                continue;
            }
            (name, SymbolConfidence::Low)
        } else {
            let ident: String = rest
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .next()
                .unwrap_or("")
                .to_string();
            if ident.is_empty() {
                continue;
            }
            let conf = if *has_pub {
                SymbolConfidence::High
            } else {
                SymbolConfidence::Medium
            };
            (ident, conf)
        };

        return Some(ExtractedSymbol {
            name,
            kind: kind.clone(),
            file_path: file_path.to_string(),
            line: line_no,
            col: 1,
            signature,
            confidence,
            parent_scope: None,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::runtime::project::ProjectRoot;

    fn make_root(dir: &TempDir) -> ProjectRoot {
        ProjectRoot::new(dir.path().to_path_buf()).unwrap()
    }

    fn write(dir: &TempDir, rel: &str, content: &str) {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn detects_pub_fn() {
        let dir = TempDir::new().unwrap();
        write(&dir, "src/lib.rs", "pub fn hello() {}\n");
        let root = make_root(&dir);
        let syms = extract_symbols(&root);
        let sym = syms.iter().find(|s| s.name == "hello").unwrap();
        assert!(matches!(sym.kind, SymbolKind::Function));
        assert!(matches!(sym.confidence, SymbolConfidence::High));
        assert_eq!(sym.line, 1);
        assert_eq!(sym.col, 1);
        assert_eq!(sym.signature, "pub fn hello() {}");
    }

    #[test]
    fn detects_bare_struct() {
        let dir = TempDir::new().unwrap();
        write(&dir, "src/lib.rs", "struct Foo {\n    x: i32,\n}\n");
        let root = make_root(&dir);
        let syms = extract_symbols(&root);
        let sym = syms.iter().find(|s| s.name == "Foo").unwrap();
        assert!(matches!(sym.kind, SymbolKind::Struct));
        assert!(matches!(sym.confidence, SymbolConfidence::Medium));
    }

    #[test]
    fn detects_impl_trait_for_type() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "src/lib.rs",
            "impl Display for Foo {\n    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { Ok(()) }\n}\n",
        );
        let root = make_root(&dir);
        let syms = extract_symbols(&root);
        let sym = syms.iter().find(|s| s.name == "Foo").unwrap();
        assert!(matches!(sym.kind, SymbolKind::Impl));
        assert!(matches!(sym.confidence, SymbolConfidence::Low));
    }

    #[test]
    fn skips_non_source_extensions() {
        let dir = TempDir::new().unwrap();
        write(&dir, "README.md", "pub fn not_a_symbol() {}\n");
        write(&dir, "config.toml", "fn also_not() {}\n");
        let root = make_root(&dir);
        let syms = extract_symbols(&root);
        assert!(
            syms.is_empty(),
            "expected no symbols from non-source files, got {syms:?}"
        );
    }

    #[test]
    fn skips_default_skip_dirs() {
        let dir = TempDir::new().unwrap();
        write(&dir, "target/debug/src.rs", "pub fn hidden() {}\n");
        write(&dir, "node_modules/pkg/index.js", "function hidden() {}\n");
        let root = make_root(&dir);
        let syms = extract_symbols(&root);
        assert!(
            syms.is_empty(),
            "expected no symbols from skip dirs, got {syms:?}"
        );
    }

    #[test]
    fn file_path_is_project_relative() {
        let dir = TempDir::new().unwrap();
        write(&dir, "src/foo.rs", "pub struct Bar;\n");
        let root = make_root(&dir);
        let syms = extract_symbols(&root);
        let sym = syms.iter().find(|s| s.name == "Bar").unwrap();
        assert_eq!(sym.file_path, "src/foo.rs");
    }

    #[test]
    fn detects_pub_enum() {
        let dir = TempDir::new().unwrap();
        write(&dir, "src/lib.rs", "pub enum Color { Red, Green, Blue }\n");
        let root = make_root(&dir);
        let syms = extract_symbols(&root);
        let sym = syms.iter().find(|s| s.name == "Color").unwrap();
        assert!(matches!(sym.kind, SymbolKind::Enum));
        assert!(matches!(sym.confidence, SymbolConfidence::High));
    }

    #[test]
    fn extract_imports_from_file_python_dotted() {
        let mut edges = Vec::new();
        extract_imports_from_file("from models.task import Task\n", "app/main.py", &mut edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from_file, "app/main.py");
        assert_eq!(edges[0].to_file, "models/task.py");
    }

    #[test]
    fn extract_imports_from_file_js_relative() {
        let mut edges = Vec::new();
        extract_imports_from_file(
            "import { Foo } from './components/foo';\n",
            "src/app.ts",
            &mut edges,
        );
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from_file, "src/app.ts");
        assert_eq!(edges[0].to_file, "./components/foo");
    }

    #[test]
    fn extract_imports_from_file_rust_crate_relative_skipped() {
        let mut edges = Vec::new();
        extract_imports_from_file(
            "use crate::tools::types::ToolInput;\n",
            "src/lib.rs",
            &mut edges,
        );
        assert!(
            edges.is_empty(),
            "crate-relative Rust import must produce no edges, got {edges:?}"
        );
    }

    #[test]
    fn extract_imports_traverses_files() {
        let dir = TempDir::new().unwrap();
        write(&dir, "app/main.py", "from models.task import Task\n");
        let root = make_root(&dir);
        let edges = extract_imports(&root);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from_file, "app/main.py");
        assert_eq!(edges[0].to_file, "models/task.py");
    }
}
