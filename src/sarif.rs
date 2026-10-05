//! SARIF 2.1.0 export (`--format sarif`).
//!
//! Replaces the per-crate JSON report array with one complete SARIF log on
//! the same log channel (compact, single line), so wrapper-mode output stays
//! machine-parsable without an external converter. The mapping mirrors the
//! external adapter in the lockbud-stable workspace
//! (`agents/lockbud_agents/adapters`) so both exports are interchangeable:
//! witness paths as `codeFlows.threadFlows`, lock types in result
//! properties, and lockbud's end-exclusive `line:col: line:col` spans
//! converted to SARIF's inclusive `endColumn`.

use serde_json::{json, Value};

use crate::detector::lock::report::DeadlockDiagnosis;
use crate::detector::report::Report;

const SARIF_SCHEMA: &str = "https://docs.oasis-open.org/sarif/sarif/v2.1.0/errata01/os/schemas/sarif-schema-2.1.0.json";

/// A parsed lockbud span. `end_col_excl` is ONE PAST the last column
/// (rustc convention); conversion to SARIF's inclusive endColumn happens in
/// [`sarif_region`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Span {
    path: String,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col_excl: u32,
}

/// Parse `src/main.rs:33:10: 33:34 (#0)` (the ` (#N)` macro-context suffix
/// is optional and dropped; it has no SARIF equivalent).
fn parse_span(text: &str) -> Option<Span> {
    let core = match text.trim().split_once(" (#") {
        Some((core, _)) => core,
        None => text.trim(),
    };
    let (head, tail) = core.split_once(": ")?;
    let (path, sl, sc) = split_line_col(head)?;
    let (el_str, ec_str) = tail.split_once(':')?;
    let el = el_str.parse::<u32>().ok()?;
    let ec = ec_str.parse::<u32>().ok()?;
    Some(Span {
        path: path.to_owned(),
        start_line: sl,
        start_col: sc,
        end_line: el,
        end_col_excl: ec,
    })
}

/// Parse `path:line:col` from the right: two numeric segments plus a
/// non-empty path prefix (paths may themselves contain colons).
fn split_line_col(s: &str) -> Option<(&str, u32, u32)> {
    let mut parts = s.rsplitn(3, ':');
    let col = parts.next()?.parse::<u32>().ok()?;
    let line = parts.next()?.parse::<u32>().ok()?;
    let path = parts.next()?;
    if path.is_empty() {
        return None;
    }
    Some((path, line, col))
}

/// Find lockbud spans inside a free-form diagnosis string, e.g.
/// "Raw ptr is used at src/main.rs:15:28: 15:32 (#53) after dropped at
/// src/main.rs:10:37: 10:38 (#0)". A span head is a `path:line:col:`
/// word directly followed by a `line:col` word (and an optional `(#N)`).
fn find_spans_in_text(text: &str) -> Vec<Span> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        if let Some(head) = word.strip_suffix(':') {
            if split_line_col(head).is_some()
                && i + 1 < words.len()
                && is_line_col(words[i + 1])
            {
                let mut candidate = format!("{} {}", word, words[i + 1]);
                let mut step = 2;
                if i + 2 < words.len()
                    && words[i + 2].starts_with("(#")
                    && words[i + 2].ends_with(')')
                {
                    candidate.push(' ');
                    candidate.push_str(words[i + 2]);
                    step = 3;
                }
                if let Some(span) = parse_span(&candidate) {
                    spans.push(span);
                    i += step;
                    continue;
                }
            }
        }
        i += 1;
    }
    spans
}

fn is_line_col(s: &str) -> bool {
    let mut parts = s.splitn(2, ':');
    match (parts.next(), parts.next()) {
        (Some(a), Some(b)) => {
            !a.is_empty() && a.parse::<u32>().is_ok() && b.parse::<u32>().is_ok()
        }
        _ => false,
    }
}

fn sarif_region(span: &Span) -> Value {
    let mut region = json!({ "startLine": span.start_line });
    region["startColumn"] = json!(span.start_col);
    if span.end_line != span.start_line {
        region["endLine"] = json!(span.end_line);
    }
    // lockbud end columns are end-exclusive; SARIF endColumn is inclusive.
    region["endColumn"] = json!(span.end_col_excl.saturating_sub(1).max(span.start_col));
    region
}

