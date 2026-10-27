//! Plays a composition template, or the branch on its `source` input, over a window of time.
//!
//! Within the window `[start, start + duration)` the content runs on its own clock:
//!
//! ```text
//! local = (frame - start) * speed * content_fps / fps + offset
//! ```
//!
//! `start` and `duration` count frames of the composition the clip is in, and `offset` counts
//! frames of the content, so a template keeps its timing at any parent frame rate. Outside the
//! window the clip is transparent and nothing upstream of it is compiled or bound, except that
//! `hold` and `loop` keep it showing after the window ends.
//!
//! A template is drawn at its own size from the top-left corner of the parent's canvas; place
//! it with a `transform` node.

use crate::{
    composition::CompositionTemplate,
    error::{ExpressionError, LumenError},
    expr::ExpressionContext,
    gpu::{CompiledOutput, GpuCompileNode},
    node::{
        ClipPropValue, ClipProps, Deferred, DelegateEvalContext, DelegateEvaluable, NodeId,
        NodeParams, PortRef,
    },
    scope::{PropValues, Routed, Scope},
};

/// The literal `duration` that plays the content to its end. Other negative literals are
/// rejected, and a negative expression shows nothing.
pub const PLAY_TO_END: f64 = -1.0;

/// What a clip shows after its window ends.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum ClipEnd {
    #[default]
    Hide = 0,
    Hold = 1,
    Loop = 2,
}

/// Plays a composition template, or its source, over a window of time.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ClipParams {
    /// Template from `compositions` to play; empty plays the `source` input instead.
    #[meta(name = "Composition", constant)]
    pub composition: String,
    /// Frame the clip starts at, in this composition's frames.
    #[meta(name = "Start", step = 1)]
    pub start: f64,
    /// Frames the clip plays for; -1 plays the template to its end, or a source to the end of
    /// this composition.
    #[meta(name = "Duration", min = -1, step = 1)]
    pub duration: f64,
    /// Frames of the content skipped at the start, in the content's frames.
    #[meta(name = "Offset", step = 1)]
    pub offset: f64,
    /// Playback rate of the content; must be greater than 0.
    #[meta(name = "Speed", step = 0.1)]
    pub speed: f64,
    /// What the clip shows after its window: nothing, its last frame, or the window again.
    #[meta(name = "End", kind = "enum", enum_type = ClipEnd, constant)]
    pub end: ClipEnd,
    /// Values for the template's props by name; expressions run in this composition's time.
    #[meta(name = "Props", constant)]
    pub props: ClipProps,
}

impl Default for ClipParams {
    fn default() -> Self {
        Self {
            composition: String::new(),
            start: 0.0,
            duration: PLAY_TO_END,
            offset: 0.0,
            speed: 1.0,
            end: ClipEnd::Hide,
            props: ClipProps::default(),
        }
    }
}

/// Plays a composition template, or its source, over a window of time.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "clip", name = "Clip", category = "compositing")]
pub struct Clip {
    pub id: NodeId,
    #[params]
    pub params: ClipParamsDelegate,
    /// The branch to play when no `composition` is set.
    #[input(optional)]
    pub source: PortRef,
}

impl Default for Clip {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: ClipParamsDelegate::default(),
            source: PortRef::empty(),
        }
    }
}

/// When a clip shows and at which of its content's frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipWindow {
    pub start: f64,
    pub duration: f64,
    pub offset: f64,
    /// Content frames per frame of the composition the clip is in.
    pub rate: f64,
    pub end: ClipEnd,
}

impl ClipWindow {
    /// The content frame shown at `frame`, or `None` while the clip is hidden.
    ///
    /// `hold` freezes on the last whole frame inside the window, and `loop` wraps back to the
    /// window's start.
    pub fn local_frame(&self, frame: f64) -> Option<f64> {
        let elapsed = frame - self.start;
        if !(elapsed >= 0.0) || !(self.duration > 0.0) {
            return None;
        }
        let elapsed = if elapsed < self.duration {
            elapsed
        } else {
            match self.end {
                ClipEnd::Hide => return None,
                ClipEnd::Hold => ((self.start + self.duration).ceil() - 1.0 - self.start).max(0.0),
                ClipEnd::Loop => elapsed.rem_euclid(self.duration),
            }
        };
        // Matching rates keep the frame as given, which may be just below a whole frame on
        // purpose, as a looped `time_remap` target is.
        let local = elapsed * self.rate + self.offset;
        Some(if self.rate == 1.0 {
            local
        } else {
            crate::scope::snap_to_whole_frame(local)
        })
    }
}

impl Clip {
    /// The template this clip plays, if it names one.
    pub fn template_name(&self) -> Option<&str> {
        match &self.params.composition {
            Deferred::Value(name) if !name.is_empty() => Some(name),
            _ => None,
        }
    }

    /// The prop values this clip sets, by name.
    pub fn props(&self) -> &ClipProps {
        static EMPTY: ClipProps = ClipProps(std::collections::BTreeMap::new());
        match &self.params.props {
            Deferred::Value(props) => props,
            Deferred::Expr(_) => &EMPTY,
        }
    }

