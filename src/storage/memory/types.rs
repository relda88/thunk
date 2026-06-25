#[derive(Debug, Clone)]
pub(crate) enum MemorySource {
    User,
    Reflection,
}

impl MemorySource {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            MemorySource::User => "user",
            MemorySource::Reflection => "reflection",
        }
    }

    pub(crate) fn from_str(s: &str) -> Self {
        match s {
            "reflection" => MemorySource::Reflection,
            _ => MemorySource::User,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MemoryFact {
    pub(crate) id: i64,
    pub(crate) text: String,
    pub(crate) category: String,
    pub(crate) scope: Option<String>,
    pub(crate) salience: f64,
    pub(crate) embedding: Option<Vec<u8>>,
    pub(crate) model_name: Option<String>,
    pub(crate) source: MemorySource,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) last_recalled_at: Option<String>,
}
