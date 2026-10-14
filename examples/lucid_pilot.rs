//! Explicit live runner. Build/tests never execute it; live use needs approval.
use split_brain_harness::{
    dream_engine::Cancellation,
    lucid_pilot::{run_pilot, PilotOptions},
};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut tasks = None;
    let mut output = None;
    let mut endpoint = "http://127.0.0.1:11434".to_string();
    let mut model = "llama3.2:3b".to_string();
    while let Some(key) = args.next() {
        if key == "--help" {
            println!("lucid_pilot --tasks FILE --output-dir NEW_DIR [--endpoint URL] [--model llama3.2:3b]\nBuild with PILOT_GIT_REVISION=<full code commit>. Explicit live approval required.");
            return Ok(());
        }
        let value = args.next().ok_or("argument value missing")?;
        match key.as_str() {
            "--tasks" => tasks = Some(PathBuf::from(value)),
            "--output-dir" => output = Some(PathBuf::from(value)),
            "--endpoint" => endpoint = value,
            "--model" => model = value,
            _ => return Err("unknown argument".into()),
        }
    }
    let code_revision = option_env!("PILOT_GIT_REVISION")
        .ok_or("build with PILOT_GIT_REVISION set to the full reviewed code commit")?
        .to_string();
    let cancel = Cancellation::default();
    let signal = cancel.clone();
    let handler = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal.cancel();
        }
    });
    let result = run_pilot(
        PilotOptions {
            tasks: tasks.ok_or("--tasks is required")?,
            output_dir: output.ok_or("--output-dir is required")?,
            endpoint,
            model,
            code_revision,
        },
        cancel,
    )
    .await;
    handler.abort();
    let report = result?;
    println!("{}: {}", report.status, report.summary.decision);
    if report.status != "completed" {
        return Err("pilot aborted; partial artifacts retained".into());
    }
    Ok(())
}
