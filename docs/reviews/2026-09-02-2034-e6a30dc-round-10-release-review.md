# Release review, раунд 10: win-nice 0.1.1 @ e6a30dc

Дата: 2026-09-02 20:34 (Europe/Berlin)

Объём: весь репозиторий на `master` (`e6a30dc`), release delta от `v0.1.0`
(`cd138b4`), а также изменения после предыдущего review `708cb53`. Проверены все
11 Windows launcher-ов, installer/PATH, новый `capm`, npm artifact, документация и
skill, Node/Pester/MSYS suites, CI и publish workflow.

Целевая версия проекта — **0.1.1**.

## Итог

**P0/P1 не найдено. Happy path, установка и release artifact работают, однако
кандидат пока нельзя считать полностью release-certified.** Перед тегом стоит
закрыть два P2:

1. новый elevated test entry point может принять внутренний флаг из обычной
   non-admin сессии и вернуть успешный результат без выполнения elevated cases;
2. аварийный путь после `WaitForSingleObject == WAIT_FAILED` игнорирует результат
   асинхронного `TerminateProcess`, поэтому заявленная тестами fail-closed
   гарантия всё ещё не доказана.

После исправлений необходимо совместно сохранить результаты обычного и elevated
Pester runs. Один elevated run принципиально не выполняет буквально «все cases»:
три non-elevated-only сценария в нём пропускаются.

Последние изменения в целом качественные: предыдущие P2 с processor-count-aware
tests и попыткой завершить orphan child закрыты по основному замыслу, все native
handles теперь принадлежат одному `try/finally`, добавлен fault-injection harness,
CI actions закреплены immutable SHA. Автоматические проверки на доступной машине
зелёные.

## Findings

### P2 — elevated helper может ложно подтвердить elevated coverage

Файлы: `test/run-elevated.ps1:18-53`, `README.md:459-465`,
`test/win-nice.Tests.ps1:578,1104-1109,1129-1139,1222-1231`.

`run-elevated.ps1` проверяет `$SelfElevated` на строке 25 и сразу запускает suite.
Фактический token проверяется только после этой ветки, на строке 46. Поэтому
прямой вызов из обычной консоли, например с `-SelfElevated -LogPath <file>`,
запускает обычный non-admin suite, пропускает три already-elevated cases и всё
равно возвращает `0`, если остальные тесты прошли. Сам штатный
`npm run test:elevated` флаг не передаёт и идёт через UAC правильно, но публично
доступная внутренняя ветка допускает ложноположительное release evidence.

Есть связанная ошибка документации. README и комментарий helper-а обещают запуск
«all cases». На самом деле suite содержит три сценария с
`-Skip:(-not $script:isAdminRunner)` и три с `-Skip:$script:isAdminRunner`.
Обычная сессия выполняет одну половину, elevated — другую; в каждой ожидаемы три
skip. Полное покрытие получается только объединением двух runs.

Рекомендация:

- вычислять `$isAdmin` до ветки `$SelfElevated`;
- при `$SelfElevated -and -not $isAdmin` завершаться с кодом 1 и ясной ошибкой;
- в child branch требовать и валидировать `LogPath`;
- исправить README, CONTRIBUTING и сообщения helper-а: elevated run запускает
  весь файл suite и добавляет три elevated-only cases, но сам пропускает три
  non-elevated-only cases;
- добавить тесты control flow, а не только parse/`Get-Command` проверки
  (`test/win-nice.Tests.ps1:1883-1896`).

### P2 — `TerminateProcess` остаётся непроверенным best effort, а тест проверяет его как гарантию

Файлы: `bin/idle.ps1:183-190` и аналогичная ветка во всех 11 embedded C#
launcher-ах; `test/win-nice.Tests.ps1:1445-1474,1588-1633,1722-1738`.

При `WAIT_FAILED` launcher сохраняет исходный Win32 error, вызывает
`TerminateProcess(hProcess, 1)`, игнорирует его boolean result, бросает исходную
ошибку и сразу закрывает handles в `finally`. В комментариях это корректно
названо best effort, но тесты называют поведение fail-closed и сразу после
возврата `TerminateProcess` требуют, чтобы PID уже отсутствовал.

По Win32 contract внешний `TerminateProcess` лишь инициирует завершение и
возвращается асинхронно; его return value необходимо проверить, а для уверенности
в завершении — дождаться signaled process handle. См. официальные документы
[TerminateProcess](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-terminateprocess)
и
[WaitForSingleObject](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-waitforsingleobject).
Если terminate сам вернёт `false` — вполне возможный исход при уже проблемном
handle — текущая диагностика это полностью скрывает, а child может продолжить
работу. Если он вернёт `true`, немедленный `Get-Process ... | Should Be $null`
остаётся race и может флакать под нагрузкой.

Рекомендация:

- проверять результат `TerminateProcess` и сохранять termination error вместе с
  исходным wait/resume error;
- после успешного terminate делать ограниченное подтверждение завершения. Если
  повторный wait на этом handle тоже невозможен, явно сохранить best-effort
  семантику и использовать безопасную bounded-проверку состояния;
- в fault adapter уметь отдельно возвращать `false` из `TerminateProcess` и
  покрыть этот исход;
- тест успешного terminate проверять polling-ом с deadline, а не мгновенным
  отсутствием PID.

Это редкий error path, поэтому finding не P1, но выпускать hardening как строгую
защиту от orphan child без проверки результата преждевременно.

### P3 — fault-injection probes не подтверждают эквивалентность остальных девяти launcher-ов

