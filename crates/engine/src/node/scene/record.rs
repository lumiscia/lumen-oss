//! How the frame graph records the nodes that output a scene or a material. `render_3d` reads its
//! scene without recording it, and validation keeps a scene or a material out of every raster
//! input, so recording one fails. Each builds the programs that drawing it adds to its scene's.

#[cfg(not(feature = "3d"))]
use super::Render3d;
#[cfg(feature = "3d")]
use super::collect::particles_programs;
#[cfg(feature = "3d")]
use super::render::{catcher_programs, light_programs};
use super::text_3d::{check_length, check_weight};
use super::{
    Camera3d, Extrude3d, ImagePlane3d, Instance3d, Lathe3d, Light3d, Material3d, Merge3d, Model3d,
    Particles3d,
    Shape3d, Text3d,
};
use crate::{
    error::{LumenError, RenderError},
    gpu::frame_graph::{FrameCx, Programs, Raster, RecordNode, Staticness, held},
    node::{Deferred, NodeId},
};

fn not_a_raster<N: crate::node::JsonNode>(node_id: NodeId) -> LumenError {
    RenderError::evaluating::<N>(
        node_id,
        "a scene node outputs a scene, not a raster; draw it through a render_3d",
    )
    .into()
}

/// Without the `3d` feature nothing draws a scene, so no light builds programs.
#[cfg(not(feature = "3d"))]
fn light_programs(_: &Light3d, _: &mut Programs<'_>) -> crate::Result<()> {
    Ok(())
}

/// Without the `3d` feature nothing draws a scene, so no shadow catcher builds programs.
#[cfg(not(feature = "3d"))]
fn catcher_programs(_: &crate::node::Deferred<bool>, _: &mut Programs<'_>) -> crate::Result<()> {
    Ok(())
}

impl RecordNode for Material3d {
    fn programs(&self, programs: &mut Programs<'_>) -> crate::Result<()> {
        catcher_programs(&self.params.shadow_catcher, programs)
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster::<Material3d>(self.id))
    }
}

impl RecordNode for Shape3d {
    fn programs(&self, _: &mut Programs<'_>) -> crate::Result<()> {
        Ok(())
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster::<Shape3d>(self.id))
    }
}

impl RecordNode for ImagePlane3d {
    fn programs(&self, _: &mut Programs<'_>) -> crate::Result<()> {
        Ok(())
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster::<ImagePlane3d>(self.id))
    }
}

impl RecordNode for Text3d {
    fn programs(&self, _: &mut Programs<'_>) -> crate::Result<()> {
        Ok(())
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster::<Text3d>(self.id))
    }

    fn literal_param_errors(&self) -> Vec<(&'static str, String)> {
        let params = &self.params;
        let lengths = [
            ("font_size", &params.font_size, false),
            ("max_width", &params.max_width, false),
            ("letter_spacing", &params.letter_spacing, true),
            ("line_height", &params.line_height, false),
            ("depth", &params.depth, false),
            ("bevel_size", &params.bevel_size, false),
        ];
        let mut errors = Vec::new();
        for (param, value, signed) in lengths {
            if let Deferred::Value(value) = value
                && let Err(details) = check_length(param, *value, signed)
            {
                errors.push((param, details));
            }
        }
        if let Deferred::Value(weight) = &params.font_weight
            && let Err(details) = check_weight(*weight)
        {
            errors.push(("font_weight", details));
        }
        errors
    }

    fn literal_font_families(&self) -> Vec<(String, &str)> {
        match &self.params.font_family {
            Deferred::Value(family) => vec![("font_family".to_string(), family.as_str())],
            Deferred::Expr(_) => Vec::new(),
        }
    }
}

impl RecordNode for Extrude3d {
    fn programs(&self, _: &mut Programs<'_>) -> crate::Result<()> {
        Ok(())
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster::<Extrude3d>(self.id))
    }

    fn literal_param_errors(&self) -> Vec<(&'static str, String)> {
        let params = &self.params;
        length_errors(&[
            ("units_per_pixel", &params.units_per_pixel),
            ("depth", &params.depth),
            ("bevel_size", &params.bevel_size),
        ])
    }
}

