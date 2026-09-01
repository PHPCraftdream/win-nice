# Code review: win-nice @ a14580c (22:29, полный независимый проход)

Дата: 2026-09-01

Объём: весь репозиторий в текущем состоянии (HEAD `1be67d5`, код — `a14580c`),
независимо от предыдущих отчётов в `docs/reviews/`. Отдельный фокус на том, чего
покоммитные ревью не смотрели целиком: сквозной lifecycle
install/upgrade/uninstall, поведение пары `.bat`/`.ps1` для каждого реального
потребителя (cmd.exe, PowerShell, Git Bash — оболочка Claude Code на Windows),
путь elevation, соответствие Pester-покрытия содержимому `bin/`, и способность
CI/publish workflow вообще пройти на реальных GitHub runner'ах (репозиторий ни
разу не пушился — CI ни разу не выполнялся).

Принятые владельцем tradeoff'ы (`cy`/`cx`, `realtime`, дублирование C#
launcher'а, `%`-порча в `.bat`, plain-copy skill для Codex) не рассматривались.

## Итог

Блокирующие проблемы есть. Все ключевые находки воспроизведены на этой машине
или подтверждены официальной документацией, а не выведены из чтения кода.

- **P0 — 1**: installer необратимо портит user `PATH` у любого пользователя, в
  чьём PATH есть не-ASCII символы (кириллическое имя профиля — типовой случай).
- **P1 — 3**: `cmd.exe /c` fallback ломается и заново открывает `&`-инъекцию,
  как только путь к `.bat`/`.cmd` содержит пробел; CI/publish Pester-шаг не
  может пройти на GitHub runner'ах (подхватывается Pester 5.9.0, синтаксис v3
  удалён); `npm test` падает на Node 18/20 (glob в `--test` не поддерживается).
- **P2 — 7**: `preuninstall` мёртв на всех поддерживаемых npm; bare-name
  `.ps1` не работает при дефолтной client execution policy; Git Bash (оболочка
  Claude Code) вообще не находит инструменты по bare name; `start /b` в пяти
  `.bat` отключает Ctrl+C у обёрнутой команды; не-elevated `admin` без нужды
  гонит `.exe` через cmd.exe (обоснование в README неверно); `cy`/`cx`-тесты
  способны запустить настоящий `claude.exe` с bypass-флагами; publish упадёт на
  `npm publish --provenance` без поля `repository`.
- **P3 — 8**.

Ни один из пунктов не повторяет уже исправленные замечания предыдущих отчётов.

## Findings

### P0 — installer портит user PATH, если в нём есть не-ASCII символы

Файлы: `install/paths.js:41-48` (`readUserPath`), `install/paths.js:50-56`
(`writeUserPath`), `install/install.js:45-52`, `install/uninstall.js:58-65`.

`readUserPath()` запускает `powershell -NoProfile -Command
"[Environment]::GetEnvironmentVariable('Path','User')"` и декодирует stdout
как `utf8`. Windows PowerShell 5.1 пишет перенаправленный stdout в OEM code
page консоли (`[Console]::OutputEncoding`; здесь CP437, на ru-RU — CP866), а не
в UTF-8. Любой не-ASCII символ декодируется неверно, после чего `install()` /
`uninstall()` записывают испорченную строку обратно через `writeUserPath()`
(эта сторона корректна: env block передаётся в UTF-16, так что мусор
сохраняется «честно»).

Воспроизведено тем же механизмом, что в `readUserPath` (Node 24, CP437):
вход `C:\Users\Марат\bin;C:\Users\José\bin` → выход
`C:\Users\?????\bin;C:\Users\Jos�\bin`.

Сценарий: пользователь с не-ASCII именем учётной записи (`C:\Users\Марат`).
Дефолтный user PATH Windows содержит `%USERPROFILE%\AppData\Local\Microsoft\WindowsApps`
(REG_EXPAND_SZ), установщик Node добавляет `%APPDATA%\npm`, .NET —
`%USERPROFILE%\.dotnet\tools`. `GetEnvironmentVariable(...,'User')` возвращает
их уже **раскрытыми**, с кириллицей в пути → после декодирования
`C:\Users\?????\AppData\Roaming\npm` → `npm install -g win-nice` перезаписывает
user PATH этим значением. Все записи относительно профиля перестают
резолвиться, включая npm-шимы `claude`/`codex` — ровно те, что оборачивают
`cy`/`cx`. Ошибки нет, installer печатает «Added ... to your PATH».
Восстановление только ручное; `npx win-nice uninstall` делает тот же round trip
ещё раз.

