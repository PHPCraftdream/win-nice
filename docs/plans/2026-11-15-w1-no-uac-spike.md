# W1 — спайк без UAC: приоритет дерева и CPU-квоты (Job Objects)

- **Дата:** 2026-11-15
- **Статус:** эксперимент завершён; все числа измерены на одной машине (Windows 10 19045, 16 логических CPU, неэлевированный процесс того же пользователя; WSL2 Ubuntu-24.04, WSL 2.7.10.0; рантайм спайка — только `windows-sys` 0.59). Вывод: **брокер UAC не нужен** для целевого сценария владельца (бенчмарки того же пользователя под 10% CPU + защита от вытеснения): непредвоснимаемая CPU-квота на job работает без привилегий, а HIGH-приоритет для всего дерева достигается бустером (job + IOCP + `SetPriorityClass`) без элевации.
- Интерпретации помечены явно («интерпретация», «не проверено»); остальное — измеренные числа или факты документации Microsoft Learn (fetching 2026-11-15).
- Карта отчёта: §1 контекст и вопросы → §2 метод (включая методологическое открытие о job-оболочке, §2.2) → §3 результаты (3.1 квоты, 3.2 бустер, 3.3 WSL2, 3.4 реальное дерево) → §4 решение → §5 рекомендации для W2 → §6 ограничения → §7 артефакты и воспроизведение; Приложение A — наблюдение о вложенных rate-лимитах.

## 1. Контекст и вопросы

### 1.1 Текущее поведение `wnice`

- `wnice -p high` сейчас: job-лимит `JOB_OBJECT_LIMIT_PRIORITY_CLASS` отклоняется без привилегии `SeIncreaseBasePriorityPrivilege` (Win32 error 1314) — `src/run/mod.rs:127-137`.
- Root-процесс всё же получает HIGH через creation flag в `CreateProcess` (`src/run/mod.rs:186`, `CREATE_SUSPENDED | creation_priority`).
- Потомки wnice остаются NORMAL: tree-wide приоритет текущим способом без привилегии недостижим.
- CPU-квота в wnice — только HARD_CAP: `src/limits.rs:258-280` (`ENABLE | HARD_CAP`, `CpuRate = percent * 100`).

### 1.2 Вопросы спайка

- **Q1.** Ставится ли CPU rate control (`HARD_CAP` и `MIN_MAX_RATE`) на job неэлевированно, и можно ли «вытеснить» квоту конкурентами с более высоким приоритетом?
- **Q2.** Можно ли неэлевированно довести HIGH до всех процессов дерева (job + IOCP + `JOB_OBJECT_MSG_NEW_PROCESS` → `OpenProcess` + `SetPriorityClass`) и с какой задержкой?
- **Q3.** Можно ли неэлевированно ограничить CPU vmmem/WSL2?
- **Q4.** Работает ли связка «внешний job + бустер» на реальном дереве глобально установленного wnice.exe (npm-установка win-nice)?

## 2. Метод

### 2.1 Стенд

- Мини-crate `spike/w1` (вне workspace, edition 2021, единственная зависимость `windows-sys` 0.59): общие хелперы job/IOCP/бустера в `src/common.rs`, подкоманды `env`, `selfset`, `exp1`, `exp2-latency`, `exp2-chain`, `wslprobe`, `wnrun` (плюс внутренние `burner`, `leaf`, `leafprio`, `lat-probe`, `chain`).
- Конкурирующая нагрузка: 12 процессов-«прожигателей» в собственном job с HARD_CAP 50% машины, обычный приоритет; варианты сценария — `THREAD_PRIORITY_HIGHEST` (база 10) или буст всей группы до HIGH бустером (флаг `--boosthigh`).
- Измеряемый job: 10 процессов-прожигателей (потенциально ~62.5% машины на 16 CPU), окно 10 c, CPU-доли через `GetProcessTimes`, 2 повтора, берётся медиана.

### 2.2 Важное методологическое открытие: job-оболочка портит измерения

