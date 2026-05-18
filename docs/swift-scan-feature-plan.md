# Swift Scan Feature Plan

## Цель
Сделать быстрый и предсказуемый поиск Swift call-sites для Kotlin API изменений в MR-режиме:
- с каскадной фильтрацией кандидатов,
- с параллельной обработкой,
- с наглядным прогрессом по этапам,
- без `mock_progress`.

## Контекст
Точка входа сейчас:
- MR pipeline: `src/mr.rs`
- iOS usage search: `src/ios_usage.rs`
- CLI wiring: `src/cli.rs`, `src/lib.rs`

Ограничения текущей версии:
- prefilter и AST-match есть, но вход подается как уже загруженные Swift `SourceFile`.
- import-фильтр захардкожен как `import shared`.
- есть `mock_progress` ветка, которую нужно убрать.

## Принципы
- Источник истины для изменений: Kotlin diff (`merge-base..HEAD` + worktree).
- Swift скан строится от Kotlin change index, а не от полного AST всего Swift кода.
- Параллелим CPU/IO-heavy этапы, но оставляем детерминированный итог.
- Прогресс в `stderr`, машинный вывод (`text/json`) в `stdout`.

## План внедрения

### 1) Удаление mock режима
- Удалить из CLI:
  - `--mock-progress`
  - `--mock-kotlin-files`
  - `--mock-ios-files`
- Удалить `run_mock_progress(...)` из `src/mr.rs`.
- Упростить `run()` в `src/lib.rs` до одного рабочего пути `run_mr(...)`.

Критерий готовности:
- В бинаре нет mock-флагов и mock-веток.

### 2) Конфигурируемый shared SDK import filter
- Добавить `--shared-sdk-name` в CLI.
- Поддержать fallback из config.
- Значение по умолчанию: `shared`.
- В фильтре учитывать:
  - `import <sdk>`
  - `import <sdk>.*`

Критерий готовности:
- Скан iOS кандидатов работает с произвольным именем shared SDK.

### 3) Каскадный pipeline сканирования Swift
- В `ios_usage` перейти от входа `&[SourceFile]` к входу по путям файлов.
- Этапы:
  1. enumerate Swift paths
  2. read file (parallel)
  3. import filter (`shared_sdk_name`)
  4. token prefilter (из Kotlin change index)
  5. AST parse only for candidates
  6. usage match per `ApiChange`

Критерий готовности:
- AST парсится только для отфильтрованных кандидатов.

### 4) Индекс изменений Kotlin для Swift поиска
- Построить структурированный индекс search terms из `ApiChange`:
  - root type для nested-символов (`A.B` -> `A`)
  - owner/member токены для methods/properties/companion
  - facade tokens для top-level (`*Kt`)
  - typealias/type/constructor tokens
- Избежать "сырого" строкового match как единственного источника.

Критерий готовности:
- Для каждого `ApiChange` есть явный набор ожидаемых токенов/признаков для Swift match.

### 5) Учет Swift файлов, уже измененных в MR
- Собрать `SwiftChangedSet` (commit diff + staged + unstaged + untracked).
- Помечать usage-hit как:
  - `already_touched` (если файл уже изменен),
  - `untouched` (если не изменен).
- Использовать это в diagnostic hint и verbose report.

Критерий готовности:
- Отчет различает уже правленные и не правленные Swift call-sites.

### 6) Каскадный параллельный прогресс
- Единый `MultiProgress` с этапами:
  - `Swift enumerate`
  - `Swift import filter`
  - `Swift token filter`
  - `Swift AST parse`
  - `Swift usage match`
- Для каждого этапа:
  - `processed/total`
  - `last file`
  - финальный elapsed summary

Критерий готовности:
- Пользователь видит последовательный каскад прогресса по этапам.

### 7) Диагностика и завершение программы
- Диагностики impact-категорий оставить отдельными кодами (`mr_*_ios_impact`).
- Улучшить hint:
  - что поменялось в Kotlin,
  - какой Swift call-site затронут,
  - touched/untouched статус файла.
- Exit code:
  - `0` — нет диагностик,
  - `1` — есть диагностики,
  - `2` — runtime error.

Критерий готовности:
- Диагностика читаемая и actionable без ручного разбора внутренних логов.

## Порядок реализации
1. Удалить mock режим и обновить CLI.
2. Добавить `shared_sdk_name`.
3. Перевести `ios_usage` на path-based pipeline.
4. Добавить `SwiftChangedSet`.
5. Внедрить каскадный progress.
6. Доточить diagnostics/hints.
7. Обновить `README.md`, `CHANGELOG.md`, профильные docs.

## Риски и контроль
- Риск: деградация скорости из-за лишнего чтения файлов.
  - Контроль: prefilter до AST + parallel read.
- Риск: ложные совпадения по токенам.
  - Контроль: структурированный search index + AST-level match.
- Риск: шумный progress при большом числе файлов.
  - Контроль: этапный progress + стабильное поле `last file`.

## Definition of Done
- Нет mock-фичи в CLI и runtime.
- Есть `--shared-sdk-name` и корректный import filter.
- Swift scan работает каскадом и параллельно.
- Прогресс показывает этапы и `last file`.
- Диагностики учитывают touched/untouched Swift статус.
- Документация синхронизирована с фактическим поведением.
