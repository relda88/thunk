pub(crate) mod embeddings;
mod extractor;
#[cfg(feature = "tree-sitter-parsing")]
pub(crate) mod tree_sitter_parser;
mod types;

pub(crate) use embeddings::{EmbeddingProvider, OllamaEmbeddingProvider};
pub(crate) use extractor::{extract_imports, extract_symbols};
pub(crate) use types::{ExtractedSymbol, ImportEdge, SymbolConfidence, SymbolKind};
