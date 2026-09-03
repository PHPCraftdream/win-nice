# Полное release-review, раунд 14: win-nice 0.2.0

Дата проверки: 2026-09-03 10:55 (Europe/Berlin)

Проверенный commit: `daa6674` (`master`), локальный `master` опережает
`origin/master` (`276cf35`) на 21 коммит.

База release delta: тег `v0.1.0` (`cd138b4`), единственный тег на origin и
единственная версия на npm (`npm view win-nice versions` -> `0.1.0`).

Объём: весь репозиторий, независимо от предыдущих 13 раундов. Runtime всех
12 инструментов (36 launcher files), embedded C# всех 11 native launchers,
installer/upgrade/skill path, фактический npm tarball, реальный upgrade
опубликованного `0.1.0` до текущего tarball, CI/publish workflows, документация,
Node/Pester/Git Bash suites, `release-check` и отдельно два не проверенных ранее
исправления в `daa6674`.

## Итог

**Найдено 0 P0, 0 P1, 1 P2, 5 P3. Тег `v0.2.0` ставить после закрытия P2-1
(правка в несколько строк плюс regression test) и двух doc-правок из P3-1/P3-2;
остальные P3 не блокируют.**

Product runtime, tarball и upgrade-путь исправны и подтверждены заново:

- Node suite 63/63 (в этом окружении Git Bash первый в PATH, поэтому 7 shim
  тестов выполнены, а не пропущены), Pester 203 passed / 0 failed / 3 skipped;
- `npm run release-check` проходит полностью, не оставляет tarball/temp dirs и
  не трогает реальные `~/.claude/skills` / `~/.agents/skills` (mtime обеих
  копий `2026-09-03 09:59:58` до и после всех прогонов);
- оба исправления раунда 13 в `scripts/release-check.js` подтверждены
  fault-injection'ом: tarball удаляется при сбое до install, gate реально
  падает на сломанных `capt`/`capm`;
- реальный upgrade опубликованного `win-nice@0.1.0` до текущего tarball
  удаляет 6 legacy `cap`/`pint` файлов, ставит 6 `capc`/`capt`, сохраняет
  чужой unmarked файл, пишет manifest `0.2.0` с 36 файлами и обновляет ранее
  установленный managed skill (byte-equal repo source), в том числе при
  удалённом manifest.

Единственный P2 - не регрессия этого цикла, а старая асимметрия installer CLI:
guard «source checkout - skipping real install» стоит только в `install()`, а
`uninstall`/`reinstall` его не имеют, поэтому `npx win-nice reinstall`
из клона репозитория (npx резолвит локальный `install/cli.js`, проверено)
удаляет реальный `%LOCALAPPDATA%\win-nice` и PATH-запись, а затем «пропускает»
установку.

## Что проверено

| Область | Результат |
| --- | --- |
| Node test suite (Node 24.12.0 / npm 11.13.0) | 63 passed, 0 failed, 0 skipped, 3.4 s |
| Pester 3.4.0 / Windows PowerShell 5.1.19041, non-admin | 203 passed, 0 failed, 3 skipped, 206 total, 141.8 s; 0 leftover `win-nice-pester-*` |
| `npm run release-check` | passed: tarball 48 files / 39,997 bytes; manifest 0.2.0; 36 launchers, 0 legacy; smoke capc+capt+capm exit 7; CHANGELOG heading и compare-base `v0.1.0` |
| Sandbox `npm test` / `release-check` | mtime `C:\Users\Computer\.claude\skills\win-nice\SKILL.md` и `...\.agents\skills\win-nice\SKILL.md` не изменился (`09:59:58`); worktree чист, `*.tgz` в repo нет |
| Fix `daa6674` #1 (cleanup до install) | `TEMP` = обычный файл: pack ok, `mkdtempSync` ENOENT в `release-check.js:65`, exit 1, tarball удалён |
| Fix `daa6674` #2 (smoke capt/capm) | копия репо со сломанными `capt.ps1`/`capm.ps1` (`exit 0`): 2 FAIL, exit 1, tarball и 3 temp dirs удалены |
| Реальный upgrade `0.1.0` (npm) -> текущий tarball | legacy 6/6 -> 0/6, `capc`/`capt` 6/6, foreign сохранён, manifest 0.2.0 / 36 files, skill refresh SHA-256 = repo; вариант без manifest - то же, unmarked файл сохранён |
| `npm pack --dry-run --json` vs `files` whitelist | 48 entries, 218,368 bytes unpacked; ровно bin (36) + install (6) + skills/SKILL.md + README/CHANGELOG/2 LICENSE + package.json; нет `test/`, `scripts/`, `docs/`, `.github/` |
| Опубликованный `win-nice-0.1.0.tgz` | 33 bin files (11 x 3); extensionless shims с CRLF (см. P3-3) |
| Syntax/static | `node --check` 14 JS, PowerShell AST 14 PS1, 1 JSON - 0 ошибок |
| Line endings / markers | 24 `.bat`/`.ps1` CRLF, 12 shims LF, marker в каждом из 36; `core.autocrlf=input` на этой машине |
| Workflows | `ci.yml`/`publish.yml` SHA-pins совпадают и подтверждены через `git ls-remote`: `actions/checkout@11d5960a` = `v4.4.0`, `actions/setup-node@49933ea5` = `v4.4.0`; `fetch-depth: 0` в publish есть |
| Docs | локальные Markdown links 6/6 существуют; stale `cap`/`pint` вне `docs/` и секции 0.1.0 CHANGELOG - 0; два doc-расхождения (P3-1, P3-2) |
| Git integrity | `git fsck --no-dangling` чисто; `git diff --check v0.1.0..HEAD` - 2 trailing-whitespace в `docs/reviews/...round-13...md:3-4` (markdown hard break, косметика) |

