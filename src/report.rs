use anyhow::Result;
use serde::Serialize;

use crate::config::Severity;
use crate::model::Diagnostic;

#[derive(Debug, Serialize)]
struct Summary {
    total: usize,
    errors: usize,
    warnings: usize,
    infos: usize,
}

#[derive(Debug, Serialize)]
struct JsonReport<'a> {
    summary: Summary,
    diagnostics: &'a [Diagnostic],
}

pub fn render_text(diagnostics: &[Diagnostic]) -> String {
    let summary = summarize(diagnostics);
    let mut output = format!(
        "Diagnostics: {} total ({} error, {} warning, {} info)",
        summary.total, summary.errors, summary.warnings, summary.infos
    );

    if diagnostics.is_empty() {
        output.push_str("\nNo problems found.");
        return output;
    }

    for diagnostic in diagnostics {
        output.push_str("\n\n");
        output.push_str(&format!("[{}] {}", diagnostic.code, diagnostic.message));

        if let Some(kotlin_location) = &diagnostic.kotlin_location {
            output.push_str(&format!(
                "\n  Kotlin: {}:{}:{}",
                kotlin_location.path, kotlin_location.line, kotlin_location.column
            ));
        }

        if let Some(ios_location) = &diagnostic.ios_location {
            output.push_str(&format!(
                "\n  iOS: {}:{}:{}",
                ios_location.path, ios_location.line, ios_location.column
            ));
        }

        if diagnostic.base_ref.is_some() || diagnostic.head_ref.is_some() {
            output.push_str(&format!(
                "\n  Context: base={} head={}",
                diagnostic.base_ref.as_deref().unwrap_or("-"),
                diagnostic.head_ref.as_deref().unwrap_or("-"),
            ));
        }

        output.push_str(&format!("\n  Hint: {}", diagnostic.hint));
        if !diagnostic.evidence.is_empty() {
            output.push_str(&format!("\n  Evidence: {}", diagnostic.evidence.join("; ")));
        }
    }

    output
}

pub fn render_json(diagnostics: &[Diagnostic]) -> Result<String> {
    let report = JsonReport {
        summary: summarize(diagnostics),
        diagnostics,
    };

    Ok(serde_json::to_string_pretty(&report)?)
}

fn summarize(diagnostics: &[Diagnostic]) -> Summary {
    let mut errors = 0;
    let mut warnings = 0;
    let mut infos = 0;

    for diagnostic in diagnostics {
        match diagnostic.severity {
            Severity::Error => errors += 1,
            Severity::Warning => warnings += 1,
            Severity::Info => infos += 1,
        }
    }

    Summary {
        total: diagnostics.len(),
        errors,
        warnings,
        infos,
    }
}
