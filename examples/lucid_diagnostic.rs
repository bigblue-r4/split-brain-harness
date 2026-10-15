//! Ten-call diagnostic. Requires separate live approval; never starts a service.
use split_brain_harness::{
    dream_engine::Cancellation,
    research_ollama::diagnostic::{run, DiagnosticOptions},
};
use std::path::PathBuf;
#[tokio::main]
async fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut output = None;
    let mut pilot = None;
    let mut endpoint = "http://127.0.0.1:11434".to_string();
    let mut confirm = false;
    while let Some(key) = args.next() {
        match key.as_str() {
            "--help" => {
                println!("lucid_diagnostic --pilot-report FILE --output-dir PRIVATE_DIR --confirm-server-debug-1 [--endpoint URL]\nSeparate Chris approval required. Operator starts OLLAMA_DEBUG=1 server, saving ollama-serve.log in that mode0700 directory (logs0600). No service is started by this example. Build with DIAGNOSTIC_GIT_REVISION=<full reviewed commit>. Offline received-parameter log review is required afterward.");
                return Ok(());
            }
            "--confirm-server-debug-1" => confirm = true,
            "--pilot-report" => {
                pilot = Some(PathBuf::from(args.next().ok_or("missing pilot report")?))
            }
            "--output-dir" => {
                output = Some(PathBuf::from(
                    args.next().ok_or("missing output directory")?,
                ))
            }
            "--endpoint" => endpoint = args.next().ok_or("missing endpoint")?,
            _ => return Err("unknown diagnostic argument".into()),
        }
    }
    let code_revision = option_env!("DIAGNOSTIC_GIT_REVISION")
        .ok_or("build with DIAGNOSTIC_GIT_REVISION set to reviewed commit")?
        .to_string();
    let cancel = Cancellation::default();
    let signal = cancel.clone();
    let handler = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal.cancel();
        }
    });
    let outcome = run(
        DiagnosticOptions {
            output_dir: output.ok_or("--output-dir required")?,
            pilot_report: pilot.ok_or("--pilot-report required")?,
            endpoint,
            code_revision,
            confirm_server_debug: confirm,
        },
        cancel,
    )
    .await;
    handler.abort();
    let report = outcome?;
    println!(
        "diagnostic {}: {} attempts; log audit pending",
        report.status,
        report.calls.len()
    );
    if report.status != "completed" {
        return Err("diagnostic aborted; partial artifacts retained".into());
    }
    Ok(())
}
