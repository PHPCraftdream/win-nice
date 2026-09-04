# Полное статическое release-review — раунд 20

Дата: 2026-09-04 09:56 (Europe/Berlin)

Проверенный commit: `94ae5738d4dc984e18b02396492ab79ad1fcbd47` (`master`)

База предыдущего review: `164fc13` (round 19, отчёт в `53133db`)

Новая реализация после него: `94ae573`

## Вердикт

**P0/P1/P2 не найдено. Все находки раунда 19, взятые в работу, статически
закрыты корректно; два пункта, которые раунд 19 сам вынес за рамки (дата
релиза в CHANGELOG и test-only hook в production `caps.ps1`), остаются в том
статусе, в котором их оставили — первый закрывается в момент тегирования,
второй после релиза.** Новых release-blocking дефектов при полном проходе по
всем 103 tracked-файлам не обнаружено. Найдено пять P3: одно устаревшее
обоснование лимита `<seconds>` в упакованных доках и usage-строке `caps`
(остаток от удалённого relative-wait дизайна, который раунд 19 при проверке
не заметил), три небольших hardening/документационных пробела вне `caps`
(утечка `MSYS2_ARG_CONV_EXCL` из Git Bash shim в окружение обёрнутой команды,
bare `powershell` в `.bat` при cwd-first поиске cmd.exe, `OverflowException`
в `capt` под 32-bit PowerShell) и перенос P3-2 раунда 19.

Это намеренно **только статическое review**. По условиям задачи тесты, Pester,
`npm pack`, `npm run release-check`, сборка и живые launcher-прогоны не
запускались. Результаты, указанные в commit message `94ae573` (`npm test`
72/72, Pester non-elevated 280/0/3, release-check green), рассмотрены как
предоставленное автором свидетельство и в этом раунде независимо не
подтверждались. То же относится к elevated-прогону из checkpoint
`docs/checkpoints/2026-09-04-0754.md` (270/0/4 из 274) — он сделан до
`05c5e7a`, то есть до +9 case, добавленных раундами 17–19.

## Scope и состояние репозитория

Прочитаны целиком (не выборочно) все 103 tracked-файла и проверены основные
контракты проекта:

- 42 launcher-файла в `bin/` (14 инструментов × `.ps1`/`.bat`/extensionless
  Git Bash shim): Win32 P/Invoke (layout структур на x64/x86, calling
  convention, проверка return value и захват Win32 error до следующего
  native call), lifecycle Job Object (создание, limit flags, assign, release
  `KILL_ON_JOB_CLOSE` на success path vs backstop на abnormal paths, cleanup),
  forwarding/quoting аргументов, включая задокументированное `%`-ограничение
  cmd.exe fallback, suspend/resume/wait/exit-code, `try`/`finally` покрытие
  handles;
- `install/*.js` (installer, uninstaller, manifest, PATH через реестр, skill
  install/update) и `scripts/release-check.js` (tarball allowlist, smoke
  tests, CHANGELOG/tag cross-check);
- `test/win-nice.Tests.ps1` (3243 строки, fault-injection harness) и восемь
  Node test-файлов, `test/run-elevated.ps1`;
