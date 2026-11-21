# Интеграция записи приёмки исполнителя v5 для Arbitrum (chainId 42161)

Статус: **блокер запуска Arbitrum**. Делает серверный ИИ на узле Arbitrum — не в агентах и не в этой сборке
(правка `live_record.rs` без папки записи ломает CI: `include_str!` на несуществующий файл). Этот файл —
точный патч, который серв применяет ПОСЛЕ записи на узле, в одном коммите с папкой данных.

## Зачем

Текущая сборка `contracts/out/` — v5 (`code::current_version()` → `Version::V5`, решение 488). Кнопка «Развернуть
исполнитель» закрыта, пока запись приёмки на живых пулах для пары (версия × chainId сети кошелька) не приняла хотя бы
один класс шага (`deploy_gate::refusal_at`, решения 400, 461). Вшитых записей две: v3 × chainId 56 (BSC) и
v4 × chainId 42161 (Arbitrum). **Записи v5 нет ни для одной сети.** Значит развернуть v5-исполнитель на кошельке
Arbitrum (arb_dex, `dex.evm_chain_id` = 42161) сейчас нельзя — `find(V5, Some(42161))` = None → `missing` → отказ.

Чтобы запустить Arbitrum, нужна запись **v5 × chainId 42161** на живых пулах сети Arbitrum.

## Факт по коду (проверено)

- `src/venues/bsc_dex/exec/executor/live_record.rs`: записи — массив `SOURCES: [Source; 2]`; `find(v, chain)` ищет по
  `j.version == v && j.chain == Some(chain)`; сеть записи читается `chain_in(chain_doc, summary)` — ровно одна строка
  `«- chainId: N»` в README (или README_repo.md) и совпадающее поле `chain_id` у строк summary.json.
- `verdicts` считает `live_rule::judge(readme, summary, reject, code::runtime_sha256_of(s.version))` — запись
  действительна, только если SHA-256 bin-runtime записи совпадает с bin-runtime версии в сборке
  (`code::runtime_sha256_of(Version::V5)` = хеш `contracts/out/ArbExecutor.bin-runtime`). Иначе класс не принимается.
- Модель папки — v4: `tests/data/executor_v4_live_2026-11-20/` (README.md, summary.json, hooks_reject.json,
  calls.json, pools.json, raw_rpc.json, slots.json). Код читает только README.md, summary.json, hooks_reject.json.
- Инструмент записи v4 — `tools/arbitrum_record/exec_v4_live.py` (решение 459, конвейер v4live_*). Для v5 — адаптировать
  его под сборку v5 (те же шаги + новые классы v5: AERO_BASIC = 26, BALANCER_V3_BUF = 27, CURVE_ETH = 28, решение 488).

## Шаги серва

1. **Записать на узле Arbitrum.** Запустить запись приёмки v5 против 292 живых пулов Uniswap V3, найденных поиском по
   фабрике на нашем узле Arbitrum (сейчас это единственное подключённое семейство — остальные DEX сети подключаются
   задачами покрытия, docs/notes/arbitrum/dex-coverage-arbitrum.md; запись приёмки нужна по тем классам шага, что реально
   есть на живых пулах). Инструмент — копия `tools/arbitrum_record/exec_v4_live.py` под v5 (bin-runtime из `contracts/out/`
   текущей сборки). Прогон — на том же узле Arbitrum One, что отдаёт пулы боту.

2. **Создать папку** `tests/data/executor_v5_live_<ГГГГ-ММ-ДД>/` с файлами (как у v4):
   - `README.md` — со строкой РОВНО `- chainId: 42161` (одна; `chain_in` требует ровно одну). Остальной текст — как в
     README v4 (дата, узел, H прогона, класс(ы), ссылки).
   - `summary.json` — массив строк прогона; у каждой строки с полем `chain_id` оно = `42161` (иначе `chain_in` вернёт
     ошибку и запись станет «broken» — отказ с причиной, деплой закрыт).
   - `hooks_reject.json` — отвергнутые крючки (как у v4).
   - (остальные файлы — для разбора/воспроизводимости, кодом не читаются, но кладутся в тот же коммит.)

3. **Патч `src/venues/bsc_dex/exec/executor/live_record.rs`** (в том же коммите):
   - `const SOURCES: [Source; 2]` → `[Source; 3]`.
   - Добавить третий элемент массива (по образцу элемента v4, заменив версию, папку и даты):
     ```rust
     Source {
         version: Version::V5,
         dir: "tests/data/executor_v5_live_<ГГГГ-ММ-ДД>",
         readme: include_str!(concat!(
             env!("CARGO_MANIFEST_DIR"),
             "/tests/data/executor_v5_live_<ГГГГ-ММ-ДД>/README.md"
         )),
         summary: include_str!(concat!(
             env!("CARGO_MANIFEST_DIR"),
             "/tests/data/executor_v5_live_<ГГГГ-ММ-ДД>/summary.json"
         )),
         reject: include_str!(concat!(
             env!("CARGO_MANIFEST_DIR"),
             "/tests/data/executor_v5_live_<ГГГГ-ММ-ДД>/hooks_reject.json"
         )),
         // chain_doc — README записи (в нём строка «- chainId: 42161»); отдельный README_repo.md не нужен,
         // если строка chainId есть прямо в README (у v4 chain_doc = её же README).
         chain_doc: include_str!(concat!(
             env!("CARGO_MANIFEST_DIR"),
             "/tests/data/executor_v5_live_<ГГГГ-ММ-ДД>/README.md"
         )),
     },
     ```
   - Дополнить шапку файла строкой SOURCES: «v5, chainId 42161 — tests/data/executor_v5_live_<дата> (свой узел
     Arbitrum One; H = …)» — как требует комментарий «Новая запись … — новая строка SOURCES … в том же коммите».

4. **Собрать в CI** (не локально). Тесты, уже проверяющие механизм: `tests/bsc_dex_executor_v4_live.rs` — сделать
   аналог `tests/bsc_dex_executor_v5_live.rs` (принятые классы, SHA-совпадение, chainId 42161) по его образцу.

5. **Проверить снятие запрета.** После сборки `deploy_gate::refusal_at(Some(42161))` для v5 должен вернуть `None`
   (принят ≥ 1 класс) — кнопка «Развернуть исполнитель» в блоке сети Arbitrum на «Маппинге кошельков» открывается
   (это и показывает правка решения 491: блок берёт отказ по своей сети). На BSC (chainId 56) записи v5 по-прежнему нет —
   это нормально: на BSC развёрнуты v3-исполнители.

## Порядок после записи (Тигран — вручную)

1. Серв записал v5 × 42161, собрал в CI, выкатил сборку.
2. Тигран на «Маппинге кошельков» → кошелёк Arbitrum (arb_dex) → «Развернуть исполнитель» (кнопка открыта).
3. Тигран включает направления Arbitrum (venue.enabled / направления решения 85) и жмёт «Старт».

До шага 1 Arbitrum-сделки DEX не исполняются (деплой закрыт) — запуск безопасен.
