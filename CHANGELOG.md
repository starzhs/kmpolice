# Changelog

## 0.1.0

### Summary
- First release focused on fast Kotlin->Swift impact checks in KMP repositories.
- Main reliability target is diff-aware analysis (`git` / `mr` modes).
- Tool is best-effort and designed to complement (not replace) full project builds.

### Core Functionality
- Kotlin interface vs Swift protocol compatibility checks.
- Swift conformance checks after Kotlin contract changes.
- Kotlin class method call checks from Swift (argument count and labels).
- Kotlin constructor call checks from Swift.
- Kotlin property usage checks from Swift:
  - missing/renamed properties,
  - mutability mismatches (`val` vs assignment),
  - nullability mismatches,
  - typed-read mismatches.
- Enum/sealed case coverage checks in Swift `switch onEnum`.
- Top-level Kotlin symbol checks (`*Kt.member`).
- Companion object/member checks.

### Diff-aware Reliability Enhancements
- `kotlin_type_usage_missing` now uses diff facts:
  - Kotlin symbol removed in diff,
  - same-name Swift symbol added in diff,
  - dependency manifest changes in diff.
- In ambiguous cases severity is softened to warning.
- In strong factual cases severity stays error.

### Explainability
- Added `evidence[]` to diagnostics JSON output.
- Text output now includes evidence lines when available.

### Test Coverage
- Added/expanded unit tests for constructor/property/enum-sealed/type/top-level/companion scenarios.
