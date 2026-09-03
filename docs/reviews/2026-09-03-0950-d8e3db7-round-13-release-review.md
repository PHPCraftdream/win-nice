# Полное release-review, раунд 13: win-nice 0.2.0

Дата проверки: 2026-09-03 09:50 (Europe/Berlin)  
Проверенный commit: `d8e3db7` (`master`)  
База release delta: локальный тег `v0.1.0`; `master` содержит 20 коммитов после него.

## Итог

Блокирующих проблем уровня P0/P1/P2 не найдено. Текущий код, installer/upgrade-путь,
launcher matrix и фактический npm tarball выглядят готовыми к выпуску `0.2.0` после
операционных шагов из checklist ниже.

Раунд 12 закрыт полностью:

- publish workflow теперь получает полную историю и теги (`fetch-depth: 0`);
- Node CLI tests и `release-check` изолируют `WIN_NICE_SKILL_HOME` и не трогают
  реальный home разработчика;
- `release-check` больше не использует небезопасный `shell: true` и проходит на
  путях с пробелами;
- проверяется полный набор из 36 launcher-файлов (12 инструментов × 3 варианта),
  включая запрет старых `cap`/`pint` имен;
- README/CHANGELOG описывают фактический source-only статус `release-check` и
  auto-refresh уже установленного managed skill.

## Что проверено

| Область | Результат |
| --- | --- |
| Node test suite (Node 24.12.0) | 56 passed, 0 failed, 7 skipped |
| Node matrix (18.20.8, 20.20.2, 22.23.2) | на каждой версии 56 passed, 0 failed, 7 skipped |
| Git Bash/MSYS shims | 7 passed, 0 failed, 0 skipped при MSYS `bash` первым в PATH |
| Pester 3.4.0 / Windows PowerShell 5.1 | 203 passed, 0 failed, 3 skipped, 206 total |
| `npm run release-check` | passed: tarball 48 files, 39,997 bytes; manifest 0.2.0; 36 launchers; capc exit-code smoke; CHANGELOG/tag checks |
| `%TEMP%` с пробелом | `release-check` passed |
| Реальный upgrade `v0.1.0` → текущий tarball | старые 6 `cap`/`pint` launchers удаляются, новые 6 `capc`/`capt` появляются, manifest `0.2.0` содержит 36 файлов, foreign-файл сохраняется |
| Syntax/static checks | 14 JS `node --check`, 14 PowerShell AST, 1 JSON — без ошибок |
| Artifact matrix | ровно 36 launcher-файлов, managed markers и line endings корректны |
| Git integrity | `git diff --check v0.1.0..HEAD`, `git fsck --no-dangling` — чисто |

Пропуски Node Git Bash-тестов в обычном запуске ожидаемы: на этой машине системный
`bash` — WSL/non-MSYS. Отдельный запуск с Git Bash первым в PATH прошел все 7 тестов.
Три Pester skip относятся к already-elevated сценариям; отдельный UAC-run требует
интерактивного подтверждения и не выполнялся в этой сессии.

## Остаточные улучшения (P3, не блокируют выпуск)

### P3-1: cleanup `release-check` не охватывает ошибки до внешнего `try/finally`

Файл: `scripts/release-check.js:48-64`.

`npm pack`, `JSON.parse(packOut)` и три `mkdtempSync` выполняются до `try/finally`.
При сбое npm/поврежденном JSON/ошибке создания временной директории tarball или уже
созданные temp roots могут остаться. На штатном пути cleanup проходит.

Рекомендация: инициализировать пути как `null`, охватить pack и создание temp roots
внешним `try/finally`, удаляя только созданные ресурсы. Это улучшит повторяемость
release-gate после аварийных запусков.

### P3-2: artifact gate smoke-тестирует только `capc.ps1`

Файл: `scripts/release-check.js:109-123`.

Gate проверяет exact names/manifest всех 36 файлов, но исполняет из установленного
tarball только один `.ps1` вариант. Ошибка в `capt`, `capm`, другом `.bat` или
extensionless shim может пройти gate при корректном `capc`.

Рекомендация: добавить недорогой smoke для каждого семейства (`capc`, `capt`,
`capm`) и/или проверить содержимое всех 36 файлов теми же marker/launcher
инвариантами, которые используются для source tree.

## Release checklist

- [ ] Убедиться, что целевая версия именно `0.2.0` (текущие `package.json` и
      `CHANGELOG.md` согласованы; rename `cap`→`capc`, `pint`→`capt` — breaking).
- [ ] Запустить `npm run test:elevated` на доверенной Windows-машине и сохранить
      результат вместе с обычным Pester run; команда требует UAC consent.
- [ ] Проверить финальную дату записи `## [0.2.0]` непосредственно перед tag.
- [ ] Запушить `master` и дождаться зеленой CI matrix; локальный `master` сейчас
      опережает `origin/master` на 19 коммитов (до коммита этого отчета).
- [ ] Проверить настройки npm Trusted Publisher/provenance.
- [ ] Создать annotated tag `v0.2.0` на проверенном commit и дождаться publish
      workflow. На origin и npm пока присутствует только `v0.1.0`/`0.1.0`.
- [ ] После tag проверить, что опубликованный пакет содержит ожидаемые 48 files и
      что `npm view win-nice version` показывает `0.2.0`.

До выполнения этих внешних шагов release не считается опубликованным, но локальный
release candidate и автоматические проверки находятся в зеленом состоянии.
