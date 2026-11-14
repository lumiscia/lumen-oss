//! Every route of the API as data: [`OPERATIONS`]. `definitions/openapi.json` is generated from this
//! table and the types it names, and `lumen-server`'s tests check its router and its answers
//! against it.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
        }
    }
}

/// What a request or a response carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    /// Nothing.
    Empty,
    /// JSON of the named type of this crate.
    Json(&'static str),
    /// Raw bytes: a file being uploaded or downloaded.
    Bytes,
    /// A WebSocket, which sends JSON messages of the named type.
    Socket(&'static str),
}

/// A `{name}` segment of an operation's path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathParam {
    pub name: &'static str,
    pub description: &'static str,
    /// A whole number of at least 1, where the others are strings.
    pub integer: bool,
}

/// One route: a method on a path, what it takes and what it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Operation {
    /// Its name in the OpenAPI document and the generated SDK tables, such as `createRender`.
    pub id: &'static str,
    pub method: Method,
    /// The path, with each parameter as `{name}`.
    pub path: &'static str,
    pub tag: &'static str,
    pub summary: &'static str,
    pub params: &'static [PathParam],
    pub request: Body,
    /// Each status that means success, with its body.
    pub success: &'static [(u16, Body)],
    /// Each error status of the route's own, with the codes it carries. [`Operation::errors`]
    /// adds the ones every route with a token or a JSON body has.
    pub errors: &'static [(u16, &'static [&'static str])],
    /// Whether the route needs the bearer token.
    pub token: bool,
}

/// In an operation's error codes: any code a failed render can have, an engine diagnostic code
/// included.
pub const ANY_RENDER_FAILURE: &str = "*";

