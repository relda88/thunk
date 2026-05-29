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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SymbolConfidence {
    High,
    Medium,
    Low,
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
