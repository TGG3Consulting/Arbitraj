> **ЖЕЛЕЗОБЕТОННОЕ ПРАВИЛО ТИГРАНА (20.11.2026):** «Не пропустить ни единого DEX. Все DEX, которые есть, — без разделения на мелкие/крупные, без «можно потом». Ты ничего не решаешь в плане приоритетов.»

# Две сети, шаг 5: площадка DEX сети Arbitrum (`arb_dex`) — решение по архитектуре и план (решение 433, 19.11.2026)

Основа: ветка `t217/arb-step5-adapter` от `feat/arbitrum` `1bbe7864` (шаги 1–2 сделаны: решения 416, 430).
Источник — только факт кода (номера строк — на `1bbe7864`), записанные ответы своего узла Arbitrum
(`tests/data/arbitrum_node_2026-11-19/`) и официальная документация Arbitrum (`docs/notes/arbitrum/docs-check-2026-11-20.md`).
cargo локально не запускался — проверка в CI. Параллельно идёт шаг 4 (ветка `t216/arb-step4-launch`): кошелёк «глазами
площадки» (ctx_wallet.rs, pools/exec_callback.rs, exec/executor/onchain.rs, check/pass_view.rs, check/pass_cycle.rs,
развёртывание исполнителя по площадке) — эти файлы шаг 5 не трогает.

## 1. Решение: один адаптер на все EVM-сети, вторая площадка того же кода с id `arb_dex`

**Не копировать 689 файлов `src/venues/bsc_dex/`.** Факты кода, на которых решение держится:

1. **Все настройки адаптера читаются через область площадки** — `Ctx::at()` = `At { venue: Some(ID) }`
   (venues/bsc_dex/ctx.rs:62-67), `ctx.int/dec/text/flag/addr` (ctx.rs:70-98). Значит, второй экземпляр с другим id
   читает свои значения тех же имён (`dex.chain`, `dex.evm_chain_id`, `dex.gas_ticker`, `index.*`, `exec.*`, `send.*`,
   `feed.*`, `venue.*`) без единой правки в местах чтения. Порядок поиска значения — монета → площадка → вся система
   (settings/store.rs:154-179): значение «вся система» остаётся общим запасным, значение площадки — сильнее.
2. **Адресов контрактов в коде нет** — все из настроек (разбор шага 0: multichain-plan, «Адаптер DEX»): фабрики,
   роутеры, квотеры, Permit2, Multicall3, обёртка, исполнитель — `dex.*` в области «DEX» и `exec.*` по площадке.
3. **Математика, котировка, граф, пробы (eth_simulateV1), узел (JSON-RPC, WS, multicall)** — EVM-общие
   (src/venues/bsc_dex/math, quote/, node/). На своём узле Arbitrum подтверждено фактом: eth_call со state override
   (02_*.json), eth_simulateV1 (03_*.json), eth_estimateGas (04), WS newHeads и logs (06, 07).
4. Имя папки `bsc_dex` остаётся (переименование = правка всех путей, README, тестов и ссылок в журнале решений —
   отдельной механической задачей, когда обе сети заработают; на поведение не влияет). Тип `BscDex` — тоже.

### Что по сети, а что общее

