// InvestigationGraph — graph-shaped candidate tracker.
// Owned by InvestigationState. All graph operations live here.
// InvestigationState consults self.graph but never implements graph logic.

use std::collections::HashMap;

use petgraph::graph::{Graph, NodeIndex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Relation {
    Imports,
}

#[derive(Debug, Clone)]
pub(crate) struct FileNode {
    pub(crate) path: String,
    pub(crate) read: bool,
}

pub(crate) struct InvestigationGraph {
    graph: Graph<FileNode, Relation>,
    file_to_node: HashMap<String, NodeIndex>,
}

impl InvestigationGraph {
    pub(crate) fn new() -> Self {
        Self {
            graph: Graph::new(),
            file_to_node: HashMap::new(),
        }
    }

    /// Record that path was read and extract its imports.
    /// Promoted candidates are unread nodes connected to any read node.
    pub(crate) fn record_read(&mut self, path: &str, content: &str) {
        let node_idx = self.get_or_create_node(path.to_string());
        self.graph[node_idx].read = true;

        let imports = Self::extract_imports(content);
        for import_path in imports {
            let import_idx = self.get_or_create_node(import_path);
            self.graph.add_edge(node_idx, import_idx, Relation::Imports);
        }
    }

    /// Returns true if the graph has any import edges.
    pub(crate) fn has_edges(&self) -> bool {
        self.graph.edge_count() > 0
    }

    /// Returns unread files imported by any already-read file, in insertion order.
    pub(crate) fn promoted_candidates(&self) -> Vec<String> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for node_idx in self.graph.node_indices() {
            if !self.graph[node_idx].read {
                continue;
            }
            for neighbor_idx in self.graph.neighbors(node_idx) {
                if !self.graph[neighbor_idx].read {
                    let path = self.graph[neighbor_idx].path.clone();
                    if seen.insert(path.clone()) {
                        result.push(path);
                    }
                }
            }
        }
        result
    }

    fn extract_imports(content: &str) -> Vec<String> {
        let mut imports = Vec::new();

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
                        imports.push(format!("{path}.py"));
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
                            imports.push(format!("{path}.py"));
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
                if let Some(path) = Self::extract_js_import_path(trimmed) {
                    if path.contains('/') && !path.starts_with("http") {
                        imports.push(path);
                    }
                }
            }
        }

        imports
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

    fn get_or_create_node(&mut self, path: String) -> NodeIndex {
        if let Some(&idx) = self.file_to_node.get(&path) {
            return idx;
        }
        let idx = self.graph.add_node(FileNode {
            path: path.clone(),
            read: false,
        });
        self.file_to_node.insert(path, idx);
        idx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_imports_python_basic() {
        let content = "from models.task import Task\n";
        let imports = InvestigationGraph::extract_imports(content);
        assert!(
            imports.contains(&"models/task.py".to_string()),
            "expected models/task.py in {imports:?}"
        );
    }

    #[test]
    fn extract_imports_rust_basic() {
        let content = "use crate::tools::types::ToolInput;\n";
        let imports = InvestigationGraph::extract_imports(content);
        assert!(
            imports.is_empty(),
            "crate-relative Rust import should produce no candidates, got {imports:?}"
        );
    }

    #[test]
    fn extract_imports_skips_stdlib() {
        let content = "import os\nimport sys\n";
        let imports = InvestigationGraph::extract_imports(content);
        assert!(
            imports.is_empty(),
            "stdlib imports should produce no candidates, got {imports:?}"
        );
    }

    #[test]
    fn promoted_candidates_returns_unread_imports() {
        let mut graph = InvestigationGraph::new();
        let content = "from models.task import Task\nfrom services.runner import Runner\n";
        graph.record_read("app/main.py", content);

        let promoted = graph.promoted_candidates();
        assert!(
            promoted.contains(&"models/task.py".to_string()),
            "expected models/task.py promoted, got {promoted:?}"
        );
        assert!(
            promoted.contains(&"services/runner.py".to_string()),
            "expected services/runner.py promoted, got {promoted:?}"
        );
    }

    #[test]
    fn promoted_candidates_empty_before_any_read() {
        let graph = InvestigationGraph::new();
        let promoted = graph.promoted_candidates();
        assert!(
            promoted.is_empty(),
            "expected empty before any reads, got {promoted:?}"
        );
    }
}
