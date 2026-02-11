use std::{
    collections::HashMap,
    io::{self, BufRead, Write},
    sync::Arc,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use lumen::{
    compiler::compile_sequence,
    font::{FONT_ARIAL, FontManager},
    plan::RenderPlan,
    render::Renderer,
    sequence::{Sequence, Transform},
    skia::{FontMgr, FontStyle, Typeface},
    time::FrameIndex,
};
use serde::{Deserialize, Serialize};

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut editor = EditorRuntime::default();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) if !line.trim().is_empty() => line,
            Ok(_) => continue,
            Err(err) => {
                let _ = write_response(
                    &mut stdout,
                    EditorResponse::error("io_error", format!("failed to read command: {err}")),
                );
                break;
            }
        };

        let command = match serde_json::from_str::<EditorCommand>(&line) {
            Ok(command) => command,
            Err(err) => {
                let _ = write_response(
                    &mut stdout,
                    EditorResponse::error(
                        "invalid_command",
                        format!("failed to parse command json: {err}"),
                    ),
                );
                continue;
            }
        };

        let response = match editor.handle(command) {
            Ok(response) => response,
            Err(err) => EditorResponse::error("command_failed", err),
        };

        if write_response(&mut stdout, response).is_err() {
            break;
        }
    }
}

#[derive(Default)]
struct EditorRuntime {
    sequence: Option<Sequence>,
    plan: Option<Arc<RenderPlan>>,
    selected_clip_id: Option<String>,
    transform_overrides: HashMap<String, Transform>,
}

impl EditorRuntime {
    fn handle(&mut self, command: EditorCommand) -> Result<EditorResponse, String> {
        match command {
            EditorCommand::LoadSequence { sequence } => self.load_sequence(sequence),
            EditorCommand::RenderFrame { frame_index } => self.render_frame(frame_index),
            EditorCommand::SelectClip { clip_id } => self.select_clip(clip_id),
            EditorCommand::UpdateTransform { clip_id, transform } => {
                self.update_transform(clip_id, transform)
            }
            EditorCommand::GetState => {
                Ok(EditorResponse::ok(EditorPayload::State(EditorStateView {
                    selected_clip_id: self.selected_clip_id.clone(),
                    selected_transform: self.selected_transform(),
                })))
            }
        }
    }

    fn load_sequence(&mut self, sequence: Sequence) -> Result<EditorResponse, String> {
        let plan = compile_sequence(&sequence).map_err(|err| err.to_string())?;
        self.sequence = Some(sequence);
        self.plan = Some(Arc::new(plan));
        self.selected_clip_id = None;
        self.transform_overrides.clear();

        Ok(EditorResponse::ok(EditorPayload::Loaded))
    }

    fn render_frame(&mut self, frame_index: u64) -> Result<EditorResponse, String> {
        let plan = self
            .plan
            .clone()
            .ok_or_else(|| "no sequence loaded".to_string())?;

        if frame_index >= plan.total_frames {
            return Err(format!(
                "frame {frame_index} is out of range (max {})",
                plan.total_frames.saturating_sub(1)
            ));
        }

        let mut renderer = Renderer::new_without_media(plan.clone(), EditorFontManager::new())
            .map_err(|err| err.to_string())?;
        renderer
            .draw_frame(FrameIndex(frame_index))
            .map_err(|err| err.to_string())?;
        let png = renderer.encode_png().map_err(|err| err.to_string())?;

        Ok(EditorResponse::ok(EditorPayload::Preview(PreviewFrame {
            frame_index,
            png_base64: BASE64.encode(png),
            selected_clip_id: self.selected_clip_id.clone(),
            selected_transform: self.selected_transform(),
        })))
    }

    fn select_clip(&mut self, clip_id: String) -> Result<EditorResponse, String> {
        self.ensure_clip_exists(&clip_id)?;
        self.selected_clip_id = Some(clip_id.clone());

        Ok(EditorResponse::ok(EditorPayload::Selection(
            EditorSelection {
                clip_id,
                transform: self.selected_transform(),
            },
        )))
    }