Node 18/20/22 matrix локально не повторялась (других версий Node на машине
нет); `install/` и `test/` не менялись с `fbc7f8c` (раунд 11), где раунд 13
получил 56/0/7 на каждой версии, а CI matrix повторит это при push.
`npm run test:elevated` не запускался - требует интерактивного UAC.

## Findings

### P2-1: `uninstall`/`reinstall` из source checkout обходят guard и сносят реальную установку

Файлы: `install/cli.js:27-33`, `install/install.js:21-23,56-62`,
`install/uninstall.js:27-74`.

`install()` при отсутствии `WIN_NICE_HOME` и наличии `.git` рядом с `install/`
печатает «Running from a source checkout - skipping real install» и выходит -
guard задуман, чтобы работа в клоне не трогала реальный
`%LOCALAPPDATA%\win-nice`. Но `uninstall()` такого guard не имеет, а
`reinstall` в `cli.js` - это безусловный `uninstall()` + `install()`. Итог для
`reinstall` из клона: реальные файлы и PATH-запись удалены (`uninstall.js:50-71`),
установка пропущена, exit code 0.

Это не гипотетический путь: README и SKILL.md документируют
`npx win-nice reinstall` как штатную команду, а `npx` внутри клона резолвит
именно локальный `install/cli.js` (без `node_modules` и без обращения к
registry):

```text
D:\dev\win-nice> npx --no win-nice status
win-nice 0.1.0 installed at 2026-09-02T12:09:52.821Z
bin dir: C:\Users\Computer\AppData\Local\win-nice\bin
```

Воспроизведение без касания реальной установки (временный `LOCALAPPDATA`,
`WIN_NICE_NO_PATH=1`, `WIN_NICE_HOME` не задан):

```text
--- seed a fake real install under LOCALAPPDATA=D:\...\wn14-lad
win-nice 0.2.0 installed: abovenormal, abovenormal.bat, abovenormal.ps1, admin, ...
files before: 36
--- reinstall from the source checkout with NO WIN_NICE_HOME
Running from a source checkout - skipping real install. Set WIN_NICE_HOME to force a target directory, or install the published package.
files after: 0
install root contents after: 0 entries
```

Без `WIN_NICE_NO_PATH` дополнительно удаляется пользовательская PATH-запись.
Поведение есть и в `0.1.0`, то есть это не регрессия релиза, но исправление
тривиально, а последствие - тихая потеря установки - хуже, чем у прошлых P2.

Исправление: применить тот же guard к `uninstall` и `reinstall` в `cli.js`
(или внутри `uninstall()`), чтобы без `WIN_NICE_HOME` из source checkout они
печатали то же сообщение и ничего не удаляли; добавить regression test в
`test/cli.test.js` по схеме выше (временный `LOCALAPPDATA` + `WIN_NICE_NO_PATH`,
без `WIN_NICE_HOME`, `reinstall`, файлы должны остаться). Явный `uninstall` из
клона можно оставить разрешённым, но тогда `reinstall` обязан отказываться
до `uninstall()`, если `install()` заведомо будет no-op.

