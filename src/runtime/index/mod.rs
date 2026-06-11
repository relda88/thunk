pub(crate) mod embeddings;
mod extractor;
#[cfg(feature = "tree-sitter-parsing")]
pub(crate) mod tree_sitter_parser;
mod types;

pub(crate) use embeddings::{EmbeddingProvider, OllamaEmbeddingProvider};
pub(crate) use extractor::{
    extract_imports, extract_imports_for_file, extract_symbols, extract_symbols_for_file,
};
pub(crate) use types::{ExtractedSymbol, ImportEdge, SymbolConfidence, SymbolKind};