fn sarif_location(span: &Span) -> Value {
    json!({ "physicalLocation": {
        "artifactLocation": { "uri": span.path },
        "region": sarif_region(span),
    }})
}

fn span_anchor(span: &Span) -> String {
    format!("{}:{}:{}", span.path, span.start_line, span.start_col)
}

fn level_for(possibility: &str) -> &'static str {
    match possibility {
        // SARIF has no certainty gradation; the original word is kept in
        // result properties.
        _ => "warning",
    }
}

fn cwe_for(kind: &str) -> &'static str {
    match kind {
        "DoubleLock" | "ConflictLock" | "CondvarDeadlock" => "CWE-833",
        "UseAfterFree" => "CWE-416",
        "InvalidFree" => "CWE-590",
        "AtomicityViolation" => "CWE-362",
        _ => "",
    }
}

fn short_description(kind: &str) -> String {
    let text = match kind {
        "DoubleLock" => "Same non-reentrant lock acquired again while held",
        "ConflictLock" => {
            "Two locks acquired in opposite orders in different threads"
        }
        "CondvarDeadlock" => "Condvar wait/notify deadlock",
        "UseAfterFree" => "Raw pointer used or escaping after the pointee is dropped",
        "InvalidFree" => "Uninitialized memory dropped as an owning type",
        "AtomicityViolation" => {
            "Non-atomic read-modify-write split across atomic load and store"
        }
        _ => "lockbud finding",
    };
    if matches!(text, "lockbud finding") {
        format!("lockbud {} finding", kind)
    } else {
        text.to_owned()
    }
}

/// One SARIF result under construction, plus the artifact URIs it touched.
struct ResultDraft {
    kind: String,
    result: Value,
    uris: Vec<String>,
}

fn nesting_locations(spans: &[Option<Span>]) -> Value {
    let mut locations = Vec::new();
    let mut nesting = 0;
    for span in spans.iter().flatten() {
        nesting += 1;
        locations.push(json!({
            "location": sarif_location(span),
            "nestingLevel": nesting,
        }));
    }
    json!(locations)
}

fn deadlock_thread_flow(pair: &DeadlockDiagnosis, index: usize) -> (Value, Vec<String>) {
    let first = parse_span(&pair.first_lock_span);
    let second = parse_span(&pair.second_lock_span);
    let chain: Vec<Span> = pair
        .callchains
        .iter()
        .flat_map(|chain| chain.iter())
        .flat_map(|frames| frames.iter())
        .filter_map(|frame| parse_span(frame))
        .collect();
    let mut event_spans: Vec<Option<Span>> = vec![first.clone()];
    event_spans.extend(chain.into_iter().map(Some));
    event_spans.push(second.clone());
    let thread_flow = json!({
        "id": format!("thread-{}", index),
        "message": { "text": format!(
            "lock acquisition path: {} at {} -> {} at {}",
            pair.first_lock_type,
            first.as_ref().map(span_anchor).unwrap_or_default(),
            pair.second_lock_type,
            second.as_ref().map(span_anchor).unwrap_or_default(),
        )},
        "locations": nesting_locations(&event_spans),
    });
    let mut uris = Vec::new();
    for span in event_spans.iter().flatten() {
        uris.push(span.path.clone());
    }
    (thread_flow, uris)
}

