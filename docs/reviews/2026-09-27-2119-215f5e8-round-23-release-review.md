# Release-review — раунд 23 (0.2.1, prebuilt launcher assemblies)

Дата: 2026-09-27 21:19 (Europe/Berlin)

Проверенный commit: `215f5e8ed9be2d4b47ca219de8691720c530ba8b` (`master`, тег
`v0.2.1`, совпадает с `origin/master`)

База предыдущего review: `7b11882` (round 22, отчёт в `69223f5`)

Новая реализация после него: `5069a37` (исправления round 21/22), `05d93cf`
(CI, тег `v0.2.0`), `215f5e8` (релиз `0.2.1`)

Главный вопрос раунда (требование владельца): **пользователь не должен ничего
компилировать или собирать на своей машине — через npm должно приходить готовое
решение.**

## Вердикт

**Требование выполнено для установки из npm registry — проверено эмпирически на
фактически опубликованном tarball `win-nice@0.2.1`, а не только статически.**
P0/P1 не найдено. Найден один P2 в процессе сборки: сборка helper-DLL
недетерминирована, а `pretest`/`prepack` пересобирают все 14 DLL при каждом
`npm test`/`npm pack`. Из-за этого рабочее дерево после каждого прогона
тестов становится грязным, байты закоммиченных DLL не совпадают с
опубликованными, и нет ни одной проверки, что закоммиченная DLL собрана из
текущего C#-исходника в `.ps1`. Плюс пять P3 (документация, мёртвый скрипт,
дублирование списков, дедупликация в загрузчике, непроверенный PowerShell 7).

## Scope и метод

Разобран полный diff `3152bbb..215f5e8` с акцентом на `215f5e8`:
`bin/load-launcher.ps1`, 13 `.ps1` (переход с `Add-Type` на
`Import-WinNiceLauncherAssembly`), 14 DLL + 14 sidecar `.dll.managed`,
`scripts/build-launcher-assemblies.ps1`, `scripts/EnvironmentNotifier.cs`,
`install/{install,uninstall,manifest,paths}.js`, `scripts/release-check.js`,
`package.json`, `.gitattributes`, README/CONTRIBUTING/CHANGELOG, workflows.
Отдельно проверено закрытие round 21/22 в `5069a37` и CI-фикс `05d93cf`.

Динамические проверки, выполненные в этом раунде (вне репозитория, во
временных каталогах; рабочее дерево осталось чистым):

1. **Опубликованный пакет.** Tarball `win-nice@0.2.1` скачан напрямую по
   `dist.tarball` из registry, SHA-1 совпал с `dist.shasum`. Содержит 14 DLL,
   14 sidecar `.dll.managed` и `bin/load-launcher.ps1`; `dist.fileCount = 83`
   совпадает с allowlist `release-check.js`.
2. **Установка из него.** `node package/install/cli.js install` с временными
   `WIN_NICE_HOME`/`WIN_NICE_SKILL_HOME` и `WIN_NICE_NO_PATH=1`: rc 0, в
   `bin` установлено 71 файл.
3. **Запуск установленных инструментов.** `capn.ps1 3`, `capt.ps1 1`,
   `caps.ps1 5`, `capm.ps1 50`, `capc.ps1 50`, `idle.bat`, Git Bash shim
   `idle`: все вернули ожидаемые коды выхода обёрнутой команды (5, 6, 8, 9,
   10, 7, 11). Новые сборки в AppDomain после запуска — `CapmLauncher`,
   `CapcLauncher` (и в отдельном прогоне `IdleLauncher`, `CapsLauncher`) с
   `Location = ''` и `IsDynamic = False`, т.е. загружены из байтов готовой DLL;
   сборок `Add-Type` (временные файлы в `%TEMP%`) нет, процессов `csc.exe` нет.
4. **Статически.** `grep -i 'Add-Type|csc.exe|CodeDom|TypeDefinition'` по
   `bin/` и `install/` — ноль совпадений. Единственный вызов компилятора —
   `scripts/build-launcher-assemblies.ps1`, который не входит в `files` пакета.
   `postinstall` = `node install/cli.js install` — только копирование файлов и
   PATH; `broadcastEnvironmentChange` теперь грузит готовую
   `EnvironmentNotifier.dll` вместо прежнего `Add-Type -MemberDefinition`.
   `load-launcher.ps1` при отсутствии DLL бросает ошибку, а не откатывается на
   компиляцию. `prepack`/`pretest` зависимости npm при установке из registry не
   выполняет.