Первый прогон шёл внутри Job Object процесса-оболочки эксперимента (агент, запускающий измерения, имел собственный Job Object). Результаты этого прогона невалидны:

- IDLE-кламп: `SetPriorityClass` возвращает «ok», но класс остаётся 64 (IDLE);
- breakaway запрещён, gle=5;
- фактический кап ~9% машины на измеряемый job;
- вложенные rate-лимиты умножаются: вложенный «10%» дал ~0.9% машины, «50%» — ~4.5% (см. Приложение A).

Повторный прогон выполнен через WMI `Win32_Process.Create`: запущенный процесс не наследует job оболочки; тот же неэлевированный пользователь; UAC не запрашивался — это тот же «broker»-механизм, что задокументирован в README win-nice как исключение из tree-лимитов.

Гейт валидности job-free прогона: у WMI-процесса p1 `SetPriorityClass(HIGH)` → `class_now=128` (HIGH); стартовый класс NORMAL (32).

**Все числа ниже — из job-free прогона** (кроме отдельно помеченных наблюдений sandbox-прогона).

### 2.3 Факты из документации (Microsoft Learn, fetching 2026-11-15)

- `JOBOBJECT_CPU_RATE_CONTROL_INFORMATION`: флаги `ENABLE` 0x1, `WEIGHT_BASED` 0x2, `HARD_CAP` 0x4, `NOTIFY` 0x8, `MIN_MAX_RATE` 0x10; `MIN_MAX_RATE` не сочетается с `HARD_CAP` и `WEIGHT_BASED`; ставки задаются как «percentage times 100».
- `MinRate` — «minimum portion … reserve during each scheduling interval»; сумма `MinRate` всех job'ов системы ≤ 100%. `MaxRate` — «After the job reaches this limit for a scheduling interval, no threads … until the next scheduling interval».
- Минимальные версии: Windows 8 / Server 2012. Отдельных версионных ограничений на `MinRate` в доках нет; упомянуто ограничение для Remote Desktop + DFSS.
- Job-лимит приоритета (`JOBOBJECT_BASIC_LIMIT_INFORMATION.PriorityClass`): «Processes and threads cannot modify their priority class. The calling process must enable the SE_INC_BASE_PRIORITY privilege» — привилегия требуется именно на установку job-лимита. В статье `SetPriorityClass` требование привилегии НЕ упоминается (ниже подтверждено практикой).
- `.wslconfig`, секция `[wsl2]`: `processors` — только число логических процессоров; `memory` — RAM. Процентной квоты CPU хоста в доках WSL нет.

### 2.4 Обозначения в таблицах

- `main` (main_pct_of_machine) — суммарная доля CPU машины (16 логических CPU = 100%), потреблённая измеряемым job за окно 10 c по `GetProcessTimes`.
- `comp` — суммарная доля CPU 12 процессов-конкурентов.
- `share_of_busy` — доля измеряемого job среди занятого CPU (main / (main + comp)).
- `boost N ok / M fail` — число успешных/неуспешных `SetPriorityClass` бустера по процессам конкурентов за прогон.

## 3. Результаты по вопросам

Сводка ключевых чисел (детали и метки доверия — в §3.1–3.4):

| Что | Результат |
|---|---|
| `HARD_CAP` 10% под конкурентами HIGH | 9.74% машины (`hard10-boosthigh`) |
| `MIN_MAX` Min=Max=10% под любыми конкурентами | 9.24–9.88% машины |
| floor `MinRate=10%` (Max=100%) | 24.60–25.22% машины, floor не нарушен |
| без управления (`none-normal`) | 48.04 против 49.42% — честное разделение ~пополам |
| бустер: доставка `NEW_PROCESS` | 0 пропусков (30 + 56 + 3×2 процессов) |
| бустер: задержка до HIGH | median 0.004 мс; до ~35 мс в burst (gen2) |
| vmmem/WSL2 неэлевированно | `OpenProcess` — gle=5 на все пробы |

