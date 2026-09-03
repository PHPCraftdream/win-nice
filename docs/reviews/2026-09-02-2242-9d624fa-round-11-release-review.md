# Release review, раунд 11: win-nice 0.2.0 @ 9d624fa

Дата: 2026-09-02 22:42 (Europe/Berlin)

Объём: весь репозиторий на `master` (`9d624fa`), полный release delta от
опубликованного `v0.1.0` (`cd138b4`) и изменения после review раунда 10
(`e6a30dc`). Проверены все 12 инструментов и 36 launcher files, rename
`cap → capc` / `pint → capt`, installer и upgrade path, opt-in skill, npm
artifact, документация, Node/Pester/MSYS suites, CI и publish workflow.

Текущий release candidate имеет версию **0.2.0**. Для pre-1.0 проекта minor bump
согласуется с удалением двух публичных команд; это review проверяет готовность
именно `0.2.0`.

## Итог

**P0/P1 не найдено. Runtime, штатный upgrade и npm artifact исправны, но тег
`v0.2.0` пока ставить рано.** До релиза следует закрыть три P2:

1. CHANGELOG строит release history через несуществующий `v0.1.1` и прячет под
   ним значительную часть фактического delta `0.1.0 → 0.2.0`;
2. штатный package upgrade не обновляет ранее установленный managed skill — он
   продолжает советовать удалённые команды `cap`/`pint`;
3. rename-cleanup зависит от читаемого install manifest: без него upgrade
   завершается успешно, но оставляет все шесть старых launcher files на `PATH`.

Все три проблемы воспроизведены или подтверждены на фактическом опубликованном
`win-nice@0.1.0`. Они не ломают чистую установку `0.2.0`, поэтому это не P1, но
это реальные дефекты перехода для существующих пользователей.

Исправления раунда 10 в основном реализованы корректно: elevated helper теперь
проверяет настоящий admin token, `TerminateProcess` failures диагностируются,
тесты ждут асинхронное завершение с deadline, все 11 native C# launcher bodies
компилируются и выполняются, а README честно ограничивает test scripts source
checkout-ом.

## Findings

### P2 — CHANGELOG ссылается на невыпущенный `0.1.1`

Файл: `CHANGELOG.md:5-72`.

В репозитории оформлены датированные секции `0.1.1` и `0.2.0`, а ссылка
`[0.2.0]` сравнивает `v0.1.1...v0.2.0`. Но на момент review:

- npm registry содержит только `win-nice@0.1.0` (`npm view win-nice version
  versions --json`);
