use viz::{
    Body, Handler, IntoResponse, Request, RequestExt, Response, ResponseExt, StatusCode, Transform,
    async_trait, header,
};

#[derive(Clone, Debug)]
pub struct Config {
    pub secret: String,
}

impl Config {
    pub fn new(secret: String) -> Self {
        Self { secret }
    }
}

impl<H> Transform<H> for Config
where
    H: Clone,
{
    type Output = AuthorizationMiddleware<H>;

    fn transform(&self, h: H) -> Self::Output {
        AuthorizationMiddleware {
            h,
            config: self.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AuthorizationMiddleware<H> {
    h: H,
    config: Config,
}

#[async_trait]
impl<H, O> Handler<Request> for AuthorizationMiddleware<H>
where
    H: Handler<Request, Output = viz::Result<O>>,
    O: IntoResponse,
{
    type Output = viz::Result<Response>;

    async fn call(&self, req: Request) -> Self::Output {
        match req.header::<_, String>(header::AUTHORIZATION) {
            Some(header) => {
                let mut split = header.split(" ");

                if split.next() != Some("Bearer") {
                    let mut resp = Response::with(
                        Body::Full(r#"{"error":"Unauthorized"}"#.into()),
                        "application/json",
                    );

                    *resp.status_mut() = StatusCode::UNAUTHORIZED;

                    return Ok(resp);
                }

                if split.next() != Some(&self.config.secret) {
                    let mut resp = Response::with(
                        Body::Full(r#"{"error":"Unauthorized"}"#.into()),
                        "application/json",
                    );

                    *resp.status_mut() = StatusCode::UNAUTHORIZED;

                    return Ok(resp);
                }

                self.h.call(req).await.map(IntoResponse::into_response)
            }
            None => {
                let mut resp = Response::with(
                    Body::Full(r#"{"error":"Unauthorized"}"#.into()),
                    "application/json",
                );

                *resp.status_mut() = StatusCode::UNAUTHORIZED;

                Ok(resp)
            }
        }
    }
}