### 3.1 Q1 — CPU rate control без привилегий

Приём конфигураций неэлевированно:

- `SetInformationJobObject` с `MIN_MAX_RATE` (Min = Max = 1000, т.е. 10%/10%) принимается: в логах `exp1` нет ни одной строки `exp1_error` (`gle=0`) для конфигураций `soft10` и `softfloor`.
- `HARD_CAP` тоже принимается (это и есть существующий `capc` в wnice).

Таблица (main_pct_of_machine — % от 16 логических CPU; comp — суммарная доля конкурентов; share_of_busy — доля измеряемого job среди занятого CPU; boost — конкуренты подняты бустером до HIGH, 12/12 `ok` в каждом прогоне; все значения — медиана 2 повторов):

| Сценарий | main, % машины | comp, % | share_of_busy | boost |
|---|---|---|---|---|
| `hard10-alone` | 9.62 (9.44 / 9.79) | — | — | — |
| `hard10-normal` | 9.58 | 49.05 | 0.163 | — |
| `hard10-boosthigh` | 9.74 | 46.57 | 0.173 | 24 ok / 0 fail |
| `soft10-alone` (MIN_MAX Min=Max=10%) | 9.36 | — | — | — |
| `soft10-normal` | 9.24 | 47.50 | 0.163 | — |
| `soft10-threadhigh` (конкуренты `THREAD_PRIORITY_HIGHEST`, база 10) | 9.85 | 46.79 | 0.174 | — |
| `soft10-boosthigh` | 9.88 | 46.12 | 0.177 | 24 ok |
| `none-normal` (без управления) | 48.04 | 49.42 | 0.493 | — |
| `softfloor-normal` (MIN_MAX Min=10%, Max=100%) | 25.22 | 49.35 | 0.338 | — |
| `softfloor-boosthigh` | 24.60 | 48.93 | 0.334 | 24 ok / 0 fail |

Выводы:

- **ИЗМЕРЕНО:** и hard cap, и `MIN_MAX` с Min = Max = 10% держат ~9.2–9.9% машины независимо от конкурентов (обычные / база-10 / HIGH) — «вытеснить» квоту приоритетом конкурентов НЕ УДАЛОСЬ ни в одной конфигурации.
- **ИЗМЕРЕНО:** floor-only `MinRate = 10%` ни разу не нарушен (main ≥ ~24.6% при demand 62.5%); HIGH-буст конкурентов меняет результат в пределах шума (25.2 → 24.6).
- **ИЗМЕРЕНО:** контрольная конфигурация `none-normal` (без управления) показывает честное разделение ~пополам (48.04 / 49.42, share 0.493).
- **ИНТЕРПРЕТАЦИЯ:** на 19045 `MIN_MAX` с Min = Max ведёт себя как hard cap (9.36% alone — «soft»-лимит режет и без конкуренции).
- **ИНТЕРПРЕТАЦИЯ:** floor не был биндингом; истинный «голодный» тест невозможен при лимите конкурентов ≤ 50% (пусть даже с demand main 62.5% суммарно ~112%).
- **ИНТЕРПРЕТАЦИЯ / НЕ ПРОВЕРЕНО:** «никто не вытеснит» в смысле «чужие пользователи/админ» не тестировалось — без элевации невозможно.

### 3.2 Q2 — бустер (job + IOCP + `JOB_OBJECT_MSG_NEW_PROCESS`)

Схема бустера: IOCP привязан к job (`JOBOBJECT_ASSOCIATE_COMPLETION_PORT`), поток-бустер читает пакеты; на `NEW_PROCESS` — `OpenProcess(QUERY_LIMITED | SET_INFORMATION)` + `SetPriorityClass(HIGH)`.

Приёмочные проверки `selfset` (неэлевированно):