fn result_from_report(report: &Report) -> ResultDraft {
    match report {
        Report::DoubleLock(content) => {
            let d = &content.diagnosis;
            let second = parse_span(&d.second_lock_span);
            let first = parse_span(&d.first_lock_span);
            let (thread_flow, mut uris) = deadlock_thread_flow(d, 0);
            let result = json!({
                "ruleId": "lockbud/DoubleLock",
                "level": level_for(&content.possibility),
                "message": { "text": format!(
                    "DoubleLock: the {} guard acquired at {} is still held when the same lock is acquired again at {} ({}).",
                    d.first_lock_type,
                    first.as_ref().map(span_anchor).unwrap_or_default(),
                    second.as_ref().map(span_anchor).unwrap_or_default(),
                    content.explanation,
                )},
                "locations": [second.as_ref().map(sarif_location)
                    .unwrap_or_else(unknown_location)],
                "relatedLocations": first.iter().map(sarif_location).collect::<Vec<_>>(),
                "codeFlows": [{
                    "message": { "text": thread_flow["message"]["text"].clone() },
                    "threadFlows": [thread_flow],
                }],
                "properties": {
                    "detector": "lockbud",
                    "bugKind": "DoubleLock",
                    "possibility": content.possibility,
                    "first_lock_type": d.first_lock_type,
                    "second_lock_type": d.second_lock_type,
                },
            });
            if let Some(first) = &first {
                uris.push(first.path.clone());
            }
            if let Some(second) = &second {
                uris.push(second.path.clone());
            }
            ResultDraft {
                kind: "DoubleLock".into(),
                result,
                uris,
            }
        }
        Report::ConflictLock(content) => {
            let pairs = &content.diagnosis;
            let mut locations = Vec::new();
            let mut related = Vec::new();
            let mut uris = Vec::new();
            let mut flows = Vec::new();
            let mut lock_pairs = Vec::new();
            for (idx, pair) in pairs.iter().enumerate() {
                let first = parse_span(&pair.first_lock_span);
                let second = parse_span(&pair.second_lock_span);
                if idx == 0 {
                    if let Some(second) = &second {
                        locations.push(sarif_location(second));
                        uris.push(second.path.clone());
                    }
                    if let Some(first) = &first {
                        related.push(sarif_location(first));
                        uris.push(first.path.clone());
                    }
                } else {
                    for span in [&first, &second].into_iter().flatten() {
                        related.push(sarif_location(span));
                        uris.push(span.path.clone());
                    }
                }
                let (thread_flow, flow_uris) = deadlock_thread_flow(pair, idx);
                uris.extend(flow_uris);
                flows.push(thread_flow);
                lock_pairs.push(json!({
                    "first_lock_type": pair.first_lock_type,
                    "first_lock_span": pair.first_lock_span,
                    "second_lock_type": pair.second_lock_type,
                    "second_lock_span": pair.second_lock_span,
                }));
            }
            if locations.is_empty() {
                locations.push(unknown_location());
            }
            let result = json!({
                "ruleId": "lockbud/ConflictLock",
                "level": level_for(&content.possibility),
                "message": { "text": format!(
                    "ConflictLock: {} acquisition sequences lock the same pair of locks in opposite orders ({}); the waits can form a cycle.",
                    pairs.len(), content.explanation,
                )},
                "locations": locations,
                "relatedLocations": related,
                "codeFlows": [{
                    "message": { "text": format!("Witness paths ({})", flows.len()) },
                    "threadFlows": flows,
                }],
                "properties": {
                    "detector": "lockbud",
                    "bugKind": "ConflictLock",
                    "possibility": content.possibility,
                    "lock_pairs": lock_pairs,
                },
            });
            ResultDraft {
                kind: "ConflictLock".into(),
                result,
                uris,
            }
        }
        Report::AtomicityViolation(content) => {
            let d = &content.diagnosis;
            let writer = parse_span(&d.atomic_writer);
            let reader = parse_span(&d.atomic_reader);
            let result = json!({
                "ruleId": "lockbud/AtomicityViolation",
                "level": level_for(&content.possibility),
                "message": { "text": format!(
                    "AtomicityViolation: in {} the atomic store at {} is {}-dependent on the load of the same atomic at {}.",
                    d.fn_name,
                    writer.as_ref().map(span_anchor).unwrap_or_default(),
                    d.dep_kind.to_lowercase(),
                    reader.as_ref().map(span_anchor).unwrap_or_default(),
                )},
                "locations": [writer.as_ref().map(sarif_location)
                    .unwrap_or_else(unknown_location)],
                "relatedLocations": reader.iter().map(sarif_location).collect::<Vec<_>>(),
                "properties": {
                    "detector": "lockbud",
                    "bugKind": "AtomicityViolation",
                    "possibility": content.possibility,
                    "fn_name": d.fn_name,
                    "dep_kind": d.dep_kind,
                },
            });
            let mut uris = Vec::new();
            for span in [&writer, &reader].into_iter().flatten() {
                uris.push(span.path.clone());
            }
            ResultDraft {
                kind: "AtomicityViolation".into(),
                result,
                uris,
            }
        }
        Report::InvalidFree(content) | Report::UseAfterFree(content) => {
            string_report(kind_name(report), content)
        }
        Report::CondvarDeadlock(content) => {
            // No toy in the corpus exercises this kind; emit a generic
            // result with the serialized diagnosis in properties and spans
            // scanned from every string in it.
            let diagnosis = serde_json::to_value(&content.diagnosis)
                .unwrap_or_else(|_| Value::Null);
            let mut spans = Vec::new();
            collect_spans_from_value(&diagnosis, &mut spans);
            let primary = spans.first().cloned();
            let result = json!({
                "ruleId": "lockbud/CondvarDeadlock",
                "level": level_for(&content.possibility),
                "message": { "text": content.explanation },
                "locations": [primary.as_ref().map(sarif_location)
                    .unwrap_or_else(unknown_location)],
                "relatedLocations": spans.iter().skip(1).map(sarif_location)
                    .collect::<Vec<_>>(),
                "properties": {
                    "detector": "lockbud",
                    "bugKind": "CondvarDeadlock",
                    "possibility": content.possibility,
                    "diagnosis": diagnosis,
                },
            });
            let uris = spans.iter().map(|s| s.path.clone()).collect();
            ResultDraft {
                kind: "CondvarDeadlock".into(),
                result,
                uris,
            }
        }
    }
}