### P3-1: README и publish.yml всё ещё описывают smoke только для `capc`

Файлы: `README.md:484`, `.github/workflows/publish.yml:79-80`.

После `daa6674` gate выполняет три smoke (`capc 50`, `capt 1`, `capm 50`), а
документация по-прежнему говорит «a `capc` exit-code smoke test» и
«smoke-tests capc's exit code». Обновить оба места одной строкой.

### P3-2: документация занижает число cases, пропускаемых под elevation (3 vs 4)

Файлы: `README.md:473-475`, `CONTRIBUTING.md:19-21`,
`test/run-elevated.ps1:6-7,71`; факт: `test/win-nice.Tests.ps1:1130,1140,1223,1974`.

Все три документа обещают «a different, disjoint 3 cases ... Skip under
elevation», но `-Skip:$script:isAdminRunner` стоит на четырёх `It`
(два admin `%`-checks, admin builtin routing и `run-elevated.ps1`
-SelfElevated guard). Elevated run из checklist покажет `Skipped: 4`
(5 на 1-процессорной машине из-за `capt 1 capt 2`), и человек, сверяющий
цифры с README, решит, что что-то пропущено лишнее. Elevated-only группа
действительно из 3 cases (`:579,1105,1110`) - это совпадает с 3 skip
normal run. Поправить число в трёх местах.

### P3-3: `.gitattributes` не фиксирует LF для extensionless shims - CI-артефакт отличается от локального `npm pack`

Файлы: `.gitattributes:1-2`, `bin/<tool>` (12 файлов), `scripts/release-check.js:121-144`.

`.gitattributes` пинит только `*.bat`/`*.ps1` на CRLF. У extensionless
`#!/bin/sh` shims атрибута нет, поэтому checkout с `core.autocrlf=true`
(default Git for Windows, и, судя по артефакту, GitHub windows runner) даёт
CRLF. Это не теория - опубликованный `0.1.0` уже такой:

```text
bin/idle: POSIX shell script, ASCII text executable, with CRLF line terminators
0000000   #   !   /   b   i   n   /   s   h  \r  \n
```

Локальный `npm pack` (здесь `core.autocrlf=input`) даёт LF, а `release-check`
в publish job установит CRLF-версию и никогда её не исполнит (smoke только
`.ps1`). Функционально не страшно: MSYS bash проверен с CRLF-копией shim -
exit code 7 через shebang и через bare name в PATH, аргументы
`/c /d C:\Windows "with space" & % ""` доходят byte-exact. Но байты
опубликованного `0.2.0` будут отличаться от локально проверенного tarball, а
любой будущий sh-конструкт, чувствительный к `\r`, сломается только в CI.

Исправление: добавить в `.gitattributes` строку `bin/* text eol=lf` выше
CRLF-правил (позже стоящие `*.bat`/`*.ps1` перекроют её) и, желательно,
`* text=auto`; опционально добавить в `release-check` один shim smoke через
`bash` (есть на windows runners).

### P3-4: smoke в `release-check` зависит от execution policy и глушит диагностику

Файл: `scripts/release-check.js:134`.

`execFileSync('powershell', ['-NoProfile', '-File', ps1, ...], { stdio: 'ignore' })`
не передаёт `-ExecutionPolicy Bypass`, в отличие от `.bat`/shims. На машине с
policy `Restricted` (default Windows client) gate упадёт с
`smoke test: ... exited 1, expected 7` без единой строки stderr, хотя артефакт
исправен; здесь `CurrentUser=RemoteSigned`, поэтому прошло. GitHub runners не
Restricted, publish это не затрагивает. Добавить `-ExecutionPolicy Bypass` и
`stdio: ['ignore', 'ignore', 'inherit']` (или собирать stderr в сообщение FAIL).

### P3-5: `ci.yml` не запускает `release-check` - регрессия gate видна только в publish job

Файлы: `.github/workflows/ci.yml:20-49`, `.github/workflows/publish.yml:73-81`.

Gate исполняется только в tag-triggered publish. Если он сломается (как в
раундах 12-13: shallow checkout, `shell: true`), это выяснится после push
тега, а повтор требует нового тега или ручного перезапуска. Добавить шаг
`npm run release-check` хотя бы на одной ноге matrix (нужен
`fetch-depth: 0` для `git tag --list`).