Рекомендация: читать значение из реестра без раскрытия и без OEM-кодировки —
`[Console]::OutputEncoding=[Text.Encoding]::UTF8;
(Get-Item HKCU:\Environment).GetValue('Path','','DoNotExpandEnvironmentNames')`
(или отдавать Base64); писать обратно `Set-ItemProperty -Type ExpandString` и
самостоятельно рассылать `WM_SETTINGCHANGE` (`[Environment]::SetEnvironmentVariable`
делает broadcast, но принудительно пишет REG_SZ — см. P3). Добавить
regression-тест, прогоняющий не-ASCII значение через тот же reader на
scratch-переменной (никогда не на `Path`).

### P1 — `cmd.exe /c` fallback ломается и заново открывает `&`-инъекцию, когда путь к target нуждается в кавычках

Файлы: `bin/cap.ps1:219-222`, `bin/idle.ps1:157-160`,
`bin/belownormal.ps1:157-160`, `bin/abovenormal.ps1:158-161`,
`bin/high.ps1:158-161`, `bin/realtime.ps1:162-165`, `bin/pint.ps1:217-220`,
`bin/admin.ps1:154-157` и `bin/admin.ps1:198`, `bin/cy.ps1:151-154`,
`bin/cx.ps1:151-154`.

Fallback собирает `"<System32>\cmd.exe" /c <escaped args>` — без `/S` и без
внешней пары кавычек вокруг всей команды. Правило cmd.exe для `/C` (см.
`cmd /?`): если текст после `/c` начинается с кавычки и не подходит под
«ровно две кавычки, без спецсимволов, файл существует», cmd удаляет **первую
и последнюю** кавычку всей строки. Первый токен берётся в кавычки всякий раз,
когда путь к `.bat`/`.cmd` содержит пробел (`C:\Program Files\...`,
`D:\my projects\...`, профиль с пробелом в имени) — и как только есть ещё хоть
один аргумент в кавычках, кавычки снимаются не с тех мест.

Воспроизведено: `powershell -File bin\cap.ps1 50 "<TEMP>\wn review N\t.bat" "A&B" plain`

```text
'D:\system_artefact\Temp\wn' is not recognized as an internal or external command,
'B' is not recognized as an internal or external command,
exit=1
```

Два отказа сразу: путь разрезан по пробелу, и `B` выполнен как отдельная
команда — тот самый `&`, ради нейтрализации которого существует весь слой
quoting, снова живой. То же с `idle.ps1` и аргументом `"A B"`. С аргументами
без кавычек (`plain`) всё работает — поэтому существующие тесты зелёные:
Pester-тест `still preserves cmd.exe metacharacters when the target is a .bat
file (fallback path)` (`test/win-nice.Tests.ps1:349-359`) кладёт target в
`$env:TEMP`; у пользователя с пробелом в профиле
(`C:\Users\John Smith\AppData\Local\Temp`) этот тест падает как есть.

Затрагивает и `admin.ps1:198` (не-elevated ветка: та же строка уходит в
elevated cmd.exe — инъецированный `B` выполняется с правами администратора).

Рекомендация (проверено здесь же): `cmd.exe /d /s /c "<escaped args>"` — с
`/S` cmd снимает ровно внешнюю пару, внутренние кавычки нетронуты:
`cmd.exe /d /s /c ""...\wn review N\t.bat" "A&B" plain"` → `BATOUT="A&B" plain`,
exit 0. Так делает libuv/Node (`/d /s /c`). Добавить regression-тест с
каталогом target'а, содержащим пробел, плюс хотя бы один аргумент в кавычках —
для `.ps1` fallback и для builder'а командной строки `admin`.

### P1 — Pester-шаг CI/publish не может пройти на GitHub runner'ах: подхватывается Pester 5.9.0, а suite написан в удалённом синтаксисе v3

Файлы: `.github/workflows/ci.yml:30-35`, `.github/workflows/publish.yml:44-48`,
`test/win-nice.Tests.ps1` (76 assertion'ов вида `Should Be` / `Should Match` /
`Should Not Match` / `Should Not Throw`, 0 вида `Should -Be`).