5. **Детерминированность сборки.** Один и тот же C#-исходник `IdleLauncher`,
   дважды скомпилированный тем же `csc.exe` с теми же флагами, что у
   `build-launcher-assemblies.ps1`, дал разные SHA-256 (`79E26BC6…` и
   `E9616DD2…`), и оба отличаются от закоммиченной `bin/IdleLauncher.dll`
   (`935D0D6C…`). `csc.exe` из `Microsoft.NET\Framework64\v4.0.30319` — это
   компилятор «C# 5» (4.8.4084.0), и на `/deterministic` он отвечает
   `fatal error CS2007: Unrecognized option: '/deterministic'`.
6. **Разрядность.** `ProcessorArchitecture` у DLL = `MSIL` (AnyCPU),
   `ImageRuntimeVersion` = `v4.0.30319`: грузится и в 64-bit, и в 32-bit
   Windows PowerShell.

Не запускались: `npm test`, Pester, `npm run release-check` — все три
вызывают `pretest`/`prepack` и из-за P2-1 ниже переписали бы 14 tracked DLL в
рабочем дереве. Результаты CI для `v0.2.1` рассматриваются как свидетельство
автора. PowerShell 7 (`pwsh`) на этой машине не установлен (см. P3-5).

## Findings

### P2-1 — сборка helper-DLL недетерминирована, а свежесть закоммиченных DLL ничем не проверяется

**Где:** `package.json` (`pretest`, `prepack`),
`scripts/build-launcher-assemblies.ps1`, `bin/*.dll`, `bin/*.dll.managed`,
`test/win-nice.Tests.ps1:242-254`.

DLL закоммичены в git и одновременно пересобираются хуками `pretest` и
`prepack`, т.е. при каждом `npm test`, `npm pack`, `npm publish` и
`npm run release-check`. Legacy-компилятор .NET Framework детерминированную
сборку не поддерживает (п. 5 выше: MVID и timestamp меняются от сборки к
сборке). Последствия:

- после любого локального `npm test` или `release-check` в `git status` —
  14 изменённых бинарных файлов без изменения исходников; их либо
  приходится откатывать руками, либо они попадают в следующий commit как
  шум;
- байты DLL в теге `v0.2.1` не совпадают с байтами DLL в опубликованном
  tarball (publish пересобрал их через `prepack`). Опубликованный
  артефакт от этого не хуже, но по git нельзя воспроизвести или сверить,
  что именно ушло в npm;
- **главное:** тесты проверяют только наличие DLL и sidecar
  (`Test-Path`), но не то, что DLL собрана из текущего блока
  `$source = @"…"@` в соответствующем `.ps1`. Если кто-то изменит C# в
  `.ps1` и закоммитит без пересборки (или закоммитит пересобранную DLL от
  другой ревизии), HEAD будет исполнять старый код. Fault-injection
  probes при этом компилируют и проверяют *новый* исходник — тесты
  зелёные на коде, который в DLL не попал. CI и publish от этого
  защищены только потому, что `npm test` идёт до Pester и сам
  пересобирает DLL. Любой локальный Pester-прогон без предварительного
  `npm test` проверяет устаревшие DLL.

**Рекомендация:** сделать свежесть проверяемой, не требуя детерминизма:

1. `build-launcher-assemblies.ps1` записывает в sidecar `.dll.managed`,
   помимо маркера, SHA-256 извлечённого C#-исходника (и версию `csc`):
   `# win-nice: managed-file source-sha256=<hex>`. Маркер остаётся на месте —
   `manifest.hasMarker` ищет подстроку, так что установщик не меняется.
2. Сборка пропускает DLL, если хеш в sidecar совпадает с хешем текущего
   исходника; пересобирает только при расхождении (или с явным `-Force`).
3. Node-тест без компилятора (быстрый, в `npm test`) сверяет для каждой из 14
   сборок хеш в sidecar с хешем текущего блока `$source` (для
   `EnvironmentNotifier` — с `scripts/EnvironmentNotifier.cs`) и падает с
   понятным сообщением «запусти scripts/build-launcher-assemblies.ps1».
4. После этого `pretest` можно убрать (тест из п. 3 ловит устаревшие DLL), а
   `prepack` станет no-op на свежем дереве. `npm publish` тогда публикует
   ровно те байты, что лежат в теге. Дополнительный плюс: установка из git
   (`npm i github:…`), если npm выполнит для неё `prepack`, тоже перестанет
   что-либо компилировать у пользователя на свежем дереве.

### P3-1 — README описывает установку «`idle` helper assembly»