## Проверка исправлений раунда 13

Оба изменения `daa6674` затрагивают только `scripts/release-check.js`
(`git diff --stat d8e3db7..HEAD`: этот файл и отчёт раунда 13).

**P3-1 раунда 13 (cleanup до внешнего `try/finally`)** - подтверждено.
`tarballPath`/`home`/`installPrefix`/`skillHome` инициализируются `null`
(`:50-53`), `npm pack` и три `mkdtempSync` внутри `try` (`:55-72`), `finally`
удаляет только созданное (`:146-151`). Fault injection: `TEMP`/`TMP` указывают
на обычный файл:

```text
release-check: npm pack...
release-check: ok - packed win-nice-0.2.0.tgz (48 files, 39997 bytes)
Error: ENOENT: no such file or directory, mkdtemp 'D:\...\wn14-temp-is-a-file\win-nice-release-check-home-XXXXXX'
    at Object.<anonymous> (D:\dev\win-nice\scripts\release-check.js:65:15)
EXIT_WITH_FILE_TEMP=1
ls *.tgz -> (пусто)
```

До `daa6674` tarball остался бы в корне репозитория. Первая попытка с
несуществующей директорией не сработала как инъекция: Node сам создал её под
`node-compile-cache/` - особенность окружения, не репозитория.

**P3-2 раунда 13 (smoke только `capc`)** - подтверждено. В `git archive`-копии
репозитория с собственным `git init` + `v0.1.0` последняя строка
`capt.ps1`/`capm.ps1` заменена на `exit 0`:

```text
release-check: ok - smoke test: capc (installed from the tarball) propagates the wrapped exit code
release-check: FAIL - smoke test: "capt 1 cmd.exe /c exit 7" exited 0, expected 7
release-check: FAIL - smoke test: "capm 50 cmd.exe /c exit 7" exited 0, expected 7
release-check: ok - CHANGELOG.md's latest heading matches package.json (0.2.0)
release-check: ok - CHANGELOG.md's compare-base tag v0.1.0 exists
release-check: FAILED - see above
BROKEN_COPY_EXIT=1
```

После FAIL в копии не осталось ни `*.tgz`, ни `win-nice-release-check-*` в
`%TEMP%`. Остаток той же рекомендации раунда 13: `.bat` и extensionless
варианты, а также 8 priority/cy/cx/admin `.ps1` из tarball gate по-прежнему
не исполняет (см. также P3-3).

## Выполненные проверки

- `npm test`, Node 24.12.0 / npm 11.13.0: **63 passed, 0 failed, 0 skipped**
  (7 Git Bash shim tests выполнены реально - `bash` здесь MSYS);
- Pester 3.4.0, Windows PowerShell 5.1.19041.7548, non-admin session,
  `$ErrorActionPreference='Continue'`: **203 passed, 0 failed, 3 skipped,
  206 total, 141.76 s**; три skip - elevated-only admin cases; leftover
  `win-nice-pester-*` в `%TEMP%`: 0;
- `npm run release-check`: **all checks passed** (48 files, 39,997 bytes;
  manifest 0.2.0; 36/36 launchers, 0 legacy; smoke capc/capt/capm; CHANGELOG
  heading; compare-base tag); tarball и 3 temp roots удалены;
- sandbox-контроль: SHA-256 и mtime `C:\Users\Computer\.claude\skills\win-nice\SKILL.md`
  и `C:\Users\Computer\.agents\skills\win-nice\SKILL.md` до/после `npm test`,
  `release-check`, Pester и всех инъекций - без изменений
  (`2026-09-03T09:59:58`, hash = repo source); отдельно подтверждено, что
  `node install/cli.js install` из checkout без `WIN_NICE_HOME` выходит по
  guard до `updateInstalledSkill()` и mtime не меняет;
- fault injection #1 (TEMP = файл): exit 1, tarball удалён; #2 (сломанные
  capt/capm в копии): 2 FAIL, exit 1, без утечек;
- реальный upgrade: `curl` опубликованного `win-nice-0.1.0.tgz` (33 bin files,
  manifest 0.1.0) -> `npm install` в изолированный prefix/`WIN_NICE_HOME`/
  `WIN_NICE_SKILL_HOME`, `skill install` из 0.1.0 (5 строк с `pint`), foreign
  `not-ours.txt` в bin -> `npm install` текущего tarball поверх:
  **legacy 6/6 -> 0/6, capc/capt 6/6, bin = 36 + 1 foreign, manifest 0.2.0 /
  36 files / 0 legacy, skill `pint` 0 / `capt` 5, SHA-256 = repo**; повтор с
  удалённым manifest и unmarked `keep.txt`: legacy 0/6, new 6/6, `keep.txt`
  сохранён;
