# Полное статическое release-review — раунд 21

Дата: 2026-09-04 12:27 (Europe/Berlin)

Проверенный commit: `3152bbb003bfadb222c507a7c8f0638ed56c1f5f` (`master`)

База предыдущего review: `94ae573` (round 20, отчёт в `0c37b7f`)

Новая реализация после него: `3152bbb`

## Вердикт

**Релиз `0.2.0` пока не готов: найден один P1 security blocker.** В
не-elevated ветках `admin.ps1` и `uiup.ps1` системные программы передаются в
`Start-Process -Verb RunAs` неполным именем. Для shell-based запуска пустой
`WorkingDirectory` означает поиск executable в текущем каталоге; поэтому
подложенный рядом с проектом `cmd.exe`, `powershell.exe` или executable с
именем запрошенной команды может получить административные права после
ожидаемого пользователем UAC prompt. Исправление round 20 защитило первый
`.bat -> powershell.exe` hop, но не следующий privilege-boundary hop внутри
`.ps1`.

Дополнительно найдены один P2 release-process пункт и два P3:

- P2: перед тегом дата `0.2.0` в `CHANGELOG.md` должна быть заменена с
  `2026-09-03` на фактическую дату релиза;
- P3: `install/paths.js` запускает фиксированную системную зависимость
  `powershell` через Node/libuv с тем же cwd-first поиском, но без повышения
  привилегий;
- P3: у двух новых hardening-исправлений round 20 неполное regression
  coverage: нет проверки абсолютного PowerShell path во всех `.bat`, а тест
  `capt` проверяет helper и текст связи, но не ошибочную ветку скрипта.

P0 не найдено. После устранения P1, обновления даты в tagging commit и
обязательного полного CI/release-check прогона оснований блокировать релиз по
результатам этого статического review не останется. P3 разумно закрыть до тега,
пока изменения малы, но они сами по себе релиз не блокируют.

Это намеренно **только статическое review**. По условиям задачи не запускались
Node/Pester-тесты, `npm pack`, `npm run release-check`, сборка, установщик,
launchers или какие-либо live probes. Указанные в commit message `3152bbb`
результаты (`npm test` 72/72, Pester 281/0/3, release-check green) приняты как
предоставленное автором свидетельство и независимо не подтверждались.

## Scope и состояние репозитория

Проведён полный статический проход по 104 tracked-файлам и отдельно проверены:

- все 42 launcher-файла в `bin/` (14 инструментов × `.ps1`/`.bat`/Git Bash
  shim), включая Win32 declarations, структуры x86/x64, error paths,
  CreateProcess/cmd fallback, Job Object lifecycle, timeout и cleanup;
- `install/*.js`, manifest/PATH/skill install-uninstall и CLI routing;
- `scripts/release-check.js`, tarball allowlist и publish/CI workflows;
- `test/win-nice.Tests.ps1`, восемь Node test-файлов и elevated runner —
  только чтением;
- public contract в `README.md`, `CHANGELOG.md`, packaged skill,
  `package.json`, security/contribution docs;
- полный diff `v0.1.0..3152bbb` и новый diff `53133db..3152bbb`, включая
  реализацию всех пяти пунктов round 20.

Статические факты на момент review:

- рабочее дерево было чистым до добавления этого отчёта;
- `master` по локальной tracking-ссылке опережал `origin/master` на 37
  commits; `git fetch` не выполнялся;
- локально существует только тег `v0.1.0`; `package.json` содержит `0.2.0`;
- со времени `v0.1.0`: 79 файлов, 11 779 additions / 832 deletions;
- после отчёта round 19 (`53133db`): 19 файлов, 653 additions / 81 deletions;
- фактический `bin/` по-прежнему равен контракту 14 × 3 = 42, восемь Node
  test-файлов присутствуют;
- `git diff --check 53133db..HEAD` чист;
- allowlist `release-check.js` по-прежнему описывает 54 shipped path: 42
  launcher-файла, шесть `install/*.js`, packaged skill, `package.json` и
  четыре root docs/licenses.

## Findings

### P1-1 — UAC запускает executable из текущего каталога вместо проверенного системного/разрешённого файла

**Где:** `bin/admin.ps1:245-280`, особенно `:268`, `:270-280`;
`bin/uiup.ps1:7-12`. Связанный ошибочный вывод прошлого review:
`docs/reviews/2026-09-04-0956-94ae573-round-20-release-review.md`, P3-3.

В not-yet-elevated ветке `admin` сначала вызывает `Get-Command` и корректно
определяет, является ли target приложением. Однако найденный абсолютный
`$resolved.Path` отбрасывается: функция возвращает только строку `Direct`, а
`Start-Process @startArgs` получает исходный `$Command[0]`. Если target требует
cmd fallback, код передаёт буквально `cmd.exe`. `uiup` аналогично передаёт
литеральное `powershell`:

