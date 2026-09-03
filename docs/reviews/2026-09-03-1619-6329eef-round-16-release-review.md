# Полное статическое release-review, раунд 16: win-nice 0.2.0 @ 6329eef

Дата: 2026-09-03 16:19 (Europe/Berlin)

Проверенный commit: `6329eef` (`master`). Предыдущий review commit — `521a97b`;
новая разработка после него сосредоточена в одном commit `6329eef` (18 файлов,
1853 добавления, 53 удаления). База release delta — локальный тег `v0.1.0`
(`cd138b4`). В начале review рабочее дерево было чистым, а `master` опережал
локальный tracking ref `origin/master` на 27 коммитов.

Режим проверки: **только чтение и статический анализ по прямому указанию
пользователя**. Не запускались Node/Pester suites, launchers, `npm pack`,
`release-check`, сборка, installer, shell shims и любые другие исполняемые проверки.
Единственные изменения после анализа — этот файл отчёта и запрошенный commit.

Объём: весь текущий репозиторий и полный release delta `v0.1.0..HEAD`, с подробным
разбором `6329eef`: новые `caps`/`capn`, изменения `capc`/`capt`/`capm`, Job Object
lifecycle, argument validation/quoting, installer и 42 launcher-файла, npm artifact
contract, Node/Pester test code, CI/publish workflows, README, skill и CHANGELOG.

## Итог

**Найдено 0 P0, 1 P1, 1 P2 и 1 P3. В текущем виде commit `6329eef` не следует
выпускать как `0.2.0`: P1 меняет успешный normal-exit путь и может принудительно
завершать оставленные командой рабочие/daemon-процессы.**

Новые `caps` и `capn` в целом последовательно встроены в проект: у каждого есть
`.ps1`, `.bat` и extensionless shim; installer подхватывает их динамически;
release gate ожидает 14 tools / 42 launchers / 54 tarball entries; README, skill,
CHANGELOG, Git Bash и Pester test code обновлены. P3 раунда 15 с неполным npm
allowlist статически закрыт: `release-check.js` теперь сравнивает весь список
tarball paths, а не только `bin/`, и добавлен отдельный regression suite для
лишних `bin/`, `install/` и `skills/` файлов.

Однако заявленное `KILL_ON_JOB_CLOSE` hardening основано на неверной предпосылке,
что завершение непосредственно обёрнутого процесса автоматически опустошает Job
Object. Descendant может продолжать жить после завершения родителя; именно такое
поведение текущая документация отдельно обещает для build daemon и watcher.

## Findings

### P1-1: normal exit wrapper теперь принудительно убивает surviving descendants

Файлы: `bin/capc.ps1:230-246,347-371`, `bin/capt.ps1:196-210,306-330`,
`bin/capm.ps1:279-288,384-408`, `bin/caps.ps1:244-256,355-400`,
`bin/capn.ps1:217-243,339-363`, `README.md:201-216`,
`skills/win-nice/SKILL.md:94-102`, `test/win-nice.Tests.ps1:1925-1998`.

Все пять Job-Object launcher'ов устанавливают
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, ждут через `WaitForSingleObject(hProcess, …)`
только handle непосредственно запущенного процесса, а затем безусловно закрывают
`hJob` в `finally`. Handle процесса становится signaled при завершении именно этого
процесса, а не всей его descendant-группы. Если он успешно завершился, оставив
живой daemon, compiler server, watcher или иной worker, закрытие последнего Job
Object handle немедленно завершит все ещё связанные с job процессы.