    /// The template this clip plays in `scope`, or `None` when it plays its source.
    fn template<'a>(&self, scope: &Scope<'a>) -> crate::Result<Option<&'a CompositionTemplate>> {
        let Some(name) = self.template_name() else {
            return Ok(None);
        };
        scope.root.compositions.get(name).map(Some).ok_or_else(|| {
            self.error(
                None,
                format!("composition `{name}` is not defined in `compositions`"),
            )
        })
    }

    /// When the clip shows in `scope`, evaluated at `ctx`'s frame.
    pub fn window(
        &self,
        scope: &Scope<'_>,
        ctx: &ExpressionContext<'_>,
    ) -> crate::Result<ClipWindow> {
        let number = |value: &Deferred<f64>, name: &str| {
            let number = value.eval(self.id, name, ctx)?;
            if number.is_finite() {
                Ok(number)
            } else {
                Err(self.error(ctx.path.clone(), format!("`{name}` must be finite")))
            }
        };
        let start = number(&self.params.start, "start")?;
        let plays_to_end = matches!(self.params.duration, Deferred::Value(PLAY_TO_END));
        let duration = number(&self.params.duration, "duration")?.max(0.0);
        let offset = number(&self.params.offset, "offset")?;
        let speed = number(&self.params.speed, "speed")?;
        if speed <= 0.0 {
            return Err(self.error(
                ctx.path.clone(),
                format!("`speed` must be greater than 0, got {speed}"),
            ));
        }
        let template = self.template(scope)?;
        let content_fps = template.map_or(scope.timeline.fps, |template| template.timeline.fps);
        let rate = speed * f64::from(content_fps) / f64::from(scope.timeline.fps);
        let duration = if !plays_to_end {
            duration
        } else {
            match template {
                Some(template) => (f64::from(template.timeline.duration_frames) - offset) / rate,
                None => f64::from(scope.timeline.duration_frames) - start,
            }
        };
        Ok(ClipWindow {
            start,
            duration,
            offset,
            rate,
            end: self.params.end.eval(&DelegateEvalContext {
                node_id: self.id,
                property_path: "end",
                expr: ctx,
            })?,
        })
    }

    /// What the clip shows at `frame` in `scope`: its content's output in the content's own
    /// frame and scope, or `None` while it is hidden.
    pub(crate) fn route<'a>(
        &self,
        scope: &Scope<'a>,
        frame: f64,
        media: Option<&dyn crate::media::MediaStore>,
    ) -> crate::Result<Option<Routed<'a>>> {
        let ctx = scope.expr_context(frame, format!("{}.params", self.id), media);
        let Some(local) = self.window(scope, &ctx)?.local_frame(frame) else {
            return Ok(None);
        };
        let Some(template) = self.template(scope)? else {
            return Ok(Some(Routed {
                port: self.source.clone(),
                frame: local,
                scope: scope.clone(),
                instance: Some(self.id),
            }));
        };
        let child = Scope::template(scope.root, template, self.instance_props(template, &ctx)?);
        let output = child.media_output()?;
        let Some(crate::node::NodeKind::MediaOutput(output)) = child.node(output) else {
            unreachable!("media_output returns a media output node");
        };
        Ok(Some(Routed {
            port: output.source.clone(),
            frame: local,
            scope: child,
            instance: Some(self.id),
        }))
    }

    /// The template's props for this instance: each value this clip sets, evaluated at `ctx`,
    /// or the template's default.
    fn instance_props(
        &self,
        template: &CompositionTemplate,
        ctx: &ExpressionContext<'_>,
    ) -> crate::Result<PropValues> {
        let values = &self.props().0;
        template
            .props
            .iter()
            .map(|(name, def)| {
                let value = match values.get(name) {
                    None => Ok(def.default.clone()),
                    Some(ClipPropValue::Literal(value)) => def.kind.literal(value),
                    Some(ClipPropValue::Expr(expr)) => def.kind.check(expr.evaluate(ctx)?),
                };
                value.map(|value| (name.clone(), value)).map_err(|details| {
                    self.error(
                        Some(format!("{}.props.{name}", self.id)),
                        format!("prop `{name}`: {details}"),
                    )
                })
            })
            .collect()
    }

    fn error(&self, path: Option<String>, details: String) -> LumenError {
        LumenError::Expression(ExpressionError::Evaluate {
            path: path.or_else(|| Some(format!("{}.params", self.id))),
            details,
        })
    }
}

impl GpuCompileNode for Clip {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(ctx.missing_output(self.id, &port.port));
        }
        ctx.compile_clip(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{ClipEnd, ClipWindow};

    fn window(end: ClipEnd) -> ClipWindow {
        ClipWindow {
            start: 10.0,
            duration: 4.0,
            offset: 2.0,
            rate: 0.5,
            end,
        }
    }

    #[test]
    fn local_frames_follow_start_offset_and_rate() {
        let clip = window(ClipEnd::Hide);
        assert_eq!(clip.local_frame(9.0), None);
        assert_eq!(clip.local_frame(10.0), Some(2.0));
        assert_eq!(clip.local_frame(13.0), Some(3.5));
        assert_eq!(clip.local_frame(14.0), None);
    }

    #[test]
    fn end_modes_hold_the_last_frame_or_loop() {
        assert_eq!(window(ClipEnd::Hold).local_frame(20.0), Some(3.5));
        assert_eq!(window(ClipEnd::Hold).local_frame(9.0), None);
        assert_eq!(window(ClipEnd::Loop).local_frame(15.0), Some(2.5));
        assert_eq!(window(ClipEnd::Loop).local_frame(19.0), Some(2.5));
        let fractional = ClipWindow {
            duration: 66.5,
            ..window(ClipEnd::Hold)
        };
        assert_eq!(fractional.local_frame(100.0), Some(66.0 * 0.5 + 2.0));
    }
}