impl RecordNode for Lathe3d {
    fn programs(&self, _: &mut Programs<'_>) -> crate::Result<()> {
        Ok(())
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster::<Lathe3d>(self.id))
    }

    fn literal_param_errors(&self) -> Vec<(&'static str, String)> {
        let params = &self.params;
        let mut errors = length_errors(&[("units_per_pixel", &params.units_per_pixel)]);
        if let Deferred::Value(sweep) = &params.sweep
            && !sweep.is_finite()
        {
            errors.push((
                "sweep",
                format!("`sweep` must be a finite number, got {sweep}"),
            ));
        }
        errors
    }
}

/// What is wrong with each of the literal lengths `lengths`, which are not signed.
fn length_errors(lengths: &[(&'static str, &Deferred<f64>)]) -> Vec<(&'static str, String)> {
    lengths
        .iter()
        .filter_map(|&(param, value)| match value {
            Deferred::Value(value) => check_length(param, *value, false)
                .err()
                .map(|details| (param, details)),
            Deferred::Expr(_) => None,
        })
        .collect()
}

impl RecordNode for Light3d {
    fn programs(&self, programs: &mut Programs<'_>) -> crate::Result<()> {
        light_programs(self, programs)
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster::<Light3d>(self.id))
    }
}

impl RecordNode for Model3d {
    fn programs(&self, _: &mut Programs<'_>) -> crate::Result<()> {
        Ok(())
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster::<Model3d>(self.id))
    }

    // A model playing a clip poses differently with the frame, until its clip is over or before
    // it starts, which `add_model` finds out from the clip.
    fn staticness(&self, reads_frame: bool) -> Staticness {
        if !self.animates() {
            held(reads_frame)
        } else if cfg!(feature = "3d") && !reads_frame {
            Staticness::WhileHeld
        } else {
            Staticness::Never
        }
    }
}

impl RecordNode for Camera3d {
    fn programs(&self, _: &mut Programs<'_>) -> crate::Result<()> {
        Ok(())
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster::<Camera3d>(self.id))
    }
}

impl RecordNode for Merge3d {
    fn programs(&self, _: &mut Programs<'_>) -> crate::Result<()> {
        Ok(())
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster::<Merge3d>(self.id))
    }
}

impl RecordNode for Instance3d {
    fn programs(&self, _: &mut Programs<'_>) -> crate::Result<()> {
        Ok(())
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster::<Instance3d>(self.id))
    }

    // Per-copy params are evaluated in a scope of their own, which `frame_values` doesn't
    // evaluate them in.
    fn staticness(&self, reads_frame: bool) -> Staticness {
        if reads_frame {
            Staticness::Never
        } else {
            held(reads_frame)
        }
    }
}

/// Without the `3d` feature nothing draws a scene, so particles build no programs.
#[cfg(not(feature = "3d"))]
fn particles_programs(_: &mut Programs<'_>) -> crate::Result<()> {
    Ok(())
}

impl RecordNode for Particles3d {
    fn programs(&self, programs: &mut Programs<'_>) -> crate::Result<()> {
        particles_programs(programs)
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster::<Particles3d>(self.id))
    }

    // Particles move with the time unless they are all born at once, live forever and stay
    // still.
    fn staticness(&self, reads_frame: bool) -> Staticness {
        if !reads_frame && self.is_still() {
            Staticness::Always
        } else {
            Staticness::Never
        }
    }
}

/// Without the `3d` feature a `render_3d` fails to record, and builds no programs.
#[cfg(not(feature = "3d"))]
impl RecordNode for Render3d {
    fn programs(&self, _: &mut Programs<'_>) -> crate::Result<()> {
        Ok(())
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(
            RenderError::evaluating::<Render3d>(self.id, crate::error::NO_3D_SUPPORT.to_string())
                .into(),
        )
    }
}