```powershell
# admin.ps1
Start-Process -FilePath 'cmd.exe' ... -Verb RunAs
$startArgs = @{ FilePath = $Command[0]; Verb = 'RunAs'; ... }

# uiup.ps1
Start-Process powershell -Verb RunAs ...
```

`-Verb RunAs` использует OS shell. Документация .NET для
`ProcessStartInfo.WorkingDirectory` прямо говорит: при `UseShellExecute=true`
это каталог поиска executable, а при пустом значении текущий каталог считается
каталогом executable. Документация `Start-Process` также относит `-Verb` к
shell-execute parameter set. Microsoft отдельно рекомендует передавать полные
пути в `CreateProcess`/`ShellExecute` для защиты от preloading/search-order
атак:

- <https://learn.microsoft.com/en-us/dotnet/api/system.diagnostics.processstartinfo.workingdirectory?view=net-10.0>
- <https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.management/start-process?view=powershell-5.1>
- <https://learn.microsoft.com/en-us/windows/win32/dlls/dynamic-link-library-security>

Практические сценарии:

1. Пользователь находится в скачанном/чужом каталоге с подложенным
   `powershell.exe` и вызывает `uiup`. Round 20 гарантирует, что `.bat` сначала
   откроет настоящий Windows PowerShell, но строка `uiup.ps1:11` затем снова
   ищет bare `powershell` уже для UAC-запуска. Подложенный файл выполняется
   elevated после обычного для `uiup` consent prompt.
2. `admin dir`/`admin some-script.cmd` попадает в fallback; подложенный
   `cmd.exe` из cwd получает elevation.
3. `admin node ...` или `admin notepad ...`: `Get-Command` может подтвердить
   настоящий executable из PATH, но код не использует его path. ShellExecute
   повторно разрешает исходное bare name и может выбрать одноимённый `.exe` в
   cwd.

Это не только defense-in-depth: функция инструмента — ожидаемо запросить UAC,
а дефект меняет субъект, которому выдаются административные права. Поэтому P1
и release blocker.

**Рекомендация:**

- в `admin.ps1` возвращать из resolver не только route, но и абсолютный
  `$resolved.Path`, и передавать его в direct `Start-Process`;
- для fallback использовать абсолютный
  `[Environment]::SystemDirectory + '\cmd.exe'`;
- в `uiup.ps1` использовать абсолютный Windows PowerShell path (лучше получить
  его один раз из trusted system directory, не из cwd/PATH);
- добавить regression tests, которые не показывают UAC: source-shape guard на
  абсолютные system paths плюс чистый resolver test, доказывающий, что direct
  route сохраняет именно resolved path. При возможности отдельный изолированный
  Windows integration test должен подложить одноимённый `.exe` в cwd и
  проверить выбранный path без запуска elevated payload.

Исправляется до тега.

### P2-1 — дата релиза в CHANGELOG уже не совпадает с фактической датой будущего тега

**Где:** `CHANGELOG.md:7`, `package.json:3`, `.github/workflows/publish.yml`.

В `package.json` версия уже `0.2.0`, но latest heading —
`## [0.2.0] - 2026-09-03`. Сегодня 2026-09-04, тега `v0.2.0` ещё нет. Это был
осознанно отложенный пункт round 19/20, а не неожиданная регрессия, но теперь
он остаётся обязательной частью release commit. Текущий `release-check`
сопоставляет version/heading/compare-base tag, но статически не видно проверки,
что дата heading равна дате релиза, поэтому зелёный gate сам этот drift не
поймает.

**Рекомендация:** в том же commit, который будет тегирован `v0.2.0`, поставить
фактическую дату релиза. После этого запустить полный release gate. Как
follow-up — добавить чистую проверку формата/ожидаемой даты либо явно оставить
дату ручным пунктом release checklist.

Не исправлять заранее отдельным днём, если тег снова будет перенесён; но тег с
текущей строкой ставить нельзя.

### P3-1 — installer разрешает фиксированный `powershell` через cwd-first поиск Node/libuv

**Где:** `install/paths.js:69-73`; вызывается registry read/write и environment
broadcast путями установки/удаления.

`runPowershell()` делает `execFileSync('powershell', ...)` без абсолютного
пути. На Windows Node child processes реализованы через libuv. Его актуальная
`search_path()` документирует и реализует для bare filename порядок «cwd
first, затем PATH» (если это не отключено `NoDefaultCurrentDirectoryInExePath`):

- <https://github.com/libuv/libuv/blob/v1.x/src/win/process.c#L2653-L2701>
- <https://learn.microsoft.com/en-us/windows/win32/api/processenv/nf-processenv-needcurrentdirectoryforexepatha>