| Проба | Действие | Результат |
|---|---|---|
| p1 | `SetPriorityClass(HIGH)` на себя | ok, класс 128 (HIGH) |
| p2 | `SetPriorityClass(ABOVE_NORMAL)` | ok (32768) |
| p4 | suspended-child через creation flag | ребёнок стартует 128 (HIGH) |
| p5 | `OpenProcess` по PID + `SetPriorityClass` | ok |
| p6 | job-лимит HIGH (`JOB_OBJECT_LIMIT_PRIORITY_CLASS`) | fail gle=1314 (премисса) |
| p7 | `SetThreadPriority(THREAD_PRIORITY_HIGHEST)` | ok |

Доставка сообщений (`exp2-latency`, n=30 детей):

- 30 `NEW_PROCESS`, 0 пропусков; `set_ok=30`, `set_fail=0`, `dead_at_boost=0`.
- Счётчик сообщений: `NEW_PROCESS=30`, `EXIT_PROCESS=30`, `ACTIVE_PROCESS_ZERO=11`.

Задержка до смены приоритета (ребёнок сам поллит `GetPriorityClass` с первой инструкции; 30/30 увидели HIGH):

| Статистика | Значение |
|---|---|
| median | 0.004 мс |
| p90 | 0.006 мс |
| max | 0.007 мс |
| отдельные случаи | 0.000–0.009 мс |

Цепочка 3 поколений (`exp2-chain`: 8 gen2 × 6 gen3 = 48 листьев; времена жизни листьев 5/10/40/120/250/400 мс):

- 56/56 `NEW_PROCESS`, `set_ok=56`, fail=0, dead=0.
- ВСЕ 48 листьев, включая 5-мс и 10-мс, успели увидеть HIGH (latency 0.010–0.045 мс).
- gen2: 8/8, latency 28.6–35.2 мс — массовый burst-спавн 8 процессов подряд, бустер нагоняет очередь (интерпретация).

Payload-факт (важно для реализации):

- В текущей странице MS Learn `JOBOBJECT_ASSOCIATE_COMPLETION_PORT` message id лежит в `lpNumberOfBytesTransferred`, а PID — в значении `lpOverlapped` (исторически доки описывали наоборот).
- Рантайм подтвердил: PID из `lpOverlapped` — 30/30 `open`/`set` успешны; в `wnrun` PID'ы записей бустера совпали с self-reported PID детей.
- В `windows-sys` 0.59 констант `JOB_OBJECT_MSG_*` нет — использован литерал `6`; `NEW_PROCESS = 6`, НЕ 4 (4 — это `ACTIVE_PROCESS_ZERO`).

### 3.3 Q3 — WSL2/vmmem

- Найдены: vmmem (pid один), `wslservice.exe`, `vmwp.exe`.
- Для vmmem все три пробы `OpenProcess` неэлевированно — отказ:

| Проба | Права доступа | Результат |
|---|---|---|
| a | `QUERY_LIMITED \| VM_READ` | gle=5 (Access denied) |
| b | `QUERY_LIMITED` | gle=5 |
| c | `SET_INFORMATION \| SET_QUOTA` | gle=5 |

- `wslservice` и `vmwp`: probe b (`QUERY_LIMITED`) → gle=5; probe c (`SET_INFO | SET_QUOTA`) → gle=5.
- Вывод: неэлевированно хост-процесс VM не открывается даже на чтение мета-информации → job-квоты на vmmem поставить нельзя.
- Замер сделан после boot WSL через read-only `wsl.exe -d Ubuntu-24.04 -e cat /etc/os-release`; обе дистрибуции в остальное время Stopped.
- Документированный способ ограничения CPU хоста WSL2: только `processors=N` (число vCPU) в `.wslconfig`; процентной доли нет (`memory` есть, CPU-% нет).
- **ИНТЕРПРЕТАЦИЯ:** ограничение числа vCPU ограничивает параллелизм, но не duty-cycle.

### 3.4 Q4 — реальное дерево wnice

Глобально установленный wnice.exe (npm-установка win-nice), цепочка wnice→cmd→child, внешний job + бустер:

```text
wnice -c 10 -- cmd /c <child>          # вариант A
wnice -p high -c 10 -- cmd /c <child>  # вариант B
```

- **Вариант A (без `-p`):** бустер получил 3 `NEW_PROCESS` (wnice, cmd, child), `open_ok=set_ok=true`, gle=0; child (внук wnice) сам отчитался `class_at_t0=128` (HIGH), `saw_high=true`, latency 0.000 мс.
- **Вариант B (`-p high`):** то же — 3/3 `set_ok`, child HIGH; stderr wnice дословно: «wnice: note: high priority applies to the root process only (the job refused tree-wide priority: Win32 error 1314)» — премисса воспроизведена; бустер при этом довёл HIGH до внука.
- **Ограничение:** всё дерево wnice живёт <100 мс — периодический сэмплер (100 мс) не поймал ни одного сэмпла; per-pid данные — из записей бустера и self-report ребёнка.

## 4. Решение: брокер UAC не нужен

**Для целевого сценария владельца** (бенчмарки того же пользователя под 10% CPU + защита от вытеснения) **брокер UAC НЕ НУЖЕН.**

1. **Непредвоснимаемая квота уже есть.** Hard cap 10% держится под конкурентами HIGH: 9.74% measured (`hard10-boosthigh`). Приоритет конкурентов не влияет на rate-квоту ядра (Q1).
2. **Дерево-HIGH без UAC достижимо бустером** (job + IOCP + `SetPriorityClass` по `NEW_PROCESS`): 0 пропусков на 92 процессах суммарно (30 + 56 + 3 + 3), задержки до ~35 мс в burst, даже 5-мс листья покрыты (Q2, Q4).
3. **Единственный отказ** (1314 на job-лимит приоритета) обходится бустером. UAC нужен только если:
   - (a) процессы ДРУГИХ пользователей / чужие HIGH-приоритеты должны вытесняться-гарантироваться — не тестировалось и недостижимо без элевации;
   - (b) требуется управление vmmem/WSL (`OpenProcess` = 5, Q3);
   - (c) решено полагаться на job-лимит приоритета вместо бустера.

## 5. Рекомендации для W2

- Добавить в wnice опциональный режим «booster»: тот же job + IOCP + поток-бустер; на `NEW_PROCESS`: `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_INFORMATION)` + `SetPriorityClass`; PID брать из `lpOverlapped` (документировать расхождение доков); корректное завершение потока через `PostQueuedCompletionStatus`.
- Задержки: 0–35 мс (burst); листья короче ~5 мс могут умереть до буста — для билд-инструментов неактуально.
- Квоту оставить на `HARD_CAP`; `MIN_MAX_RATE` не даёт преимуществ на 19045 (Min=Max ≡ hard cap по числам; floor-only режет ниже fair share — применять только если нужен именно floor); не комбинировать `MIN_MAX_RATE` с `HARD_CAP`/`WEIGHT_BASED`.
- Fallback без изменения класса: `THREAD_PRIORITY_HIGHEST` (база 10) — работает без привилегий (p7).
- Предупредить в документации: вложенные CPU rate умножаются (`capc` в `capc`), и job-лимит приоритета выше NORMAL требует привилегии (1314) — бустер её не требует.
- Для «10% у WSL2» — только `.wslconfig` `processors=N` (число vCPU), % нет; vmmem без элевации недоступен (gle=5).

## 6. Ограничения и что не проверено

