mod kotlin;
mod swift;

use anyhow::Result;

use crate::model::{AnalysisResult, ProjectSnapshot};

pub fn analyze(snapshot: &ProjectSnapshot) -> Result<AnalysisResult> {
    let kotlin_contracts = kotlin::parse_kotlin_files(&snapshot.kotlin_files)?;
    let (swift_protocols, swift_types) = swift::parse_swift_files(&snapshot.ios_files)?;

    Ok(AnalysisResult {
        kotlin_contracts,
        swift_protocols,
        swift_types,
    })
}