- локальный и удалённый repository содержат только tag `v0.1.0`;
- [GitHub tags](https://github.com/PHPCraftdream/win-nice/tags) также показывает
  только `v0.1.0`, а [GitHub Releases](https://github.com/PHPCraftdream/win-nice/releases)
  пока пуст.

Следствие: обе compare-ссылки для `0.1.1`/`0.2.0` сейчас разорваны. Главное —
пользовательский delta от единственной опубликованной версии включает `capm`,
native hardening, chaining и rename, но секция `0.2.0` перечисляет только rename
и последнюю доработку `TerminateProcess`. Секция `0.1.1` выглядит как история
выпущенной версии, которой не было.

Рекомендация для прямого релиза `0.1.0 → 0.2.0`:

- объединить содержимое `0.1.1` и `0.2.0` в одну итоговую секцию `0.2.0`;
- описывать в ней конечные имена `capc`/`capt`, добавление `capm` и существенный
  native hardening;
- удалить невыпущенную секцию/ссылку `0.1.1`;
- изменить `[0.2.0]` на compare `v0.1.0...v0.2.0`.

Альтернатива — действительно сначала собрать, проверить, тегировать и
опубликовать `0.1.1`, но текущая история не содержит финального `0.1.1` commit со
всеми исправлениями раунда 10: они объединены с breaking rename и bump до
`0.2.0` в `9d624fa`. Для текущего состояния объединение CHANGELOG проще и
надёжнее.

### P2 — package upgrade оставляет установленный skill со старыми командами

Файлы: `package.json:34`, `install/install.js:41-70`, `install/skill.js:30-45`,
`README.md:372-390`, `skills/win-nice/SKILL.md`.

Skill устанавливается отдельно через `win-nice skill install`, а `postinstall`
вызывает только обычный bin installer. Это было нормальным opt-in поведением до
breaking rename, но теперь ранее установленная marked copy остаётся содержимым
`0.1.0`: она рекомендует `cap`/`pint`, не знает `capm` и после package upgrade
вызывает уже удалённые команды.

Воспроизведение на реальных tarballs:

1. установить `win-nice@0.1.0` в изолированный home;
2. выполнить его `skill install`;
3. обновить пакет до собранного `win-nice@0.2.0` обычным `npm install`;
4. сравнить `~/.agents/skills/win-nice/SKILL.md` до и после.

Результат: upgrade exit `0`, файл byte-for-byte не изменился, содержит `cap` и
`pint`, не содержит `capc`. Ручной повторный `win-nice skill install` успешно
обновляет marked copy до `capc`/`capt`, то есть механизм обновления уже есть —
он просто не участвует в upgrade.

Рекомендация: при package `postinstall` обновлять только уже существующие
`win-nice: managed-skill` copies. Не создавать skill автоматически, если
пользователь ранее не делал opt-in, и не трогать unmarked/foreign files.
Добавить integration test для трёх инвариантов: marked old copy обновляется,
отсутствующая остаётся отсутствующей, foreign copy не перезаписывается.

Если автоматическое обновление сознательно отклоняется, минимально допустимый
вариант перед релизом — заметное upgrade instruction в `CHANGELOG.md` и README:
после `0.1.x → 0.2.0` повторно выполнить `npx win-nice skill install`. Он хуже,
потому что stale инструкция будет жить именно в файле, который читает агент, а
не пользователь.

### P2 — без valid manifest upgrade не удаляет `cap`/`pint`

Файлы: `install/install.js:24-39`, `install/uninstall.js:23-40`,
`test/install-uninstall.test.js:150-183`, `CHANGELOG.md:11-12`.

`cleanupStaleFiles()` немедленно возвращается, если manifest отсутствует,
повреждён или имеет не-array `files`. Новый regression test покрывает только
идеальный случай: искусственно добавляет старые имена в валидный текущий
manifest. При реальном upgrade с потерянным manifest старые marked files вообще
не рассматриваются.

Подтверждённый сценарий:

1. установить фактический `win-nice@0.1.0` — появляются `cap`, `cap.bat`,
   `cap.ps1`, `pint`, `pint.bat`, `pint.ps1`;
2. удалить только `install-manifest.json`;
3. установить текущий tarball `0.2.0`.

Upgrade вернул `0`, но **6 из 6** старых файлов остались рядом с `capc`/`capt`.
Это противоречит CHANGELOG-фразе «the old names no longer exist». README уже
признаёт missing/corrupt manifest поддерживаемым recovery case для uninstall,
где используется безопасный marker-based fallback; install такого fallback не
имеет.

Рекомендация: если manifest отсутствует/повреждён, просканировать только
известный `binDir`, удалить marked `win-nice: managed-file` entries, которых нет
в `currentFiles`, и оставить любые unmarked files без изменений. Переиспользовать
ту же path-containment защиту, что и uninstall. Добавить два upgrade tests:
missing manifest и corrupt manifest; отдельно подтвердить, что unrelated
unmarked file сохраняется.

### P3 — детальные failure branches всё ещё выполняются только для двух launcher-ов

Файл: `test/win-nice.Tests.ps1:1572-1598,1623-1845,1868-1916`.

Раунд 10 заметно улучшен: `$allLauncherProbes` компилирует все 11 embedded C#
bodies, и каждый имеет настоящий success-path execution. Но `FailWait`,
`FailTerminate`, `FailResume`, `FailAssign`, `FailSetInfo` и
`FailGetExitCode` по-прежнему запускаются только через `idle` и `capc`.

Комментарий утверждает byte-identical failure bodies, но автоматический
source-shape guard проверяет лишь число `CloseHandle(` и наличие P/Invoke
declarations; нормализованные `Run()` bodies он не сравнивает. Сейчас все 17
`TerminateProcess` call sites при ручной проверке согласованы, поэтому найденной
production-регрессии нет. Однако следующий drift в `admin`, `capt` или `capm`
скомпилируется и пройдёт success smoke.

Рекомендация: раз все probes уже создаются, параметризовать failure cases по
`$allLauncherProbes` и применимым shapes. Долгосрочно лучше генерировать общий
native runner из одного источника вместо поддержки 11 почти одинаковых копий.

### P3 — publish workflow не проверяет фактический artifact и release metadata

Файлы: `.github/workflows/publish.yml:34-69`, `package.json`, `CHANGELOG.md`.

Publish job проверяет tag/package version и запускает suites из checkout, затем
сразу делает `npm publish`. Он не устанавливает созданный tarball и не проверяет,
что текущая release section/compare base соответствует реально существующим
тегам. Именно поэтому неверная CHANGELOG chain не блокирует workflow; ошибка в
`files` whitelist также могла бы пройти source-tree tests.

Текущий artifact был проверен вручную и исправен, поэтому это не blocker само по
себе. Рекомендация: добавить release-check script, который выполняет `npm pack
--json`, устанавливает полученный `.tgz` с изолированным `WIN_NICE_HOME`, сверяет
package/manifest version и ожидаемые launcher names, запускает короткий smoke и
проверяет latest CHANGELOG heading/base tag. Publish должен использовать именно
этот уже проверенный artifact.

## Проверка исправлений раунда 10

- `test/run-elevated.ps1` вычисляет настоящий admin token до обработки
  `-SelfElevated`, требует `LogPath` и имеет regression tests для обоих отказов.
- README/CONTRIBUTING/helper больше не обещают, что один elevated run выполняет
  буквально все cases; полное покрытие корректно определено как normal + elevated.
- Все 17 best-effort kill call sites проверяют boolean result
  `TerminateProcess` и добавляют отдельный Win32 error при его отказе.
- Fault test умеет симулировать `TerminateProcess == false`, а успешное
  асинхронное завершение проверяется polling-ом с 5-second deadline.
- Все 11 native launchers используют единый ownership `try/finally`, компилируются
  в probes и проходят реальный success path без double-close.
- README явно говорит, что `npm test`/`test:elevated` доступны только из source
  checkout, поскольку `test/` не публикуется.
- Rename согласован в active source/docs/tests: вне исторических changelog/review/
  checkpoint файлов исполняемых ссылок на старые команды не найдено.

## Выполненные проверки

- `npm test`, Node 24.12.0 / npm 11.13.0:
  **48 passed, 0 failed, 7 skipped** (55 total; Git Bash не был на исходном PATH);
- полный Node suite на Node 18.20.8, 20.20.2 и 22.23.2:
  в каждом run **48 passed, 0 failed, 7 skipped**;
- реальный Git Bash/MSYS run с Git for Windows на PATH:
  **7 passed, 0 failed, 0 skipped**;
- Pester 3 integration suite, normal non-admin session:
  **161 passed, 0 failed, 3 skipped** (164 total, 137.66 s);
- parse всех `.ps1`: **0 ошибок**;
- `node --check` для всех `.js`: **0 ошибок**;
- parse всех JSON: **0 ошибок**;
- 12 tools имеют полный набор из extensionless/`.bat`/`.ps1`, всего 36 files;
  marker присутствует во всех;
- `.bat`/`.ps1` имеют CRLF, extensionless shims — LF;
- локальные Markdown links в README/CONTRIBUTING/SECURITY/CHANGELOG/skill:
  **0 missing targets**;
- `git diff --check v0.1.0..HEAD`: чисто;
- `npm pack --dry-run --json`: `win-nice@0.2.0`, **48 files**, 38,616 bytes,
  214,970 bytes unpacked;
- установка фактического tarball: package и install manifest имеют `0.2.0`,
  все 36 launcher files присутствуют;
- artifact smoke: `capc`, `capt`, `capm` передали exit codes `7`, `6`, `5`;
- реальный upgrade tarball `0.1.0 → 0.2.0` с valid manifest: успешен, все 6
  legacy files удалены, новые имена установлены;
- negative upgrade без manifest: воспроизведён P2, все 6 legacy files остались;
- real installed-skill upgrade: воспроизведён P2, old skill не изменился;
  повторный ручной `skill install` успешно обновил его;
- npm registry и origin tags: опубликован/помечен только `0.1.0`; `0.2.0`
  доступна для публикации;
- текущая shell не elevated, поэтому интерактивный UAC run не запускался;
- `HEAD` на 14 commits впереди `origin/master`; до отчёта worktree был чист.

## Release checklist 0.2.0

До тега:

- [ ] Свести CHANGELOG к фактическому release path `v0.1.0 → v0.2.0`.
- [ ] Обновлять ранее установленный marked skill либо добавить обязательную и
      заметную migration-команду `npx win-nice skill install`.
- [ ] Добавить safe stale-file fallback для missing/corrupt manifest и regression
      tests.
- [ ] Решить P3 по coverage сейчас либо явно перенести его после релиза.
- [ ] На финальном commit повторить Node 18/20/22/24, normal Pester, Git Bash и
      actual tarball install/upgrade smoke.
- [ ] Из trusted source checkout выполнить `npm run test:elevated`, принять один
      UAC prompt и сохранить результат вместе с normal Pester run.
- [ ] Проверить exact Trusted Publisher configuration в npm account.
- [ ] Push `master`, дождаться зелёной CI matrix для всех четырёх Node versions;
      сейчас candidate существует только локально и на 14 commits впереди origin.
- [ ] Убедиться, что worktree содержит только намеренные release files.
- [ ] Создать `v0.2.0` на проверенном commit и дождаться успешного publish workflow.

После закрытия трёх P2 и normal/elevated release gates новых оснований задерживать
`0.2.0` по результатам раунда 11 нет.