impl Operation {
    /// Every error status of the route with its codes, by status: its own, `401` when it needs the
    /// token, and what a JSON body or a path segment that can't be read answers.
    pub fn errors(&self) -> Vec<(u16, Vec<&'static str>)> {
        let mut errors: Vec<(u16, Vec<&'static str>)> = Vec::new();
        let mut add = |status: u16, codes: &[&'static str]| match errors
            .iter_mut()
            .find(|(known, _)| *known == status)
        {
            Some((_, known)) => known.extend_from_slice(codes),
            None => errors.push((status, codes.to_vec())),
        };
        if self.token {
            add(401, &["unauthorized"]);
        }
        if matches!(self.request, Body::Json(_)) {
            add(400, &["invalid_request_body"]);
            add(413, &["request_too_large"]);
            add(415, &["invalid_request_body"]);
            add(422, &["invalid_request_body"]);
        }
        // A path segment that can't be read, and a socket request that isn't a WebSocket's.
        if !self.params.is_empty() {
            add(400, &["invalid_request"]);
        }
        for (status, codes) in self.errors {
            add(*status, codes);
        }
        errors.sort_by_key(|(status, _)| *status);
        errors
    }

    /// The operation a request for `method` on `path` is, with `path` a real one such as
    /// `/renders/r_1`. As a router does, a path that is a route's own wins over one that fits a
    /// parameter, so `GET /renders/check` is no operation: that path only takes `POST`.
    pub fn of(method: &str, path: &str) -> Option<&'static Operation> {
        let literal_segments = |operation: &&Operation| {
            operation
                .path
                .split('/')
                .filter(|segment| !segment.starts_with('{'))
                .count()
        };
        let route = OPERATIONS
            .iter()
            .filter(|operation| operation.matches(path))
            .max_by_key(literal_segments)?
            .path;
        OPERATIONS.iter().find(|operation| {
            operation.path == route && operation.method.as_str().eq_ignore_ascii_case(method)
        })
    }

    fn matches(&self, path: &str) -> bool {
        let mut template = self.path.split('/');
        let mut segments = path.split('/');
        loop {
            match (template.next(), segments.next()) {
                (None, None) => return true,
                (Some(expected), Some(segment)) => {
                    let is_param = expected.starts_with('{') && expected.ends_with('}');
                    if is_param && segment.is_empty() || !is_param && expected != segment {
                        return false;
                    }
                }
                _ => return false,
            }
        }
    }
}

const RENDER_ID: PathParam = PathParam {
    name: "id",
    description: "The render's id.",
    integer: false,
};
const MEDIA_ID: PathParam = PathParam {
    name: "id",
    description: "The media's id.",
    integer: false,
};
const UPLOAD_ID: PathParam = PathParam {
    name: "uploadId",
    description: "The multipart upload's id, as starting it answered.",
    integer: false,
};
const PART_NUMBER: PathParam = PathParam {
    name: "partNumber",
    description: "The part's place in the file, from 1.",
    integer: true,
};

const RENDER_NOT_FOUND: (u16, &[&str]) = (404, &["render_not_found"]);
const MEDIA_NOT_FOUND: (u16, &[&str]) = (404, &["media_not_found"]);

/// Every route of the API.
pub const OPERATIONS: &[Operation] = &[
    Operation {
        id: "health",
        method: Method::Get,
        path: "/health",
        tag: "health",
        summary: "Whether the server is up, and which version of this API it implements.",
        params: &[],
        request: Body::Empty,
        success: &[(200, Body::Json("Health"))],
        errors: &[],
        token: false,
    },
    Operation {
        id: "createRender",
        method: Method::Post,
        path: "/renders",
        tag: "renders",
        summary: "Queues a render of a composition and answers with it.",
        params: &[],
        request: Body::Json("RenderRequest"),
        success: &[(202, Body::Json("CreateRenderResponse"))],
        errors: &[
            (400, &["webhooks_unsupported"]),
            (
                422,
                &[
                    "invalid_media_alias",
                    "media_manifest_too_large",
                    "invalid_project_payload",
                    "invalid_quality",
                    "invalid_output",
                    "output_too_large",
                ],
            ),
            (500, &["check_failed"]),
            (503, &["render_queue_full"]),
        ],
        token: true,
    },
    Operation {
        id: "checkRender",
        method: Method::Post,
        path: "/renders/check",
        tag: "renders",
        summary: "Says what is wrong with a composition, without rendering it.",
        params: &[],
        request: Body::Json("CheckRequest"),
        success: &[(200, Body::Json("CheckReport"))],
        errors: &[
            (422, &["invalid_media_alias", "media_manifest_too_large"]),
            (500, &["check_failed"]),
        ],
        token: true,
    },
    Operation {
        id: "getRender",
        method: Method::Get,
        path: "/renders/{id}",
        tag: "renders",
        summary: "The render as it is now.",
        params: &[RENDER_ID],
        request: Body::Empty,
        success: &[(200, Body::Json("RenderResponse"))],
        errors: &[RENDER_NOT_FOUND],
        token: true,
    },
    Operation {
        id: "getRenderProgress",
        method: Method::Get,
        path: "/renders/{id}/progress",
        tag: "renders",
        summary: "The render as it is now, for polling. The same answer as reading the render.",
        params: &[RENDER_ID],
        request: Body::Empty,
        success: &[(200, Body::Json("RenderResponse"))],
        errors: &[RENDER_NOT_FOUND],
        token: true,
    },
    Operation {
        id: "cancelRender",
        method: Method::Post,
        path: "/renders/{id}/cancel",
        tag: "renders",
        summary: "Cancels a render. `200` when it has ended, cancelled or not; `202` while a running one stops at its next frame.",
        params: &[RENDER_ID],
        request: Body::Empty,
        success: &[
            (200, Body::Json("RenderResponse")),
            (202, Body::Json("RenderResponse")),
        ],
        errors: &[RENDER_NOT_FOUND],
        token: true,
    },
    Operation {
        id: "renderSocket",
        method: Method::Get,
        path: "/renders/{id}/socket",
        tag: "renders",
        summary: "A WebSocket that sends the render's state when it opens, then each change, and closes when the render ends.",
        params: &[RENDER_ID],
        request: Body::Empty,
        success: &[(101, Body::Socket("RenderEvent"))],
        // `426` is a request that can't become a WebSocket, such as one over HTTP/1.0.
        errors: &[RENDER_NOT_FOUND, (426, &["invalid_request"])],
        token: true,
    },
    Operation {
        id: "getRenderArtifact",
        method: Method::Get,
        path: "/renders/{id}/artifact",
        tag: "renders",
        summary: "The output of a render that succeeded, as the file its format says.",
        params: &[RENDER_ID],
        request: Body::Empty,
        success: &[(200, Body::Bytes)],
        errors: &[
            RENDER_NOT_FOUND,
            (409, &["artifact_not_ready", ANY_RENDER_FAILURE]),
        ],
        token: true,
    },
    Operation {
        id: "createMedia",
        method: Method::Post,
        path: "/media",
        tag: "media",
        summary: "Creates media to upload the content of, and says where to send it.",
        params: &[],
        request: Body::Json("CreateMediaRequest"),
        success: &[(201, Body::Json("UploadTargetResponse"))],
        errors: &[
            (413, &["media_too_large"]),
            (500, &["media_store_failed"]),
            (503, &["media_upload_limit"]),
        ],
        token: true,
    },
    Operation {
        id: "createMediaFromUrl",
        method: Method::Post,
        path: "/media/url",
        tag: "media",
        summary: "Downloads a file once and keeps it as media.",
        params: &[],
        request: Body::Json("UrlMediaRequest"),
        success: &[(201, Body::Json("MediaResponse"))],
        errors: &[
            (413, &["media_download_too_large"]),
            (
                422,
                &["media_download_failed", "media_download_forbidden_host"],
            ),
            (500, &["media_store_failed"]),
            (502, &["media_download_failed", "media_stage_failed"]),
        ],
        token: true,
    },
    Operation {
        id: "getMedia",
        method: Method::Get,
        path: "/media/{id}",
        tag: "media",
        summary: "The media as it is now.",
        params: &[MEDIA_ID],
        request: Body::Empty,
        success: &[(200, Body::Json("MediaResponse"))],
        errors: &[MEDIA_NOT_FOUND],
        token: true,
    },
    Operation {
        id: "deleteMedia",
        method: Method::Delete,
        path: "/media/{id}",
        tag: "media",
        summary: "Deletes the media and its content.",
        params: &[MEDIA_ID],
        request: Body::Empty,
        success: &[(204, Body::Empty)],
        errors: &[MEDIA_NOT_FOUND, (500, &["media_store_failed"])],
        token: true,
    },
    Operation {
        id: "putMediaContent",
        method: Method::Put,
        path: "/media/{id}/content",
        tag: "media",
        summary: "Uploads the media's content in one request. The server then checks what the file is.",
        params: &[MEDIA_ID],
        request: Body::Bytes,
        success: &[(200, Body::Json("MediaResponse"))],
        errors: &[
            (400, &["media_size_mismatch", "invalid_request_body"]),
            MEDIA_NOT_FOUND,
            (409, &["media_already_uploaded", "media_upload_in_progress"]),
            (413, &["media_too_large"]),
            (500, &["media_store_failed"]),
        ],
        token: true,
    },
    Operation {
        id: "getMediaContent",
        method: Method::Get,
        path: "/media/{id}/content",
        tag: "media",
        summary: "The media's content, once it has been checked.",
        params: &[MEDIA_ID],
        request: Body::Empty,
        success: &[(200, Body::Bytes)],
        errors: &[
            MEDIA_NOT_FOUND,
            (409, &["media_not_validated"]),
            (500, &["media_store_failed"]),
        ],
        token: true,
    },
    Operation {
        id: "startMultipartUpload",
        method: Method::Post,
        path: "/media/{id}/multipart",
        tag: "media",
        summary: "Starts uploading the media's content in parts.",
        params: &[MEDIA_ID],
        request: Body::Empty,
        success: &[(201, Body::Json("MultipartUploadResponse"))],
        errors: &[
            MEDIA_NOT_FOUND,
            (409, &["media_already_uploaded", "media_upload_in_progress"]),
            (500, &["media_store_failed"]),
        ],
        token: true,
    },
    Operation {
        id: "putMultipartPart",
        method: Method::Put,
        path: "/media/{id}/multipart/{uploadId}/parts/{partNumber}",
        tag: "media",
        summary: "Uploads one part. Sending a part again replaces it, and the same bytes answer the same `etag`.",
        params: &[MEDIA_ID, UPLOAD_ID, PART_NUMBER],
        request: Body::Bytes,
        success: &[(200, Body::Json("PartResponse"))],
        errors: &[
            (400, &["invalid_upload_parts", "invalid_request_body"]),
            (404, &["media_not_found", "upload_not_found"]),
            (
                409,
                &[
                    "media_already_uploaded",
                    "media_upload_in_progress",
                    "upload_part_in_progress",
                ],
            ),
            (413, &["media_too_large"]),
            (500, &["media_store_failed"]),
        ],
        token: true,
    },
    Operation {
        id: "completeMultipartUpload",
        method: Method::Post,
        path: "/media/{id}/multipart/{uploadId}/complete",
        tag: "media",
        summary: "Joins the listed parts in order into the media's content. The server then checks what the file is.",
        params: &[MEDIA_ID, UPLOAD_ID],
        request: Body::Json("CompleteMultipartRequest"),
        success: &[(200, Body::Json("MediaResponse"))],
        errors: &[
            (400, &["invalid_upload_parts", "media_size_mismatch"]),
            (404, &["media_not_found", "upload_not_found"]),
            (409, &["media_already_uploaded", "media_upload_in_progress"]),
            (500, &["media_store_failed"]),
        ],
        token: true,
    },
];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::ErrorCode;

