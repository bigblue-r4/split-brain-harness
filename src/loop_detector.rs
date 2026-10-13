//! Pure advisory heuristics over ordered turns; never invokes Lucid or tools.
//! Thresholds are deliberately conservative and do not diagnose a defect.
use crate::{
    audit::fingerprint,
    dream::{GeneratedClaim, SpeculativeTrust, SuggestedLucid},
    types::TraceEntry,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct LoopTurn {
    pub output: String,
    /// Source trace referenced by ordinal and fingerprint, never echoed verbatim.
    pub trace: TraceEntry,
}

/// Analyzes the trailing 128 turns, excluding outputs larger than 16KiB without
/// compressing turn order. Requires >=3 comparable turns for a pattern.
/// Flags: normalized exact output repeated >=3 times; two consecutive pairs
/// with token-set Jaccard >=0.85 (>=4 distinct tokens); hedging/refusal on >=3
/// within a window of 4 turns; or >=4 turns sharing >=3 non-stopword opening tokens with Jaccard >=0.6.
/// Refusals produce only review advice, never an instruction to circumvent safety.
pub fn suggest_lucid(turns: &[LoopTurn]) -> Option<SuggestedLucid> {
    if turns.len() < 3 {
        return None;
    }
    let offset = turns.len().saturating_sub(128);
    let turns = &turns[offset..];
    let normalized: Vec<String> = turns
        .iter()
        .map(|t| {
            if t.output.len() > 16_384 {
                String::new()
            } else {
                normalize(&t.output)
            }
        })
        .collect();
    let tokens: Vec<BTreeSet<&str>> = normalized
        .iter()
        .map(|s| s.split_whitespace().collect())
        .collect();
    let mut flagged = BTreeSet::new();
    let mut reasons = BTreeSet::new();
    let mut windows: BTreeMap<usize, BTreeSet<(usize, usize)>> = BTreeMap::new();
    let mut identical: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, text) in normalized.iter().enumerate().filter(|(_, s)| !s.is_empty()) {
        identical.entry(text).or_default().push(index);
    }
    for indices in identical.values().filter(|i| i.len() >= 3) {
        reasons.insert("repeated output pattern");
        flagged.extend(indices.iter().copied());
    }
    for index in 2..turns.len() {
        let comparable = (index - 2..=index).all(|i| tokens[i].len() >= 4);
        if comparable
            && jaccard(&tokens[index - 2], &tokens[index - 1]) >= 0.85
            && jaccard(&tokens[index - 1], &tokens[index]) >= 0.85
        {
            reasons.insert("high similarity across consecutive turns");
            flagged.extend(index - 2..=index);
        }
    }
    let refusals: Vec<bool> = normalized.iter().map(|s| refusal(s)).collect();
    let hedges: Vec<bool> = tokens
        .iter()
        .map(|t| {
            ["maybe", "perhaps", "might", "uncertain"]
                .iter()
                .any(|word| t.contains(word))
        })
        .collect();
    for (matches, reason) in [
        (&refusals, "repeated refusal requiring safety review"),
        (&hedges, "repeated hedging"),
    ] {
        for start in 0..turns.len().saturating_sub(2) {
            let end = (start + 4).min(turns.len());
            if matches[start..end]
                .iter()
                .filter(|matched| **matched)
                .count()
                >= 3
            {
                reasons.insert(reason);
                for index in (start..end).filter(|i| matches[*i]) {
                    flagged.insert(index);
                    windows
                        .entry(index)
                        .or_default()
                        .insert((offset + start + 1, offset + end));
                }
            }
        }
    }
    let anchors: Vec<Vec<&str>> = normalized
        .iter()
        .map(|s| {
            s.split_whitespace()
                .filter(|w| {
                    !["the", "a", "an", "i", "we", "is", "it", "and", "to", "of"].contains(w)
                })
                .take(3)
                .collect()
        })
        .collect();
    for start in 0..turns.len().saturating_sub(3) {
        if anchors[start].len() == 3
            && (start + 1..start + 4)
                .all(|i| anchors[i] == anchors[start] && jaccard(&tokens[start], &tokens[i]) >= 0.6)
        {
            reasons.insert("possible reasoning fixation on the same opening premise");
            flagged.extend(start..start + 4);
        }
    }
    if reasons.is_empty() {
        return None;
    }
    let rationale = format!("Advisory review only: {}. These patterns do not establish a defect. Review task progress and legitimate safety refusals with an operator before considering Lucid. Preserve safety policy; never bypass a refusal or automatically invoke Lucid.", reasons.into_iter().collect::<Vec<_>>().join("; "));
    let supporting_trace = flagged.into_iter().map(|index| {
        // Only bounded fingerprints and ordinals enter the suggestion. Caller
        // resolves these references against the original ordered trace sequence.
        let encoded = serde_json::to_vec(&turns[index].trace).ok()?;
        let window_note = windows.get(&index).map(|ranges| format!("; matched hedge/refusal windows: {}", ranges.iter().map(|(start,end)| format!("turns {start}..{end}")).collect::<Vec<_>>().join(", "))).unwrap_or_default();
        Some(TraceEntry {stage:format!("loop-detector:turn:{}", offset + index + 1),claim:"Advisory pattern reference; no defect established".into(),
            evidence:Some(format!("source trace fingerprint {}",fingerprint(&encoded))),passed:false,
            note:Some(format!("Review source turn; safety refusal remains legitimate and grants no bypass authority{window_note}"))})
    }).collect::<Option<Vec<_>>>()?;
    Some(SuggestedLucid {
        rationale: GeneratedClaim {
            text: rationale,
            trust: SpeculativeTrust::Hypothetical,
        },
        supporting_trace,
    })
}
fn normalize(text: &str) -> String {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
fn jaccard(left: &BTreeSet<&str>, right: &BTreeSet<&str>) -> f64 {
    let union = left.union(right).count();
    if union == 0 {
        0.0
    } else {
        left.intersection(right).count() as f64 / union as f64
    }
}
fn refusal(text: &str) -> bool {
    [
        "cannot help",
        "can t help",
        "cannot assist",
        "won t help",
        "must refuse",
        "i refuse",
        "not able to assist",
    ]
    .iter()
    .any(|phrase| text.contains(phrase))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn turns(outputs: &[&str]) -> Vec<LoopTurn> {
        outputs
            .iter()
            .map(|s| LoopTurn {
                output: (*s).into(),
                trace: TraceEntry {
                    stage: "source".into(),
                    claim: "private source text".into(),
                    evidence: None,
                    passed: false,
                    note: None,
                },
            })
            .collect()
    }
    #[test]
    fn empty_short_blank_and_diverse_sequences_have_no_suggestion() {
        for output in [
            vec![],
            vec!["single answer"],
            vec!["", "", ""],
            vec![
                "First identify inputs",
                "Then calculate result",
                "Finally verify independent evidence",
            ],
        ] {
            assert!(suggest_lucid(&turns(&output)).is_none());
        }
    }
    #[test]
    fn repeated_output_is_deterministic_and_references_source_without_echo() {
        let sequence = turns(&["Same OUTPUT!", "same output", "Same output."]);
        let suggestion = suggest_lucid(&sequence).unwrap();
        suggestion.validate().unwrap();
        assert_eq!(
            serde_json::to_string(&suggestion).unwrap(),
            serde_json::to_string(&suggest_lucid(&sequence).unwrap()).unwrap()
        );
        assert_eq!(suggestion.supporting_trace.len(), 3);
        assert!(suggestion.rationale.text.contains("repeated output"));
        assert!(!serde_json::to_string(&suggestion)
            .unwrap()
            .contains("private source text"));
    }
    #[test]
    fn token_similarity_flags_consecutive_paraphrased_order() {
        let suggestion = suggest_lucid(&turns(&[
            "Check alpha beta gamma delta",
            "Alpha beta check delta gamma",
            "Beta gamma delta check alpha",
        ]))
        .unwrap();
        assert!(suggestion.rationale.text.contains("high similarity"));
    }
    #[test]
    fn repeated_hedging_is_advisory() {
        let suggestion = suggest_lucid(&turns(&[
            "Maybe inspect inputs",
            "Perhaps ask for evidence",
            "We might verify assumptions",
        ]))
        .unwrap();
        assert!(suggestion.rationale.text.contains("repeated hedging"));
        assert_eq!(suggestion.rationale.trust, SpeculativeTrust::Hypothetical);
    }
    #[test]
    fn fixation_requires_four_shared_opening_premises() {
        let outputs = [
            "Always assume network failure now",
            "Always assume network failure again",
            "Always assume network failure later",
            "Always assume network failure first",
        ];
        assert!(suggest_lucid(&turns(&outputs[..3])).is_none());
        assert!(suggest_lucid(&turns(&outputs))
            .unwrap()
            .rationale
            .text
            .contains("fixation"));
    }
    #[test]
    fn single_and_repeated_safety_refusals_never_imply_bypass() {
        let refusal = "I cannot help with unsafe instructions";
        assert!(suggest_lucid(&turns(&[refusal])).is_none());
        assert!(suggest_lucid(&turns(&[
            "Reviewing task",
            refusal,
            "Suggest a safe alternative"
        ]))
        .is_none());
        let suggestion = suggest_lucid(&turns(&[refusal, refusal, refusal])).unwrap();
        assert!(suggestion
            .rationale
            .text
            .contains("legitimate safety refusals"));
        assert!(suggestion.rationale.text.contains("never bypass"));
        assert!(suggestion.supporting_trace.iter().all(|t| !t.passed));
    }
    #[test]
    fn limits_keep_trailing_turns_and_exclude_oversized_outputs() {
        let oversized = "x".repeat(16_385);
        assert!(suggest_lucid(&turns(&[&oversized, &oversized, &oversized])).is_none());
        let suggestion = suggest_lucid(&turns(&vec!["repeat"; 130])).unwrap();
        assert_eq!(suggestion.supporting_trace.len(), 128);
        assert_eq!(suggestion.supporting_trace[0].stage, "loop-detector:turn:3");
        assert_eq!(
            suggestion.supporting_trace[127].stage,
            "loop-detector:turn:130"
        );
        let suggestion =
            suggest_lucid(&turns(&[&oversized, "repeat", "repeat", "repeat"])).unwrap();
        assert_eq!(suggestion.supporting_trace.len(), 3);
        assert_eq!(suggestion.supporting_trace[0].stage, "loop-detector:turn:2");
        assert!(suggest_lucid(&turns(&[
            "Maybe consider alpha",
            &oversized,
            "Perhaps evaluate beta"
        ]))
        .is_none());
    }

    fn diverse_turns() -> Vec<LoopTurn> {
        turns(
            &(0..30)
                .map(|index| format!("distinct{index} topic{index} result{index}"))
                .collect::<Vec<_>>()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        )
    }
    #[test]
    fn scattered_hedges_do_not_flag_a_diverse_session() {
        let mut sequence = diverse_turns();
        sequence[1].output = "Maybe inspect inputs".into();
        sequence[15].output = "Perhaps ask for evidence".into();
        sequence[29].output = "We might verify assumptions".into();
        assert!(suggest_lucid(&sequence).is_none());
    }
    #[test]
    fn scattered_refusals_do_not_flag_a_diverse_session() {
        let mut sequence = diverse_turns();
        sequence[1].output = "I cannot help with unsafe instructions".into();
        sequence[15].output = "I cannot assist with private credentials".into();
        sequence[29].output = "I must refuse this harmful request".into();
        assert!(suggest_lucid(&sequence).is_none());
    }
    #[test]
    fn local_windows_record_original_ordinals_in_long_sessions() {
        let mut sequence = diverse_turns();
        sequence.extend(diverse_turns());
        sequence.extend(diverse_turns());
        sequence.extend(diverse_turns());
        sequence.extend(diverse_turns());
        // Unique outputs avoid the unrelated exact repetition heuristic.
        for (index, turn) in sequence.iter_mut().enumerate() {
            turn.output = format!("unique{index} subject{index} output{index}");
        }
        sequence[145].output = "Maybe inspect inputs".into();
        sequence[147].output = "Perhaps ask for evidence".into();
        sequence[148].output = "We might verify assumptions".into();
        let suggestion = suggest_lucid(&sequence).unwrap();
        assert_eq!(
            suggestion
                .supporting_trace
                .iter()
                .map(|t| t.stage.as_str())
                .collect::<Vec<_>>(),
            vec![
                "loop-detector:turn:146",
                "loop-detector:turn:148",
                "loop-detector:turn:149"
            ]
        );
        assert!(suggestion.supporting_trace.iter().all(|t| t
            .note
            .as_deref()
            .unwrap()
            .contains("turns 146..149")));
    }
}