**Где:** `README.md:464`.

«This copies every tool above and the `idle` helper assembly into …» —
остаток промежуточного состояния, когда готовой была только `IdleLauncher`.
Сейчас копируются 14 DLL (13 launcher + `EnvironmentNotifier`), их sidecar и
`load-launcher.ps1`. Исправить на «every tool above, the shared loader, and
their prebuilt helper assemblies».

### P3-2 — мёртвый `scripts/build-idle-launcher.ps1`

**Где:** `scripts/build-idle-launcher.ps1`.

Двухстрочная обёртка над `build-launcher-assemblies.ps1`. Её нигде не вызывают
(`grep` по репозиторию — ноль ссылок) и в пакет она не входит. Судя по
имени, это такой же остаток промежуточного этапа, как P3-1. Удалить.

### P3-3 — список из 14 сборок продублирован вручную в четырёх местах

**Где:** `scripts/build-launcher-assemblies.ps1` (`$launcherSources` +
отдельный `EnvironmentNotifier`), `install/install.js` (`SUPPORT_FILES`),
`scripts/release-check.js` (`helperAssemblies`),
`test/install-uninstall.test.js:58-62`.

Новый launcher нужно добавить во все четыре места. Если забыть
`install.js`, DLL попадёт в tarball, но не установится, и инструмент упадёт с
`missing prebuilt helper assembly`. Сейчас это ловит только поздний gate
(`release-check`, сверка установленного manifest), а не `npm test`.
**Рекомендация:** один источник истины — например, `install.js` экспортирует
список, а остальные его импортируют или сверяют с ним (для PowerShell-скрипта —
Node-тест, который сравнивает `$launcherSources` с экспортом).

### P3-4 — загрузчик дедуплицирует сборку только по простому имени

**Где:** `bin/load-launcher.ps1:12-14`.

Если в AppDomain уже есть сборка с именем `IdleLauncher`, функция молча
выходит. Два следствия:

1. В долгоживущей PowerShell-сессии после обновления win-nice продолжает
   работать старая DLL, пока сессия не будет перезапущена.
2. Если в сессии уже загружена *чужая* сборка с таким же простым именем, ей
   без проверки передаётся управление. Вероятность мала: имена
   `*Launcher` без namespace, DLL без strong name.

Это не регрессия относительно `Add-Type` (там «type already exists»
давал бы ошибку). `Location` у сборки, загруженной через `Load(bytes)`, пустой,
поэтому сравнивать по нему нельзя. Практичнее запоминать SHA-256 файла при
первой загрузке, а при повторном вызове с другим хешем выдавать понятное
предупреждение «перезапустите PowerShell после обновления win-nice».

### P3-5 — работа под PowerShell 7 (`pwsh`) не проверена

**Где:** `bin/load-launcher.ps1`, все 13 `.ps1`.

Раньше `Add-Type` компилировал C# под ту среду выполнения, в которой запущен
PowerShell. Теперь в любую среду загружается сборка .NET Framework 4
(`v4.0.30319`). Если пользователь вызывает `idle` по короткому имени из
`pwsh`, `idle.ps1` выполняется в .NET (Core) и загружает netfx-сборку. Скорее
всего это работает: используются только `System.Runtime.InteropServices` и
базовые типы, которые переадресуются. Но ни CI, ни этот раунд это не
проверяют — `pwsh` на машине review отсутствует, а README поддержку `pwsh` не
описывает ни в одну сторону. **Рекомендация:** добавить в CI smoke-шаг
`pwsh -NoProfile -File bin\idle.ps1 cmd /c exit 7` (на windows runner `pwsh`
предустановлен) либо явно задокументировать, что поддерживается только Windows
PowerShell 5.1.

## Проверка исправлений round 21/22 (`5069a37`, `05d93cf`)

Статически подтверждено:

- **Round 21 P1-1 (UAC executable substitution):** закрыт. `admin.ps1`
  передаёт в `Start-Process -Verb RunAs` абсолютный путь, полученный через
  `Get-Command` → `GetFullPath` (`$script:AdminLaunchPath`), а cmd-fallback
  запускает `[Environment]::SystemDirectory + '\cmd.exe'`. `uiup.ps1` —
  `SystemDirectory\WindowsPowerShell\v1.0\powershell.exe`.
- **Round 21 P3-1 (installer cwd-first):** закрыт. `install/paths.js`
  `powershellPath()` строит путь от `SystemRoot`/`WINDIR` и отвергает
  неабсолютный путь; покрыт `test/paths.test.js`.