- Сэмплинг в `wnrun`: период 100 мс, дерево wnice живёт <100 мс — ни одного сэмпла не поймано; per-pid данные только из записей бустера и self-report ребёнка.
- Payload-доказательство для `exp2-latency` косвенное (30/30 `open`/`set` по PID из `lpOverlapped`) + прямое в `wnrun` (совпадение PID'ов бустера с self-reported PID детей).
- Голод при demand глубже ~112% (main 62.5% + конкуренты 50%) не создавался — floor как биндинг не проверен.
- Чужие пользователи, `REALTIME`, другие машины/версии Windows — не тестировалось.
- Время замеров: ~5 мин активной нагрузки суммарно в job-free прогоне + ~4 мин в невалидном первом прогоне.
- Sandbox-прогон (под job-оболочкой) невалиден для абсолютных чисел — сохранён только как наблюдение о вложенных rate-лимитах (Приложение A).

## 7. Артефакты и воспроизведение

### 7.1 Артефакты

- Код: `spike/w1/{Cargo.toml,.gitignore,src/main.rs,src/common.rs,src/exp1.rs,src/exp2.rs,src/exp34.rs}` (5 rs-файлов, все ≤1000 строк, lint:structure ok).
- Логи: `spike/w1/out/*.log`: `env`, `selfset`, `wmi-selfset`, `exp2-lat`, `exp2-chain`, `exp1`, `wmi-exp1`, `wmi-exp1-softfloor`, `wslprobe`, `wmi-wslprobe`, `wnrun-a`, `wnrun-b`, `wmi-wnrun-a`, `wmi-wnrun-b`, `wsl-list`, `wsl-version`.
- Префикс `wmi-` в имени лога означает запуск через WMI-обёртку из §7.4; остальные — прямые запуски из §7.3. Все числовые результаты отчёта — из job-free прогонов; строки `exp1_error` отсутствуют для конфигураций `soft10`/`softfloor`.

### 7.2 Сборка

```text
cd spike/w1
cargo build --release
```

Бинарь: `spike/w1/target/release/w1-spike.exe`.

### 7.3 Прямые команды (запускать только вне любой job-оболочки — см. §2.2)

```text
target/release/w1-spike.exe env
target/release/w1-spike.exe selfset
target/release/w1-spike.exe exp1 --seconds 10 --reps 2
target/release/w1-spike.exe exp1 --seconds 10 --reps 2 --boosthigh
target/release/w1-spike.exe exp1 --only softfloor --seconds 10 --reps 2 --boosthigh
target/release/w1-spike.exe exp2-latency --n 30
target/release/w1-spike.exe exp2-chain
target/release/w1-spike.exe wslprobe
target/release/w1-spike.exe wnrun --wnice <path-to-wnice.exe> --variant A
target/release/w1-spike.exe wnrun --wnice <path-to-wnice.exe> --variant B
```

(`<path-to-wnice.exe>` — путь к глобально установленному wnice.exe (npm-установка win-nice); `--only` — фильтр сценариев `exp1` по подстроке имени.)

### 7.4 WMI-обёртка (job-free запуск)

Общий вид (плейсхолдер `<repo>` — корень репозитория; обёртка `spike/w1/out/wmi-run.cmd` переходит в каталог `spike/w1`, запускает бинарь со всеми аргументами и пишет вывод в `out/last-wmi.log`):

```powershell
powershell -Command "Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = 'cmd.exe /c <repo>\spike\w1\out\wmi-run.cmd <args>'; CurrentDirectory = '<repo>\spike\w1' }"
```

`<args>` — те же аргументы подкоманд, что в §7.3. WMI-процесс гейтится пробой `selfset`: `SetPriorityClass(HIGH)` → `class_now=128` (HIGH); стартовый класс NORMAL (32).

Прямые запуски §7.3 внутри чужой job-оболочки дают невалидные числа — см. §2.2.

## Приложение A. Самоустранение: sandbox-прогон и вложенные rate-лимиты

Первый прогон под job-оболочкой дал искажённые числа и исключён из результатов: IDLE-кламп (`SetPriorityClass` «ok», класс остаётся 64), кап ~9% машины, вложенные rate-лимиты умножаются: вложенный «10%» дал ~0.9% машины, «50%» — ~4.5%, `none` дал 4.5+4.5=9.0. Наблюдение «nested CPU rate умножается» сохранено и согласуется с доками: rate вложенного job — доля от родительского.