Файл: `test/win-nice.Tests.ps1:1569-1576,1788-1814`.

Harness реально компилирует только `idle.ps1` и `cap.ps1`, исходя из утверждения,
что остальные восемь priority и два Job Object launcher-а имеют идентичные тела.
Но «source-shape» test для остальных файлов проверяет только общее количество
строк `CloseHandle(` и наличие трёх P/Invoke declarations. Он не сравнивает тела
`Run`, не проверяет положение `CloseHandle` внутри ownership `finally` и не
исполняет уникальные пути `admin`, `cy`, `cx`, `pint` и `capm`.

Текущий production code при ручной проверке имеет корректную single-owner
структуру; это gap в защите от будущего drift, а не найденная утечка handle.

Рекомендация: либо компилировать fault probe для каждого launcher-а в отдельном
namespace, либо вынести общий native runner в один источник. Минимальный
компромисс — structural/AST contract, который действительно доказывает, что все
закрытия принадлежат нужному `finally`, и сравнение нормализованных общих тел.

### P3 — опубликованный manifest содержит test scripts, но artifact не содержит `test/`

Файлы: `package.json` (`files`, `scripts`), `README.md:437-465`.

В `package.json` опубликованы `test` и новый `test:elevated`, но whitelist `files`
не включает каталог `test`. Проверенный tarball поэтому содержит оба script
entry, но не содержит ни Node tests, ни `test/run-elevated.ps1`; после установки
из registry эти команды завершаются file-not-found. Для maintainer-команд в
source checkout это обычная практика, но README с этими инструкциями входит в
тот же artifact и не обозначает ограничение.

Рекомендация: явно пометить раздел Testing как «from a source checkout». Включать
весь `test/` в package стоит только если поддерживается запуск тестов
потребителем; для runtime release он не нужен.

## Проверка исправлений предыдущего review

- Во всех 11 `.ps1` lifetime `hJob`/`hProcess`/`hThread` теперь охвачен единым
  outer `try/finally`; handles присваиваются ownership-переменным только после
  успешного acquisition.
- Для `WAIT_FAILED` добавлена попытка завершить child; оставшаяся проблема
  результата/подтверждения описана выше.
- `pint` tests теперь учитывают реальный `ProcessorCount`; Node 18/20/22/24
  matrix прошёл.
- Fault-injection tests действительно исполняют ошибки wait, resume, exit-code,
  assignment и affinity для двух основных template shapes и проверяют отсутствие
  double-close.
- CI и publish используют immutable SHA. Проверено через официальный GitHub API:
  [`actions/checkout@11d5960...`](https://github.com/actions/checkout/commit/11d5960a326750d5838078e36cf38b85af677262)
  и
  [`actions/setup-node@49933ea...`](https://github.com/actions/setup-node/commit/49933ea5288caeca8642d1e84afbd3f7d6820020)
  соответствуют `v4.4.0`.
- CHANGELOG теперь описывает итоговое пользовательское поведение, не внутренние
  pre-release регрессии.
- `.bat`/`.ps1` имеют CRLF, extensionless Git Bash shims — LF.

## Выполненные проверки

- `npm test`, Node 24.12.0 / npm 11.13.0:
  **47 passed, 0 failed, 7 skipped** (54 total);
- тот же полный Node suite на Node 18.20.8, 20.20.2 и 22.23.2:
  в каждом run **47 passed, 0 failed, 7 skipped**;
- Pester 3 integration suite, обычная non-admin сессия:
  **147 passed, 0 failed, 3 skipped** (150 total, около 150 секунд);
- реальный Git Bash/MSYS suite:
  **7 passed, 0 failed, 0 skipped**;
- parse всех `.ps1`: **0 ошибок**;
- `node --check` для всех `.js`: **0 ошибок**;
- parse всех JSON: **0 ошибок**;
- `git diff --check v0.1.0..HEAD`: чисто;
- `npm pack --dry-run`: `win-nice@0.1.1`, 48 files, 39,291 bytes,
  210,330 bytes unpacked;
- установка фактического tarball в изолированный `WIN_NICE_HOME`: успешно,
  manifest `0.1.1`, 36 launcher files, `capm` присутствует;
- smoke из установленного tarball: `capm 100m cmd.exe /c exit 7` вернул `7`,
  `win-nice status` вернул `0`;
- registry latest на момент review: `win-nice@0.1.0`; `0.1.1` ещё свободна для
  публикации;
- текущая shell не elevated, поэтому интерактивный UAC run намеренно не
  запускался и остаётся ручным release gate.

## Release checklist 0.1.1

До тега:

- [ ] Исправить проверку настоящего admin token в `run-elevated.ps1` и тесты её
      control flow.
- [ ] Исправить обещание «all cases» и сохранять два результата: normal и
      elevated Pester.
- [ ] Проверить результат/завершение после `TerminateProcess` и добавить negative
      fault case.
- [ ] На финальном commit выполнить обычный Pester run.
- [ ] Из trusted source checkout выполнить `npm run test:elevated`, принять один
      UAC prompt и подтвердить ожидаемый набор passed/skipped без failures.
- [ ] Повторить Node 18/20/22/24, Git Bash и tarball install/smoke на финальном
      commit.
- [ ] Проверить exact Trusted Publisher configuration в npm account.
- [ ] Убедиться, что worktree содержит только намеренные release files.
- [ ] Создать `v0.1.1` на проверенном commit и дождаться успешного publish
      workflow.

После закрытия двух P2 и совместного normal/elevated прогона новых оснований
задерживать `0.1.1` по результатам раунда 10 нет.
