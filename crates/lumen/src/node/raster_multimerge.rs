use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

use crate::{
    error::{LumenError, RenderError},
    node::{
        BlendMode, InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue,
        merge::Merge,
    },
    raster::RasterFrame,
    render::RenderContext,
};

static INPUT_PORT_DEFS_CACHE: OnceLock<Mutex<HashMap<u16, &'static [InputPortDef]>>> =
    OnceLock::new();

const OUTPUT_PORT_DEFS: [OutputPortDef; 1] = [OutputPortDef {
    name: "output",
    kind: PortKind::RasterFrame,
}];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RasterMultiMerge {
    pub blend_mode: BlendMode,
    pub opacity: f32,
    pub input_count: u16,
}

impl Default for RasterMultiMerge {
    fn default() -> Self {
        Self {
            blend_mode: BlendMode::Normal,
            opacity: 1.0,
            input_count: 2,
        }
    }
}

impl RasterMultiMerge {
    fn normalized_input_count(&self) -> u16 {
        self.input_count.max(1)
    }

    pub fn input_port_name(index: u16) -> String {
        format!("input_{index}")
    }

    fn cached_input_port_defs(&self) -> &'static [InputPortDef] {
        let input_count = self.normalized_input_count();
        let cache = INPUT_PORT_DEFS_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let mut cache_guard = match cache.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        if let Some(existing_defs) = cache_guard.get(&input_count) {
            return existing_defs;
        }

        let defs: Vec<InputPortDef> = (0..input_count)
            .map(|index| {
                let name: &'static str = Box::leak(Self::input_port_name(index).into_boxed_str());
                InputPortDef {
                    name,
                    kind: PortKind::RasterFrame,
                    optional: true,
                }
            })
            .collect();
        let defs: &'static [InputPortDef] = Box::leak(defs.into_boxed_slice());

        cache_guard.insert(input_count, defs);
        defs
    }

    fn transparent_output(ctx: &RenderContext) -> Result<PortValue, LumenError> {
        let pixel_count = ctx
            .request
            .width()
            .checked_mul(ctx.request.height())
            .and_then(|count| count.checked_mul(4))
            .ok_or(RenderError::SurfaceAllocation {
                width: ctx.request.width(),
                height: ctx.request.height(),
            })?;

        Ok(PortValue::RasterFrame(RasterFrame::bitmap(
            Arc::new(vec![0; pixel_count as usize]),
            ctx.request.width(),
            ctx.request.height(),
        )))
    }
}

impl NodeEval for RasterMultiMerge {
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
        let mut layers = Vec::new();
        for index in 0..self.normalized_input_count() {
            let port_name = Self::input_port_name(index);
            if let Some(raster) = inputs.get_raster_optional(&port_name)? {
                layers.push(raster.clone());
            }
        }

        let mut layers = layers.into_iter();
        let Some(mut acc) = layers.next() else {
            return Self::transparent_output(ctx);
        };

        if self.opacity <= 0.0 {
            return Ok(PortValue::RasterFrame(acc));
        }

        let merge = Merge {
            blend_mode: self.blend_mode,
            opacity: self.opacity,
        };

        for overlay in layers {
            let mut merge_inputs = NodeInputs::new();
            merge_inputs.insert("base", PortValue::RasterFrame(acc));
            merge_inputs.insert("overlay", PortValue::RasterFrame(overlay));
            acc = match merge.evaluate(&merge_inputs, ctx)? {
                PortValue::RasterFrame(output) => output,
                PortValue::Vector(_) => unreachable!("merge outputs raster"),
            };
        }

        Ok(PortValue::RasterFrame(acc))
    }
}