fn kind_name(report: &Report) -> &str {
    match report {
        Report::DoubleLock(_) => "DoubleLock",
        Report::ConflictLock(_) => "ConflictLock",
        Report::CondvarDeadlock(_) => "CondvarDeadlock",
        Report::AtomicityViolation(_) => "AtomicityViolation",
        Report::InvalidFree(_) => "InvalidFree",
        Report::UseAfterFree(_) => "UseAfterFree",
    }
}

fn string_report(
    kind: &str,
    content: &crate::detector::report::ReportContent<String>,
) -> ResultDraft {
    let spans = find_spans_in_text(&content.diagnosis);
    let primary = spans.first().cloned();
    let result = json!({
        "ruleId": format!("lockbud/{}", kind),
        "level": level_for(&content.possibility),
        "message": { "text": content.diagnosis },
        "locations": [primary.as_ref().map(sarif_location)
            .unwrap_or_else(unknown_location)],
        "relatedLocations": spans.iter().skip(1).map(sarif_location)
            .collect::<Vec<_>>(),
        "properties": {
            "detector": "lockbud",
            "bugKind": kind,
            "possibility": content.possibility,
            "diagnosis": content.diagnosis,
        },
    });
    let uris = spans.iter().map(|s| s.path.clone()).collect();
    ResultDraft {
        kind: kind.to_owned(),
        result,
        uris,
    }
}

fn unknown_location() -> Value {
    json!({ "physicalLocation": {
        "artifactLocation": { "uri": "<unknown>" },
        "region": { "startLine": 1 },
    }})
}

fn collect_spans_from_value(value: &Value, spans: &mut Vec<Span>) {
    match value {
        Value::String(s) => spans.extend(find_spans_in_text(s)),
        Value::Array(items) => {
            for item in items {
                collect_spans_from_value(item, spans);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                collect_spans_from_value(item, spans);
            }
        }
        _ => {}
    }
}

