# Полное статическое release-review, раунд 18: win-nice 0.2.0 @ c5653b1 + working tree

Дата: 2026-09-03 23:21 (Europe/Berlin)

Базовый commit: `c5653b1` (`master`, отчёт раунда 17). Новые разработки на
момент review не имели собственного commit: поверх базы были изменены
`bin/capc.ps1`, `bin/capt.ps1`, `bin/capm.ps1`, `bin/caps.ps1`,
`bin/capn.ps1` и `test/win-nice.Tests.ps1`. Working-tree delta: 6 файлов,
565 добавлений и 160 удалений. Именно этот snapshot является объектом ревью.

В начале проверки локальный `master` опережал tracking ref `origin/master` на
30 commits. В репозитории было 100 tracked files и один release tag —
аннотированный `v0.1.0`, указывающий на commit `cd138b4`. Текущая release
version в `package.json` и CHANGELOG — `0.2.0`.

Режим проверки: **только чтение и статический анализ по прямому указанию
пользователя**. Не запускались Node/Pester suites, launchers, `npm pack`,
`release-check`, installer, shell shims, parsers, linters или любые другие
исполняемые проверки. После анализа добавляется только этот отчёт; исходные
working-tree изменения не редактируются и не включаются в review commit.

Объём: весь текущий репозиторий, release delta `v0.1.0..HEAD` и все шесть
незакоммиченных файлов. Особое внимание уделено переходу `caps` с poll loop на
absolute waitable timer, порядку process/timer signals, handle ownership,
fault-injection harness, сохранению Job Object limits после normal exit,
installer/artifact contract, CI/publish workflow и packaged documentation.

## Итог

**Найдено 0 P0, 1 P1, 2 P2 и 2 P3. Текущий snapshot нельзя выпускать как
`0.2.0`: Pester suite статически гарантированно имеет минимум три failing
assertions, а `caps` всё ещё может принять post-deadline exit за успех.**

Переход на absolute waitable timer устраняет остаток относительного poll-slice
после sleep и является правильным направлением. Однако process handle стоит в
массиве ожидания первым. Если к моменту возобновления wrapper thread уже
signaled и process, и timer, Windows возвращает самый низкий signaled index —
process. Код затем считает команду успевшей, снимает kill-on-close guard и
возвращает её exit code, не устанавливая, случился exit до или после deadline.

Два P3 раунда 17 обработаны содержательно: release warning теперь включает
немедленно захваченный Win32 error, fault harness умеет отказать на конкретном
SetInfo call, а сохранение affinity/memory/process-count limits получило более
сильные source и behavioral/kernel-query regressions. Но при расширении suite
появились независимые ошибки счётчиков и была случайно удалена вся проверяющая
часть старого GetExitCode failure test.

## Findings

### P1-1: новые handle/source-count assertions гарантированно делают Pester suite красным

Файл: `test/win-nice.Tests.ps1:1689-1702,1954-1964,1986-1997,2189-2216`;
связанный код: `bin/caps.ps1:183-184,243-258,549-559`,
`bin/capc.ps1:147,221`.

Статически видны три несовместимых ожидания:

1. Строка `caps` в `$launcherExecCases` оставляет `ExpectedHandles = 3`, хотя
   successful `CapsLauncher.Run()` теперь владеет и закрывает четыре handle:
   `hThread`, `hProcess`, `hJob`, `hTimer`. Success-path smoke использует именно
   `ExpectedHandles`, поэтому для `caps` сравнит фактические 4 с ожидаемыми 3.
2. Source-shape test устанавливает для `caps` expected count 5: одна декларация
   `CloseHandle` плюс четыре close в ownership `finally`. Но новый публичный
   `ProbePastDueTimerWait()` имеет ещё один `CloseHandle(hTimer)` в собственном
   `finally`. В текущем `bin/caps.ps1` строка `CloseHandle(` встречается 6 раз,
   поэтому assertion `Count Should Be 5` не может пройти.
3. Release-struct test для `capc` требует, чтобы
   `JobObjectCpuRateControlInformation` встретился во всём embedded C# ровно
   один раз. На самом деле identifier встречается дважды: в объявлении
   константы и в setup-вызове `SetInformationJobObject`. Assertion с expected 1
   гарантированно падает даже при корректном release block.

Это не предположение о runtime и не результат запуска тестов: сравниваемые
литеральные значения и regex counts уже противоречат текущему source. Publish
workflow запускает Pester перед `npm publish`, поэтому релиз остановится на
gate даже без остальных findings.

