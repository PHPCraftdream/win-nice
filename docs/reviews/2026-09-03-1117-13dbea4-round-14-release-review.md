# Полное release-review, раунд 14: win-nice 0.2.0 @ 13dbea4

Дата: 2026-09-03 11:17 (Europe/Berlin)

Проверенный commit: `13dbea4` (`master`). База release delta — опубликованный
тег `v0.1.0` (`cd138b4`). На момент начала проверки локальный `master`
опережал `origin/master` на 23 коммита; в npm и origin по-прежнему существует
только версия/тег `0.1.0` / `v0.1.0`.

Объём: весь репозиторий и полный delta `v0.1.0..HEAD`, отдельно исправления
`daa6674` и `13dbea4`; runtime 12 инструментов и 36 launcher-файлов,
installer/uninstaller/upgrade/skill, npm artifact, Node/Pester/MSYS suites,
CI/publish workflows, package metadata, документация и release notes.

## Итог

**Найдено 0 P0, 0 P1, 0 P2 и 2 P3. Блокирующих кодовых проблем для выпуска
`0.2.0` не найдено.**

Исправления промежуточного review раунда 14 (`b23ef5e`) подтверждены:

- source-checkout guard теперь одинаково защищает `install`, `uninstall` и
  `reinstall`; реальная установка не удаляется локальным `npx win-nice`;
- документация корректно различает 3 elevated-only и 4 non-elevated-only
  Pester cases;
- extensionless Git Bash shims закреплены как LF, а `.bat`/`.ps1` — как CRLF;
- artifact smoke использует `-ExecutionPolicy Bypass` и сохраняет stderr при
  ошибке;
- `release-check` добавлен в обычный CI на Node 24, checkout получает полную
  историю тегов; workflow проходит `actionlint` и локальный Git Bash run.

Оставшиеся P3 не затрагивают runtime или upgrade. Первый усиливает точность
artifact gate, второй — полноту release notes. CHANGELOG-пункт стоит добавить
до тега; artifact hardening можно закрыть в этом цикле либо сразу после релиза.

## Findings

### P3-1: `release-check` не запрещает лишние файлы в npm tarball

Файлы: `package.json:24-33`, `scripts/release-check.js:56-61,91-114`.

`package.json.files` включает каталог `bin` целиком. Gate выводит
`packInfo.entryCount`, но не сравнивает `packInfo.files` с ожидаемым allowlist;
exact-set проверка выполняется только по installer manifest. Файл с
неподдерживаемым расширением попадает в tarball, но `listSourceFiles()` его не
устанавливает, поэтому manifest остаётся корректным и gate считает пакет
здоровым.

Контролируемое воспроизведение в отдельной копии с
`bin/unexpected.txt`:

```text
release-check: ok - packed win-nice-0.2.0.tgz (49 files, 40180 bytes)
release-check: ok - all 36 expected launcher files present ...
release-check: ok - smoke test: capc ...
release-check: ok - smoke test: capt ...
release-check: ok - smoke test: capm ...
release-check: all checks passed
PROBE_EXIT=0
```

Текущий настоящий tarball корректен: ровно 48 entries, лишних файлов нет.
Finding относится к ложноположительному результату будущего gate.

Исправление: сверять `packInfo.files.map(f => f.path)` с ожидаемым package
allowlist или как минимум требовать ровно 36 разрешённых путей под `bin/` и
запрещать остальные. Заодно полезно выполнить из установленного tarball хотя
бы один `.bat` и один extensionless shim: сейчас artifact smoke запускает
только `capc.ps1`, `capt.ps1`, `capm.ps1`, тогда как `.bat`/shim варианты
проверяются из source tree.

### P3-2: CHANGELOG не упоминает новый user-visible fix `uninstall/reinstall`

Файлы: `CHANGELOG.md:7-46`, `install/paths.js:7-13`,
`install/uninstall.js:27-39`.

`13dbea4` исправляет существовавшую с `0.1.0` потерю реальной установки:
`uninstall`/`reinstall`, запущенные через локальный source checkout без
`WIN_NICE_HOME`, могли удалить `%LOCALAPPDATA%\win-nice` и PATH entry, после
чего `install()` пропускал восстановление. Это непосредственно наблюдаемое
пользовательское поведение, но секция `0.2.0` перечисляет только новые tools,
rename/native hardening и skill refresh.

Исправление: добавить в `0.2.0` секцию `Fixed` с коротким пунктом о едином
source-checkout guard для mutating installer commands. CI/release
инфраструктуру перечислять в пользовательском CHANGELOG необязательно.

## Проверка исправлений `13dbea4`

### Source-checkout guard

`isSourceCheckout()` вынесен в `install/paths.js`, используется и install, и
uninstall. Новый Node regression test создает fake `LOCALAPPDATA`, seed-ит
туда 36 файлов, запускает `uninstall` и `reinstall` без `WIN_NICE_HOME` и
проверяет, что install не исчез.