- `README.md`, `skills/win-nice/SKILL.md`, `CHANGELOG.md`, `package.json`,
  `SECURITY.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, issue/PR templates;
- `.github/workflows/ci.yml` и `publish.yml`, `.gitattributes`, `.gitignore`;
- полный diff `v0.1.0..94ae573` и отдельно новый diff `53133db..94ae573`.

Статические факты на момент review:

- рабочее дерево было чистым до добавления этого отчёта;
- `master` по локальной tracking-ссылке опережает `origin/master` на 35
  commit (`git fetch` не выполнялся — read-only режим);
- локально существует только тег `v0.1.0`; `v0.2.0` не создан;
- `package.json` содержит версию `0.2.0`;
- после отчёта раунда 19 изменено 4 файла: 118 additions / 12 deletions
  (`CHANGELOG.md`, `README.md`, `skills/win-nice/SKILL.md`,
  `test/win-nice.Tests.ps1`); `bin/` и `install/` с раунда 19 не менялись;
- со времени `v0.1.0`: 76 файлов, 11 301 additions / 820 deletions;
- фактический tracked `bin/` совпадает с контрактом 14 × 3 = 42, allowlist
  `release-check.js` — 54 path (42 + 6 `install/*.js` + `SKILL.md` +
  `package.json` + 4 root docs/licenses), что подтверждает
  `test/release-check-allowlist.test.js`;
- `git diff --check 164fc13..HEAD` чист; в историческом diff от `v0.1.0`
  остаются те же две Markdown trailing-space строки в отчёте раунда 13, на
  runtime/tarball не влияют.

## Findings

### P3-1 — обоснование максимума `<seconds>` в `caps` описывает удалённый relative-wait дизайн

**Где:** `bin/caps.ps1:14-17`, `bin/caps.ps1:42-53`, `bin/caps.ps1:419-422`,
`README.md:257-262`, `CHANGELOG.md:22-26`, `skills/win-nice/SKILL.md:87-91`.

Во всех четырёх местах лимит `4294967294` ms (~49.7 дней) объясняется тем,
что «`WaitForMultipleObjects`' `dwMilliseconds` is a uint32 whose `0xFFFFFFFF`
value is reserved as the wait-forever sentinel». После фикса раундов 17/18
это утверждение больше не описывает код: `WaitForMultipleObjects` вызывается
с `dwMilliseconds = 0xFFFFFFFF` (INFINITE) безусловно
(`bin/caps.ps1:487`), а сам deadline — 64-битный absolute `FILETIME` в
`SetWaitableTimer` (`bin/caps.ps1:471-472`). Никакой uint32-границы у
deadline больше нет; `[1, 0xFFFFFFFE]` — это остаток валидации под старый
`WaitForSingleObject(hProcess, timeoutMs)`. Комментарий на
`bin/caps.ps1:419-422` («the value can never collide with the 0xFFFFFFFF
INFINITE sentinel below») тоже относится к прежней схеме — ниже INFINITE
передаётся намеренно.

Практического вреда нет: лимит сам по себе безопасен и проверен тестом
`rejects a seconds value whose millisecond conversion overflows...`
(`test/win-nice.Tests.ps1:2701-2727`), а 49.7 дней с запасом покрывают любой
реальный сценарий. Но это ровно тот класс расхождения, который раунд 18
(P3-1) поднимал как «packaged docs описывают удалённый poll-loop», а раунд 19
в разделе проверки исправлений посчитал закрытым («README, CHANGELOG и skill
больше не описывают удалённый poll loop»). Эта проверка была неполной: сам
poll-loop из доков убрали, а его обоснование uint32-лимита осталось в
CHANGELOG-записи релиза, в README, в skill, который устанавливается агентам,
и в usage-строке, которую видит пользователь при ошибке.

**Рекомендация:** либо переформулировать обоснование честно («верхняя
граница — сознательный usage-лимит; deadline хранится как absolute
FILETIME, uint32 здесь ни при чём»), либо убрать «because ...» вовсе,
оставив число. Изменение — только текст (три public doc + usage-строка +
два комментария), `test/docs-sync.test.js` эти фразы не проверяет, но после
правки CHANGELOG нужно повторить `release-check` (он парсит heading/compare
link). Не блокирует тег: можно сделать вместе с проставлением даты релиза
или сразу после.

### P3-2 — Git Bash shim экспортирует `MSYS2_ARG_CONV_EXCL='*'` в окружение обёрнутой команды

**Где:** `bin/idle:8-11` (и идентично в остальных 13 extensionless shim),
`README.md:54`, `skills/win-nice/SKILL.md:189`.

Shim делает `export MSYS2_ARG_CONV_EXCL='*'` и затем `exec powershell ...
"$@"`. Экспорт нужен, чтобы MSYS-runtime самого shim не переписал `/c`, `/d`
и Windows-пути при spawn native `powershell.exe` — это правильно и покрыто
`test/gitbash-shims.test.js`. Но переменная остаётся в environment
`powershell.exe`, наследуется обёрнутой командой и всеми её потомками. Для
native-программ (`npm.cmd`, `node.exe`, `cmd.exe`) это ничего не меняет.
Для MSYS-программ внутри обёрнутого дерева — меняет: `bash.exe`/`sh.exe` из
Git for Windows при spawn *своих* native-детей перестанут конвертировать
POSIX-пути и slash-опции.

Конкретный сценарий: из Git Bash `caps 600 bash -c 'node /c/proj/run.js'`.
Без win-nice `bash -c 'node /c/proj/run.js'` работает — MSYS переводит
`/c/proj/run.js` в `C:\proj\run.js` при spawn native `node`. Через shim
внутренний `bash` уже видит `MSYS2_ARG_CONV_EXCL='*'`, `node` получает
буквальное `/c/proj/run.js` и падает с `Cannot find module`. Аналогично для
git hooks (`#!/bin/sh`), вызывающих native-инструменты POSIX-путями, если
`git` обёрнут через shim.

Это не нарушение задокументированной гарантии (README:54 обещает только,
что *аргументы самого shim* дойдут нетронутыми), но это наблюдаемое отличие
поведения обёрнутой команды от прямого запуска, нигде не описанное.

**Рекомендация после релиза:** документировать в строке таблицы про Git
Bash (README/SKILL) как известное ограничение shim; альтернативно — сузить
исключение до реально нужного или снимать переменную на стороне `.ps1` по
sentinel-флагу, который ставит shim. Для `0.2.0` достаточно doc-оговорки.

### P3-3 — `.bat`-обёртки вызывают bare `powershell`, cmd.exe ищет его сначала в текущем каталоге

**Где:** `bin/idle.bat:12`, `bin/belownormal.bat:13`, `bin/abovenormal.bat:13`,
`bin/high.bat:12`, `bin/realtime.bat:13`, `bin/capc.bat:10`,
`bin/capt.bat:8`, `bin/capm.bat:12`, `bin/caps.bat:10`, `bin/capn.bat:8`,
`bin/admin.bat:12`, `bin/uiup.bat:4`.

Все 12 PowerShell-делегирующих `.bat` (cy/cx вызывают `claude`/`codex` — там
bare name неизбежен) запускают `powershell -NoProfile -ExecutionPolicy
Bypass -File "%~dp0X.ps1" %*`. Правило поиска команд cmd.exe для batch-файлов:
сначала текущий каталог, потом `PATH`, по `PATHEXT` (если не выставлен
`NoDefaultCurrentDirectoryInExePath`). Подложенный в cwd `powershell.exe`,
`powershell.cmd` или `powershell.bat` будет выполнен вместо системного.
Сценарий: `admin.bat npm install` (или любой `idle.bat`/`caps.bat`) из
каталога скачанного/чужого проекта, куда положили `powershell.cmd` — из cmd.exe
или через PATHEXT-резолюцию Node `child_process` (README:53 явно направляет
такие вызовы в `.bat`). Тот же тип угрозы, что у любого `.bat` с bare
name, но у `admin.bat` он усилен elevation-целью инструмента.

Показательно, что на C#-стороне тот же риск для `cmd.exe` уже закрыт
намеренно: `Environment.SystemDirectory + "\\cmd.exe"` во всех 13 launcher
(`bin/capc.ps1:316`, `bin/caps.ps1:377` и т.д.), с комментарием про `/d`
против user-writable AutoRun. Shim-ам это не грозит: POSIX `sh` ищет только
по `PATH`. `uiup.ps1`/`run-elevated.ps1` используют `Start-Process
powershell`, PowerShell cwd не просматривает.

**Рекомендация после релиза:** `"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"`
вместо bare `powershell` во всех 12 файлах (для WOW64-cmd
`%SystemRoot%\System32` корректно перенаправляется в `SysWOW64`). Не
блокирует `0.2.0` — это defense-in-depth, а не регрессия; SECURITY.md
классифицирует такие вещи как «bypass the argument-safety guarantees
beyond what's documented», сюда это не попадает.

### P3-4 — `capt` под 32-bit PowerShell: thread-count 33–63 проходит валидацию и падает `OverflowException`

**Где:** `bin/capt.ps1:7-8`, `bin/capt.ps1:219-221`, `bin/capt.ps1:387-395`;
прецедент правильной обработки — `bin/capm.ps1:488-499`.

`$maxCount = [Math]::Min([Environment]::ProcessorCount, 63)`, маска
`([uint64]1 -shl $countValue) - 1` и `Affinity = (UIntPtr)affinityMask`.
В 32-bit процессе (`SysWOW64\WindowsPowerShell\v1.0\powershell.exe`)
`UIntPtr` четырёхбайтный, и явное преобразование `ulong → UIntPtr` для
значения выше `0xFFFFFFFF` бросает `OverflowException`. На машине с 48
логическими процессорами `[Environment]::ProcessorCount` под WOW64 вернёт
48, `capt 40 <cmd>` пройдёт валидацию, `Run()` создаст job, а инициализатор
структуры на `bin/capt.ps1:220` бросит исключение; `finally` корректно
закроет `hJob`, PowerShell-catch напечатает «Arithmetic operation resulted
in an overflow.» и выйдет с 1. Fail-closed, ничего не запускается, но
сообщение не usage-класса и не объясняет причину. `capm` для того же
`SIZE_T`-ограничения имеет явную проверку по `[UIntPtr]::Size` с внятной
ошибкой и советом использовать 64-bit PowerShell.

Сценарий редкий (32-bit PowerShell на >32-поточной машине), поэтому P3.

**Рекомендация после релиза:** в `capt.ps1` ограничить `$maxCount` ещё и
`[UIntPtr]::Size * 8` (32 под 32-bit) с сообщением в стиле `capm`, и
отразить это в README:158-160 / SKILL:57-58.

### P3-5 — (перенос из round 19, P3-2) test-only `ProbePastDueTimerWait` по-прежнему в production `caps.ps1`

**Где:** `bin/caps.ps1:248-276`, `test/win-nice.Tests.ps1:2873-2918`,
`test/win-nice.Tests.ps1:2237-2244`.

Без изменений с раунда 19: public static метод с собственным
handle-lifecycle существует в shipped launcher ради двух Pester case и
явно исключён из source-shape guard (`$runSrc` начинается с `Run(`). Commit
message `94ae573` осознанно откладывает это на после релиза; статус
подтверждаю — не blocker, рекомендация раунда 19 (добавлять метод в копию
класса внутри fault-probe transform) остаётся в силе.

## Проверка исправлений round 19

Статически подтверждено:

- **P2-1, system-clock semantics `caps`:** закрыт по минимальному варианту
  (документирование). `README.md:218-224` — «hard wall-clock deadline»
  заменено на «wall-clock deadline», формулировка «hasn't exited within
  `<seconds>`» на «still running when `<seconds>` have passed»;
  `README.md:238-242` добавляет, что due time — значение системных часов и
  ручная/сервисная корректировка часов во время ожидания сокращает или
  удлиняет фактическое ожидание (sleep/suspend по-прежнему учитывается);
  `README.md:264-269` убирает «N seconds is the hard limit, no matter what»
  и ссылается на оговорку. `CHANGELOG.md:33-35` и
  `skills/win-nice/SKILL.md:69-70,80-83` содержат ту же оговорку. Семантика
  описана верно: kernel при `KeSetSystemTime` перебазирует interrupt-time
  due всех absolute timer так, что system-time момент срабатывания
  сохраняется — сдвиг часов вперёд ускоряет deadline, назад — откладывает;
  `GetProcessTimes` tie-break остаётся внутренне согласованным на той же
  шкале. Оборот «moves it with the clock» в README:239-240 сам по себе
  двусмысленен, но следующее предложение однозначно фиксирует следствие,
  поэтому не считаю это находкой.
- **P3-1, fault coverage `GetProcessTimes`:** закрыт. Флаг
  `FailGetProcessTimes` добавлен в shared probe state и `ResetProbe()`
  (`test/win-nice.Tests.ps1:1436,1446`); stub (`1566-1590`) по тому же
  conditional-anchor паттерну, что и timer stubs, инициализирует все четыре
  `out FILETIME` до возврата `false` (корректно для C#-семантики `out`),
  ставит `ERROR_INVALID_HANDLE` через настоящий `SetLastError`. Тест
  (`2082-2146`) не вакуумный: `$message | Should Be 'GetProcessTimes failed:
  6'` упадёт, если `Run()` вернёт exit code вместо throw; проверяются
  `TerminateCalls = 0` (на этом пути ребёнок уже вышел сам), ровно четыре
  уникальных `CloseHandle` без failures и — главное для fail-closed —
  смерть внука, оставленного в job, от закрытия `hJob` с всё ещё взведённым
  `KILL_ON_JOB_CLOSE` (bounded poll 15 s). PID внука пишется ребёнком до его
  выхода, поэтому гонки с `finally` нет. Комментарий harness
  (`1686-1703`) теперь честно говорит, что покрыты «every FAILURE branch the
  harness injects», а `TerminateJobObject` probe не имеет.
- **P2-2, дата релиза:** намеренно открыт. `CHANGELOG.md:7` по-прежнему
  `## [0.2.0] - 2026-09-03`; тега `v0.2.0` нет (локально только `v0.1.0`),
  публикации не было, поэтому дефектом это пока не является. Остаётся
  пунктом чеклиста: дата ставится в том же commit, на который ляжет тег.
- **P3-2, shipped test hook:** без изменений, перенесён как P3-5 выше.

## Остальные результаты полного прохода

- **P/Invoke и layout.** `JOBOBJECT_BASIC_LIMIT_INFORMATION` = 64 байта на
  x64 / 48 на x86 (padding под `long`), `IO_COUNTERS` = 48,
  `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` = 144/112,
  `JOBOBJECT_CPU_RATE_CONTROL_INFORMATION` = 8 (`CpuRate` как первый член
  union), `MEMORYSTATUSEX` = 64, `STARTUPINFO` Unicode с `cb =
  Marshal.SizeOf`, `FILETIME` = 2 × uint, сборка 64-битного значения
  zero-extended через `(long)uint`. `SetWaitableTimer(ref long, int, IntPtr,
  IntPtr, bool)` и `WaitForMultipleObjects(uint, IntPtr[], bool, uint)`
  соответствуют нативным сигнатурам; `bool` маршалится как 4-байтный BOOL.
  Константы: `JobObjectExtendedLimitInformation = 9`,
  `JobObjectCpuRateControlInformation = 15`, `KILL_ON_JOB_CLOSE = 0x2000`,
  `AFFINITY = 0x10`, `JOB_MEMORY = 0x200`, `ACTIVE_PROCESS = 0x8`,
  `CREATE_SUSPENDED = 0x4`, priority classes `0x40/0x4000/0x8000/0x80/0x100`
  — верны. `CpuRate = percent * 100` даёт 100..10000 в единицах 1/100 %.
- **Job lifecycle.** Во всех пяти Job launcher порядок
  `CreateJobObject → SetInformationJobObject (limit + KILL_ON_JOB_CLOSE) →
  CreateProcess(CREATE_SUSPENDED) → AssignProcessToJobObject → ResumeThread
  → wait → GetExitCodeProcess → release (тот же limit без kill-on-close) →
  finally` сохранён; job/timer/process handles не inheritable
  (`lpAttributes = NULL`), поэтому потомки не могут удержать job живым.
  Release-структура re-applies собственный limit (`capt` affinity, `capm`
  `JobMemoryLimit`, `capn` `ActiveProcessLimit`), `capc` не трогает
  отдельный CPU-rate info class. Каждый failure branch захватывает Win32
  error до `TerminateProcess`; timeout `caps` использует
  `TerminateJobObject`, а на abnormal paths закрытие `hJob` с взведённым
  флагом остаётся backstop-ом. Tie-break `exitFileTime > timerDueTime`
  корректен на tick-квантованной общей шкале (timer срабатывает не раньше
  due; exit time и due — один и тот же system-time clock).
- **Аргументы и quoting.** `ArgvQuote` — стандартный CRT-алгоритм
  (удвоение backslash перед `"`, хвостовые backslash удваиваются);
  cmd.exe fallback — `/d /v:off /s /c` + внешняя пара кавычек, `%`-check до
  любого `CreateProcess` на fallback; `.bat`/`.cmd` target пропускает direct
  attempt. PowerShell-сторона без `param()`, чтобы `-p`/`-c`/`-s` не
  биндились. Валидация чисел через `TryParse` (`capm`/`caps`/`capn`) —
  400-значные строки дают usage-ошибку, а не raw conversion error.
  Отрицательные exit codes (`0xC0000005` и т.п.) доходят до `%ERRORLEVEL%`
  через `(int)exitCode` → `exit`.
- **Installer.** `cleanupStaleFiles` работает и по manifest (без marker), и
  по marker-gated scan; `isInsideDir` отвергает traversal/absolute entries;
  реестр читается/пишется через Base64 и env-переменные без
  OEM-codepage потерь, `REG_EXPAND_SZ` сохраняется, `WM_SETTINGCHANGE`
  рассылается с `SMTO_ABORTIFHUNG`/5 s; source-checkout guard есть у
  `install`/`uninstall`/`reinstall`; skill update только для marked copy.
- **Release gates.** `release-check.js` без `shell:true`, изолирует
  `WIN_NICE_HOME`/`WIN_NICE_SKILL_HOME`/`WIN_NICE_NO_PATH`, сверяет полный
  54-path allowlist, manifest version, отсутствие legacy `cap`/`pint`,
  smoke-тесты пяти `.ps1` + `idle.bat` + shim, heading и compare-base
  CHANGELOG. CI матрица Node 18/20/22/24, `timeout-minutes: 20`, actions
  закреплены по SHA (`checkout@11d5960a`, `setup-node@49933ea5`), Pester
  pinned `-MaximumVersion 3.99` с reset `$ErrorActionPreference`;
  `publish.yml` — `contents: read`/`id-token: write`, concurrency по ref,
  tag↔`package.json` проверка до publish, npm pinned `11.5.1`, OIDC +
  `--provenance --access public`.
- **Тесты.** Проверены на вакуумность ключевые assertions:
  `FailWait`/`FailTerminate`/`FailResume`/`FailAssign`/`FailSetInfo`/
  `FailGetExitCode`/release-call `SetInfoFailOnCall` (3 для `capc`, 2 для
  остальных), timer/`GetProcessTimes` stubs, tie-break driver с
  `bWaitAll = true` перед возвратом индекса 0, `ExpectedHandles` 2/3 против
  `HandlesAtWait` 2/3/4 — все различают старое и новое поведение. Source-
  shape guard считает `CloseHandle(` только от `Run(` (4 для `caps`).
  `docs-sync` фразы присутствуют в обоих документах в нужных секциях. Число
  elevation-gated case (3 `-Skip:(-not isAdminRunner)`, 4
  `-Skip:isAdminRunner`) совпадает с README/CONTRIBUTING.
- **Мелочи вне findings (без действия перед тегом):** README:577-580 и
  CONTRIBUTING:14 всё ещё оценивают Pester-прогон в «1-2 minutes» —
  комментарий в самом suite (`test/win-nice.Tests.ps1:462-465`) упоминает
  338 s для 210 case, сейчас их 283 с несколькими 3–15 s bounded wait;
  README:158-160 объясняет потолок 63 тем, что «a single affinity mask
  can't address more» — 64-битная маска адресует 64, реальная причина в
  `-shl` по модулю 64 у `[uint64]`; namespace `WinNiceFaultProbePint` для
  `capt` (`test/win-nice.Tests.ps1:1721`) — след старого имени `pint`;
  failure branch `TerminateJobObject` (`bin/caps.ps1:535-536`) осознанно
  без probe, но fail-closed через `finally`.
- Новых command-injection путей сверх задокументированного `%`-ограничения
  `.bat`/cmd.exe fallback и P3-3 выше статически не найдено. Runtime
  dependencies по-прежнему отсутствуют.

## Release checklist

1. Перед тегом проставить фактическую дату публикации в `CHANGELOG.md:7`
   (P2-2 раунда 19) — в том же commit, который будет помечен `v0.2.0`.
2. Решить по P3-1: править текст сейчас (три public doc + usage + два
   комментария, только текст) или сразу после релиза; если сейчас — после
   правки повторить `npm test` (docs-sync) и `release-check`.
3. Получить свежий remote state (локальная tracking-ссылка показывает
   `ahead 35`), убедиться, что release commit лежит на публикуемой ветке.
4. Вне read-only review прогнать на финальном commit: `npm test`, normal
   Pester, `npm run test:elevated` (последний зафиксированный elevated-прогон
   274 case сделан до `05c5e7a`), `npm run release-check`.
5. Только после green gates — annotated `v0.2.0`, push ветки и тега,
   контроль завершения `publish.yml` и npm-артефакта.
6. После релиза: P3-2..P3-5 (doc-оговорка про `MSYS2_ARG_CONV_EXCL`,
   абсолютный путь `powershell.exe` в `.bat`, 32-bit guard в `capt`, вынос
   `ProbePastDueTimerWait` из production).

## Итог

Round 20 не обнаружил ни одного release-blocking дефекта: P0/P1/P2 нет.
Правки `94ae573` по раунду 19 корректны и не вакуумны; contract `caps`
теперь честно описан как absolute system-clock deadline, а единственный
непокрытый fault-injection syscall из раунда 19 покрыт с проверкой
fail-closed судьбы потомка. Из нового — только P3: остаточное объяснение
uint32-лимита из удалённого дизайна в упакованных доках/usage (стоит
поправить, пока правится дата в CHANGELOG), и три небольших hardening-
пункта вне `caps`, которые разумно закрыть следующим патч-релизом. До тега
остаются дата релиза, свежие динамические gates и сам тег.