Readme образов `windows-latest` (и windows-2022, и windows-2025) перечисляют
`Pester: 3.4.0, 5.9.0`; 5.9.0 ставится через `Install-Module -Scope AllUsers`
(`Install-PowerShellModules.ps1`, версия зафиксирована в `toolset-2025.json`),
то есть в `C:\Program Files\WindowsPowerShell\Modules` — виден Windows
PowerShell 5.1, которым выполняется `shell: powershell`. Bare `Invoke-Pester`
автозагружает старшую версию → 5.9.0. Pester 5 breaking changes: «Legacy
syntax `Should Be` (without `-`) is removed». Каждый `It` падает →
`FailedCount -gt 0` → exit 1 → CI красный, а `publish.yml` никогда не доходит
до `npm publish`. Локально есть только 3.4.0, поэтому все предыдущие прогоны
это не показали.

Рекомендация: `Import-Module Pester -RequiredVersion 3.4.0` (или
`-MaximumVersion 3.99`) перед `Invoke-Pester` в обоих workflow и в команде из
README; либо мигрировать suite на синтаксис v5 и пиновать `-MinimumVersion 5`.
После этого — реально запушить и посмотреть на прогон.

### P1 — `npm test` падает на Node 18/20 в CI-матрице: glob в `--test` не раскрывается

Файлы: `package.json:23` (`"test": "node --test test/*.test.js"`),
`.github/workflows/ci.yml:15-16`.

npm на Windows запускает scripts через `cmd.exe`, который `*` не раскрывает.
Glob-паттерны в `--test` появились в Node 21: документация v22 — «one or more
glob patterns can be provided as the final argument(s)», документация v20 —
только «one or more paths can be provided», про glob ни слова. На Node 18/20
литерал `test/*.test.js` идёт в `stat` → «Could not find '...\test\*.test.js'»
→ exit 1. Локальный Node 24 это маскирует; комментарий в ci.yml «test the
floor, not just latest» описывает именно то, что не работает.

Рекомендация: `"test": "node --test"` (дефолтный discovery `**/*.test.js`
находит все шесть файлов и ничего лишнего) или перечислить файлы явно.

### P2 — `npm uninstall -g win-nice` ничего не удаляет: `preuninstall` мёртв на всех поддерживаемых npm

Файлы: `package.json:22`, `README.md:193-196`.

Документация npm ≥ 7 (`scripts.md`, «A Note on a lack of npm uninstall
scripts»): «`uninstall` lifecycle scripts are not implemented and will not
function». `engines.node >= 18` означает npm ≥ 8, то есть нет ни одной
поддерживаемой конфигурации, где `preuninstall` выполнится. После
`npm uninstall -g win-nice` пакет из global `node_modules` исчезает, а
`%LOCALAPPDATA%\win-nice\bin`, запись в PATH и manifest остаются; инструменты
продолжают работать осиротевшими. README:195-196 обещает обратное.

Рекомендация: убрать мёртвый `preuninstall`, в README документировать
`npx win-nice uninstall` как единственный путь удаления (работает, потому что
npx скачивает пакет заново; SKILL.md уже учит этому, README — нет). Опционально
печатать эту подсказку в конце `postinstall`.

### P2 — при дефолтной execution policy Windows-клиента ни один инструмент не запускается из PowerShell

Файлы: `README.md:42-46`, `skills/win-nice/SKILL.md:87-90`, `README.md:248-253`,
`bin/*.ps1`.

Задокументированный «безопасный» entry point (bare name из PowerShell → `.ps1`)
подчиняется execution policy сессии; `.bat`-обёртки передают
`-ExecutionPolicy Bypass`, но bare-name resolution их обходит. Дефолт Windows
10/11 client — `Restricted`. Воспроизведено:
`powershell -ExecutionPolicy Restricted -Command "& '...\idle.ps1' cmd /c 'echo hi'"` →
«File ...\idle.ps1 cannot be loaded because running scripts is disabled on this
system», exit -1. Тот же класс отказа, что у `npm.ps1`. На этой машине
`CurrentUser = RemoteSigned`, поэтому предыдущие ревью этого не видели.
Раздел Requirements говорит только «PowerShell is bundled with Windows».

Рекомендация: документировать `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`;
в `postinstall` (он и так вызывает PowerShell) проверять `Get-ExecutionPolicy`
и печатать предупреждение.

