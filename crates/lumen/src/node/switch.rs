use std::{
    collections::HashMap,
    ops::Range,
    sync::{Arc, Mutex, OnceLock},
};

use crate::{
    error::{LumenError, RenderError},
    node::{InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue},
    raster::RasterFrame,
    render::RenderContext,
};

static INPUT_PORT_DEFS_CACHE: OnceLock<Mutex<HashMap<Vec<u16>, &'static [InputPortDef]>>> =
    OnceLock::new();

const OUTPUT_PORT_DEFS: [OutputPortDef; 1] = [OutputPortDef {
    name: "output",
    kind: PortKind::RasterFrame,
}];

#[derive(Debug, Clone, Default)]
pub struct Switch {
    pub map: HashMap<u16, Range<u32>>,
}

impl Switch {
    pub fn new(map: HashMap<u16, Range<u32>>) -> Self {
        Self { map }
    }

    fn sorted_indices(&self) -> Vec<u16> {
        let mut indices: Vec<u16> = self.map.keys().copied().collect();
        indices.sort_unstable();
        indices
    }

    fn input_port_name(index: u16) -> String {
        format!("input_{index}")
    }

    fn transparent_output(ctx: &RenderContext) -> Result<PortValue, LumenError> {
        let pixel_count = ctx
            .width
            .checked_mul(ctx.height)
            .and_then(|count| count.checked_mul(4))
            .ok_or(RenderError::SurfaceAllocation {
                width: ctx.width,
                height: ctx.height,
            })?;

        Ok(PortValue::RasterFrame(RasterFrame::bitmap(
            Arc::new(vec![0; pixel_count as usize]),
            ctx.width,
            ctx.height,
        )))
    }

    fn cached_input_port_defs(&self) -> &'static [InputPortDef] {
        let indices = self.sorted_indices();
        if indices.is_empty() {
            return &[];
        }

        let cache = INPUT_PORT_DEFS_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let mut cache_guard = match cache.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        if let Some(existing_defs) = cache_guard.get(&indices) {
            return existing_defs;
        }

        let defs: Vec<InputPortDef> = indices
            .iter()
            .map(|index| {
                let name: &'static str = Box::leak(Self::input_port_name(*index).into_boxed_str());
                InputPortDef {
                    name,
                    kind: PortKind::RasterFrame,
                    optional: true,
                }
            })
            .collect();
        let defs: &'static [InputPortDef] = Box::leak(defs.into_boxed_slice());

        cache_guard.insert(indices, defs);
        defs
    }
}

impl NodeEval for Switch {
    fn input_port_defs(&self) -> &'static [InputPortDef] {
        self.cached_input_port_defs()
    }

    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        &OUTPUT_PORT_DEFS
    }

    fn evaluate(
        &self,
        inputs: &NodeInputs,
        ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        let selected_index = self
            .map
            .iter()
            .filter_map(|(index, frame_range)| frame_range.contains(&ctx.frame).then_some(*index))
            .min();

        let Some(index) = selected_index else {
            return Self::transparent_output(ctx);
        };

        let input_port = Self::input_port_name(index);
        let Some(raster) = inputs.get_raster_optional(&input_port)? else {
            return Self::transparent_output(ctx);
        };

        Ok(PortValue::RasterFrame(raster.clone()))
    }
}