| Часть | Общее (код один) | По сети (настройка площадки DEX или модель сети) |
| --- | --- | --- |
| Сеть | — | `dex.chain`, `dex.evm_chain_id` (решение 416), сверка eth_chainId перед подпиской (node/chain_id.rs) |
| Газ: тикер, обёртка | код | `dex.gas_ticker`, `dex.native_ticker`, `dex.native_wrapped` (уже по площадке) |
| Газ: модель | расчёт | **новое** `dex.gas_mode`: `legacy` (BSC — как сейчас, send/tx.rs:144-170) / `eip1559_l1` (Arbitrum: тип 2, газ = eth_estimateGas с L1-частью; компоненты — NodeInterface `dex.node_interface`) |
| Узел HTTP/WS | транспорт | `bsc.node_*` (21 настройка, сейчас «вся система», читаются через `At::default()`) → область «вся система + площадка»: чтение `At{venue}` — у bsc_dex прежнее глобальное значение, у arb_dex своё |
| Справочник пулов | directory/ | `index.network` (уже по площадке: `bsc` / `arbitrum`), `index.*` |
| DEX и виды пулов | pools/, math/ | **ключ области «DEX» с сетью** (см. 1.2): id GeckoTerminal у сетей пересекаются |
| Каналы отправки | send/ (выбор, трекер, отмена) | список каналов по сети: у BSC 14 каналов билдеров (send/channels.rs:14-29); у Arbitrum — **канал секвенсора** (адрес — настройка), бандлов нет |
| Исполнитель | контракт и сборка | адрес — по площадке (`exec.executor_address`, шаг 4 — по кошельку и площадке); развёртывание — в каждой сети |
| Индексатор | код | `index.network`, ключ CoinGecko — общий секрет |
| Время блока | node/block_ms.rs (BEP-520, BSC) | у Arbitrum timestamp в секундах — без доли из mixHash (откат к секундам уже есть) |
| Опрос головы | node/heads.rs (`bsc.blocks_poll_ms`) | у Arbitrum блок 250 мс — значение по площадке; подписка newHeads по WS работает (06_*.json) — отдельная задача |

### 1.1 Id площадки — из окружения, а не константа

| Место | Сейчас | Станет |
| --- | --- | --- |
| venues/bsc_dex/ctx.rs:26 | `pub const ID = "bsc_dex"` | поле `Ctx.id: &'static str` (из main.rs; строка живёт весь процесс) |
| ctx.rs:58, :64, :204 | `ID` | `self.id` |
| errors.rs:37, :159 (`err`, `bad` без Ctx) | площадка ошибки = `bsc_dex` | вызывающие с Ctx — `ctx.err`; остаток `errors::err` без Ctx — площадку проставляет шлюз (gateway, `tag`) |
| node/ws_task.rs:139, :184 | сырые сообщения и журнал под `bsc_dex` | `ctx.id` |
| settings_wait.rs:48 | `At{venue: Some(ID)}` | `ctx.at()` |
| directory/store.rs:68-133, directory/gt_http.rs:70 | venue_state и сырой журнал под `bsc_dex` | `ctx.id` (ключи `net` уже по `index.network`) |
| pools/history_store.rs:49-96, pools/deploy_block.rs:38-59 | venue_state под `bsc_dex` | `ctx.id` (у bsc_dex ключи базы — прежние) |
| main.rs:280-313 | один `BscDex` | второй `BscDex` с `id = "arb_dex"` — **только если задана настройка «вся система» `dex.venues` со списком площадок DEX** (см. 3); без неё — как сейчас один bsc_dex |
| main.rs:300-304 (перенос кошелька 132) | `ID` | остаётся `bsc_dex` (перенос относится только к прежнему кошельку BSC) |
| core/deal_card.rs:25 | `const DEX = "bsc_dex"` — кошелёк сделки показывается только у bsc_dex | признак «площадка DEX» — `Deal.dex_account` есть (он заполняется только у площадки DEX), без сравнения с id; ядро не знает id адаптера |
| notify/human.rs:50 | `"bsc_dex" => "DEX"` | имя площадки DEX — `dex.chain` в тексте («DEX bsc», «DEX arbitrum») |
| secrets/mod.rs:186-237 | ключи узла `bsc.node_*_key` — одни | свой узел Arbitrum без ключа ({key} нет — секрет не нужен, ctx_url.rs:28-35); имя секрета по площадке — задача, когда понадобится провайдер с ключом |

### 1.2 Область «DEX» с сетью

