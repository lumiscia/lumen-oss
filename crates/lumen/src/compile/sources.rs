use std::collections::HashMap;

use crate::model::{Source, SourceKind, SourceMedia};

use super::{CompileError, CompiledSource};

#[derive(Debug, Clone)]
pub(super) struct CompiledSourceRef {
    pub(super) index: usize,
    pub(super) id: String,
    pub(super) media: SourceMedia,
    pub(super) path: String,
}

pub(super) fn compile_sources(
    sources: &[Source],
) -> Result<HashMap<String, CompiledSourceRef>, CompileError> {
    let mut lookup = HashMap::with_capacity(sources.len());
    for (index, source) in sources.iter().enumerate() {
        if lookup.contains_key(source.id.as_str()) {
            return Err(CompileError::DuplicateSourceId(source.id.clone()));
        }

        let path = match &source.kind {
            SourceKind::File { path } => path.clone(),
            SourceKind::Url { .. } => {
                return Err(CompileError::UrlSourceUnsupported {
                    source_id: source.id.clone(),
                });
            }
        };

        lookup.insert(
            source.id.clone(),
            CompiledSourceRef {
                index,
                id: source.id.clone(),
                media: source.media,
                path,
            },
        );
    }
    Ok(lookup)
}

pub(super) fn sorted_sources(lookup: &HashMap<String, CompiledSourceRef>) -> Vec<CompiledSource> {
    let mut entries = lookup.values().cloned().collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.index);
    entries
        .into_iter()
        .map(|entry| CompiledSource {
            id: entry.id,
            media: entry.media,
            path: entry.path,
        })
        .collect()
}