Исправление:

- разделить `HandlesBeforeWait`, `HandlesAtWait` и `HandlesOnSuccess`; для
  `caps` значения должны быть 3, 4 и 4 соответственно;
- либо считать оба ownership scope в `caps` и ожидать 6 occurrences, либо
  ограничить source-shape regex только телом `Run()`;
- для `capc` проверять отсутствие CPU-rate identifier только в substring после
  `var releaseInfo`, либо ожидать два штатных occurrences во всём source.

### P2-1: process-first tie в `WaitForMultipleObjects` сохраняет post-deadline success race

Файл: `bin/caps.ps1:407-469,482-547`;
тесты: `test/win-nice.Tests.ps1:2625-2669`.

Absolute `SetWaitableTimer` решает прошлую проблему оставшегося относительного
slice: если срок прошёл во время sleep, timer уже signaled после wake. Но
production wait получает массив `{ hProcess, hTimer }`. Документация
`WaitForMultipleObjects` явно говорит: при нескольких signaled objects и
`bWaitAll = false` возвращается объект с самым низким index:
<https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-waitformultipleobjects>.

Реальный сценарий:

1. Машина находится в sleep дольше deadline; absolute timer становится signaled.
2. После wake scheduler запускает child раньше wrapper; child успевает завершиться.
3. Когда wrapper thread продолжает wait, оба handles уже signaled.
4. Из-за index 0 возвращается `WAIT_OBJECT_0` для process, а не index 1 timer.
5. `caps` получает exit code и выполняет normal release path вместо exit 124.

Та же race возможна без suspend под scheduler contention: timer сигнализируется
первым, process завершается до фактического возврата wrapper thread, после чего
process-first ordering скрывает порядок событий. Комментарий на строках
463-468 называет это «photo-finish exit before deadline», хотя API сообщает
только текущее состояние двух objects и не доказывает, какой signal был первым.

Новые tests ждут один past-due timer без process handle. Они подтверждают
absolute timer semantics, но не проверяют production ambiguity «оба handles
уже signaled». Поэтому suite останется зелёным относительно этой race после
исправления P1-1.

Корректное исправление должно определить контракт и проверить реальный порядок.
Для точного «child exited before deadline» результата можно после process
signal получить process exit timestamp через `GetProcessTimes` и сравнить его
с absolute deadline; Microsoft документирует отдельный exit time для
terminated process:
<https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocesstimes>.
Более простая fail-closed политика — поставить timer первым, но она способна
ошибочно timeout'ить process, завершившийся до deadline, если wrapper не был
запланирован до момента сигнала timer. Нужен deterministic regression, где оба
objects pre-signaled, а ожидаемый outcome закреплён выбранной политикой.

### P2-2: GetExitCodeProcess fault test стал vacuous для всех 13 launchers

Файл: `test/win-nice.Tests.ps1:1874-1887`; base version:
`c5653b1:test/win-nice.Tests.ps1:1835-1850`.

Test по-прежнему называется `reports the exit-code error and still closes every
handle`, устанавливает `FailGetExitCode = true`, ловит exception в `$message`,
но затем сразу выполняет cleanup/reset. Из working-tree diff удалены все
assertions:

- `$message Should Be 'GetExitCodeProcess failed: 6'`;
- `TerminateCalls Should Be 0`;
- количество и уникальность закрытых handles;
- `CloseHandleFailures Should Be 0`.

Таким образом, 13 параметризованных cases проходят независимо от текста
ошибки, kill behavior и handle cleanup; `$message`, `$HandlesAtWait` и `$WaitFn`
вычисляются, но не используются. Для `caps` это особенно важно: новый timer
handle существует на GetExitCode failure path и должен увеличить ожидаемый
count до 4.

Исправление: восстановить удалённые assertions, используя `$HandlesAtWait` для
count/uniqueness и сохранив ожидание `GetExitCodeProcess failed: 6`. Добавить
статический sanity guard против `It` blocks без `Should`/явного assertion,
поскольку текущая ошибка синтаксически валидна и не определяется обычным
запуском как пропущенный test.

### P3-1: packaged docs и несколько test comments всё ещё описывают удалённый poll-loop

