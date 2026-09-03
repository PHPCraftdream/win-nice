# Полное release-review, раунд 15: win-nice 0.2.0 @ 6abcc55

Дата: 2026-09-03 12:31 (Europe/Berlin)

Проверенный commit: `6abcc55` (`master`). База release delta — опубликованный тег
`v0.1.0` (`cd138b4`). На момент начала проверки локальный `master` опережал
`origin/master` на 25 коммитов; в npm и origin по-прежнему опубликованы только
версия/тег `0.1.0` / `v0.1.0`.

Объём: весь репозиторий и полный delta `v0.1.0..HEAD`, отдельно новый commit
`6abcc55`; runtime 12 инструментов и 36 launcher-файлов, installer/uninstaller,
upgrade и skill lifecycle, npm artifact, Node/Pester/Git Bash suites, CI/publish
workflows, package metadata, документация, CHANGELOG и release checklist.

## Итог

**Найдено 0 P0, 0 P1, 0 P2 и 1 P3. Блокирующих кодовых проблем для выпуска
`0.2.0` не найдено.**

Исправления после предыдущего review подтверждены:

- npm tarball сейчас содержит ровно 36 ожидаемых файлов под `bin/`, а
  `release-check` отклоняет лишний `bin/unexpected-review-probe.txt`;
- artifact gate запускает из установленного tarball не только `.ps1`, но также
  `idle.bat` и extensionless `idle` через настоящий Git Bash;
- оба варианта проходят при `%TEMP%`, содержащем пробелы;
- CHANGELOG описывает source-checkout guard для `uninstall`/`reinstall`;
- Pester-проверка сообщения `-SelfElevated` больше не зависит от переноса строки.

Оставшийся P3 относится только к полноте будущего artifact gate: текущий
48-файловый tarball корректен, runtime и upgrade не затронуты. Его разумно закрыть
до тега, но оснований блокировать релиз после обязательных внешних шагов не видно.

## Finding

### P3-1: exact allowlist проверяет только `bin/`, а не весь npm tarball

Файлы: `package.json:24-30`, `scripts/release-check.js:75-92`.

`package.json.files` включает целиком три каталога: `bin`, `install` и `skills`.
Новая проверка использует `packInfo.files`, но сразу отфильтровывает только пути с
префиксом `bin/`. Поэтому случайный файл в `install/` или `skills/` будет реально
опубликован, хотя gate завершится успешно. Это может быть отладочный артефакт,
черновик или иной файл, который не должен входить в npm package.

Контролируемое воспроизведение в отдельной копии с
`install/unexpected-review-probe.txt`:

```text
release-check: ok - packed win-nice-0.2.0.tgz (49 files, 40262 bytes)
release-check: ok - packed tarball's bin/ contains exactly the 36 expected launcher files
release-check: ok - all 36 expected launcher files present ...
release-check: all checks passed
PROBE_EXIT=0
```

Контрольный второй probe с дополнительным `bin/unexpected-review-probe.txt`
корректно завершился с exit 1 и сообщением
`unexpected in tarball: bin/unexpected-review-probe.txt`. То есть исправление
раунда 14 работает ровно в заявленной области, но не защищает остальную часть
пакета.

Исправление: сравнивать весь `packInfo.files.map(f => f.path)` с exact allowlist.
Для текущего пакета это 48 путей: 36 launchers плюс `package.json`, четыре корневых
документа/лицензии, шесть `install/*.js` и `skills/win-nice/SKILL.md`. Желательно
закрепить отрицательным regression-тестом лишние файлы и в `bin/`, и в `install/`
или `skills/`.

## Проверка нового commit `6abcc55`

### Artifact contract и launcher variants

Чистый `npm run release-check` прошёл с 48 entries и размером 40,199 bytes.
Exact-проверка подтвердила 36 launcher-файлов. Установленный manifest имеет версию
`0.2.0`, содержит ровно 36 файлов и не содержит шесть legacy-имён `cap*`/`pint*`.
Smoke-тесты `capc.ps1`, `capt.ps1`, `capm.ps1`, `idle.bat` и extensionless `idle`
передали exit code 7.

