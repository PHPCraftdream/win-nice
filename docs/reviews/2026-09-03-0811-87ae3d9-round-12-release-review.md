# Release review, раунд 12: win-nice 0.2.0 @ 87ae3d9

Дата: 2026-09-03 08:11 (Europe/Berlin)

Объём: весь репозиторий на `master` (`87ae3d9`), полный release delta от
опубликованного `v0.1.0` (`cd138b4`) и отдельно изменения после review раунда 11:
`fbc7f8c` (исправления пяти findings) и `87ae3d9` (checkpoint). Проверены runtime
всех 12 инструментов, 36 launcher files, installer/upgrade/skill, фактический npm
artifact, документация, Node/Pester/MSYS suites, CI и publish workflow.

Текущий release candidate имеет версию **0.2.0**. В npm и origin по-прежнему
опубликован/помечен только `0.1.0`; локальный `master` опережает
`origin/master` на 17 commits.

## Итог

**Тег `v0.2.0` пока ставить нельзя. Найдено 2 P1, 3 P2 и 2 P3.**

Production runtime и реальный upgrade `0.1.0 -> 0.2.0` исправны: полный Pester
run прошёл, все launcher variants присутствуют в текущем tarball, старые
`cap`/`pint` удаляются даже при потерянном manifest, unmanaged sentinel
сохраняется, а ранее установленный managed skill обновляется. Блокеры находятся
в новом release gate и тестовой изоляции:

1. publish workflow получает shallow checkout без предыдущих тегов, поэтому
   добавленный `release-check` гарантированно отвергает корректную ссылку на
   `v0.1.0` и не допускает job до `npm publish`;
2. `npm test` и `npm run release-check` не изолируют `WIN_NICE_SKILL_HOME` и
   переписывают реальные managed skill-файлы разработчика.

Кроме того, `release-check` ломается на обычном Windows-пути с пробелом из-за
`shell: true`, README противоречит новому auto-refresh поведению skill, а
CHANGELOG уже датирует ещё не состоявшийся релиз прошлым днём.

## Findings

### P1 — publish workflow не получает базовый тег, который сам же требует release-check

Файлы: `.github/workflows/publish.yml:27`, `scripts/release-check.js:120-138`.