Факт кода: вид пула (`Family`) = id DEX индексатора (pools/families.rs:405-428); `families::all` берёт **все** `dex.kind`
из настроек (families.rs:432-445), `dex_ids::extra` — все id области «DEX» (pools/dex_ids.rs:91-97),
`event_flags` — по `Scope::Dex(f.id)` (pools/event_flags.rs:90); таблица видов `family_intern` — по id
(pools/family_intern.rs:24). У GeckoTerminal id DEX Arbitrum без метки сети: `camelot`, `camelot-v3`, `camelot-v4`,
`ramses`, `ramses-v2`, `chronos`, `zyberswap`, `3xcalibur`… (geckoterminal/dexes_page1.json); у BSC 72 из 139 id без
метки сети (docs-check). Без разделения второй экземпляр подхватит виды и адреса DEX BSC и наоборот.

Правило (как ключи пар решения 430: у сети по умолчанию — прежние, у другой — с суффиксом «@сеть»):
`Ctx::dex_key(id)` = `id`, если `dex.chain` площадки = сеть по умолчанию (`dex.default_chain`; не задана — сеть
по умолчанию, как при одной DEX), иначе `id@<dex.chain>` (`camelot@arbitrum`). Через `dex_key` идут: ctx.rs:101-150
(`dex_addr/flag/int/dec/text/list`), families.rs:432-445 (`all` — только ключи своей площадки), :471,
dex_ids.rs:91-97, event_flags.rs:90, разбор справочника (id пула → ключ). `Family.id` = ключ (уникален между
сетями, family_intern без правок). Интерфейс: экран DEX показывает ключ как есть («camelot@arbitrum»).
У bsc_dex все ключи прежние байт в байт — живые настройки BSC не переносятся.

### 1.3 Общие на процесс переменные (статики) — по площадке

Ключ без площадки ломается, когда у двух сетей совпадает адрес (детерминированные развёртывания CREATE2:
Permit2, Multicall3, исполнитель с тем же nonce, токены-мосты). Разбор по файлам:

| Файл:строка | Ключ | Что делать |
| --- | --- | --- |
| exec/executor/onchain.rs:37 `CHECKED` | исполнитель | по площадке (**шаг 4 правит onchain.rs — после слияния**) |
| exec/executor/v1_notice.rs:22 `SENT` | (кошелёк, исполнитель) | + площадка |
| exec/executor/pool_pass.rs:90-91 `FACTS`, `KINDS` | пул/исполнитель | + площадка |
| exec/executor/callback_at.rs:25 `SEEN` | адрес | + площадка |
| exec/presell_slot_find.rs:48 `CACHE` | токен | + площадка |
| pools/registry_key.rs:38 `KEYS` | пул | + площадка |
| pools/exec_callback.rs:49 `SEEN` | пул (**шаг 4**) | + площадка после слияния |
| pools/load_curve_ng.rs:33, load_balancer_fixed.rs:37, prefetch_memo.rs:26, fee_probe.rs:37, ladder_counter.rs:42, load_smardex.rs:77, solidly_gate_read.rs:87, off_map_bins_log.rs:33 | пул/вид | + площадка (load_smardex `BLOCK_TIME` — время блока сети: только по площадке) |
| quote/bound.rs:48 `MEMO`, quote/cycle_tally.rs:217 `LAST`, quote/cycle_audit.rs:55-63 | пул/цикл | + площадка |
| send/tip_gas.rs:38 `UNITS` | (кошелёк, адрес, сумма) | + площадка (газ перевода чаевых в разных сетях разный) |
| node/chain_id.rs:90 `LAST_WARN`, check/hooks_head.rs:119 `FLAT_SAID` | текст / флаг | + площадка (иначе предупреждение одной сети глушит другую) |
| wallet/auto_release.rs:49 `RUNNING` | хеш транзакции | общий (хеш уникален между сетями) — без правок |
| quote/cpu.rs:53, quote/par_pool.rs:50, quote/cpu_local.rs, quote/shallow_memo.rs, quote/quote_timing.rs, quote/abandon.rs, node/prefetch.rs:119, node/req_trail.rs:37 | процессор / задача | общие на процесс намеренно (ядра процессора одни на обе сети; задача котировки — одной площадки) — без правок |
| exec/executor/live_proof.rs:47, math/ten.rs:26, math/balancer_log_exp_table.rs:40, check/state.rs:89 | константы кода/записи | общие — без правок |

