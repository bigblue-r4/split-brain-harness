//! Normalizer-only benchmark: reads JSONL rows from a file, runs the
//! normalizer pass, reports which entries were caught and the detection types.
//! Usage: cargo run --example norm_bench -- <jsonl_file> [--fn-only]

use split_brain_harness::normalizer;
use std::io::{BufRead, BufReader};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("usage: norm_bench <jsonl_file>");
    let fn_only = args.iter().any(|a| a == "--fn-only");

    let f = std::fs::File::open(path).expect("cannot open file");
    let reader = BufReader::new(f);

    let mut total = 0usize;
    let mut caught = 0usize;
    let mut missed: Vec<String> = Vec::new();

    for line in reader.lines() {
        let line = line.unwrap();
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(&line).expect("bad json");

        let outcome = v["outcome"].as_str().unwrap_or("");
        let text = v["text"].as_str().unwrap_or("");

        if fn_only && outcome != "FN" {
            continue;
        }

        total += 1;
        let r = normalizer::run(text);

        let detected = r.obfuscation_score > 0.10;
        if detected {
            caught += 1;
        }

        let kinds: Vec<String> = r.detections.iter().map(|d| d.kind.to_string()).collect();
        let label = if detected { "✓" } else { "✗" };
        let norm_preview = &r.normalized[..r.normalized.len().min(60)];
        println!(
            "{label}  score={:.2}  [{:<30}]  {:?}",
            r.obfuscation_score,
            kinds.join(","),
            norm_preview
        );
        if !detected {
            missed.push(text[..text.len().min(80)].to_string());
        }
    }

    println!();
    println!("──────────────────────────────────────────────");
    println!(
        "Caught:  {caught}/{total}  ({:.0}%)",
        caught as f64 / total.max(1) as f64 * 100.0
    );
    println!("Missed:  {}/{total}", total - caught);
    if !missed.is_empty() {
        println!("\nMissed inputs:");
        for m in &missed {
            println!("  {:?}", m);
        }
    }
}