`actions/checkout` вызывается без `fetch-depth`. Его default — один commit и без
остальных tags; для всей истории и тегов официальный пример требует
`fetch-depth: 0` ([actions/checkout documentation](https://github.com/actions/checkout/blob/main/README.md#fetch-all-history-for-all-tags-and-branches)).
Одновременно `release-check` выполняет локальный `git tag --list v*` и требует,
чтобы compare-base из CHANGELOG (`v0.1.0`) находился среди этих локальных тегов.

Локальный запуск в полном clone проходит и маскирует проблему. Репрезентативное
воспроизведение в отдельном shallow clone с временным `v0.2.0` дало:

```text
SHALLOW=true
TAGS=v0.2.0
release-check: FAIL - ... base "v0.1.0" ... is not an actual git tag
RELEASE_CHECK_EXIT=1
```

То есть tag-triggered publish job остановится до публикации независимо от
исправности artifact.

Исправление: явно получить историю и теги в publish job:

```yaml
- uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262
  with:
    fetch-depth: 0
```

После этого повторить именно shallow/tag simulation либо добавить regression
проверку workflow-контракта.

### P1 — `npm test` и `release-check` выходят из sandbox и обновляют реальные skills

Файлы: `test/cli.test.js:11-24`, `scripts/release-check.js:48-56`,
`install/install.js:91-96`, `install/skill.js:54-67`.

После раунда 11 каждый `install()` вызывает `updateInstalledSkill()`. В
`test/install-uninstall.test.js` для этого правильно добавили отдельный
`WIN_NICE_SKILL_HOME`, но два других caller-а забыты:

- helper `run()` в `test/cli.test.js` задаёт только `WIN_NICE_HOME` и
  `WIN_NICE_NO_PATH`;
- `release-check` создаёт временные `home`/`installPrefix`, но передаёт tarball
  postinstall те же две переменные без `WIN_NICE_SKILL_HOME`.

В обоих случаях `updateInstalledSkill()` разрешает targets через настоящий
`os.homedir()` и переписывает найденные marked copies. Controlled reproduction
с временным `USERPROFILE` положила в обе цели stale sentinel, запустила только
`node --test test/cli.test.js` и получила 12/12 passed, одновременно заменив
обе цели текущим repo skill:

```text
.claude\skills\win-nice\SKILL.md UPDATED_TO_REPO=True
.agents\skills\win-nice\SKILL.md UPDATED_TO_REPO=True
CLI_TEST_EXIT=0
```

Во время этого review первоначальные диагностические `npm test`/
`release-check` также реально перезаписали:

- `C:\Users\Computer\.claude\skills\win-nice\SKILL.md`;
- `C:\Users\Computer\.agents\skills\win-nice\SKILL.md`.

Оба файла были managed, их итоговый SHA-256 совпадает с repo source, поэтому
содержимое не потеряно; изменился `LastWriteTime` (до
`2026-09-03T08:22:48+02:00`). Тем не менее тест и заявленный как изолированный
release gate не должны менять настоящий user home.

Исправление:

- в `test/cli.test.js` передавать дочернему CLI временный
  `WIN_NICE_SKILL_HOME` (можно поместить его внутрь уже очищаемого `home`);
- в `release-check` создавать/очищать отдельный `skillHome` и передавать его
  tarball install;
- добавить regression test с marked sentinel за пределами overrides и
  проверкой неизменных content/mtime после каждого runner-а.

### P2 — `shell: true` делает release-check неработоспособным в путях с пробелами

Файл: `scripts/release-check.js:17-26,38-55`.

Комментарий считает аргументы безопасными, потому что пути «сгенерированы
скриптом». Но `repoRoot` и база `os.tmpdir()` приходят из внешнего окружения и
совершенно нормально содержат пробелы (`C:\Users\Jane Doe\...`), а также могут
содержать shell metacharacters. Node прямо выдаёт `DEP0190`: при `shell: true`
массив аргументов конкатенируется без escaping.

Полное воспроизведение с `%TEMP%` вида `...\<guid> space`:

```text
release-check: ok - packed win-nice-0.2.0.tgz
npm error ENOENT ... D:\dev\win-nice\space\win-nice-release-check-npm-...\package.json
RELEASE_CHECK_WITH_SPACED_TEMP_EXIT=1
TRUNCATED_PREFIX_CREATED=True
```

Даже без полного install простой probe `npm prefix --prefix <path with space>`
через тот же wrapper вернул только часть пути до первого пробела.

Исправление: из `npm run release-check` использовать `process.env.npm_execpath`
как JS entry point npm и запускать его без shell:

```js
execFileSync(process.execPath, [process.env.npm_execpath, ...args], options)
```

Для прямого `node scripts/release-check.js` без `npm_execpath` лучше вывести
понятную инструкцию запускать через npm либо реализовать безопасный fallback.
Добавить end-to-end test из repo/temp path с пробелом.

### P2 — README обещает отсутствие postinstall-обновлений skill, но код их выполняет

Файлы: `README.md:378-390,424-427`, `install/install.js:91-96`,
`install/skill.js:48-69`.

README говорит: «Separate, opt-in install step — not run automatically by
`postinstall`». После исправления раунда 11 это верно только для *первичного
создания*: отсутствующий skill остаётся opt-in, но любая обычная package install
или upgrade автоматически перезаписывает существующую marked copy. Описание
`WIN_NICE_SKILL_HOME` также упоминает только явные `skill install|uninstall`, хотя
переменная теперь управляет target-ами обычного postinstall.

Это важный filesystem side effect, поэтому двусмысленность лучше не оставлять.
Исправление: явно написать, что initial install остаётся opt-in, а последующие
package upgrades автоматически refresh-ят только существующие marked copies;
расширить описание env var и добавить короткий пункт в CHANGELOG 0.2.0.

### P2 — CHANGELOG датирует ещё не выпущенный `0.2.0` 2026-09-02

Файл: `CHANGELOG.md:5-9`; неполная проверка: `scripts/release-check.js:104-117`.

На момент review (2026-09-03) нет ни origin tag `v0.2.0`, ни npm release
`0.2.0`, но CHANGELOG уже содержит `## [0.2.0] - 2026-09-02`. По принятому в
этом же файле Keep a Changelog формату это release date, а не дата подготовки.
Если тег создаётся 2026-09-03 или позже, публичная история будет неверной.

Новый gate проверяет только номер верхней версии и потому считает эту секцию
корректной. Исправление: до финального tag commit заменить дату фактической датой
выпуска; более строгий процесс — держать изменения под `[Unreleased]` до дня
тега.

### P3 — artifact gate проверяет только 12 `.ps1` из обещанных 36 entry points

Файлы: `scripts/release-check.js:69-78`, `README.md:46-56`,
`CHANGELOG.md:55`.

Контракт проекта — три варианта для каждого инструмента: extensionless, `.bat`,
`.ps1`. Но `release-check` проверяет только `${tool}.ps1` и затем сообщает
«all 12 expected launchers present».

В отдельной копии были удалены `bin/capc` и `bin/capc.bat`. Получившийся tarball
содержал 46 вместо 48 entries, однако gate завершился успешно:

```text
release-check: ok - packed win-nice-0.2.0.tgz (46 files, ...)
release-check: ok - all 12 expected launchers present in the installed manifest
release-check: all checks passed
RELEASE_CHECK_EXIT=0
```

Текущий artifact здоров — в нём действительно есть все 36 файлов; finding про
ложноположительный gate. Исправление: построить exact expected set как
`12 tools x ['', '.bat', '.ps1']`, сверить его с manifest и фактическим `bin/`,
а также явно запретить шесть legacy-имён `cap`, `cap.bat`, `cap.ps1`, `pint`,
`pint.bat`, `pint.ps1`.

### P3 — опубликованный package.json рекламирует отсутствующий release-check

Файлы: `package.json:24-37`, `README.md:432-453`.

`package.json` содержит `"release-check": "node scripts/release-check.js"`, но
`files` whitelist не включает `scripts/`. Проверенный tarball имеет 48 entries и
не содержит `scripts/release-check.js`. Это похоже на уже документированные
source-only `test`/`test:elevated`, но для новой команды такого пояснения нет.
После установки package команда видна в npm scripts и падает с missing module.

Сам gate требует git history/tags, поэтому делать его пользовательской tarball
командой необязательно. Минимальное исправление — добавить `npm run
release-check` в CONTRIBUTING/README как maintainer-only command, которая
работает только из полного source checkout. Если её всё же включать в artifact,
нужно отдельно определить корректное поведение без `.git`.

## Проверка исправлений раунда 11

- CHANGELOG теперь корректно объединяет невыпущенный `0.1.1` в `0.2.0` и
  сравнивает `v0.1.0...v0.2.0`.
- Реальный tarball upgrade `win-nice@0.1.0 -> 0.2.0` с удалённым manifest:
  **6/6 legacy files до upgrade, 0/6 после; 6/6 новых `capc`/`capt` variants;
  unmanaged sentinel сохранён; skill изменён и byte/hash-equivalent текущему;
  manifest `0.2.0`, 36 files**.
- Missing/corrupt-manifest fallback покрыт Node tests и не удаляет unmarked
  соседние файлы.
- Fault-injection failure branches теперь реально параметризованы по всем 11
  embedded native launchers; Pester выполнил их без failures.
- Идея проверки фактического tarball правильная, normal full-clone run проходит,
  но четыре findings выше (`fetch-depth`, skill sandbox, shell quoting, неполный
  exact-set check) нужно закрыть, прежде чем считать gate надёжным.

## Выполненные проверки

- `npm test`, Node 24.12.0 / npm 11.13.0:
  **55 passed, 0 failed, 7 skipped** (Git Bash не был первым `bash` на PATH);
- та же полная Node suite на Node 18.20.8, 20.20.2 и 22.23.2:
  для каждого run **55 passed, 0 failed, 7 skipped**;
- отдельный реальный Git Bash/MSYS run:
  **7 passed, 0 failed, 0 skipped**;
- Pester 3.4.0, Windows PowerShell 5.1, normal non-admin session:
  **203 passed, 0 failed, 3 skipped** (206 total, 99.64 s);
- `npm run release-check` в полном clone: все заявленные checks passed, но с
  `DEP0190`; отдельно воспроизведены shallow-tag failure, spaced-temp failure и
  false-positive при двух отсутствующих variants;
- `node --check`: **14 JS files, 0 errors**;
- PowerShell AST parse: **14 PS1 files, 0 errors**;
- JSON parse: **1 JSON file, 0 errors**;
- launcher matrix: **12 tools / 36 exact files**, без missing/extra; managed
  marker присутствует в каждом; `.bat`/`.ps1` имеют CRLF, extensionless — LF;
- локальные Markdown links в README/CONTRIBUTING/SECURITY/CHANGELOG/skill:
  **0 missing targets**;
- `npm pack --dry-run --json`: `win-nice@0.2.0`, **48 files**, 39,130 bytes,
  216,980 bytes unpacked; `scripts/release-check.js` и `test/` отсутствуют;
- tarball smoke: installed `capc.ps1` передал exit code `7`; installed
  extensionless `idle` разрешился bare-name через Git Bash и передал exit `7`;
- `git diff --check v0.1.0..HEAD`, `git fsck --no-dangling`: чисто;
- npm registry и origin tags: существует только `0.1.0` / `v0.1.0`;
- до записи этого отчёта worktree был чист, `HEAD` опережал origin на 17 commits.

Первый Node 18 probe был отвергнут внешним `NODE_OPTIONS` с Node-24-only флагом
`--no-network-family-autoselection`; повтор без этой ambient переменной прошёл.
Это свойство текущего окружения, не finding репозитория.

Elevated suite не запускалась: она требует интерактивного UAC consent. Три
normal-run skip поэтому ожидаемы, но не заменяют обязательный pre-tag elevated
run.

## Release checklist 0.2.0

До тега:

- [ ] Исправить checkout publish job (`fetch-depth: 0`) и повторить shallow/tag
      reproduction.
- [ ] Изолировать `WIN_NICE_SKILL_HOME` в CLI tests и release-check; добавить
      regression на отсутствие изменений за пределами temp roots.
- [ ] Убрать `shell: true`, прогнать release-check из repo и `%TEMP%` с пробелами.
- [ ] Привести README/env-var/CHANGELOG wording к фактическому auto-refresh
      поведению managed skill.
- [ ] Заменить дату `0.2.0` на фактическую дату релиза.
- [ ] Расширить artifact gate до exact 36-file contract; решить/document
      source-only статус `release-check`.
- [ ] Повторить Node 18/20/22/24, normal Pester, Git Bash, actual tarball и
      реальные upgrade smokes после исправлений.
- [ ] Из trusted source checkout выполнить `npm run test:elevated`, принять один
      UAC prompt и сохранить итог вместе с normal Pester result.
- [ ] Проверить Trusted Publisher configuration в npm account.
- [ ] Push `master` и дождаться зелёной CI matrix: текущий candidate существует
      только локально и на 17 commits впереди origin.
- [ ] Убедиться в чистом финальном worktree, создать annotated `v0.2.0` на
      проверенном commit и дождаться успешного publish workflow/npm provenance.

После закрытия P1/P2 других причин задерживать `0.2.0` не видно. P3 лучше
закрыть в том же release-hardening цикле: обе правки малы и относятся именно к
достоверности нового gate.