Дополнительный реальный CLI smoke из корня репозитория:

```text
npx --no win-nice uninstall
Running from a source checkout - skipping real uninstall.

npx --no win-nice reinstall
Running from a source checkout - skipping real uninstall.
Running from a source checkout - skipping real install.

uninstall_exit=0 reinstall_exit=0
```

### Line endings и workflow

`git check-attr` возвращает:

```text
bin/idle      text=set eol=lf
bin/idle.bat  text=set eol=crlf
bin/idle.ps1  text=set eol=crlf
README.md     text=auto
```

`git ls-files --eol` подтверждает LF для всех 12 shims и CRLF в working tree
для всех `.bat`/`.ps1`. `actionlint` не нашёл ошибок. Новый CI-вызов
`npm run release-check` отдельно выполнен из настоящего Git Bash и прошёл.

### Artifact smoke diagnostics

`release-check` передаёт `-ExecutionPolicy Bypass`, перехватывает stderr и
включает его в FAIL-сообщение. Штатные `capc`/`capt`/`capm` smokes из
установленного tarball передали exit code 7. Cleanup после обычных, spaced-TEMP
и негативных прогонов не оставил `.tgz` или созданных этим review temp roots.

## Выполненные проверки

| Проверка | Результат |
| --- | --- |
| Node 24.12.0 / npm 11.13.0 | 64 total: 57 passed, 0 failed, 7 skipped |
| Node 18.20.8 / 20.20.2 / 22.23.2 | на каждой версии 57 passed, 0 failed, 7 skipped |
| Git Bash/MSYS shims | 7 passed, 0 failed, 0 skipped |
| Pester 3.4.0 / Windows PowerShell 5.1, non-admin | 203 passed, 0 failed, 3 skipped, 206 total, 179.78 s |
| `npm run release-check` из PowerShell | passed; 48 files, 40,125 bytes; manifest 0.2.0; 36 launchers; 3 smoke; CHANGELOG/tag |
| `npm run release-check` из Git Bash | passed с теми же проверками |
| `release-check` при `%TEMP%` с пробелом | passed |
| `npm pack --dry-run --json` | 48 entries, 40,125 bytes, 219,004 bytes unpacked |
| Published `0.1.0` → текущий tarball | manifest 33→36; 6 legacy→0; 6 новых `capc`/`capt`; foreign-файл сохранён; обе skill-копии byte/hash-equal source |
| Syntax/static | 14 JS, 14 PS1, 1 JSON — 0 ошибок |
| Workflow lint | `actionlint` — 0 ошибок |
| Local Markdown links | 4 проверено, 0 missing |
| Runtime dependencies | `npm ls --all --omit=dev` — empty |
| Git integrity | `git fsck --no-dangling` — чисто; diff нового commit без whitespace errors |

Обычный Node run пропустил 7 shim tests, потому что первым `bash` в ambient
PATH был WSL/non-MSYS. Отдельный запуск с `C:\Program Files\Git\usr\bin`
первым в PATH реально выполнил все семь. Первоначальная попытка Node 18 была
отклонена внешним `NODE_OPTIONS` с Node-24-only flag; повтор с очищенным
`NODE_OPTIONS` прошёл. Это свойства окружения, не findings репозитория.

`git diff --check v0.1.0..HEAD` по-прежнему сообщает только два намеренных
Markdown hard-break в отчёте раунда 13 (`...round-13-release-review.md:3-4`);
изменения `13dbea4` whitespace-clean.

Реальные managed skill files после всех штатных suite/gate прогонов остались
byte-equal source; их mtime не изменился (`2026-09-03 09:59:58`). Все temp
roots и tarballs, созданные этим review, удалены.

## Release checklist

- [ ] Добавить P3-2 в `CHANGELOG.md`; при желании закрыть P3-1 до тега.
- [ ] Из trusted checkout выполнить `npm run test:elevated` и принять один
      UAC prompt. Ожидание: 3 elevated-only cases активны, 4 другие skipped.
- [ ] Убедиться, что дата `## [0.2.0] - 2026-09-03` совпадает с днём тега;
      при переносе релиза обновить её.
- [ ] Push `master` и дождаться зелёной CI matrix Node 18/20/22/24, включая
      новый Node-24 artifact gate.
- [ ] Проверить npm Trusted Publisher и provenance settings.
- [ ] Создать annotated tag `v0.2.0` на проверенном commit и дождаться
      успешного publish workflow.
- [ ] После публикации проверить `npm view win-nice version` = `0.2.0`, 48
      ожидаемых entries и provenance опубликованного tarball.

После обязательных внешних шагов (elevated run, push/CI, tag/publish) причин
задерживать `0.2.0` по runtime-коду не видно.