Проверка отдельно выполнена с Git Bash первым в PATH и с `%TEMP%` равным
`D:\system_artefact\Temp\win nice round15 1231`: `.bat` и shim корректно работают
с путями, содержащими пробелы. Созданные tarball и temp roots удалены.

### CHANGELOG и Pester flake

Секция `0.2.0 / Fixed` точно описывает защиту реальной установки и PATH при запуске
mutating CLI-команд из source checkout. Изменённая Pester-проверка успешно прошла
в составе полного non-admin suite; результат — 203 passed, 0 failed, 3 skipped.

## Выполненные проверки

| Проверка | Результат |
| --- | --- |
| Node 24.12.0 / npm 11.13.0 | 64 total: 57 passed, 0 failed, 7 skipped |
| Node 18.20.8 / 20.20.2 / 22.23.2 | полный Node suite на каждой версии: exit 0 |
| Git Bash/MSYS shims | 7 passed, 0 failed, 0 skipped |
| Pester 3.4.0 / Windows PowerShell 5.1, non-admin | 203 passed, 0 failed, 3 skipped, 206 total, 104.04 s |
| `npm run release-check`, Git Bash доступен | passed; 48 files, 40,199 bytes; 36 launchers; 5 variant/family smokes |
| `release-check` при `%TEMP%` с пробелом | passed, включая `idle.bat` и extensionless shim |
| Отрицательный extra-`bin` probe | rejected, exit 1 |
| Отрицательный extra-`install` probe | gate пропустил 49 files, exit 0 — P3-1 |
| `npm pack --dry-run --json` | 48 entries, 40,199 bytes, 219,323 bytes unpacked |
| Published `0.1.0` → текущий tarball | manifest 33→36; legacy names 6→0; новые `capc`/`capt` присутствуют |
| Syntax/static | 14 JS, 14 PS1, 1 JSON — 0 ошибок |
| Workflow lint | `actionlint` — 0 ошибок |
| Runtime dependencies | `npm ls --all --omit=dev` — empty |
| Line endings | 12 shims LF; `.bat`/`.ps1` CRLF согласно `.gitattributes` |
| Локальные Markdown-ссылки | 5 ссылок на существующие repo-файлы |
| Git integrity | `git fsck --no-dangling` — чисто; новый commit whitespace-clean |

Обычный Node run пропустил семь shim-тестов, потому что первым `bash` в ambient PATH
был WSL/non-MSYS. Отдельный прогон с `C:\Program Files\Git\usr\bin` первым в PATH
реально выполнил все семь. Для Node 18/20/22 был очищен внешний `NODE_OPTIONS` с
Node-24-only флагом; это свойство локального окружения, не finding репозитория.

`git diff --check v0.1.0..HEAD` сообщает только о двух намеренных Markdown hard-break
в отчёте раунда 13 (`...round-13-release-review.md:3-4`). Delta нового commit
`f4a0159..6abcc55` whitespace-clean. Все временные каталоги и tarballs этого review
удалены; рабочее дерево перед записью отчёта было чистым.

## Release checklist

- [ ] По возможности закрыть P3-1 полным 48-path allowlist до тега.
- [ ] Из trusted checkout выполнить `npm run test:elevated` и принять один UAC
      prompt. Ожидание: три elevated-only cases активны, четыре другие skipped.
- [ ] Убедиться, что дата `## [0.2.0] - 2026-09-03` совпадает с днём тега; при
      переносе релиза обновить её.
- [ ] Push `master` и дождаться зелёной CI matrix Node 18/20/22/24, включая
      Node-24 artifact gate.
- [ ] Проверить npm Trusted Publisher и provenance settings.
- [ ] Создать annotated tag `v0.2.0` на проверенном release commit и дождаться
      успешного publish workflow.
- [ ] После публикации проверить `npm view win-nice version` = `0.2.0`, exact
      48-file tarball и provenance attestation.

После обязательных внешних шагов (elevated run, push/CI, tag/publish) причин
задерживать `0.2.0` по runtime-коду не найдено.