Это точная документированная семантика Windows: Microsoft указывает, что при
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` закрытие последнего handle завершает все
associated processes и, для nested job, процессы дочерних jobs:
<https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects>.
Process handle, переданный в `WaitForSingleObject`, сигнализируется после завершения
этого процесса:
<https://learn.microsoft.com/en-us/windows/win32/procthread/terminating-a-process>.

Следовательно, комментарии вида «the wait … has already reaped the child
(emptying the job)» неверны: завершившийся root не означает пустой job. Это также
прямо противоречит README/SKILL, где обещано, что лимит остаётся на daemon на весь
срок его жизни. Практический эффект — например, успешно завершившийся build может
оставить `VBCSCompiler`, MSBuild/Gradle daemon или watcher, после чего wrapper
жёстко убьёт его. `caps` на successful-before-deadline пути аналогично может
вернуть успешный exit code root-процесса, одновременно убив его background worker.
Принудительное завершение нельзя обработать или отложить со стороны descendant.

Новые тесты проверяют полезный аварийный сценарий — launcher taskkill'нут, пока root
и grandchild оба живы — и текстовое наличие флага во всех пяти файлах. Обратного
normal-exit сценария нет: root запускает bounded grandchild и штатно завершается,
после чего проверяется документированный lifecycle grandchild. Поэтому suite может
быть зелёным при этой регрессии.

Исправление зависит от выбранного контракта:

1. Если documented daemon-survival нужно сохранить, на успешном normal-exit пути
   перед закрытием `hJob` снять `KILL_ON_JOB_CLOSE`, повторно установив остальные
   лимиты (`CPU` остаётся отдельным info class; affinity/memory/process-count нужно
   сохранить в extended structure). Ошибка снятия флага не должна проходить тихо.
   На abnormal/failure/timeout путях флаг остаётся backstop.
2. Если новый контракт намеренно привязывает весь job к lifetime wrapper, признать
   это user-visible breaking behavior, удалить противоположное обещание из
   README/SKILL и ясно описать, что любой surviving descendant всегда убивается
   даже после exit 0 root-процесса.

В обоих случаях добавить behavioural regression для clean root exit + still-live
grandchild как минимум для `capc` и `caps`, а source-shape/parameterized проверкой
закрепить выбранный normal-exit путь во всех пяти launchers.

### P2-1: `caps` не считает время system sleep, хотя обещает wall-clock deadline

Файлы: `bin/caps.ps1:42-53,224,351-384`, `README.md:218-240`,
`skills/win-nice/SKILL.md:69-82`, `CHANGELOG.md:17-29`.

`caps` один раз передаёт относительный `timeoutMs` в
`WaitForSingleObject(hProcess, timeoutMs)` и называет это hard wall-clock timeout.
На Windows 8+ `dwMilliseconds` не включает время в low-power states. Поэтому
`caps 60 ...`, запущенный перед sleep/suspend, после пробуждения продолжит ждать
остаток своей минуты вместо немедленного обнаружения давно прошедшего абсолютного
deadline. Официальное описание различия Windows 7 и Windows 8+ находится в
документации `WaitForSingleObject`:
<https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-waitforsingleobject>.

Для desktop/laptop-инструмента это не теоретически невозможная среда, а расхождение
с основной формулировкой новой функции. Исправление: либо реализовать абсолютный
UTC deadline и после каждого возврата/возобновления пересчитывать remaining time,
либо в README/SKILL/CHANGELOG честно назвать лимит временем активной работы системы,
которое приостанавливается вместе с sleep. Арифметику deadline/remaining полезно
вынести в тестируемую pure function, поскольку CI обычно не может надёжно вводить
runner в suspend.

### P3-1: комментарии fault-injection harness содержат устаревшие количества

Файл: `test/win-nice.Tests.ps1:1576-1588,1636-1643`.

Комментарий говорит о «2 of the 4 original Job-Object ones», хотя до `caps`/`capn`
таких launcher'ов было три (`capc`, `capt`, `capm`), а сейчас их пять. Ниже другой
комментарий утверждает, что `JobArg` non-null «only for the 4 Job Object launchers»,
но тут же перечисляет пять аргументов и таблица действительно содержит пять строк.
Runtime и assertions не затронуты, однако эти пояснения описывают архитектуру
сложного fault-injection harness и должны быть исправлены на `2 of 3 original` и
`5 Job Object launchers` соответственно.

## Статическая проверка новых разработок

### `caps`

- Валидация принимает положительное целое/decimal число в invariant culture,
  отклоняет overflow и резервный `INFINITE`, затем передаёт `uint32` milliseconds.
- Direct `CreateProcess` и cmd fallback повторяют существующий fail-closed `%`
  контракт; `.bat` и Git Bash shim согласованы с другими инструментами.
- Timeout path использует `TerminateJobObject` и exit 124; success path получает и
  возвращает exit code root-процесса.
- Выделение unmanaged structure обёрнуто в `try/finally`, handles имеют единый
  owner cleanup.
- Помимо P1/P2 новых статических дефектов в основном timeout path не найдено.

### `capn`

- `uint32.TryParse` соответствует типу `ActiveProcessLimit`, диапазон 1..4294967295
  проверяется без raw conversion exception.
- Root создаётся suspended, job получает `ACTIVE_PROCESS` limit, затем root
  назначается в job и возобновляется; первый процесс действительно занимает слот.
- Argument quoting, `%` fail-closed, error paths и handle ownership следуют
  существующему шаблону.
- Документация описывает отличие failed spawn от kill/throttle и особенности
  nested limits; кроме общего P1 новых статических дефектов не найдено.

### Artifact/release wiring

- `expectedTools` содержит 14 имён, формируя 42 launcher paths.
- Полный allowlist содержит ещё 12 literal paths и ожидает 54 tarball entries.
- `test/release-check-allowlist.test.js` статически покрывает clean set, extra
  `bin/`, `install/`, `skills/`, missing path и uniqueness/count contract.
- `release-check` добавил normal exit-code smoke для `caps` и `capn`.
- `package.json`, publish workflow, README, skill и CHANGELOG согласованы по новым
  именам; line-ending metadata для shims — LF, для `.bat`/`.ps1` — CRLF.
- Delta `521a97b..6329eef` не содержит whitespace errors по `git diff --check`.

## Что не проверено в этом раунде

По указанию пользователя **не выполнялось ничего из следующего**:

- `npm test` на Node 18/20/22/24;
- Pester non-admin и elevated suites;
- Git Bash shim suite;
- `npm pack --dry-run`, `npm run release-check` и allowlist negative probes;
- реальные `caps` timeout/kill-tree и `capn` process-limit сценарии;
- published `0.1.0` → текущий tarball upgrade;
- syntax parsers, `actionlint`, installer или любые UAC/runtime smokes.

Commit message `6329eef` содержит авторские результаты предыдущих запусков
(`npm test 72/72`, Pester `255/0/3`, release-check green), но этот review их не
перезапускал и не считает независимо подтверждёнными. Статическое наличие тестов не
доказывает корректность runtime semantics, особенно для P1 normal-exit сценария,
которого в suite сейчас нет.

## Release checklist

- [ ] Исправить или явно переопределить normal-exit lifecycle из P1-1 и добавить
      clean-root-exit + surviving-grandchild regression.
- [ ] Исправить либо документировать sleep semantics `caps` из P2-1.
- [ ] Исправить два устаревших количества в Pester комментариях (P3-1).
- [ ] После исправлений выполнить полный Node 18/20/22/24 suite, обычный и
      elevated Pester, Git Bash shims и `npm run release-check`.
- [ ] Повторить реальный upgrade `0.1.0` → release tarball и negative allowlist
      probes.
- [ ] Push release commit и дождаться зелёной CI matrix.
- [ ] Только затем создать annotated `v0.2.0` и проверить publish/provenance.

До закрытия P1-1 выпуск `0.2.0` не рекомендуется.