    fn update_transform(
        &mut self,
        clip_id: String,
        transform: Transform,
    ) -> Result<EditorResponse, String> {
        self.ensure_clip_exists(&clip_id)?;
        self.transform_overrides.insert(clip_id.clone(), transform);

        let sequence = self
            .sequence
            .as_mut()
            .ok_or_else(|| "no sequence loaded".to_string())?;
        update_sequence_transform(sequence, &clip_id, transform)?;

        let plan = compile_sequence(sequence).map_err(|err| err.to_string())?;
        self.plan = Some(Arc::new(plan));

        if self.selected_clip_id.is_none() {
            self.selected_clip_id = Some(clip_id.clone());
        }

        Ok(EditorResponse::ok(EditorPayload::Selection(
            EditorSelection {
                clip_id,
                transform: Some(transform),
            },
        )))
    }

    fn ensure_clip_exists(&self, clip_id: &str) -> Result<(), String> {
        let sequence = self
            .sequence
            .as_ref()
            .ok_or_else(|| "no sequence loaded".to_string())?;

        let exists = sequence
            .tracks
            .iter()
            .any(|track| track.clips.iter().any(|clip| clip.id == clip_id));

        if !exists {
            return Err(format!("clip `{clip_id}` does not exist"));
        }

        Ok(())
    }

    fn selected_transform(&self) -> Option<Transform> {
        let clip_id = self.selected_clip_id.as_ref()?;
        self.transform_overrides
            .get(clip_id)
            .copied()
            .or_else(|| lookup_sequence_transform(self.sequence.as_ref()?, clip_id))
    }
}

fn update_sequence_transform(
    sequence: &mut Sequence,
    clip_id: &str,
    transform: Transform,
) -> Result<(), String> {
    for track in &mut sequence.tracks {
        if let Some(clip) = track.clips.iter_mut().find(|clip| clip.id == clip_id) {
            clip.transform = transform;
            return Ok(());
        }
    }

    Err(format!("clip `{clip_id}` does not exist"))
}

fn lookup_sequence_transform(sequence: &Sequence, clip_id: &str) -> Option<Transform> {
    sequence
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter())
        .find(|clip| clip.id == clip_id)
        .map(|clip| clip.transform)
}

fn write_response(stdout: &mut io::Stdout, response: EditorResponse) -> io::Result<()> {
    serde_json::to_writer(&mut *stdout, &response)?;
    stdout.write_all(b"\n")?;
    stdout.flush()
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum EditorCommand {
    LoadSequence {
        sequence: Sequence,
    },
    RenderFrame {
        frame_index: u64,
    },
    SelectClip {
        clip_id: String,
    },
    UpdateTransform {
        clip_id: String,
        transform: Transform,
    },
    GetState,
}

#[derive(Debug, Serialize)]
struct EditorResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    payload: Option<EditorPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<EditorError>,
}

impl EditorResponse {
    fn ok(payload: EditorPayload) -> Self {
        Self {
            ok: true,
            payload: Some(payload),
            error: None,
        }
    }

    fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            payload: None,
            error: Some(EditorError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

#[derive(Debug, Serialize)]
struct EditorError {
    code: String,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum EditorPayload {
    Loaded,
    Preview(PreviewFrame),
    Selection(EditorSelection),
    State(EditorStateView),
}

#[derive(Debug, Serialize)]
struct PreviewFrame {
    frame_index: u64,
    png_base64: String,
    selected_clip_id: Option<String>,
    selected_transform: Option<Transform>,
}

#[derive(Debug, Serialize)]
struct EditorSelection {
    clip_id: String,
    transform: Option<Transform>,
}

#[derive(Debug, Serialize)]
struct EditorStateView {
    selected_clip_id: Option<String>,
    selected_transform: Option<Transform>,
}

struct EditorFontManager(FontMgr);

impl EditorFontManager {
    fn new() -> Self {
        Self(FontMgr::new())
    }
}

impl FontManager for EditorFontManager {
    fn skia(&self) -> &FontMgr {
        &self.0
    }

    fn named(&self, name: &str) -> Option<Typeface> {
        self.0
            .match_family_style(name, FontStyle::normal())
            .or_else(|| self.0.match_family_style(FONT_ARIAL, FontStyle::normal()))
    }
}