- **Round 22 P3-1 (`run-elevated.ps1`):** закрыт тем же абсолютным путём.
- **Round 22 P3-2 / round 21 P2-1 (дата релиза):** закрыт.
  `release-check.js` требует точный формат `## [X.Y.Z] - YYYY-MM-DD` и реальную
  календарную дату; дата `0.2.0` = `2026-09-04` совпадает с датой тега, дата
  `0.2.1` = `2026-09-25` — с датой commit. (Сам тег `v0.2.1` создан
  2026-09-25 13:13, т.е. в тот же день.)
- **`05d93cf`:** явный `exit 0` после `Invoke-Pester` в `ci.yml` и
  `publish.yml` корректен: без него код выхода шага наследовал остаточный
  `$LASTEXITCODE` последнего native-вызова внутри suite, даже при
  `FailedCount = 0`.

## Остальные результаты прохода

- **Блокировки файлов при обновлении.** `Assembly.Load(ReadAllBytes(...))`
  не держит DLL открытой, поэтому `copyFileSync` поверх неё при
  `npm i -g win-nice@newer` не упирается в sharing violation, даже если
  инструмент в этот момент запущен. Для этого сценария это верный выбор
  вместо `LoadFrom`.
- **Маркеры и удаление.** `manifest.hasMarker` для `.dll` читает sidecar;
  `orderManagedFiles` удаляет `.dll` раньше sidecar, так что маркер ещё на
  месте в момент проверки DLL (в том числе в marker-gated fallback без
  manifest). `.gitattributes` `bin/*.dll binary -eol` стоит после
  `bin/* text eol=lf` и выигрывает — DLL не портятся нормализацией EOL.
- **Установка.** `listSourceFiles()` подхватывает `load-launcher.ps1` через
  фильтр `.ps1`, а 28 файлов DLL/sidecar — через `SUPPORT_FILES`; итог 71 =
  42 launcher + loader + 28, совпадает с allowlist и с фактической установкой
  в п. 2.
- **Исходник DLL остаётся в `.ps1`.** Блок `$source = @"…"@` больше не
  исполняется, он служит входом для сборки и fault-injection probes. Это
  оправдано (один источник для DLL и тестов), но делает P2-1 существенным:
  связь «исходник ↔ DLL» сейчас держится только на дисциплине.
- **Установка из git.** npm для git-зависимостей может выполнить
  lifecycle-скрипты упаковки. Тогда `prepack` запустит `csc.exe` на машине
  пользователя; компилятор есть в любой Windows с .NET Framework 4.x, но это
  расходится с целью «ничего не собирать». Основной канал — registry, поэтому
  это не находка, а дополнительный аргумент за no-op-сборку из P2-1.
- **Опубликованная версия.** В registry: `0.1.0`, `0.2.0`, `0.2.1`. Локальная
  npm-политика с `before`-датой (min release age) не даёт `npm pack
  win-nice@0.2.1` скачать версию младше недели. На пользователей без такой
  политики это не влияет; tarball в этом раунде получен напрямую по
  `dist.tarball` со сверкой SHA-1.

## Checklist

1. P2-1: хеш исходника в sidecar, пропуск пересборки при совпадении,
   Node-тест свежести, убрать `pretest`. После этого `npm test` не
   должен менять ни одного tracked-файла (проверка: `git status --short` пуст
   после `npm test` на чистом дереве).
2. P3-1/P3-2: поправить README:464, удалить `scripts/build-idle-launcher.ps1`.
3. P3-3: один источник списка сборок.
4. P3-4/P3-5: по желанию — проверка хеша в загрузчике, smoke-шаг `pwsh` в CI
   или явная оговорка о поддержке только Windows PowerShell 5.1.
5. Если правки попадут в релиз — `0.2.2` (версию поднимать только по явному
   решению владельца), полный CI + `release-check`, затем тег.

## Итог

Цель «пользователь ничего не компилирует» для установки из npm registry
достигнута и подтверждена на реальном опубликованном `0.2.1`: tarball несёт
готовые AnyCPU-сборки, `postinstall` только копирует файлы, каждый запуск
грузит DLL из байтов, а `csc`/`Add-Type` в пользовательском пути нет.
Слабое место — процесс сборки: legacy-`csc` недетерминирован, DLL
пересобираются на каждом `npm test`/`npm pack`, и ничто не гарантирует,
что закоммиченная DLL соответствует C#-исходнику в `.ps1`. Это стоит
закрыть до следующего релиза (P2-1). Остальное — мелкие P3.
