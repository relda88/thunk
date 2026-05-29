#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    TypeAlias,
    Constant,
    Static,
    Impl,
    Class,
    Interface,
    Unknown,
}

impl SymbolKind {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            SymbolKind::Function => "Function",
            SymbolKind::Struct => "Struct",
            SymbolKind::Enum => "Enum",
            SymbolKind::Trait => "Trait",
            SymbolKind::TypeAlias => "TypeAlias",
            SymbolKind::Constant => "Constant",
            SymbolKind::Static => "Static",
            SymbolKind::Impl => "Impl",
            SymbolKind::Class => "Class",
            SymbolKind::Interface => "Interface",
            SymbolKind::Unknown => "Unknown",
        }
    }

    pub(crate) fn from_str(s: &str) -> Self {
        match s {
            "Function" => SymbolKind::Function,
            "Struct" => SymbolKind::Struct,
            "Enum" => SymbolKind::Enum,
            "Trait" => SymbolKind::Trait,
            "TypeAlias" => SymbolKind::TypeAlias,
            "Constant" => SymbolKind::Constant,
            "Static" => SymbolKind::Static,
            "Impl" => SymbolKind::Impl,
            "Class" => SymbolKind::Class,
            "Interface" => SymbolKind::Interface,
            _ => SymbolKind::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SymbolConfidence {
    High,
    Medium,
    Low,
}

impl SymbolConfidence {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            SymbolConfidence::High => "High",
            SymbolConfidence::Medium => "Medium",
            SymbolConfidence::Low => "Low",
        }
    }

    pub(crate) fn from_str(s: &str) -> Self {
        match s {
            "High" => SymbolConfidence::High,
            "Low" => SymbolConfidence::Low,
            _ => SymbolConfidence::Medium,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ExtractedSymbol {
    pub(crate) name: String,
    pub(crate) kind: SymbolKind,
    /// Project-relative path.
    pub(crate) file_path: String,
    /// 1-indexed line number.
    pub(crate) line: usize,
    /// Always 1 for heuristic extraction.
    pub(crate) col: usize,
    /// Full trimmed definition line.
    pub(crate) signature: String,
    pub(crate) confidence: SymbolConfidence,
}

#[derive(Debug, Clone)]
pub(crate) struct ImportEdge {
    pub(crate) from_file: String,
    pub(crate) to_file: String,
}
