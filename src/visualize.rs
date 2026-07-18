//! `sbh visualize` — render a HarnessResult trace as a single self-contained HTML
//! page (no external assets). Consumes the already-serializable trace, including
//! the `timing:<stage>` entries the stage pipeline emits, to draw a per-stage
//! flow with durations — the observability "instrument panel" (phase B).

use crate::types::HarnessResult;

/// Minimal HTML escaping for interpolated, potentially model-controlled text.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Per-stage durations parsed from the `timing:<stage>` trace entries.
fn timings(r: &HarnessResult) -> Vec<(String, f64)> {
    r.trace
        .iter()
        .filter_map(|t| {
            let name = t.stage.strip_prefix("timing:")?;
            let micros: f64 = t.claim.split_whitespace().next()?.parse().ok()?;
            Some((name.to_string(), micros / 1000.0)) // ms
        })
        .collect()
}

fn risk_accent(risk: &str) -> &'static str {
    match risk {
        "high" => "#ff5d6c",
        "medium" => "#f5b544",
        "low" => "#5fd08a",
        _ => "#8fa2b4",
    }
}

/// Render the full page.
pub fn render_html(r: &HarnessResult) -> String {
    let tel = &r.telemetry;
    let v = &r.verification;
    let risk = tel.intent_matrix.manipulation_risk.as_str();
    let accent = risk_accent(risk);
    let times = timings(r);
    let total_ms: f64 = times.iter().map(|(_, ms)| ms).sum();
    let max_ms = times.iter().map(|(_, ms)| *ms).fold(0.0_f64, f64::max);

    let mut b = String::new();
    b.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">");
    b.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    b.push_str("<title>SBH trace</title>");
    b.push_str("<style>");
    b.push_str(
        ":root{--bg:#0d141b;--panel:#131e28;--border:#263644;--ink:#dce6ef;--dim:#8fa2b4;--faint:#5f7183;\
        --mono:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;--sans:system-ui,-apple-system,Segoe UI,Roboto,sans-serif}\
        *{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--ink);font-family:var(--sans);line-height:1.5}\
        .wrap{max-width:960px;margin:0 auto;padding:clamp(18px,4vw,44px)}\
        h1{font-size:22px;margin:0 0 4px;font-weight:650}.sub{color:var(--dim);font-family:var(--mono);font-size:12px;margin:0 0 22px}\
        .badge{display:inline-block;padding:2px 10px;border-radius:999px;font-family:var(--mono);font-size:12px;\
        letter-spacing:.1em;text-transform:uppercase;border:1px solid}\
        .grid{display:grid;gap:14px;grid-template-columns:repeat(auto-fit,minmax(230px,1fr));margin:0 0 18px}\
        .card{background:var(--panel);border:1px solid var(--border);border-radius:12px;padding:16px 18px}\
        .card h3{margin:0 0 10px;font-family:var(--mono);font-size:11px;letter-spacing:.14em;text-transform:uppercase;color:var(--dim)}\
        .kv{display:flex;justify-content:space-between;gap:12px;font-size:14px;padding:2px 0}\
        .kv .k{color:var(--dim)}.kv .v{font-family:var(--mono);text-align:right}\
        .flags li{font-family:var(--mono);font-size:12.5px;color:#ffb3bd;padding:3px 0 3px 14px;position:relative;list-style:none}\
        .flags li::before{content:'\\25B8';position:absolute;left:0;color:#ff5d6c}.flags ul{margin:0;padding:0}\
        .flags .none{color:var(--faint);font-family:var(--mono);font-size:12.5px}\
        .stages{display:flex;flex-direction:column;gap:6px}\
        .stage{display:grid;grid-template-columns:130px 1fr 68px;align-items:center;gap:10px;font-size:13px}\
        .stage .n{font-family:var(--mono);color:var(--ink)}.stage .ms{font-family:var(--mono);color:var(--dim);text-align:right}\
        .bar{height:10px;border-radius:5px;background:linear-gradient(90deg,#2f6f8f,#35c9d6);min-width:2px}\
        table{width:100%;border-collapse:collapse;font-size:12.5px;font-family:var(--mono)}\
        th,td{text-align:left;padding:5px 8px;border-bottom:1px solid var(--border);vertical-align:top}\
        th{color:var(--dim);font-weight:500;letter-spacing:.06em}td.p{color:#5fd08a}td.f{color:#ff8b98}\
        .scroll{overflow-x:auto}.muted{color:var(--faint)}",
    );
    b.push_str("</style></head><body><div class=\"wrap\">");

    // Header
    b.push_str("<h1>Split-Brain Harness — trace</h1>");
    b.push_str(&format!(
        "<p class=\"sub\">risk <span class=\"badge\" style=\"color:{a};border-color:{a}\">{r}</span> \
         &nbsp;·&nbsp; confidence {c:.2} &nbsp;·&nbsp; {sas} &nbsp;·&nbsp; total {t:.1} ms</p>",
        a = accent,
        r = esc(risk),
        c = v.confidence,
        sas = if v.stop_and_ask { "⚠ stop_and_ask" } else { "no stop" },
        t = total_ms
    ));

    // Cards: telemetry + verification
    b.push_str("<div class=\"grid\">");
    b.push_str("<div class=\"card\"><h3>Telemetry</h3>");
    for (k, val) in [
        ("emotion", tel.affective_telemetry.primary_emotion.clone()),
        (
            "intensity",
            format!("{:.2}", tel.affective_telemetry.emotional_intensity),
        ),
        (
            "urgency",
            format!("{:.2}", tel.cognitive_state.urgency_vector),
        ),
        (
            "coherence",
            format!("{:.2}", tel.cognitive_state.coherence_rating),
        ),
        ("tone", tel.affective_telemetry.structural_tone.join(", ")),
    ] {
        b.push_str(&format!(
            "<div class=\"kv\"><span class=\"k\">{k}</span><span class=\"v\">{}</span></div>",
            esc(&val)
        ));
    }
    b.push_str("</div>");

    b.push_str("<div class=\"card\"><h3>Verification</h3>");
    b.push_str(&format!(
        "<div class=\"kv\"><span class=\"k\">verdict</span><span class=\"v\">{}</span></div>",
        if v.passed { "passed" } else { "flagged" }
    ));
    b.push_str(&format!(
        "<div class=\"kv\"><span class=\"k\">confidence</span><span class=\"v\">{:.2}</span></div>",
        v.confidence
    ));
    b.push_str(&format!(
        "<div class=\"kv\"><span class=\"k\">flags fired</span><span class=\"v\">{} / {}</span></div>",
        v.disagreement.flag_count, 8
    ));
    b.push_str(&format!(
        "<div class=\"kv\"><span class=\"k\">injection fp</span><span class=\"v\">{}</span></div>",
        v.disagreement.injection_fingerprint
    ));
    b.push_str("</div>");
    b.push_str("</div>"); // grid

    // Objective / subtext
    b.push_str("<div class=\"card\" style=\"margin-bottom:18px\"><h3>Intent</h3>");
    b.push_str(&format!(
        "<div class=\"kv\"><span class=\"k\">objective</span><span class=\"v\" style=\"max-width:70%\">{}</span></div>",
        esc(&tel.intent_matrix.stated_objective)
    ));
    b.push_str(&format!(
        "<div class=\"kv\"><span class=\"k\">subtext</span><span class=\"v\" style=\"max-width:70%\">{}</span></div>",
        esc(&tel.intent_matrix.subtextual_motive)
    ));
    b.push_str("</div>");

    // Stage timings
    if !times.is_empty() {
        b.push_str("<div class=\"card\" style=\"margin-bottom:18px\"><h3>Pipeline stage timings</h3><div class=\"stages\">");
        for (name, ms) in &times {
            let pct = if max_ms > 0.0 {
                (ms / max_ms) * 100.0
            } else {
                0.0
            };
            b.push_str(&format!(
                "<div class=\"stage\"><span class=\"n\">{}</span>\
                 <div class=\"bar\" style=\"width:{:.0}%\"></div>\
                 <span class=\"ms\">{:.1} ms</span></div>",
                esc(name),
                pct.max(2.0),
                ms
            ));
        }
        b.push_str("</div></div>");
    }

    // Refinement
    if let Some(ref rf) = r.refinement {
        b.push_str("<div class=\"card\" style=\"margin-bottom:18px\"><h3>Refinement</h3>");
        b.push_str(&format!(
            "<div class=\"kv\"><span class=\"k\">verdict</span><span class=\"v\">{} (iter {})</span></div>",
            esc(&rf.decision.verdict.to_string()),
            rf.decision.chosen_iteration
        ));
        for it in &rf.iterations {
            b.push_str(&format!(
                "<div class=\"kv\"><span class=\"k\">iter {}</span><span class=\"v\">conf {:.2} · {} · {} flags</span></div>",
                it.iteration,
                it.confidence,
                if it.passed { "passed" } else { "flagged" },
                it.flag_count
            ));
        }
        b.push_str("</div>");
    }

    // Tool-use risk (phase C)
    if let Some(ref tr) = r.tool_risk {
        b.push_str("<div class=\"card\" style=\"margin-bottom:18px\"><h3>Tool-use risk</h3>");
        for (label, on) in [
            ("code execution", tr.code_execution),
            ("web access", tr.web_access),
            ("file write", tr.file_write),
            ("network", tr.network),
            ("shell", tr.shell),
        ] {
            b.push_str(&format!(
                "<div class=\"kv\"><span class=\"k\">{label}</span><span class=\"v\">{}</span></div>",
                if on { "⚠ yes" } else { "—" }
            ));
        }
        b.push_str(&format!(
            "<div class=\"kv\"><span class=\"k\">sources</span><span class=\"v\">{}</span></div>",
            esc(&tr.sources.join(", "))
        ));
        b.push_str("</div>");
    }

    // Formal (phase F)
    if let Some(ref f) = r.formal {
        b.push_str("<div class=\"card\" style=\"margin-bottom:18px\"><h3>Formal checks</h3>");
        b.push_str(&format!(
            "<div class=\"kv\"><span class=\"k\">domains</span><span class=\"v\">{}</span></div>",
            esc(&f.domains.join(", "))
        ));
        b.push_str(&format!(
            "<div class=\"kv\"><span class=\"k\">rules evaluated</span><span class=\"v\">{}</span></div>",
            f.checked.len()
        ));
        b.push_str(&format!(
            "<div class=\"kv\"><span class=\"k\">result</span><span class=\"v\">{}</span></div>",
            if f.passed {
                "✓ pass".to_string()
            } else {
                format!("⚠ {} violation(s)", f.violations.len())
            }
        ));
        for viol in &f.violations {
            b.push_str(&format!(
                "<div class=\"kv\"><span class=\"k\">[{}] {}</span><span class=\"v\">{}</span></div>",
                esc(viol.severity.as_str()),
                esc(&viol.rule_id),
                esc(&viol.message)
            ));
        }
        b.push_str("</div>");
    }

    // Advocate (phase E)
    if let Some(ref a) = r.advocate {
        b.push_str("<div class=\"card\" style=\"margin-bottom:18px\"><h3>Devil's Advocate</h3>");
        b.push_str(&format!(
            "<div class=\"kv\"><span class=\"k\">verdict</span><span class=\"v\">{}{}</span></div>",
            esc(&a.verdict),
            if a.dissented { " ⚠ DISSENT" } else { "" }
        ));
        b.push_str(&format!(
            "<div class=\"kv\"><span class=\"k\">confidence</span><span class=\"v\">{:.2}</span></div>",
            a.confidence
        ));
        b.push_str(&format!(
            "<div class=\"kv\"><span class=\"k\">gate</span><span class=\"v\">{}</span></div>",
            esc(&a.gate_reason.join(", "))
        ));
        if !a.argument.is_empty() {
            b.push_str(&format!(
                "<div class=\"kv\"><span class=\"k\">argument</span><span class=\"v\">{}</span></div>",
                esc(&a.argument)
            ));
        }
        b.push_str("</div>");
    }

    // Flags
    b.push_str("<div class=\"card\" style=\"margin-bottom:18px\"><h3>Consistency flags</h3><div class=\"flags\">");
    if v.consistency_flags.is_empty() {
        b.push_str("<span class=\"none\">none</span>");
    } else {
        b.push_str("<ul>");
        for f in &v.consistency_flags {
            b.push_str(&format!("<li>{}</li>", esc(f)));
        }
        b.push_str("</ul>");
    }
    b.push_str("</div></div>");

    // Full trace
    b.push_str("<div class=\"card\"><h3>Trace</h3><div class=\"scroll\"><table>");
    b.push_str("<tr><th>stage</th><th>claim</th><th>ok</th></tr>");
    for t in &r.trace {
        b.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td class=\"{}\">{}</td></tr>",
            esc(&t.stage),
            esc(&t.claim),
            if t.passed { "p" } else { "f" },
            if t.passed { "✓" } else { "✗" }
        ));
    }
    b.push_str("</table></div></div>");

    b.push_str("</div></body></html>");
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "telemetry":{"affective_telemetry":{"primary_emotion":"neutral","emotional_intensity":0.1,"structural_tone":["analytical"]},
        "intent_matrix":{"stated_objective":"<script>x</script>","subtextual_motive":"m","manipulation_risk":"high"},
        "cognitive_state":{"urgency_vector":0.9,"coherence_rating":0.8}},
      "verification":{"passed":false,"consistency_flags":["urgency may be manufactured"],"unsupported_claims":[],"assumptions":[],"unresolved":[],
        "confidence":0.3,"disagreement":{"flag_count":1,"flag_density":0.125,"dimension_spread":1,"injection_fingerprint":false,"adjusted_confidence":0.3},"stop_and_ask":true},
      "trace":[{"stage":"timing:normalize","claim":"1200 µs","passed":true},{"stage":"propose","claim":"risk=high","passed":true}]
    }"#;

    #[test]
    fn renders_self_contained_html_with_timings() {
        let r: HarnessResult = serde_json::from_str(SAMPLE).unwrap();
        let html = render_html(&r);
        assert!(html.starts_with("<!doctype html"));
        assert!(html.trim_end().ends_with("</html>"));
        assert!(html.contains("Pipeline stage timings"));
        assert!(html.contains("1.2 ms"), "1200 µs should render as 1.2 ms");
        // No external asset references (self-contained).
        assert!(!html.contains("http://") && !html.contains("https://") && !html.contains("src="));
    }

    #[test]
    fn escapes_model_controlled_text() {
        let r: HarnessResult = serde_json::from_str(SAMPLE).unwrap();
        let html = render_html(&r);
        assert!(
            !html.contains("<script>x</script>"),
            "must not inject raw HTML"
        );
        assert!(html.contains("&lt;script&gt;"));
    }
}