Значит, запуск `win-nice install`, `uninstall` или PATH repair из каталога с
подложенным `powershell.exe` выполнит его с правами пользователя. Elevation тут
нет, поэтому это существенно ниже P1-1 и соответствует P3-классу прежней
находки про `.bat`; тем не менее round 20 уже выбрал policy полного пути для
фиксированной системной зависимости, а installer остаётся исключением.

**Рекомендация:** вынести trusted absolute Windows PowerShell path в helper и
использовать его во всех `runPowershell()` вызовах; добавить unit/source-shape
test. Одновременно можно применить тот же helper к smoke/release scripts, где
это улучшает воспроизводимость, хотя они не входят в shipped runtime.

### P3-2 — новые hardening-исправления round 20 защищены тестами не полностью

**Где:** 12 PowerShell-делегирующих `bin/*.bat`;
`test/win-nice.Tests.ps1:614-636`; `bin/capt.ps1:27-38`.

1. Все 12 `.bat` сейчас корректно используют
   `"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"`, но поиск
   этой строки в `test/` не даёт ни одного source-shape/regression assertion.
   Случайная замена обратно на bare `powershell` останется незамеченной.
2. Новый тест `capt` AST-извлекает `Get-CaptAffinityBitLimit`, проверяет
   `4 -> 32`, `8 -> 64` и строкой подтверждает наличие сравнения. Он не
   выполняет ветку `countValue > bitLimit`, поэтому не защищает обещанное
   сообщение, exit code и порядок валидации. Реальный 32-bit host доступен на
   64-bit Windows как SysWOW64 PowerShell, но на runner с не более чем 32
   logical processors machine-count guard сработает раньше. Надёжнее вынести
   вычисление эффективного max/результат валидации в чистую функцию с
   инъекцией `processorCount` и `UIntPtrSize`, затем тестировать итоговую ветку.

Оба production-исправления статически корректны; находка только о защите от
будущего отката.

## Проверка исправлений round 20

Статически подтверждено:

- **P3-1, stale `caps` rationale:** закрыт. README, CHANGELOG, packaged skill,
  usage и code comments теперь честно называют `4294967294` ms сознательным
  usage ceiling, а не лимитом `WaitForMultipleObjects`; runtime deadline
  остаётся абсолютным 64-bit FILETIME, wait — `INFINITE` до сигнала timer или
  process.
- **P3-2, Git Bash environment leak:** не устранён в runtime, но закрыт
  выбранным в review вариантом — явно и одинаково документирован в README и
  packaged skill как known limitation. Формулировки отражают фактическое
  наследование `MSYS2_ARG_CONV_EXCL='*'`.
- **P3-3, `.bat` cwd hijack:** первый hop закрыт во всех 12 применимых `.bat`
  полным Windows PowerShell path; `cx.bat`/`cy.bat` намеренно запускают целевой
  CLI, а не PowerShell. Но полный аудит того же trust boundary выявил P1-1
  выше: второй, уже elevated hop остался bare. Фраза round 20 «PowerShell cwd
  не просматривает» была верна для обычного PowerShell command discovery, но
  неверна для `Start-Process -Verb RunAs`/ShellExecute; этот вывод настоящим
  review исправляется.
- **P3-4, `capt` 32-bit overflow:** production guard корректен: допускает 32
  affinity bits в 32-bit процессе, отбрасывает 33-63 до UIntPtr cast, а 64-bit
  путь по-прежнему ограничен 63 и не делает shift на 64. README/skill
  синхронизированы. Остался только coverage gap P3-2.
- **P3-5, shipped `ProbePastDueTimerWait`:** закрыт. Метод отсутствует в
  `bin/caps.ps1`, fault harness injects его только в тестовую копию, а
  source-shape assertion запрещает его возвращение в production source.

## Что проверить после исправлений (не выполнялось в этом раунде)

Минимальный release gate перед `v0.2.0`:

1. Regression tests для P1/P3 и весь `npm test`.
2. Полный non-elevated Pester suite; отдельно elevated suite из trusted cwd.
3. `npm run release-check`, включая реальный tarball allowlist/install/smoke.
4. Проверка, что packed `admin`/`uiup` используют абсолютные paths и что
   resolver `admin` передаёт именно найденный executable.
5. `CHANGELOG.md` с фактической датой, затем annotated/обычный release tag
   `v0.2.0` на том же commit и только после этого publish workflow.

## Итоговый release checklist

- [ ] Исправить P1-1 в `admin.ps1` и `uiup.ps1`.
- [ ] Добавить regression coverage privilege-boundary resolution.
- [ ] Перед тегом поставить фактическую дату `0.2.0` в CHANGELOG.
- [ ] По возможности закрыть P3-1/P3-2 до релиза.
- [ ] Выполнить полный Node + Pester + elevated + packed-artifact gate.
- [ ] Проверить clean tree, version/tag equality и поставить `v0.2.0`.

**Итог round 21: NO-GO до P1-1 и корректной release date.**
