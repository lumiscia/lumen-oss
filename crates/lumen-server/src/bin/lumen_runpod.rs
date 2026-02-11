use std::io::Read;

use lumen_server::runpod::{
    RunpodJobRequest, RunpodJobResponse, RunpodRenderError, handle_runpod_request,
};
use tracing_subscriber::EnvFilter;

fn print_response(response: &RunpodJobResponse) {
    match serde_json::to_string(response) {
        Ok(serialized) => {
            println!("{serialized}");
        }
        Err(err) => {
            let fallback = RunpodJobResponse {
                ok: false,
                artifact: None,
                metrics: None,
                error: Some(RunpodRenderError {
                    code: "serialization_failed".to_string(),
                    message: err.to_string(),
                    retryable: false,
                }),
            };
            if let Ok(serialized) = serde_json::to_string(&fallback) {
                println!("{serialized}");
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        let response = RunpodJobResponse {
            ok: false,
            artifact: None,
            metrics: None,
            error: Some(RunpodRenderError {
                code: "stdin_read_failed".to_string(),
                message: "failed to read runpod payload from stdin".to_string(),
                retryable: true,
            }),
        };
        print_response(&response);
        std::process::exit(1);
    }

    let request = match serde_json::from_str::<RunpodJobRequest>(&input) {
        Ok(request) => request,
        Err(err) => {
            let response = RunpodJobResponse {
                ok: false,
                artifact: None,
                metrics: None,
                error: Some(RunpodRenderError {
                    code: "invalid_request".to_string(),
                    message: err.to_string(),
                    retryable: false,
                }),
            };
            print_response(&response);
            std::process::exit(1);
        }
    };

    let response = handle_runpod_request(request).await;
    let exit_code = if response.ok { 0 } else { 1 };
    print_response(&response);
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}