Файлы: `README.md:226-231,246-250`, `CHANGELOG.md:24-30`,
`skills/win-nice/SKILL.md:74-84`,
`test/win-nice.Tests.ps1:1352-1363,1607-1612,1667-1674,1721,2453-2462`.

Рабочие изменения полностью заменяют `RemainingWaitMs`/1000-ms poll slices на
`CreateWaitableTimer` + `SetWaitableTimer` + `WaitForMultipleObjects`, но ни один
из трёх публикуемых документов не изменён. README, installed skill и CHANGELOG
продолжают утверждать, что absolute UTC timestamp «re-checks ... on a short
poll loop» и связывают uint32/INFINITE boundary с `WaitForSingleObject`.

В самом Pester файле сохранились старые описания `caps` как launcher'а, чей
timeout «bounds WaitForSingleObject», старое имя в test title и прежнее имя API
в argument-boundary comment. При этом рядом новый код уже утверждает, что
`caps` больше вообще не объявляет `WaitForSingleObject`.

Это не меняет abstract CLI usage, но публикуемые implementation guarantees и
release notes становятся фактически неверными. `test/docs-sync.test.js`
проверяет только `%` argument-safety section и такой drift не ловит.

Исправление: синхронно обновить README/SKILL/CHANGELOG и stale test comments на
absolute waitable timer + two-object wait. Добавить docs-sync guard для ключевых
`caps` deadline claims, чтобы следующий implementation switch не разошёлся с
публикуемым skill.

### P3-2: failure paths создания и взведения timer не покрыты fault injection

Файлы: `bin/caps.ps1:439-469`,
`test/win-nice.Tests.ps1:1392-1604,1689-1942`.

После `ResumeThread` у `caps` появились два новых native failure path:

- `CreateWaitableTimer` возвращает zero: child уже работает, `hTimer` ещё не
  принадлежит wrapper'у; closing `hJob` должен kill'нуть job через guard;
- `SetWaitableTimer` возвращает false: wrapper уже владеет четырьмя handles;
  timer, process, thread и job должны закрыться ровно по одному разу, а job
  снова должен fail closed.

Fault harness инструментирует wait, resume, assign, SetInfo, GetExitCode и
TerminateProcess, но не умеет отказать на CreateWaitableTimer/SetWaitableTimer.
Новый `ProbePastDueTimerWait` проверяет успешную timer semantics, а не эти
production cleanup branches. Ошибка ownership здесь может оставить running
tree или утечь handle, не покрасив suite.

Исправление: добавить отдельные stubs/flags для обоих APIs, deterministic Win32
codes и assertions на child termination, diagnostic message, exact handle
counts и отсутствие double-close.

## Статическая проверка findings раунда 17

### P2 waitable deadline — архитектурно улучшен, но не закрыт

- Poll loop и `RemainingWaitMs` удалены из production code.
- Создаётся anonymous manual-reset one-shot timer с positive UTC FILETIME.
- `fResume = false` не будит sleeping machine; absolute timer сохраняет deadline
  semantics и после wake уже signaled, если due time прошёл.
- Process и timer ждутся одним `WaitForMultipleObjects(INFINITE)`.
- Handle timer включён в central ownership `finally`.
- Однако process-first simultaneous-signal behavior оставляет P2-1.

Microsoft документирует positive values как UTC-based absolute due time и
manual-reset timer как остающийся signaled после срабатывания:
<https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-setwaitabletimer>.
P/Invoke types (`IntPtr`, `ref long`, Win32 BOOL-compatible `bool`) и FILETIME
conversion выглядят согласованными с API.

### P3 сохранение limits на daemon — закрыт в разумном объёме

- Для всех пяти release structs добавлены targeted source assertions.
- `capt` проверяет live `ProcessorAffinity` surviving daemon после exit wrapper.
- `capn` проверяет разрешённый и запрещённый spawn после exit wrapper.
- `capm` из daemon вызывает `QueryInformationJobObject(NULL, ...)` и проверяет
  kernel flags/memory limit. Microsoft подтверждает, что NULL выбирает job
  вызывающего process, а при nested jobs — immediate job:
  <https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-queryinformationjobobject>.
- `capc` проверяется source guard'ом: CPU rate живёт в отдельном info class и
  release extended struct его не меняет. Ошибка expected count входит в P1-1.

### P3 release-call failure и Win32 diagnostic — закрыт по сути

- Во всех пяти launchers Win32 error захватывается сразу после failed SetInfo,
  до `FreeHGlobal`, и включается в warning.
