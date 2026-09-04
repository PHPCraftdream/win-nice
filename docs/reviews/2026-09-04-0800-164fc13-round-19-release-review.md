# Полное статическое release-review — раунд 19

Дата: 2026-09-04 08:00 (Europe/Berlin)

Проверенный commit: `164fc132a307f619d82b670cdd51628fc28e5da9` (`master`)

База предыдущего review: `698eab6` (round 18)

Новая реализация после него: `05c5e7a` + checkpoint `164fc13`

## Вердикт

**P0/P1 не найдено. Все runtime- и test-дефекты раунда 18 статически закрыты,
но перед тегом `v0.2.0` остаются два P2: нужно принять и зафиксировать контракт
`caps` при переводе системных часов и выставить фактическую дату релиза в
CHANGELOG.** После этого код выглядит готовым к финальной динамической проверке
и тегированию.

Текущая реализация tie-break в `caps` по существу корректна для выбранной
абсолютной UTC-шкалы: `WaitForMultipleObjects` может вернуть process-handle при
одновременной сигнализации, после чего `GetProcessTimes` сравнивает настоящее
время завершения с due time таймера. Ранее найденный ложный success после
deadline больше не просматривается.

Это намеренно **только статическое review**. По запросу пользователя тесты,
Pester, `npm pack`, `npm run release-check`, сборка и живые launcher-прогоны не
запускались. Указанные в commit `05c5e7a` результаты (`npm test` 72/72, Pester
279/0/3, release-check green) рассмотрены как предоставленное автором
свидетельство, но в этом раунде независимо не подтверждались.

## Scope и состояние репозитория

Просмотрены все 102 tracked-файла и основные контракты проекта:

- 42 launcher-файла в `bin/` (14 инструментов × `.ps1`/`.bat`/Git Bash shim),
  включая Win32 ABI, command-line forwarding, Job Object lifecycle и cleanup;
- installer/uninstaller, manifest, PATH registry handling и optional skill;
- `package.json`, changelog, README, security/contributing docs и skill;
- Node/Pester тесты, fault-injection harness и release artifact allowlist;
- CI/publish workflows и локальное состояние версии/тегов;
- полный diff `v0.1.0..164fc13` и отдельно новый diff
  `698eab6..164fc13`.

Статические факты на момент review:

- рабочее дерево было чистым до добавления этого отчёта;
- `master` по локальной tracking-ссылке опережает `origin/master` на 33 commit;
- локально существует только тег `v0.1.0`; `v0.2.0` ещё не создан;
- `package.json` содержит версию `0.2.0`;
- после round 18 изменено 10 файлов: 850 additions / 188 deletions;
- со времени `v0.1.0`: 75 файлов, 10 970 additions / 820 deletions;
- фактический tracked `bin/` совпадает с контрактом 14 × 3 = 42;
- `git diff --check 698eab6..HEAD` чист; в старом историческом diff от
  `v0.1.0` остались две Markdown trailing-space строки в одном review-файле,
  на runtime/tarball они не влияют.

Remote не обновлялся: `git fetch` не выполнялся из-за read-only режима, поэтому
сравнение с `origin/master` относится к уже имеющейся локальной tracking-ссылке.

## Findings

### P2-1 — `caps` измеряет абсолютное системное время, поэтому перевод часов меняет длительность hard timeout

**Где:** `bin/caps.ps1:424-472`, `bin/caps.ps1:501-519`,
`README.md:218-238`, `CHANGELOG.md:17-33`, `skills/win-nice/SKILL.md:69-90`.

Deadline строится как `DateTime.UtcNow.AddMilliseconds(timeoutMs)` и передаётся
в `SetWaitableTimer` положительным `FILETIME`, то есть как абсолютное UTC-время.
Это правильно учитывает sleep/suspend и вместе с `GetProcessTimes` правильно
разрешает process/timer tie. Однако абсолютный timer — не монотонная длительность.
Microsoft прямо указывает, что при корректировке системного времени due time
всех ожидающих absolute timers корректируется, а изменение system time меняет
продолжительность ожидания такого timer:

- [SetWaitableTimer — absolute/relative due time и корректировка system time](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-setwaitabletimer)
- [Timer Accuracy — system-time change changes an absolute timer's wait duration](https://learn.microsoft.com/en-us/windows-hardware/drivers/kernel/timer-accuracy)

Следствие: перевод часов назад может продлить `caps 300` дольше 300 реальных
секунд, перевод вперёд — оборвать процесс раньше. `GetProcessTimes` этого не
исправляет: exit time является точкой на той же системной шкале FILETIME
([документация](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocesstimes)),
поэтому tie-break остаётся внутренне согласованным с absolute deadline, но не
возвращает контракт длительности.

Это расходится с сильными пользовательскими формулировками «hard wall-clock
deadline», «within `<seconds>`» и «N seconds is the hard limit, no matter what».
Обычный NTP slew даст небольшой эффект, но ручной/служебный скачок времени может
быть произвольным; как security boundary инструмент не заявлен, однако основной
контракт новой команды становится неоднозначным.

**Рекомендация перед релизом:** явно выбрать одно из двух:

1. Если требуются именно N elapsed seconds, использовать/учитывать шкалу,
   независимую от перевода system time и включающую sleep. Microsoft рекомендует
   `GetTickCount64` для elapsed time с учётом sleep/hibernate
   ([Interrupt Time](https://learn.microsoft.com/en-us/windows/win32/sysinfo/interrupt-time));
   при этом нужно сохранить точное разрешение гонки process/deadline и wake
   behavior, а не возвращаться к прежнему blind poll slice.
2. Если сознательно выбран absolute wall-clock contract, прямо документировать,
   что системная корректировка часов может сократить или продлить фактическую
   длительность, и убрать безусловные обещания «N seconds no matter what».

Минимально безопасный для `0.2.0` вариант — второй: это документирует уже
реализованное поведение без рискованной перед релизом переделки scheduler path.

### P2-2 — release entry датирован 2026-09-03, хотя релиза и тега ещё нет

**Где:** `CHANGELOG.md:7`, локальные git tags.

На 2026-09-04 верхняя версия всё ещё записана как
`## [0.2.0] - 2026-09-03`, но локально существует только `v0.1.0` и пакет
`0.2.0` ещё не выпущен. Если тег создаётся сегодня или позже, changelog навсегда
зафиксирует неверную дату релиза. Имеющийся `release-check.js` сверяет номер
версии и compare-base tag, но дату не проверяет.

**Рекомендация:** непосредственно перед тегом заменить дату на фактическую дату
публикации (`2026-09-04`, если релиз будет сегодня). Не создавать `v0.2.0`, пока
эта правка не находится в самом tagged commit.

### P3-1 — новый failure branch `GetProcessTimes` не входит в fault injection

**Где:** `bin/caps.ps1:514-516`, `test/win-nice.Tests.ps1:1352-1657`,
`test/win-nice.Tests.ps1:1667`, `test/win-nice.Tests.ps1:1925-2087`.

Round 18 добавил ещё один fallible native call. Production корректно проверяет
его return value и бросает `GetProcessTimes failed: <code>`; armed
`KILL_ON_JOB_CLOSE` в `finally` делает этот путь fail-closed. Но harness имеет
флаги для wait/timer/GetExitCode и не имеет `FailGetProcessTimes`, transform его
P/Invoke или тест соответствующего cleanup. При этом комментарий около
`test/win-nice.Tests.ps1:1667` всё ещё утверждает, что probes покрывают every
failure branch.

**Рекомендация:** добавить точечный stub `GetProcessTimesReal` /
`FailGetProcessTimes` и проверить код ошибки, отсутствие ложного success,
закрытие четырёх handles ровно по одному разу и fail-closed судьбу оставшегося
descendant. Заодно исправить слишком сильный комментарий, если полный coverage
не является целью.

### P3-2 — test-only timer probe поставляется в production launcher

**Где:** `bin/caps.ps1:248-276`, `test/win-nice.Tests.ps1:2776-2821`.

`ProbePastDueTimerWait` объявлен public внутри shipped `CapsLauncher` только ради
двух Pester cases. Он не ломает CLI и хорошо проверяет реальный Win32 primitive,
но расширяет runtime source и создаёт второй самостоятельный handle-lifecycle,
который уже усложнил source-shape assertions в round 18.

**Рекомендация после релиза:** в fault-probe transform добавлять тестовый метод
в копию класса либо вынести timer primitive в общий минимальный source, который
тесты компилируют отдельно. В пользовательском `caps.ps1` оставить только
production path. Для `0.2.0` это не blocker.

## Проверка исправлений round 18

Статически подтверждено:

- **P1, stale handle expectations:** для `caps` теперь различаются
  `ExpectedHandles = 3` до создания timer и `HandlesAtWait = 4` после него;
  success/failure assertions используют правильную фазу.
- **P1, CloseHandle source count:** source guard начинает с `Run()` и не считает
  отдельный `ProbePastDueTimerWait`; для `caps` ожидаются четыре ownership close.
- **P1, capc CPU info count:** проверяется отсутствие
  `JobObjectCpuRateControlInformation` именно в release block, а не неверный
  глобальный literal count.
- **P2, simultaneous-signal race:** forced wait-all stub дожидается настоящих
  process + timer signals, затем намеренно возвращает index 0; production
  `GetProcessTimes` должен переклассифицировать post-deadline exit в timeout.
- **P2, vacuous GetExitCode test:** снова присутствуют assertions сообщения,
  kill count, количества/уникальности/успешности handle close.
- **P3, timer setup failures:** `CreateWaitableTimer` и `SetWaitableTimer`
  получили отдельные injected branches и cleanup assertions.
- **P3, документация:** README, CHANGELOG и skill больше не описывают удалённый
  poll loop; везде указан waitable timer + `GetProcessTimes` tie-break.
- Во всех пяти Job launchers Win32 error release-call сохраняется сразу после
  неуспешного `SetInformationJobObject`, до `FreeHGlobal`/других native calls.

## Остальные результаты полного прохода

- P/Invoke layout новой части выглядит корректно: `FILETIME` — два `uint`,
  `GetProcessTimes` имеет четыре out-структуры, positive `long` соответствует
  absolute `LARGE_INTEGER`, manual-reset timer остаётся signaled после due time.
- `WaitForMultipleObjects` получает два distinct handles, process первым;
  smallest-index semantics явно компенсируется exit-time comparison
  ([официальный контракт](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-waitformultipleobjects)).
- Ошибки создания/arming timer происходят уже после resume, но job guard остаётся
  armed, поэтому `finally` не оставляет дерево unmanaged.
- Timeout использует `TerminateJobObject`, normal completion снимает только
  kill-on-close bit и сохраняет собственные affinity/memory/process-count limits;
  CPU-rate limit `capc` находится в отдельном info class и release call его не
  сбрасывает.
- Installer manifest-owned launchers действительно обновляются/удаляются как
  файлы пакета независимо от локальных правок; обещания сохранять их нет.
  Fallback при отсутствующем/corrupt manifest по-прежнему marker-gated, чтобы
  directory scan не удалял посторонние файлы.
- Полный tarball allowlist перечисляет 54 ожидаемых path, а source tree содержит
  ровно 42 launcher path. Новых незаявленных runtime dependency нет.
- CI и publish повторяют Node/Pester/release-check gates; actions закреплены по
  SHA, tag/version mismatch проверяется до publish, npm publish использует OIDC.
- README, skill, manifest contract и changelog используют актуальные имена
  `capc`/`capt`/`capm`/`caps`/`capn`; старые `cap`/`pint` остались только в
  истории релиза и negative legacy checks.
- Новых очевидных command-injection путей сверх явно документированного `%`
  ограничения `.bat`/cmd fallback статически не найдено.

## Release checklist

1. Закрыть P2-1 продуктовым решением; для минимального риска перед `0.2.0` —
   документировать system-clock-adjustment semantics во всех трёх public docs.
2. В день публикации обновить дату `0.2.0` в CHANGELOG и закоммитить её.
3. Получить свежий remote state, убедиться, что release commit находится на
   публикуемой ветке; сейчас локальная tracking-ссылка показывает `ahead 33`.
4. Уже вне read-only review выполнить normal Node/Pester, elevated Pester и
   `npm run release-check`; не полагаться только на commit message прошлого
   прогона после новых правок документации/metadata.
5. Только после green gates создать annotated `v0.2.0` на проверенном commit,
   push branch и tag, затем проверить завершение publish workflow и npm artifact.

## Итог

Round 19 не обнаружил нового release-blocking P0/P1. Основная сложная правка
round 17/18 стала существенно надёжнее: sleep-safe signal и simultaneous-signal
tie теперь разделены корректно, cleanup остаётся fail-closed. До релиза нужно
не потерять два контрактных/metadata пункта P2; fault coverage нового syscall и
удаление shipped test hook разумно сделать следующими небольшими hardening
задачами.