- `npm pack --dry-run --json`: `win-nice@0.2.0`, 48 entries, 39,997 bytes,
  218,368 unpacked; список сверен с `files` whitelist;
- `node --check`: 14 JS, 0 ошибок; PowerShell AST: 14 PS1, 0 ошибок; JSON: 1;
- `git ls-files --eol`: 24 `.bat`/`.ps1` + 2 `test/*.ps1` CRLF, 12 shims LF;
  `.gitattributes` покрывает только `*.bat`/`*.ps1`;
- diff launcher-вариантов: `belownormal`/`abovenormal`/`high` отличаются от
  `idle.ps1` только именем класса, usage, константой и комментарием; `cx` от
  `cy` - целью и флагом; 12 shims byte-identical по шаблону; C# всех 11
  launchers: единый `try/finally` владения handle, проверка
  `ResumeThread`/`WaitForSingleObject`/`GetExitCodeProcess`/
  `AssignProcessToJobObject`/`SetInformationJobObject`, `AllocHGlobal`/
  `FreeHGlobal` в `try/finally`, `CpuRate = percent*100`, affinity
  `(1<<n)-1` с потолком 63, `JOB_OBJECT_LIMIT_JOB_MEMORY` - без новых находок;
- CRLF-копия `bin/idle`: exit 7 через `bash <file>`, через shebang по
  абсолютному пути и bare name в PATH; аргументы `/c /d C:\Windows
  "with space" & % ""` byte-exact;
- workflows: SHA pins подтверждены `git ls-remote` (обе `v4.4.0`);
  `publish.yml` - `fetch-depth: 0`, pinned npm 11.5.1, `release-check` перед
  `npm publish --provenance --access public`; `ci.yml` без `release-check`;
- документация: 6 локальных Markdown links существуют; grep stale `cap`/`pint`
  вне `docs/` и секции 0.1.0 CHANGELOG - 0; `test/docs-sync.test.js` зелёный;
- `git fsck --no-dangling` чисто; `git diff --check v0.1.0..HEAD` - только два
  markdown hard-break в отчёте раунда 13; `git status --short` пуст до
  записи этого файла;
- registry/origin: `npm view win-nice versions` = `0.1.0`; `git ls-remote
  --tags origin` = только `v0.1.0`; `HEAD` впереди origin на 21 коммит.

Особенность окружения, не репозитория: npm здесь имеет `before=2026-08-27`,
из-за чего `npm pack win-nice@0.1.0` отвечает `notarget`; tarball 0.1.0 взят
через `npm view ... dist.tarball` + `curl`.

## Release checklist

- [ ] Закрыть P2-1 (guard для `uninstall`/`reinstall` из source checkout +
      regression test в `test/cli.test.js`).
- [ ] Поправить P3-1 (README/publish.yml: smoke capc+capt+capm) и P3-2
      (3 -> 4 cases, пропускаемых под elevation) - обе правки в одном коммите
      с P2-1.
- [ ] Решить по P3-3/P3-4/P3-5 (`.gitattributes` `bin/* text eol=lf`,
      `-ExecutionPolicy Bypass` + stderr в smoke, `release-check` в `ci.yml`);
      не блокируют тег.
- [ ] После правок повторить `npm test`, normal Pester и `npm run release-check`.
- [ ] Из trusted checkout выполнить `npm run test:elevated` (один UAC prompt),
      ожидать 3 admin cases активированы и 4 (не 3) skipped.
- [ ] Убедиться, что дата `## [0.2.0] - 2026-09-03` совпадает с днём тега;
      при переносе - обновить.
- [ ] Push `master`, дождаться зелёной CI matrix (Node 18/20/22/24).
- [ ] Проверить Trusted Publisher / provenance settings на npmjs.com.
- [ ] Annotated tag `v0.2.0` на проверенном commit, дождаться publish workflow;
      после публикации сверить `npm view win-nice version` = `0.2.0` и 48 files
      в опубликованном tarball (extensionless shims будут CRLF, пока не закрыт
      P3-3 - это ожидаемо и функционально безвредно).
