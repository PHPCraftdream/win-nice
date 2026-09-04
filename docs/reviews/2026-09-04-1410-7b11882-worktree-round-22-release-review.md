# Полное статическое release-review — раунд 22

Дата: 2026-09-04 14:10 (Europe/Berlin)

Базовый commit: `7b11882879df538dd52807614b1c47927355c632` (`master`,
отчёт round 21).

Объект review: базовый commit плюс незакоммиченный working-tree snapshot из
семи файлов с исправлениями round 21. До добавления настоящего отчёта delta
составляла 193 additions / 34 deletions:

- `CHANGELOG.md`;
- `bin/admin.ps1`;
- `bin/capt.ps1`;
- `bin/uiup.ps1`;
- `install/paths.js`;
- `test/paths.test.js`;
- `test/win-nice.Tests.ps1`.

SHA-256 этого snapshot:

```text
fdb9c75853b8c85412d83f0cc388288a49c6408ff0883e1e2db9b63d35246a7b  CHANGELOG.md
0c2ebb71904da39f47a6d2e48ddce0c3eba7eb3176446ac145ce315cbfa4ac5b  bin/admin.ps1
698a226ab3a55e90cd7729c0501058c06d29a241c20bd4b2d361956770677247  bin/capt.ps1
cd513ac95dc84163d27d5aa8924084b39c59fbf8890852b3a823e17dda706517  bin/uiup.ps1
5884123fd19f348d6c4af47b2f458f66182fdea46fd26a4eb55398ae90331911  install/paths.js
d31e7d7f8413a79c7265c531a0b18f0cd8b0e3ee0194b88622567d9ce8d8089e  test/paths.test.js
235ed92cca52550e91b1de2efcb6ef7793eca1f87b68f58f012c65659934b010  test/win-nice.Tests.ps1
```

## Вердикт

**В проверенном snapshot не найдено новых P0/P1 и не осталось runtime
release-blocker дефектов из round 21.** UAC path resolution в `admin`/`uiup`,
installer PowerShell resolution, 32-bit `capt` validation, regression coverage
и дата CHANGELOG исправлены статически корректно.

Релиз всё ещё имеет статус **NO-GO по состоянию репозитория**, а не по
production-коду: все семь исправленных файлов находятся только в working tree.
Тег на текущем `HEAD` включит отчёт round 21, но не включит сами security и
validation fixes. Это P2 release-process blocker, который закрывается отдельным
implementation commit перед тегом.

Кроме него найдены два P3 improvement:

- maintainer-only elevated test runner сохраняет тот же bare-`powershell`
  UAC pattern, который теперь устранён из shipped `uiup`;
- `release-check` проверяет CHANGELOG version и compare base, но не наличие и
  корректность release date; поэтому неоднократно возникавший date drift всё
  ещё ловится только review вручную.

После implementation commit и полного обязательного test/release gate этот
snapshot можно выпускать. P3 сами по себе тег не блокируют, хотя первый лучше
закрыть до финального elevated-прогона.

## Режим и scope

Review выполнено строго в режиме **только чтение и статический анализ** по
прямому указанию пользователя. Не запускались Node/Pester suites, parser/test
harness, launchers, installer, `npm pack`, `npm run release-check`, сборка,
shell shims или live security probes. Единственное изменение этого раунда —
настоящий Markdown-отчёт; в review commit будет добавлен только он.

Проверены все 105 tracked-файлов и весь текущий working-tree diff, в том числе:

- 42 файла в `bin/` (14 инструментов × `.ps1`/`.bat`/extensionless shim):
  argument validation/forwarding, CreateProcess/cmd fallback, P/Invoke layouts,
  return-value/error capture, Job Object limits и lifecycle, ownership/cleanup,
  timeout/timer/process race handling, UAC paths;
- `install/*.js`: source-checkout guards, file manifest, PATH registry round
  trip, environment broadcast, skill install/update и новый absolute
  PowerShell resolver;
- `scripts/release-check.js`, exact 54-path tarball allowlist, smoke contract,
  version/CHANGELOG/tag checks;
- `test/win-nice.Tests.ps1`, восемь Node test-файлов и
  `test/run-elevated.ps1` — только чтением;
- `README.md`, packaged `skills/win-nice/SKILL.md`, `CHANGELOG.md`,
  `package.json`, contribution/security docs;
- `.github/workflows/ci.yml` и `publish.yml`, permissions, pinned actions,
  timeouts и publish ordering;
- release delta `v0.1.0..HEAD` и отдельно весь незакоммиченный diff.

Статические факты на начало review:

- `master` опережал локальный tracking ref `origin/master` на 38 commits;
  `git fetch` не выполнялся;