    #[test]
    fn operations_are_named_once_and_list_only_known_codes() {
        let mut ids = HashSet::new();
        let mut routes = HashSet::new();
        for operation in OPERATIONS {
            assert!(ids.insert(operation.id), "{} is listed twice", operation.id);
            assert!(
                routes.insert((operation.method, operation.path)),
                "{} {} is listed twice",
                operation.method.as_str(),
                operation.path
            );
            for param in operation.params {
                assert!(
                    operation.path.contains(&format!("{{{}}}", param.name)),
                    "{} has no `{}` in its path",
                    operation.id,
                    param.name
                );
            }
            assert_eq!(
                operation.path.matches('{').count(),
                operation.params.len(),
                "{} doesn't describe each of its path parameters",
                operation.id
            );
            for (_, codes) in operation.errors() {
                for code in codes {
                    assert!(
                        code == ANY_RENDER_FAILURE
                            || ErrorCode::LISTED.iter().any(|(listed, _)| *listed == code),
                        "{} lists the unknown code `{code}`",
                        operation.id
                    );
                }
            }
        }
    }

    #[test]
    fn requests_find_their_operation() {
        let id = |method, path| Operation::of(method, path).map(|operation| operation.id);
        assert_eq!(id("GET", "/renders/r_1"), Some("getRender"));
        assert_eq!(id("post", "/renders/check"), Some("checkRender"));
        assert_eq!(
            id("PUT", "/media/m_1/multipart/u_1/parts/2"),
            Some("putMultipartPart")
        );
        assert_eq!(id("GET", "/renders/check"), None);
        assert_eq!(id("DELETE", "/media/url"), None);
        assert_eq!(id("GET", "/renders"), None);
        assert_eq!(id("GET", "/renders/"), None);
        assert_eq!(id("PATCH", "/renders/r_1"), None);
    }
}