- `SetInfoFailOnCall` позволяет нацелиться на release call: третий для `capc`,
  второй для остальных Job launchers.
- Test проверяет сохранённый exit code, stderr warning/error 87, call count,
  отсутствие explicit kill и exact unique handle closes.
- Сам fail-closed descendant outcome выводится из сохранённого flag и закрытия
  `hJob`, но test запускает только завершившийся root без surviving descendant;
  это допустимый остаточный test-strength риск, не отдельный finding этого раунда.

## Полное статическое состояние проекта

### Runtime и launcher contract

- В `bin/` остаются 14 tools × 3 entry points = 42 tracked launcher files.
- Все 42 имеют SPDX и `win-nice: managed-file` marker.
- Шесть изменённых `.ps1` сохраняют CRLF без mixed bare-LF; working-tree delta
  whitespace-clean по `git diff --check`.
- `caps`/`capn` присутствуют в `.ps1`, `.bat`, extensionless shim, installer,
  artifact allowlist, test tables и документации.
- Job Object limit release structs текущего snapshot статически сохраняют
  affinity, memory и process-count fields; `capc` не сбрасывает отдельный CPU
  rate info class.
- Помимо P2-1 новых runtime defects в process creation, quoting, assignment,
  timeout kill и normal cleanup при статическом чтении не найдено.

### Installer, package и artifact

- `package.json`: `0.2.0`, Windows-only, Node `>=18`, без dependencies,
  explicit `files` whitelist.
- Installer/uninstaller, manifest ownership и source-checkout guards в этом
  working-tree delta не изменялись; findings прошлых раундов не вернулись.
- `release-check.js` по-прежнему ожидает 14 tools, 42 launchers и 54 exact
  tarball paths; installed-artifact smoke table содержит все пять Job launchers.
- Latest CHANGELOG heading совпадает с package version, compare base
  `v0.1.0` существует, `v0.2.0` ещё не создан.

### CI и publish

- CI matrix остаётся Node 18/20/22/24 с Node suite, Pester и Node-24 artifact gate.
- Publish workflow повторяет suites/release-check, проверяет tag/version,
  использует pinned npm 11.5.1, OIDC и provenance.
- Из-за P1-1 Pester stage должен остановить публикацию до `npm publish`.
- Новые разработки не закоммичены, поэтому пока нет immutable commit, который
  можно push/tag и сопоставить с этим reviewed snapshot.

## Что не проверено в этом раунде

По указанию пользователя **не выполнялось ничего из следующего**:

- `npm test` на любой версии Node;
- обычный или elevated Pester suite;
- Git Bash shim suite;
- `npm pack`, dry-run pack и `npm run release-check`;
- реальные waitable-timer, suspend/resume, timeout, daemon и limit scenarios;
- installer/uninstaller/upgrade, registry/PATH mutations;
- syntax compilation/parsing, workflow lint, dependency audit и runtime smokes.

Для незакоммиченных разработок нет commit message с авторскими результатами
проверок. Этот review не пытается выводить прохождение suite из наличия test
code. P1-1, напротив, установлен чисто статически из literal expected values и
текущих source counts; для него запуск не требуется.

## Release checklist

- [ ] Исправить три гарантированных Pester failures из P1-1.
- [ ] Закрыть P2-1: определить outcome по фактическому порядку deadline/exit,
      а не по lowest-index правилу уже signaled handles.
- [ ] Восстановить все assertions GetExitCode failure test из P2-2.
- [ ] Обновить packaged README/SKILL/CHANGELOG и stale test comments (P3-1).
- [ ] Добавить CreateWaitableTimer/SetWaitableTimer failure injection (P3-2).
- [ ] Оформить исправленные разработки отдельным кодовым commit; review report
      не должен быть единственным commit поверх грязного runtime snapshot.
- [ ] После исправлений выполнить Node matrix, normal/elevated Pester, Git Bash
      и полный artifact `release-check` из trusted checkout.
- [ ] Выполнить реальный upgrade опубликованного `0.1.0` на release tarball.
- [ ] Push и дождаться зелёной CI matrix.
- [ ] Проверить дату CHANGELOG в фактический день релиза.
- [ ] Создать annotated `v0.2.0` только на проверенном clean commit и проверить
      publish workflow, npm version, exact tarball и provenance.

До закрытия P1-1 и P2 findings выпуск `0.2.0` не рекомендуется.
