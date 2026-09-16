use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    recuvora::boot::run_cli().await
}
