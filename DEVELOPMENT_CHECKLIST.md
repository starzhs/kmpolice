# kmpolice Development Checklist

Формат:
- [x] реализовано
- [ ] в план

## v0.1.1 Scope
- [x] Фокус на `git`/`mr` как на основном надёжном режиме (diff-aware)
- [x] Источник истины — Kotlin API (`base` vs `head`)
- [x] Проверяем impact в Swift по фактам добавления/удаления/изменения Kotlin-символов
- [x] Быстрый best-effort анализ, без попытки заменить полную сборку

## Реализовано в коде
- [x] Kotlin `interface` ↔ Swift `protocol` checks (members/signatures/conformance)
- [x] Kotlin class method call checks (parameter count/name)
- [x] Kotlin constructor call checks
- [x] Kotlin property usage checks (`missing`, `type`, `val/var`, `nullability`)
- [x] Enum/Sealed switch coverage checks (`switch onEnum`)
- [x] Top-level Kotlin member checks (`*Kt.member`)
- [x] Companion object/member checks
- [x] Diff-aware severity for `kotlin_type_usage_missing`

## Усиления v0.1.1
- [x] JSON evidence (`diagnostic.evidence[]`) для объяснимости вывода
- [x] Softening reasons для неоднозначных кейсов (`softened_due_to_ambiguity`, etc.)
- [x] Явный список dependency manifests для проверки изменений в diff
- [x] Проверка добавления одноимённого Swift-символа в diff как сигнал возможной замены типа
- [x] Интеграционные unit-тесты по новым диагностическим сценариям

## Git robustness improvements
- [x] (2) Shallow-clone awareness (`--is-shallow-repository`) + actionable hint for `merge-base`
- [x] (7) Noise guard for line-ending/filemode-like changes via meaningful code diff check (`*.kt`, `*.swift`)
- [x] (8) Pragmatic filtering of obvious generated/build garbage in repo scan
- [x] (10) Detached HEAD handling (explicit log, continue with refs)
- [x] (11) Unmerged/conflict guard (stop early with clear error)

## Отложено (Roadmap)
- [ ] Остальные git edge-cases из расширенного списка (вне 0.1.1 scope)
- [ ] Full-scan uncertainty model без diff (глубокая оценка происхождения символов)
- [ ] Полноценный module/type resolution через Swift toolchain (SourceKit/indexstore)
- [ ] Сложные alias/import transitive-resolution сценарии
- [ ] Interop-аннотации (`@ObjCName`, `@HiddenFromObjC`, `@Throws`)
- [ ] Visibility regressions (`public -> internal/private`)