### P2 — оболочка Claude Code (Git Bash) не находит ни один инструмент по bare name

Файлы: `install/install.js:10-12`, `package.json:12-13`, `README.md:42-55`,
`skills/win-nice/SKILL.md:99-106`.

MSYS2/Git Bash — оболочка, через которую Claude Code выполняет команды на
Windows (эта сессия работает под `C:\Program Files\Git\bin\bash.exe`), — не
применяет PATHEXT. Воспроизведено: `PATH=<dir>:$PATH wnfoo a b` →
`bash: wnfoo: command not found` (127), при этом `wnfoo.bat a b` выполняется.
Целевая аудитория получает «command not found» на `cap 50 npm test` и должна
догадаться писать `cap.bat`; ни README, ни SKILL.md этого не говорят (SKILL.md
учит агента именно bare names). npm решает ту же проблему, поставляя рядом с
`.cmd`/`.ps1` sh-шим без расширения (`...\npm\claude`, `claude.cmd`,
`claude.ps1` — видно в `where.exe claude` на этой машине).

Рекомендация: поставлять `bin/<tool>` (без расширения, `#!/bin/sh`), который
делает `exec powershell -NoProfile -ExecutionPolicy Bypass -File
"$(dirname "$0")/<tool>.ps1" "$@"`; включить в `listSourceFiles()` и manifest.
Бонус: этот entry point не имеет `.bat`-порчи `%` (MSYS строит командную строку
дочернего процесса с корректным quoting). Документировать resolution по
оболочкам (cmd → `.bat`, PowerShell → `.ps1`, bash → шим) в README и SKILL.md.

### P2 — `.bat` entry points `idle`/`belownormal`/`abovenormal`/`high`/`realtime` отключают Ctrl+C у обёрнутой команды

Файлы: `bin/idle.bat:13`, `bin/belownormal.bat:13`, `bin/abovenormal.bat:13`,
`bin/high.bat:13`, `bin/realtime.bat:14`.