Решение: тип `VenueKey = (&'static str, …)` — id площадки первым полем ключа (берётся из `ctx.id`).

### 1.4 Газ и транзакция Arbitrum (официальная документация — docs-check-2026-11-20.md)

- Транзакция — EIP-1559 (тип 2): `maxFeePerGas`, `maxPriorityFeePerGas` (ставка PGA), chainId — `dex.evm_chain_id`.
  Подпись — та же (wallet/signer.rs:62-91 уже обобщена по виду транзакции).
- Лимит газа — `eth_estimateGas` (уже включает L1-часть: факт 04 = 0x5765 = `gasEstimate` из 05). Компоненты —
  NodeInterface `gasEstimateComponents(to, contractCreation, data)` → (gasEstimate, gasEstimateForL1, baseFee,
  l1BaseFeeEstimate); адрес NodeInterface — настройка `dex.node_interface` (документация: 0x…C8).
- Стоимость в плане: `gasEstimate × min(maxFee, baseFee + tip)` — верхняя граница по правилу EIP-1559. Документация
  Arbitrum не говорит, берутся ли чаевые с L1-части газа (вопрос Тиграну 1) — взят худший вариант (со всего газа);
  факт — `effectiveGasPrice × gasUsed` квитанции.
- `maxFeePerGas` = baseFee × запас + tip; запас и tip — настройки площадки (задача 433-д, вопрос 2: сколько чаевых).
- Отправка — секвенсор (`eth_sendRawTransaction` на адрес-настройку), мемпула и бандлов нет → DEX↔DEX на Arbitrum —
  только одной транзакцией через исполнитель (бандл свопов не применяется).

## 2. Порядок работ (BSC без изменений на каждом шаге)

