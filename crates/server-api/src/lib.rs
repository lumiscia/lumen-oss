//! The HTTP API of `lumen-server`: the types its requests, responses, socket events and errors
//! (de)serialise with, and every route as data in [`OPERATIONS`]. `definitions/openapi.json` is
//! generated from both, so another implementation of the API can follow the same contract.
//!
//! Every object is camelCase on the wire. Values the engine produces, such as diagnostics and media
//! failures, are carried as JSON whose schema is the runtime schema's
//! (`definitions/runtime.schema.json`), since this crate doesn't depend on the engine.

mod error;
mod event;
mod media;
mod operation;
mod output;
mod render;
mod webhook;

use serde::{Deserialize, Serialize};

pub use error::{ErrorBody, ErrorCode};
pub use event::RenderEvent;
pub use media::{
    CompleteMultipartRequest, CompletedPart, CreateMediaRequest, FontFace, Media, MediaKind,
    MediaMetadata, MediaResponse, MediaSource, MediaStatus, MultipartUploadResponse, PartResponse,
    UploadTargetResponse, UrlMediaRequest,
};
pub use operation::{ANY_RENDER_FAILURE, Body, Method, OPERATIONS, Operation, PathParam};
pub use output::{FrameRange, OutputFormat, RenderOutput, VideoBitDepth, VideoCodec};
pub use render::{
    CheckReport, CheckRequest, CreateRenderResponse, Render, RenderFailure, RenderRequest,
    RenderResponse, RenderStatus,
};
pub use webhook::{RenderWebhook, RenderWebhookType};

/// The body of `GET /health`, the one route that needs no token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub ok: bool,
    /// What answered, `lumen-server` for this server.
    pub service: String,
    /// The version of this API the server implements: the `info.version` of the OpenAPI document
    /// it follows, such as `0.6`.
    pub version: String,
}

/// Schemas of values whose definition lives in another schema file.
#[cfg(feature = "metadata")]
mod external {
    use schemars::{Schema, SchemaGenerator, json_schema};

    fn runtime(name: &str) -> Schema {
        json_schema!({ "$ref": format!("runtime.schema.json#/$defs/{name}") })
    }

    fn runtime_list(name: &str) -> Schema {
        json_schema!({ "type": "array", "items": runtime(name) })
    }

    pub fn composition(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "$ref": "composition.schema.json" })
    }

    pub fn diagnostics(_: &mut SchemaGenerator) -> Schema {
        runtime_list("Diagnostic")
    }

    pub fn media_failure(_: &mut SchemaGenerator) -> Schema {
        runtime("MediaFailure")
    }

    pub fn animations(_: &mut SchemaGenerator) -> Schema {
        runtime_list("AnimationInfo")
    }

    pub fn materials(_: &mut SchemaGenerator) -> Schema {
        runtime_list("MaterialInfo")
    }

    pub fn cameras(_: &mut SchemaGenerator) -> Schema {
        runtime_list("CameraInfo")
    }
}