- `HEAD` — `7b11882`, локально существует только тег `v0.1.0`;
- `package.json` — `0.2.0`, рабочий `CHANGELOG.md` —
  `## [0.2.0] - 2026-09-04`;
- 105 tracked-файлов, из них 42 launcher-файла и восемь Node test-файлов;
- committed delta от `v0.1.0`: 80 файлов, 12 039 additions / 832 deletions;
- snapshot вместе с working tree: 82 файла, 12 204 additions / 838 deletions;
- `git diff --check 7b11882` не обнаружил whitespace errors; сообщения о
  будущей LF→CRLF materialization для `*.ps1` соответствуют `.gitattributes`
  и не являются ошибкой содержимого;
- packed-file contract не изменился: новые/изменённые файлы уже входят в
  существующий allowlist; review-файлы не входят в npm package.

## Findings

### P2-1 — исправления релизного блокера существуют только в working tree

**Где:** семь файлов snapshot, перечисленных в начале отчёта; базовый
`HEAD = 7b11882`.

Round 21 поставил NO-GO из-за подмены executable на UAC boundary. В текущем
working tree это исправлено, но Git object, на который можно поставить тег,
изменений не содержит. Если сейчас выполнить `git tag v0.2.0`, tagged source:

- снова передаст bare `cmd.exe`/`powershell` в shipped elevation paths;
- не будет содержать installer hardening;
- оставит старую форму `capt` coverage;
- сохранит дату CHANGELOG `2026-09-03`;
- не будет содержать новые regression tests и release notes.

Публикация из dirty local tree теоретически может упаковать исправленные файлы,
но тогда npm artifact перестанет соответствовать tagged Git source. Publish
workflow, напротив, делает чистый checkout tag и гарантированно получит старый
код. Поэтому это не косметический dirty-tree warning, а реальный release-state
blocker.

**Рекомендация:** после обработки findings этого раунда создать отдельный
implementation commit со всеми семью файлами, убедиться в clean tree, выполнить
полный gate и только затем поставить `v0.2.0`. Review commit настоящего отчёта
не должен случайно забрать implementation files через `git add -A`.

### P3-1 — elevated test runner всё ещё передаёт bare `powershell` в `RunAs`

**Где:** `package.json:36`, `test/run-elevated.ps1:71-76`; связанные
пользовательские команды — `README.md:576-590`.

Shipped `uiup.ps1` теперь правильно использует:

```powershell
[Environment]::SystemDirectory + '\WindowsPowerShell\v1.0\powershell.exe'
```

Но единственная рекомендованная точка запуска финального elevated Pester pass
выполняет тот же старый pattern:

```powershell
Start-Process powershell -Verb RunAs ...
```

А npm script, который должен вызвать helper, сам начинается с bare
`powershell`. Таким образом, политика absolute system executable применяется к
published runtime, но не к maintainer path с настоящим UAC prompt. Репозиторий
обычно является trusted cwd, а `test/` не упаковывается в npm, поэтому это P3,
не возврат P1. Тем не менее именно эту команду release checklist требует
выполнить перед тегом, и закрыть риск можно двумя строками.

**Рекомендация:** использовать абсолютный SystemDirectory PowerShell path в
`test/run-elevated.ps1`; в `package.json` также заменить первый hop на quoted
`%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe` с корректным JSON
escaping. Добавить source-shape assertion рядом с уже существующим
`Describe 'test/run-elevated.ps1'`. При желании тем же policy очистить
maintainer-only bare process launches в test/release scripts, но их не следует
смешивать с shipped-runtime severity.

### P3-2 — release gate по-прежнему не валидирует дату version heading

**Где:** `scripts/release-check.js:315-351`, `CHANGELOG.md:7`.

Дата `0.2.0` в snapshot исправлена на `2026-09-04`, то есть текущего mismatch
больше нет. Но regex gate — `^## \[(\d+\.\d+\.\d+)\]` — захватывает только
версию. Строки без даты, со старой датой или с датой будущего дня проходят
проверку, если version и compare-base tag правильны. Этот пробел уже несколько
раундов требует ручного deferred update и снова проявился в round 21, поэтому
это не гипотетическая стилистика, а повторяющийся process gap.

**Рекомендация:** минимум требовать точный формат
`## [X.Y.Z] - YYYY-MM-DD` и валидную календарную дату. Если equality с датой
тега трудно сделать переносимо из-за timezone/tag-vs-commit semantics, явно
зафиксировать её как обязательный ручной пункт release checklist и печатать
распознанную дату в `release-check`; более сильный вариант — передавать
ожидаемую release date в gate через отдельный аргумент/environment variable.

## Проверка исправлений round 21

### P1-1, UAC executable substitution — закрыт

Статически подтверждено:

- `Get-AdminLaunchRoute` сохраняет абсолютный `Get-Command` result в
  `$script:AdminLaunchPath`; direct `Start-Process` использует именно его, а не
  `$Command[0]`;
- builtin/`.bat`/`.cmd` fallback использует
  `[Environment]::SystemDirectory + '\cmd.exe'`;
- `uiup` строит Windows PowerShell path от `[Environment]::SystemDirectory`;
- argument quoting, `%` fail-closed check и различие direct/fallback routes не
  изменены;
- UAC-safe tests AST-извлекают resolver, проверяют rooted existing direct path
  и source shape обоих elevation paths без показа UAC.

В новом production diff в `bin/` больше нет bare
`Start-Process powershell -Verb RunAs` и
`Start-Process -FilePath 'cmd.exe' -Verb RunAs`.

### P2-1 round 21, release date — закрыт в snapshot

`CHANGELOG.md` содержит `## [0.2.0] - 2026-09-04`, согласованную с датой
готовящегося релиза. Добавлены release notes про search-order hardening и
32-bit `capt`. Остался только P3-2 про автоматизацию будущих проверок.

### P3-1 round 21, installer cwd-first lookup — закрыт

`install/paths.js` экспортирует чистый `powershellPath(env)`, требует
rooted Windows path через `path.win32.isAbsolute` и строит
`System32\WindowsPowerShell\v1.0\powershell.exe`; `runPowershell` передаёт этот
path в `execFileSync`. `extraEnv` по-прежнему объединяется с `process.env` и
передаётся PowerShell, поэтому registry tests/overrides не потеряны.

Node regression test проверяет абсолютную сборку path, отказ для относительного
`SystemRoot` и отсутствие literal bare `execFileSync('powershell', ...)` в
installer source. Статических противоречий с существующими вызовами
`readRegistryString`/`writeRegistryString`/broadcast нет.

### P3-2 round 21, regression coverage — закрыт

- Pester contract перечисляет ровно 12 PowerShell `.bat` wrappers, требует в
  каждом единственный полный `%SystemRoot%...powershell.exe` launch и отдельно
  подтверждает, что `cx`/`cy` остаются direct external-CLI wrappers;
- `capt` теперь использует чистую production-функцию
  `Get-CaptThreadCountValidation`; число сначала только парсится, затем эта
  функция обрабатывает machine range и pointer-width range. Тест инъецирует
  32-bit width и проверяет rejection объекта целиком: `IsValid`, exit code,
  actionable message, а также boundary acceptance 32;
- validation helper не является test-only hook: именно его результат
  production script преобразует в `Write-Error` и `exit`.

Статически все новые assertions соответствуют текущим literals/functions;
гарантированно падающих счётчиков или regex, подобных находкам round 18, не
обнаружено. Динамически это не подтверждалось из-за read-only/no-tests режима.

## Остальные release-контракты

Повторный полный проход не выявил новых дефектов в неизменённой части:

- все пять Job Object launchers сохраняют fail-closed setup и единый ownership
  cleanup; normal success снимает только `KILL_ON_JOB_CLOSE`, сохраняя
  собственно CPU/affinity/memory/process-count limits для переживших wrapper
  descendants;
- `caps` по-прежнему ждёт `{process, absolute timer}`, разрешает simultaneous
  signal через `GetProcessTimes`, учитывает sleep/suspend и документирует
  system-clock adjustment semantics;
- структуры и pointer-sized поля согласованы с x86/x64, новый `capt` guard
  отбрасывает 33–63 до `UIntPtr` cast в 32-bit process;
- launcher contract остаётся 14 × 3, legacy `cap`/`pint` не возвращены;
- packaged README/SKILL описывают текущие названия, limits, chaining,
  argument handling и известное наследование `MSYS2_ARG_CONV_EXCL`;
- publish workflow проверяет tag/version, запускает Node и Pester до tarball
  gate, использует pinned actions/npm и публикует через OIDC provenance;
- allowlist и package `files` не расходятся.

## Что обязательно сделать перед `v0.2.0`

1. Закрыть или осознанно принять P3-1/P3-2.
2. Закоммитить семь implementation-файлов отдельно от настоящего отчёта.
3. Убедиться, что рабочее дерево чистое и `HEAD` действительно содержит fixes.
4. Запустить полный `npm test`.
5. Запустить normal Pester suite.
6. Запустить `npm run test:elevated` из trusted cwd после P3-1 fix/acceptance.
7. Запустить `npm run release-check` и проверить фактический tarball.
8. Проверить `package.json = 0.2.0`, CHANGELOG date/link и только затем поставить
   tag `v0.2.0` на тот же commit.

**Итог round 22: production snapshot статически READY; repository/release
state — NO-GO, пока fixes не закоммичены и обязательные динамические gates не
пройдены.**
