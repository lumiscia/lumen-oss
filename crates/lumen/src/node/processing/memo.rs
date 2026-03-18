use crate::{
    error::PropertyError,
    node::{NodeId, NodeProperty, PortRef},
    raster::RasterFrame,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Default)]
pub struct MemoCache {
    /// Two-level map: cache_id -> (width, height, request_hash, signature_hash) -> bitmap.
    /// This avoids allocating a String on every `get` lookup.
    entries: HashMap<String, HashMap<(u32, u32, u64, u64), CachedBitmap>>,
}

impl MemoCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(
        &self,
        cache_id: &str,
        width: u32,
        height: u32,
        request_hash: u64,
        signature_hash: u64,
    ) -> Option<CachedBitmap> {
        self.entries
            .get(cache_id)?
            .get(&(width, height, request_hash, signature_hash))
            .cloned()
    }

    pub fn insert(
        &mut self,
        cache_id: String,
        width: u32,
        height: u32,
        request_hash: u64,
        signature_hash: u64,
        bitmap: CachedBitmap,
    ) {
        self.entries
            .entry(cache_id)
            .or_default()
            .insert((width, height, request_hash, signature_hash), bitmap);
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
        }
    }
}

#[node_impl]
impl Memo {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let cache_id = self.resolve_cache_id(ctx)?;
        if cache_id.trim().is_empty() {
            return Err(PropertyError::MissingProperty {
                node_id: self.id,
                property_path: "cache_id".to_string(),
            }
            .into());
        }

        Ok(ctx.eval(self.source.clone())?.as_raster()?.clone())
    }
}