/// Build one complete SARIF 2.1.0 log for one analyzed crate.
pub fn reports_to_sarif(crate_name: &str, reports: &[Report]) -> Value {
    let mut rules: Vec<Value> = Vec::new();
    let mut seen_rules: Vec<String> = Vec::new();
    let mut artifacts: Vec<Value> = Vec::new();
    let mut seen_uris: Vec<String> = Vec::new();
    let mut results: Vec<Value> = Vec::new();

    for report in reports {
        let draft = result_from_report(report);
        if !seen_rules.iter().any(|id| id == &draft.kind) {
            seen_rules.push(draft.kind.clone());
            let mut rule = json!({
                "id": format!("lockbud/{}", draft.kind),
                "name": draft.kind,
                "shortDescription": { "text": short_description(&draft.kind) },
                "fullDescription": { "text": short_description(&draft.kind) },
            });
            let cwe = cwe_for(&draft.kind);
            if !cwe.is_empty() {
                rule["properties"] = json!({ "cwe": [cwe] });
            }
            rules.push(rule);
        }
        for uri in &draft.uris {
            if !seen_uris.iter().any(|u| u == uri) {
                seen_uris.push(uri.clone());
                artifacts.push(json!({ "location": { "uri": uri } }));
            }
        }
        results.push(draft.result);
    }

    json!({
        "$schema": SARIF_SCHEMA,
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": {
                "name": "lockbud",
                "version": env!("CARGO_PKG_VERSION"),
                "informationUri": "https://github.com/BurtonQin/lockbud",
                "rules": rules,
            }},
            "results": results,
            "artifacts": artifacts,
            "automationDetails": { "id": format!("lockbud-scan/{}", crate_name) },
            "invocations": [{ "executionSuccessful": true }],
            "properties": {
                "columnConvention": "1-based lines/columns; lockbud spans are end-exclusive and are exported as SARIF's inclusive endColumn",
            },
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detector::atomic::report::AtomicityViolationDiagnosis;
    use crate::detector::lock::report::DeadlockDiagnosis;
    use crate::detector::report::ReportContent;

    fn deadlock_report() -> Report {
        Report::DoubleLock(ReportContent::new(
            "DoubleLock".to_owned(),
            "Possibly".to_owned(),
            DeadlockDiagnosis::new(
                "StdMutex(i32)".to_owned(),
                "src/main.rs:25:13: 25:19 (#0)".to_owned(),
                "StdMutex(i32)".to_owned(),
                "src/main.rs:33:10: 33:34 (#0)".to_owned(),
                vec![vec![vec!["src/main.rs:28:20: 28:38 (#0)".to_owned()]]],
            ),
            "The first lock is not released when acquiring the second lock"
                .to_owned(),
        ))
    }

    #[test]
    fn test_parse_span_with_context_suffix() {
        let span = parse_span("src/main.rs:15:28: 15:32 (#53)").unwrap();
        assert_eq!(span.path, "src/main.rs");
        assert_eq!(span.start_line, 15);
        assert_eq!(span.start_col, 28);
        assert_eq!(span.end_line, 15);
        assert_eq!(span.end_col_excl, 32);
    }

    #[test]
    fn test_parse_single_position_rejected() {
        // A single "path:line:col" position is not a lockbud span.
        assert!(parse_span("library/core/src/sync/atomic.rs:100:1").is_none());
    }

    #[test]
    fn test_find_spans_in_text() {
        let spans = find_spans_in_text(
            "Raw ptr is used at src/main.rs:15:28: 15:32 (#53) after dropped at src/main.rs:10:37: 10:38 (#0)",
        );
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].start_line, 15);
        assert_eq!(spans[1].start_line, 10);
    }

    #[test]
    fn test_double_lock_sarif_shape() {
        let sarif = reports_to_sarif("inter", &[deadlock_report()]);
        assert_eq!(sarif["version"], "2.1.0");
        let run = &sarif["runs"][0];
        assert_eq!(run["tool"]["driver"]["name"], "lockbud");
        assert_eq!(run["automationDetails"]["id"], "lockbud-scan/inter");
        let result = &run["results"][0];
        assert_eq!(result["ruleId"], "lockbud/DoubleLock");
        assert_eq!(result["level"], "warning");
        // end-exclusive 33:34 -> inclusive endColumn 33
        let region = &result["locations"][0]["physicalLocation"]["region"];
        assert_eq!(region["startLine"], 33);
        assert_eq!(region["startColumn"], 10);
        assert_eq!(region["endColumn"], 33);
        let flow = &result["codeFlows"][0]["threadFlows"][0];
        let lines: Vec<u32> = flow["locations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["location"]["physicalLocation"]["region"]["startLine"].as_u64().unwrap() as u32)
            .collect();
        assert_eq!(lines, vec![25, 28, 33]);
        assert_eq!(
            result["properties"]["first_lock_type"],
            "StdMutex(i32)"
        );
        // rules and artifacts registered
        assert_eq!(run["tool"]["driver"]["rules"][0]["id"], "lockbud/DoubleLock");
        assert_eq!(run["tool"]["driver"]["rules"][0]["properties"]["cwe"][0], "CWE-833");
        let uris: Vec<&str> = run["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["location"]["uri"].as_str().unwrap())
            .collect();
        assert_eq!(uris, vec!["src/main.rs"]);
    }

    #[test]
    fn test_conflict_lock_two_thread_flows() {
        let report = Report::ConflictLock(ReportContent::new(
            "ConflictLock".to_owned(),
            "Possibly".to_owned(),
            vec![
                DeadlockDiagnosis::new(
                    "StdMutex(i32)".to_owned(),
                    "src/main.rs:18:16: 18:40 (#0)".to_owned(),
                    "StdRwLockWrite(i32)".to_owned(),
                    "src/main.rs:25:10: 25:35 (#0)".to_owned(),
                    vec![vec![vec!["src/main.rs:20:20: 20:35 (#0)".to_owned()]]],
                ),
                DeadlockDiagnosis::new(
                    "StdRwLockRead(i32)".to_owned(),
                    "src/main.rs:29:16: 29:40 (#0)".to_owned(),
                    "StdMutex(i32)".to_owned(),
                    "src/main.rs:36:10: 36:34 (#0)".to_owned(),
                    vec![vec![vec!["src/main.rs:31:20: 31:38 (#0)".to_owned()]]],
                ),
            ],
            "Locks mutually wait for each other to form a cycle".to_owned(),
        ));
        let sarif = reports_to_sarif("conflict-inter", &[report]);
        let result = &sarif["runs"][0]["results"][0];
        let flows = &result["codeFlows"][0]["threadFlows"];
        assert_eq!(flows.as_array().unwrap().len(), 2);
        let lines_a: Vec<u32> = flows[0]["locations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["location"]["physicalLocation"]["region"]["startLine"].as_u64().unwrap() as u32)
            .collect();
        let lines_b: Vec<u32> = flows[1]["locations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["location"]["physicalLocation"]["region"]["startLine"].as_u64().unwrap() as u32)
            .collect();
        assert_eq!(lines_a, vec![18, 20, 25]);
        assert_eq!(lines_b, vec![29, 31, 36]);
        assert_eq!(result["properties"]["lock_pairs"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_use_after_free_string_diagnosis() {
        let report = Report::UseAfterFree(ReportContent::new(
            "UseAfterFree".to_owned(),
            "Possibly".to_owned(),
            "Raw ptr is used at src/main.rs:15:28: 15:32 (#53) after dropped at src/main.rs:10:37: 10:38 (#0)".to_owned(),
            "Raw ptr is used or escapes the current function after the pointed value is dropped".to_owned(),
        ));
        let sarif = reports_to_sarif("use-after-free", &[report]);
        let result = &sarif["runs"][0]["results"][0];
        assert_eq!(result["ruleId"], "lockbud/UseAfterFree");
        let region = &result["locations"][0]["physicalLocation"]["region"];
        assert_eq!(region["startLine"], 15);
        assert_eq!(region["endColumn"], 31);
        let related = result["relatedLocations"].as_array().unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(
            related[0]["physicalLocation"]["region"]["startLine"],
            10
        );
        assert_eq!(
            result["properties"]["diagnosis"].as_str().unwrap(),
            "Raw ptr is used at src/main.rs:15:28: 15:32 (#53) after dropped at src/main.rs:10:37: 10:38 (#0)"
        );
    }

    #[test]
    fn test_atomicity_violation_sarif_shape() {
        let report = Report::AtomicityViolation(ReportContent::new(
            "AtomicityViolation".to_owned(),
            "Possibly".to_owned(),
            AtomicityViolationDiagnosis {
                fn_name: "buggy_data_dep_i32".to_owned(),
                atomic_reader: "src/main.rs:33:13: 33:38 (#0)".to_owned(),
                atomic_writer: "src/main.rs:36:5: 36:35 (#0)".to_owned(),
                dep_kind: "Data".to_owned(),
            },
            "atomic::store is data/control dependent on atomic::load".to_owned(),
        ));
        let sarif = reports_to_sarif("atomic-violation", &[report]);
        let result = &sarif["runs"][0]["results"][0];
        assert_eq!(result["ruleId"], "lockbud/AtomicityViolation");
        assert_eq!(result["properties"]["dep_kind"], "Data");
        let region = &result["locations"][0]["physicalLocation"]["region"];
        assert_eq!(region["startLine"], 36);
        assert_eq!(region["startColumn"], 5);
        assert_eq!(region["endColumn"], 34);
        assert_eq!(
            result["relatedLocations"][0]["physicalLocation"]["region"]["startLine"],
            33
        );
    }
}