`start "" /low /b /wait %*` — `start /?`: «B Start application without
creating a new window. The application has ^C handling ignored. Unless the
application enables ^C processing, ^Break is the only way to interrupt».
Дочерний процесс создаётся с CREATE_NEW_PROCESS_GROUP и передаёт состояние
«Ctrl+C отключён» всему дереву. Сценарий: `idle npm run build` из cmd.exe (или
любого PATHEXT-launcher'а) → Ctrl+C → cmd спрашивает «Terminate batch job
(Y/N)?» → Y → batch завершается, а сборка продолжает работать в той же консоли,
Ctrl+C её не берёт. У `.ps1`-близнеца (обычный `CreateProcess` с priority
flags, `idle.ps1:137-138`) этого нет; у `cap.bat`/`pint.bat`/`admin.bat`
(`powershell -File`) — тоже нет. Один инструмент, два entry point'а, разная
семантика прерывания.

Рекомендация: сделать пять priority-`.bat` такими же тонкими шимами
`powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0<name>.ps1" %*`, как
остальные (заодно уходят quirks `start` с title и разбором своих switch'ей).
Цена — старт PowerShell + компиляция `Add-Type` и на cmd-пути; для
cap/pint/admin это уже так.

### P2 — не-elevated `admin` без нужды гонит `.exe` через cmd.exe; обоснование в README неверно

Файлы: `bin/admin.ps1:183-204`, `README.md:157-164`.

README: «via ShellExecute, which ... always goes through the cmd.exe /c
fallback path, never the direct-launch one — elevation has no direct-launch
equivalent to use». Это не так: ShellExecuteEx с verb `runas` принимает
`lpFile` + `lpParameters`; `Start-Process -FilePath $Command[0] -ArgumentList
<argv-quoted rest> -Verb RunAs` запускает `.exe` напрямую, его CRT разбирает
параметры ровно как на direct-`CreateProcess` пути — без cmd.exe, без
раскрытия `%`, без quote-stripping из P1. cmd.exe нужен только для
`.bat`/`.cmd`/builtin. Последствия сегодня: `admin some.exe "100%"` отклоняется
(`admin.ps1:190-195`), а `admin "C:\Program Files\x\setup.exe" "/dir=C:\My Apps"`
попадает в quote-stripping — с правами администратора. Дополнительно: окно
elevated `cmd.exe /c` закрывается сразу по завершении команды, вывод прочитать
нельзя (README говорит «opens its own console window», но не что оно исчезает).

Рекомендация: повторить решение elevated-ветки — `.exe`/bare name не `.bat` →
`Start-Process -FilePath <target> -ArgumentList <ArgvQuote'd> -Verb RunAs`;
`.bat`/`.cmd` → `cmd.exe /d /s /c "..."` с `%`-проверкой. Поправить
README/SKILL.md.

### P2 — Pester-тесты `cy`/`cx` могут запустить настоящий агент с bypass-флагами, если `claude.exe`/`codex.exe` есть на PATH

Файлы: `test/win-nice.Tests.ps1:587-609` (`Test-FakeLauncher`, PATH
дописывается в начало на 598-599), `test/win-nice.Tests.ps1:655-662`
(sequential test), `bin/cy.ps1:130-132`.

Фейк — `claude.bat`, его находит только PATHEXT-поиск cmd.exe на fallback-пути.
Direct-путь `cy.ps1` — `CreateProcess(null, "claude ...")` — дописывает `.exe`
и ищет по **всему** унаследованному PATH первым; в fake-каталоге `claude.exe`
нет, поэтому побеждает настоящий `claude.exe` дальше по PATH. Native-installer
Claude Code (сейчас рекомендуемый способ установки) кладёт `claude.exe` в
`%USERPROFILE%\.local\bin` — этот каталог уже есть в PATH данного пользователя
(сегодня там `claude.exe` нет; тесты проходят благодаря npm-шиму
`claude.cmd`). После перехода на native install два `cy`-теста запустят
настоящий `claude --dangerously-skip-permissions -p "A&B"` / `100%OFF` (реальный
prompt, реальные токены, permissions обойдены), а sequential test — `claude
--dangerously-skip-permissions` без аргументов: интерактивная сессия внутри
тестового прогона (зависание или TTY-ошибка). Комментарий на 582-586 говорит,
что этого не должно случаться никогда.

Рекомендация: в обоих helper'ах задавать дочернему процессу PATH только
`$fakeDir;$env:SystemRoot\System32;$env:SystemRoot\System32\WindowsPowerShell\v1.0`
(не prepend), и сначала проверять `$r.Output -ne $null`, чтобы запуск не того
бинарника падал быстро.

### P2 — `publish.yml` упадёт на `npm publish --provenance`: нет поля `repository`

Файлы: `package.json` (нет `repository`/`homepage`/`bugs`),
`.github/workflows/publish.yml:50-51`, `README.md:3-6` (badges →
`github.com/phpcraftdream/win-nice`).

Документация npm (Generating provenance statements): «Ensure your
package.json is configured with a public repository that matches
(case-sensitive) where you are publishing with provenance from». Registry
отклоняет provenance-публикацию с отсутствующим или несовпадающим
`repository.url`. Даже после исправления двух P1 по CI tag-workflow остановится
здесь.

Рекомендация: добавить `"repository": {"type": "git", "url":
"git+https://github.com/phpcraftdream/win-nice.git"}` (точные owner/name
реального репозитория), плюс `homepage`/`bugs`.

## Улучшения P3

### P3 — cmd.exe fallback стоит запускать с `/d` (и `/V:OFF`), особенно на elevated-пути

Файлы: те же, что в P1 про quote-stripping; `bin/admin.ps1:198`.

Без `/d` cmd.exe выполняет `HKCU\Software\Microsoft\Command Processor\AutoRun`
(доступен на запись пользователю) при каждом fallback-запуске, а на
не-elevated пути `admin` — с правами администратора. Без `/V:OFF` при
`EnableDelayedExpansion=1` в реестре аргументы `!VAR!` раскрываются — тот же
класс риска, что `%`, но fail-closed проверка его не ловит. UAC — не security
boundary, но `/d /s /c` — дешёвый стандарт (libuv). Сделать вместе с P1.

### P3 — `%`-проверка в `admin.ps1` кидает ошибку на не-строковых аргументах из PowerShell-сессии

Файл: `bin/admin.ps1:190-195`.

Bare-name `admin foo 100` из не-elevated PowerShell: `$args[1]` — `[int]`;
`$a.Contains('%')` → «Method invocation failed because [System.Int32] does not
contain a method named 'Contains'» красным, цикл продолжается (проверено:
последующие `%`-аргументы всё ещё ловятся), затем UAC prompt. Косметика, но
выглядит как сломанная защита. Исправление: `"$a".Contains('%')` или
`$a -like '*%*'`. C#-копии приводят через `[string[]]` и не затронуты.

### P3 — round trip PATH превращает REG_EXPAND_SZ в REG_SZ

Файлы: `install/paths.js:41-56`.

`[Environment]::GetEnvironmentVariable('Path','User')` возвращает раскрытое
значение; `SetEnvironmentVariable(...,'User')` пишет REG_SZ. Каждая запись
вида `%USERPROFILE%\...` в user PATH после первой установки (и повторно после
удаления) становится замороженным литералом. У этого пользователя `Path` уже
REG_SZ без `%`-записей (видимо, сплющен раньше чем-то другим), терять здесь
нечего; свежий профиль Windows хранит
`%USERPROFILE%\AppData\Local\Microsoft\WindowsApps` как ExpandString.
Исправлять вместе с P0 (сырое чтение, запись `-Type ExpandString`, broadcast
`WM_SETTINGCHANGE`).

### P3 — job/priority «прилипают» к демонам, которые обёрнутая команда оставляет после себя (документация)

Файлы: `README.md:93-116`, `skills/win-nice/SKILL.md:40-55`.

«The only way out is CREATE_BREAKAWAY_FROM_JOB» верно для дерева, но типовые
build-стеки оставляют серверы: `dotnet build` (VBCSCompiler.exe, MSBuild node
reuse), gradle daemon, `node`-watchers. `cap 30 dotnet build` ограничивает
compiler server 30% на всё время его жизни (последующие сборки без cap
замедлены); `pint 2` его прибивает к 2 потокам; и наоборот, демон, стартовавший
вне job, никогда не накрывается. Не баг — свойство Job Objects, — но для
сценария «AI-агент гоняет сборки» стоит одного абзаца плюс подсказок
`-p:UseSharedCompilation=false` / `--no-daemon`.

### P3 — `postinstall` при обновлении не удаляет инструменты, выброшенные в новой версии; uninstall оставляет корневой каталог

Файлы: `install/install.js:33-43`, `install/uninstall.js:55-56`,
`install/cli.js:30-33`.

`npm install -g win-nice@new` выполняет только `postinstall` → `install()`:
копирует текущий набор и перезаписывает manifest. Инструмент, удалённый в новой
версии, остаётся в `bin/` навсегда — его нет в manifest, manifest-based
uninstall его пропускает, а fallback-скан включается только при отсутствии
manifest. `cli reinstall` делает правильно (uninstall+install); `postinstall`
должен делать то же (сначала удалить файлы из предыдущего manifest).
`uninstall()` удаляет `bin`, но оставляет пустой `%LOCALAPPDATA%\win-nice`.

### P3 — guard-тест затронет реальный PATH, если guard регрессирует

Файл: `test/install-uninstall.test.js:148-157`.

`install()` вызывается с дефолтным `updatePath = true` и без `WIN_NICE_HOME`.
Проверяется, что guard на source checkout вернёт `null`; при регрессии guard'а
(например, прогон тестов из `npm pack`-копии без `.git`) тест установит пакет
в настоящий `%LOCALAPPDATA%\win-nice` и перепишет настоящий user PATH (с
багом P0). Передавать `{ updatePath: false }`.

### P3 — `.bat` закоммичены и выгружены с LF; нет `.gitattributes`

`git ls-files --eol`: все `bin/*.bat` — `i/lf w/lf`. cmd.exe терпит LF для
используемых здесь конструкций (тесты проходят), но line endings опубликованного
tarball'а зависят от `core.autocrlf` publish-runner'а, а любой будущий
`goto`/`call :label` под LF ломается. Добавить `.gitattributes` с
`*.bat text eol=crlf`, `*.ps1 text eol=crlf` (Windows PowerShell 5.1 читает
файлы без BOM как ANSI; все `.ps1` сегодня чистый ASCII — так и держать).

### P3 — пробелы в покрытии относительно `bin/`

- `abovenormal.bat`/`high.bat`/`realtime.bat`/`belownormal.bat` не имеют теста
  на приоритет и exit code (`test/win-nice.Tests.ps1:190-196` проверяет только
  usage); их `start /<class>` — отдельный код от `.ps1`-близнецов.
- Тест на type collision (`test/win-nice.Tests.ps1:643-681`) не включает
  `cap`/`pint`/`admin`; их классы — generic `Capper`/`Pinner`/`Runner`. `Runner`
  — правдоподобное имя для чужого `Add-Type` в профиле пользователя, что сломает
  bare-name `admin` в такой сессии. Переименовать в
  `CapLauncher`/`PintLauncher`/`AdminLauncher` и добавить в цикл.
- Временные файлы удаляются только на success-пути (нет `finally`/`AfterEach`):
  в `%TEMP%` этой машины прямо сейчас лежат 19 `win-nice-pester-*` от прежних
  упавших прогонов.
- `WIN_NICE_NO_PATH` и `WIN_NICE_SKILL_HOME` не документированы (README
  упоминает только `WIN_NICE_HOME`).

## Что проверено

- Прочитаны целиком: все 24 файла `bin/`, 6 файлов `install/`, 7 тестов,
  оба workflow, `package.json`, README, SKILL.md, 5 предыдущих отчётов и
  checkpoint.
- `git ls-files --eol` (везде LF), grep не-ASCII в `bin/` (нет), версии: Node
  24.12.0, npm 11.13.0, chcp 437, локально только Pester 3.4.0, execution
  policy `CurrentUser=RemoteSigned`, `claude`/`codex` — npm `.cmd`-шимы,
  `%USERPROFILE%\.local\bin` в PATH без `claude.exe`.
- Воспроизведено на этой машине: quote-stripping `cmd.exe /c` с пробелом в пути
  target'а (для `cap.ps1` и `idle.ps1`) и работоспособность фикса `/d /s /c`;
  кодировочный round trip `readUserPath` с кириллицей/`é`; отказ Git Bash
  находить `.bat` по bare name; отказ `.ps1` при `-ExecutionPolicy Restricted`;
  семантика `foreach`+`.Contains` на `[int]`; текст `start /?` про `^C`;
  тип значения `HKCU:\Environment\Path`.
- Подтверждено документацией: npm `scripts.md` (uninstall scripts «will not
  function»), readme образов windows-2022/2025 и `toolset-2025.json` (Pester
  3.4.0 + 5.9.0, `Install-Module -Scope AllUsers`), Pester «Breaking changes in
  v5» (legacy `Should Be` removed), Node v20 vs v22 `test.html` (paths vs glob
  patterns), npm «Generating provenance statements» (`repository` обязателен).
- Проверено и **не** вынесено в findings (работает корректно): арифметика
  affinity-маски `pint` для 3/52/53/54/60/63 остаётся `UInt64` без потери
  точности; bare-name вызов `cap 50 ...` / `pint 2 ...` с `[int]`-аргументом
  из PowerShell возвращает exit code обёрнутой команды; layout структур
  `JOBOBJECT_*`/`STARTUPINFO` и константы priority class; логика `isInsideDir`;
  marker-контракт skill install/uninstall; частичное тестирование target'а
  `t.bat.` (trailing dot) в обход `isBatOrCmd` зависло (похоже на системный
  «Open with» prompt) и не продолжалось — надуманный ввод.
- Не выполнялось: полные прогоны `npm test`/Pester (результаты для этого же
  кода зафиксированы в отчёте 18:38, код с тех пор не менялся); интерактивные
  UAC-ветки `admin`/`uiup`; elevated `realtime`.

## Рекомендуемый порядок

1. P0 — кодировка/сырое чтение PATH (вместе с P3 про ExpandString), до любой
   публикации.
2. P1 — `cmd.exe /d /s /c "..."` во всех десяти launcher'ах и в не-elevated
   `admin`, плюс regression-тест с пробелом в пути target'а.
3. P1 — CI: пин Pester 3.4.0 (или миграция на v5), `node --test` без glob;
   затем реальный push и просмотр прогона.
4. P2 — поле `repository`; убрать `preuninstall`, поправить README.
5. P2 — sh-шимы для bash; предупреждение/документация про execution policy;
   priority-`.bat` → тонкие `powershell -File` шимы (Ctrl+C).
6. P2 — прямой ShellExecute для `.exe` в не-elevated `admin`; изоляция PATH в
   `cy`/`cx`-тестах.
7. P3 по мере сил.
