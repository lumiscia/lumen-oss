use std::{
    collections::HashMap,
    sync::{Mutex, PoisonError},
};

use crate::{
    error::PropertyError,
    gpu_image::GpuImageFrame,
    node::{NodeId, NodeProperty, PortRef},
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Default)]
pub struct MemoCache {
    entries: Mutex<HashMap<String, GpuImageFrame>>,
}

impl MemoCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, cache_id: &str) -> Option<GpuImageFrame> {
        lock(&self.entries).get(cache_id).cloned()
    }

    pub fn insert(&self, cache_id: String, frame: GpuImageFrame) {
        lock(&self.entries).insert(cache_id, frame);
    }
}

#[derive(Debug, Node)]
pub struct Memo {
    pub id: NodeId,

    #[property(expected = String)]
    pub cache_id: NodeProperty,
    #[property(expected = Bool)]
    pub allow_expressions: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,

    cache: MemoCache,
}

impl Default for Memo {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            cache_id: NodeProperty::String(String::new()),
            allow_expressions: NodeProperty::Bool(false),
            source: PortRef::empty(),
            cache: MemoCache::new(),
        }
    }
}

#[node_impl]
impl Memo {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<GpuImageFrame> {
        let cache_id = self.resolve_cache_id(ctx)?;
        if cache_id.trim().is_empty() {
            return Err(PropertyError::MissingProperty {
                node_id: self.id,
                property_path: "cache_id".to_string(),
            }
            .into());
        }

        let allow_expressions = self.resolve_allow_expressions(ctx)?;
        if !allow_expressions && let Some(cached) = self.cache.get(&cache_id) {
            return Ok(cached);
        }

        let raster = ctx.eval(&self.source)?.as_raster()?.snapshot_image();

        if !allow_expressions {
            self.cache.insert(cache_id, raster.clone());
        }

        Ok(raster)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