| № | Что | Где | Статус |
| --- | --- | --- | --- |
| 433-а | План и таблица покрытия DEX Arbitrum | docs/notes/arbitrum/step5-plan.md, dex-coverage-arbitrum.md | сделано (таблица: 84 GeckoTerminal + 62 DefiLlama, у каждого DEX статус и задача T433-NNN; семейства — T433-F1…F8) |
| 433-б | Модель газа площадки `dex.gas_mode` и разбор NodeInterface `gasEstimateComponents` (`dex.node_interface`), стоимость газа Arbitrum | send/gas_mode.rs, node/arb_gas.rs, connection_gas.rs | сделано: настройки dex.gas_mode, dex.node_interface; разбор и вызов NodeInterface; часть «Газ сети» в «Проверить подключение»; тесты на записях 04/05 |
| 433-в | Сборка транзакции EIP-1559 (тип 2) | send/tx_1559.rs | сделано: сборка, maxFee/чаевые, цена по EIP-1559; тест подписи типа 2 на chainId и baseFee из записей 01/05. В отправку — 433-д |
| 433-г | Id площадки из окружения (1.1) | ctx.rs и 7 файлов | сделано (решение 435): Ctx.place (ctx_place.rs); deal_card.rs, human.rs — решением 432 |
| 433-д | Включение `dex.gas_mode` в отправку: exec/txs.rs:118-149, exec/bundle_send.rs, send/cancel.rs (отмена — тип 2 с повышением обеих цен), wallet/transfer.rs, quote/gas.rs (baseFee головы вместо eth_gasPrice), предпроверка газа exec/gas_fund.rs; запас `maxFee` и чаевые — настройки | send/, exec/, quote/ | сделано (решение 435): quote/gas.rs, gas_base.rs, exec/tx_fee.rs, send/tx_price.rs; send.max_fee_margin_pct, send.priority_fee_gwei; бандл — только legacy |
| 433-е | Канал секвенсора: отдельный канал списка send/channels.rs с адресом-настройкой, одиночная отправка; бандл на площадке без бандловых каналов — отказ до отправки с причиной | send/sequencer.rs | сделано (решение 435): sequencer.rs, channel.chain (channels.rs serves); conditional — сделан решением 436 (формат по исходникам Nitro, send/sequencer_cond.rs) |
| 433-ж | Узел по площадке: `bsc.*` → «вся система + площадка», чтение `At{venue}` (27 мест `At::default()` в адаптере) | catalog/dex.rs:65-86, node/*, ctx_url.rs:22 | сделано (решение 435): 21 настройка bsc.* — GV; узловые места At::default() → ctx.at(); остальные At::default() — не узел (общие намеренно) |
| 433-з | Область «DEX» с сетью (1.2) | ctx.rs, families.rs, dex_ids.rs, event_flags.rs, directory | сделано (решение 435): перевод на входе справочника (directory/fetch.rs), Family.id — ключ (ctx.rs dex_*, event_flags без правок), свои ключи — families::all, dex_ids::extra; check_dex_id допускает «@» |
| 433-и | Статики по площадке (1.3) | 20 файлов | сделано (решение 435); без правок по факту кода: callback_at/exec_callback SEEN (ключ — версия и сигнатура артефакта), prefetch_memo (уже по VenueId), cycle_tally (уже по площадке), fee_probe/ladder_counter/solidly_gate_read (id DEX — ключ с сетью) |
| 433-к | Регистрация `arb_dex` в main.rs по настройке `dex.venues`; до неё — проверка, что живой BSC задал `dex.default_chain` (решение 430: две DEX без неё — ошибка) | main.rs | сделано (решение 435): dex_venues.rs, Ctx::for_place; читается при запуске |
| 433-л | Голова по WS newHeads при блоке 250 мс (опрос eth_blockNumber — 4 запроса/с) | node/heads.rs | задача (вопрос 3) |
| 433-м | «Проверить подключение» и подписка arb_dex целиком (сверка chainId перед подпиской, блоки и события по WS своего узла) | connection*.rs, venue.rs | сделано (решение 435): тот же код по настройкам площадки + connection_events.rs (подписка logs) |
| T433 | Каждый DEX Arbitrum и каждое отсутствующее семейство | dex-coverage-arbitrum.md | задачи T433-001…146, T433-F1…F8, T433-R1 в TASKS |

Почему 433-г…к не в этом коммите: правки идут в горячий путь BSC (ctx.rs, чтение узла, 20 статиков), а сборка и тесты —
только в CI; шаг 4 правит соседние файлы параллельно. Каждый пункт — отдельный коммит с прогоном CI, BSC — на прежних
тестах.

## 3. Регистрация второй площадки и «BSC как раньше»

- Новая настройка «вся система» `dex.venues` (список id площадок DEX, кроме bsc_dex) — пункт 433-к; не задана —
  только bsc_dex, процесс как сейчас.
- Перед включением: `dex.default_chain = bsc` на живом боте (иначе ядро при двух DEX откажет — решение 430).
- У bsc_dex: `dex.gas_mode = legacy` (новая обязательная настройка, памятка — FIRST_RUN). После 433-д (решение 435)
  котировка газа, отправка, отмена и перевод читают её: не задана — bsc_dex не котирует и не отправляет.

## 4. Шаг 5б (решение 435, 20.11.2026, ветка t219/arb-step5b)

Сделаны 433-г, д, е, ж, з, и, к, м и ревью README записи узла (статусы — таблица раздела 2, подробности — docs/TASKS.md,
«Решение 435» и «Решение 433», «Осталось»). Не сделаны: 433-л (ждёт вопроса 2 решения 433) и
eth_sendRawTransactionConditional — сделан решением 436 (формат по исходникам Nitro; метод отключают на секвенсоре
Arbitrum One 17.01.2027; минимум газа канала — с узла, ArbGasInfo). Перед выкладкой — dex.gas_mode = legacy
у bsc_dex и dex.default_chain = bsc до dex.venues. Отличие от плана 1.2: dex_key не ставится в ctx.rs dex_* — вид пула
Family.id уже ключ, перевод id индексатора — один раз на входе справочника (directory/fetch.rs).
