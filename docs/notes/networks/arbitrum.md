# Сеть Arbitrum One для бота ARB — разбор

Дата разбора: 10.11.2026. Задача Тиграна от 10.11: схема «одна сеть — один сервер — свои кошельки», узел сети не свой
(открытый или платный). Нужно понять, подходит ли Arbitrum One боту.

Как читать: у каждого факта есть ссылка на источник. Пометка «**расчёт**» — моя арифметика на фактах, не факт.
«**Не найдено**» — в официальных источниках не нашёл или страница не открылась. «**Вторичный источник**» — факт не
с официальной страницы (новости, сторонний замер); его надо подтвердить. Код бота я только читал, ничего не менял и не
собирал. Из этой сессии сетевые запросы к узлам и API (curl) закрыты прокси, поэтому живых замеров с узла здесь нет.

---

## 0. Коротко

- **Порядок транзакций сменился.** С 24.09.2026 на Arbitrum One работает **PGA** (Priority Gas Auctions): секвенсор
  раз в 125 мс сортирует накопленные транзакции **по чаевым** (priority fee), при равных — по времени прихода.
  Timeboost (экспресс-полоса, задержка 200 мс для остальных) выключен. Блок — 250 мс, под нагрузкой до 8 блоков/с.
- **Публичного мемпула нет**, бандлов и билдеров нет. Отправка одна — в секвенсор (`eth_sendRawTransaction`
  или `eth_sendRawTransactionConditional`). Атомарность нескольких свопов — только **одной транзакцией через свой
  контракт** (наш ArbExecutor), не бандлом.
- **Есть полезная особенность:** `eth_sendRawTransactionConditional` — секвенсор отклоняет транзакцию, если слоты
  хранилища (например, состояние пула) не совпали с ожидаемыми. Неудачный арбитраж тогда не попадает в блок и не
  платит газ.
- **Fast Feed** (с 23.09.2026): платная лента, отдаёт каждую транзакцию с логами сразу после того, как секвенсор её
  исполнил, до закрытия блока. Билет — от $17 за раунд 24 ч, оплата в USDG.
- **Газ дешёвый:** простой перевод 10.11.2026 — медиана $0,000906 (OpenChainBench). Замер стоимости свопа — не найден.
- **DEX:** 10.11.2026 объём DEX за 24 ч — $224,2 млн, из них Uniswap $181,48 млн (≈81%, **расчёт**). Почти весь
  объём — пулы, математика которых у нас уже есть (Uniswap V3/V4, PancakeSwap V3, Algebra). Отличие —
  у Camelot направленные комиссии (своя комиссия на покупку и продажу).
- **Главный минус для «биржа ↔ DEX»:** монет с сетью Arbitrum на биржах в 9–11 раз меньше, чем с BSC
  (Gate: 100 против 1075; KuCoin: 46 против 371 — записанные ответы API), а зачисление на биржу — после
  **3000 подтверждений** у BingX и **1900** у KuCoin (≈12,5 и ≈8 минут, **расчёт**). На BSC у BingX — 10.

---

## 1. Как устроена сеть для арбитража

### 1.1 Секвенсор и порядок транзакций — PGA

| Что | Факт | Источник |
| --- | --- | --- |
| Кто упорядочивает | Секвенсор принимает, упорядочивает транзакции и публикует блоки; исполнение — ArbOS (STF) | https://docs.arbitrum.io/how-arbitrum-works/deep-dives/sequencer |
| Политика на Arbitrum One | PGA: сортировка по `priorityFee = min(maxPriorityFeePerGas, maxFeePerGas − baseFee)`, от большего к меньшему; равные — по времени первого прихода к секвенсору | https://docs.arbitrum.io/how-arbitrum-works/priority-gas-auction/pga ; https://docs.arbitrum.io/how-arbitrum-works/timeboost |
| Раунды | Блок B = 250 мс, раундов в блоке K = 2 → раунд 125 мс. K можно менять 1…10 два года после включения (раунд 250…25 мс). Пришедшая в середине раунда транзакция ждёт следующего | там же; форум AIP https://forum.arbitrum.foundation/t/constitutional-aip-transition-arbitrum-one-ordering-policy-to-priority-gas-auctions-pga/30942 |
| Пределы блока | Цель 32 Mgas, жёсткий предел 64 Mgas, на одну транзакцию 32 Mgas, calldata 95 000 байт, не больше 8 блоков/с; заполненный блок выпускается сразу | там же |
| Анти-голодание | Ждущей после раунда транзакции добавляется «виртуальная» прибавка p/(2K) к месту в очереди (на плату не влияет) | там же |
| Как ставить ставку | Обычная транзакция EIP-1559 (тип 2) с `maxPriorityFeePerGas`; регистрации не нужно | https://docs.arbitrum.io/how-arbitrum-works/timeboost/how-to-use-timeboost |
| Куда идут чаевые | 97% — казна Arbitrum DAO, 3% — Developer Guild | там же |
| Дата включения | 24.09.2026 (объявление Arbitrum в X, пересказ CoinMarketCal) — **вторичный источник** | https://www.tradingview.com/news/coinmarketcal:922aff461094b:0-arbitrum-pga-and-fast-feed-live-as-timeboost-is-phased-out-24-sep-2026/ ; блог Arbitrum Foundation 07.10.2026 «now live» https://blog.arbitrum.foundation/builders-block-027-learn-about-arbitrums-new-transaction-ordering-ideas-for-builders-at-founder-house-singapore/ |
| Первая неделя | >$136 000 чаевых, ≈50% транзакций с чаевыми — **вторичный источник** | https://outposts.io/project/arbitrum/article/pga-and-fast-feed-tally-136k-in-first-week-fees-on-arbitrum-cmuflskyy000004jqx7nl2o16 |

### 1.2 Timeboost (выключен)
Был с апреля 2025: экспресс-полоса на 60 с продавалась закрытым аукционом второй цены; остальным транзакциям —
задержка 200 мс. В день включения PGA аукционер перестал принимать ставки, точка экспресс-полосы закрыта, задержка
200 мс снята; деньги из контракта аукциона можно забрать в любой момент
(https://docs.arbitrum.io/how-arbitrum-works/timeboost/gentle-introduction,
https://docs.arbitrum.io/run-arbitrum-node/how-to-use-timeboost). Для бота Timeboost больше не важен.

Что это значит для бота (**расчёт**): гонка за арбитраж на Arbitrum — это **чаевые + скорость прихода** в каждом
раунде 125 мс. Сколько чаевых нужно, чтобы выиграть типичный арбитраж, — **не найдено**, это надо мерить по своим
сделкам и по Fast Feed (видны `effective_gas_price` и `base_fee` чужих транзакций).

### 1.3 Мемпул и приватность
- «The mempool stays private»: PGA не даёт никому видеть или переставлять чужие транзакции
  (https://docs.arbitrum.io/how-arbitrum-works/deep-dives/sequencer).
- `txpool_*` по умолчанию выключены; если включить, `txpool_content` возвращает пустые `pending` и `queued`
  (https://docs.arbitrum.io/arbitrum-essentials/arbitrum-vs-ethereum/rpc-methods).
- Значит, сэндвичей и перехвата из мемпула нет; приватные каналы (как MEV Guard, билдеры на BSC) не нужны и не
  существуют. Отправка — в секвенсор.

### 1.4 Отправка и атомарность нескольких свопов
- Точка секвенсора `https://arb1-sequencer.arbitrum.io/rpc` принимает **только** `eth_sendRawTransaction` и
  `eth_sendRawTransactionConditional` (https://docs.arbitrum.io/node-running/node-providers).
- Бандлов (`eth_sendBundle`) в документации Arbitrum нет: **не найдено**. Атомарность нескольких свопов — одной
  транзакцией через контракт (у нас — `contracts/ArbExecutor.sol`, режим «цикл» `exec/cycle_exec.rs`).
- **`eth_sendRawTransactionConditional`**: секвенсор проверяет условия до приёма; не выполнено — отказ с ошибкой
  JSON-RPC −32003. Условия: `knownAccounts` (корень хранилища адреса или ожидаемые значения слотов),
  `blockNumberMin/Max` (по номеру блока L1), `timestampMin/Max` (по времени L2)
  (https://docs.arbitrum.io/arbitrum-essentials/arbitrum-vs-ethereum/rpc-methods).
  **Расчёт/идея:** если положить в `knownAccounts` слоты состояния пула (slot0/резервы), транзакция при сдвинутой цене
  не попадёт в блок и не заплатит газ. Что проверка идёт до исполнения против текущего состояния секвенсора
  в момент постановки в раунд, а не при приёме, — **не найдено**, проверить на деле.
- `eth_sendRawTransactionSync` — отправка с ожиданием квитанции, возвращает полную квитанцию (там же).
- Откатившаяся транзакция, попавшая в блок, платит газ (правило EVM). Защиты от отката, как у бандлов на BSC, нет —
  кроме conditional выше.

### 1.5 Где ставить сервер (задержка до секвенсора)
- Официального места секвенсора в документации Arbitrum **не найдено**.
- Glassnode Latency Monitor: секвенсор Arbitrum One — AWS **us-east-2 (Огайо)**; меряют время ответа
  `eth_sendRawTransaction` к `arb1-sequencer.arbitrum.io/rpc` (https://latency.glassnode.com/arbitrum/about) —
  **вторичный источник**. Ближайшие точки их проб — Огайо, Ашберн, Чикаго.
- **Расчёт:** при раундах 125 мс и сортировке по чаевым, а при равных — по времени, сервер в us-east-2 даёт
  преимущество только при равных чаевых. Но важнее другое: и платный узел (данные), и биржи. Где серверы BingX/MEXC —
  отдельный вопрос; если Европа/Азия, есть компромисс «близко к секвенсору» против «близко к бирже».

### 1.6 Финальность и связь с L1
- Мягкая финальность: меньше секунды, по стандартной ленте секвенсора; держится на честности секвенсора.
  Жёсткая — когда пакет финализирован на Ethereum (https://docs.arbitrum.io/how-arbitrum-works/deep-dives/sequencer).
- Пакеты сжимаются Brotli и пишутся в Sequencer Inbox на L1 (calldata или blobs EIP-4844); интервал публикации
  **не найден** (там же).
- Принудительное включение мимо секвенсора: окно до 24 ч (`delayBlocks` = 7200) (там же). Боту не нужно.
- `block.number` в контракте на Arbitrum — **номер блока L1** (поле квитанции `l1BlockNumber` «usable for
  block.number calls»), `block.timestamp` — время L2
  (https://docs.arbitrum.io/arbitrum-essentials/arbitrum-vs-ethereum/rpc-methods). ArbExecutor использует только
  `block.timestamp` (`contracts/ArbExecutor.sol:97`) — переносится.
- EVM: TSTORE/TLOAD (EIP-1153) и MCOPY есть с ArbOS 20 Atlas
  (https://docs.arbitrum.io/run-arbitrum-node/arbos-releases/arbos20). ArbExecutor собирается под `cancun`
  с transient-переменными (`contracts/build.mjs:3,29`) — по этим опкодам подходит; деплой на Arbitrum и проверка —
  отдельно.
- Реорги: мягкое подтверждение может теоретически откатиться при сбое секвенсора; Fast Feed прямо пишет, что
  опубликованные транзакции «can be lost» при сбое, «rare» (https://docs.arbitrum.io/how-arbitrum-works/priority-gas-auction/fast-feed).
  Частота в цифрах — **не найдено**.

---

## 2. Газ

### 2.1 Модель (https://docs.arbitrum.io/arbos/l1-pricing)
- Плата = **базовая цена L2 × (газ L2 + плата за данные L1, переведённая в газ L2)**.
- Плата за данные L1 (poster fee): размер транзакции оценивается сжатием Brotli × 16 газа на байт, × текущая цена
  байта на L1, ÷ base fee L2. В квитанции — поле `gasUsedForL1`.
- `eth_estimateGas` уже включает часть L1: результат × цена газа L2 = вся плата в ETH. Оценка меняется вместе с ценой
  данных на L1. Части отдельно — `NodeInterface.gasEstimateComponents()` / `gasEstimateL1Component()`; нижний
  порог цены — `ArbGasInfo.getMinimumGasPrice`; base fee L1 — `ArbGasInfo.getL1BaseFeeEstimate`.
- Числового значения порога base fee на странице нет — **не найдено** (читать `getMinimumGasPrice` с узла).
- Чаевые (PGA) — сверху; base fee они не уменьшают.

### 2.2 Сколько стоит своп
- Простой перевод ETH: медиана **$0,000906**, p90 $0,000949, p99 $0,00132 — 10.11.2026, окно 24 ч, метод —
  `eth_feeHistory` + `eth_estimateGas` (вместе с частью L1) × цена ETH (https://openchainbench.com/benchmarks/network-fees/arbitrum).
- Стоимость **свопа** в USD с датой: **не найдено** (Arbiscan gastracker открыть не удалось, QuickNode gas tracker
  показал 8,2 gwei без времени и без цены свопа — этому числу не доверяю, на странице нет даты:
  https://www.quicknode.com/fr/gas-tracker/arbitrum-one).
- **Расчёт** (ориентир, не факт): часть L2 растёт пропорционально газу. Если своп V3 тратит в 5–10 раз больше газа,
  чем перевод (21 000), — порядок $0,005–0,01 без чаевых. Проверить: `eth_estimateGas` на реальной сделке
  (бот это уже умеет — `exec/precheck_sim.rs`, `check/simulate.rs`).
- Для бота: в стоимость сделки, кроме газа, войдут **чаевые PGA** — их размер в арбитраже не известен (п. 1.2).

---

## 3. Узлы (не свой)

### 3.1 Что нужно боту от узла (по коду)
Методы в `src/venues/bsc_dex` (grep): `eth_blockNumber`, `eth_call` (Multicall3), `eth_chainId`, `eth_estimateGas`,
`eth_gasPrice`, `eth_getBalance`, `eth_getBlockByNumber`, `eth_getBlockReceipts`, `eth_getCode`, `eth_getLogs`,
`eth_getStorageAt`, `eth_getTransactionByHash`, `eth_getTransactionCount`, `eth_getTransactionReceipt`,
`eth_sendRawTransaction`, `eth_sendPrivateTransaction`, `eth_sendBundle`, **`eth_simulateV1`**, `eth_subscribe`
(`logs`, `newHeads`), `eth_syncing`. Без `eth_simulateV1` не работают проба налогов (`check/`), сухая сделка и проба
сделки (`exec/`). Подробный разбор нагрузки — `docs/notes/networks/code-multinet.md`, раздел 4.

### 3.2 Особенности узла Arbitrum (Nitro)
- `eth_simulateV1` и `debug_traceCall` **падали** на сетях, где собираются чаевые; исправлено в Nitro **v3.12.1**
  (журнал 29.09.2026) — https://github.com/OffchainLabs/nitro/releases/tag/v3.12.1. Arbitrum One собирает чаевые с
  PGA, значит у провайдера должен быть Nitro ≥ v3.12.1. Какая версия у провайдеров — **не найдено**, проверить
  кнопкой «Проверить подключение» (вызов `eth_simulateV1`, `connection.rs`).
- `eth_getLogs`: у Nitro нет своего предела диапазона блоков, предел задаёт провайдер (rpc-methods, ссылка выше).
- `eth_syncing` у Nitro — свой формат объекта при отставании, «not a stable API» (там же). У бота есть `eth_syncing`
  в проверке — формат надо учесть.
- Публичный узел `https://arb1.arbitrum.io/rpc`: **без WebSocket**, «No uptime, latency, or rate-limit guarantees»
  (https://docs.arbitrum.io/node-running/node-providers). Годится только как резерв.

### 3.3 Платные провайдеры

| Провайдер | WebSocket на Arbitrum One (список Arbitrum) | Цена и лимиты (официальные страницы) | eth_simulateV1 на Arbitrum | debug/trace |
| --- | --- | --- | --- | --- |
| Alchemy | есть | Free: 30 млн CU/мес, 300 CU/с. PAYG: $0,525 за 1 млн CU, 10 000 CU/с (https://alchemy.com/docs/reference/pricing-plans). Цены вызовов: eth_call 26 CU, eth_getLogs 60, eth_simulateV1 40, debug_traceCall 40, eth_sendRawTransaction 40, eth_getBlockReceipts 20, eth_subscribe 10; события подписок — по трафику **0,04 CU/байт** (≈40 CU на событие 1000 байт) (https://www.alchemy.com/docs/reference/compute-unit-costs.md) | **есть**, 40 CU (https://www.alchemy.com/docs/chains/arbitrum/arbitrum-api-endpoints/eth-simulate-v-1) | Free — нет, PAYG — есть |
| Infura | есть | Free: 3 млн кредитов/сут, 500/с. Developer: 15 млн/сут, 4 000/с. Team: 75 млн/сут, 40 000/с; цен в $ на странице нет (https://docs.infura.io/get-started/pricing.md) | **не найдено** | **не найдено** |
| QuickNode | есть | Build $49: 80 млн кредитов, 50 зап/с; Accelerate $249: 450 млн, 125/с; Scale $499: 950 млн, 250/с; Business $999: 2 млрд, 500/с; перерасход $0,50–0,62 за 1 млн (https://www.quicknode.com/pricing.md) | **не найдено** для Arbitrum One (метод описан для Arbitrum Nova) | с Build |
| Chainstack | есть | Developer бесплатно: 3 млн RU, 25 зап/с; Growth $49: 20 млн, 250/с; Pro $199: 80 млн, 400/с; Business $499: 200 млн, 600/с; Enterprise от $990; выделенный узел — от $0,50/ч (https://chainstack.com/de/pricing/) | **есть** (https://docs.chainstack.com/reference/arbitrum-simulatev1) | с Growth |
| dRPC | есть | Free: только публичные узлы, 210 млн CU/30 дней; PAYG $6 за 1 млн запросов, до 100 зап/с; Pro $49: 163 млн CU, 300/с; Scale $199: 664 млн CU, 600/с (https://drpc.org/docs/pricing/requests) | **не найдено** | **не найдено** |

Список провайдеров с WebSocket для Arbitrum One (официальная страница Arbitrum): Alchemy, Allnodes, All That Node,
Ankr, Chainbase, Chainstack, dRPC, Dwellir, GetBlock, Infura, Nirvana Labs, QuickNode, SwiftNodes, Tenderly,
Validation Cloud. Без WebSocket: 1RPC, BlockPi, Chainnodes, Lava, Moralis, NodeReal, NOWNodes, Pocket, PublicNode,
Unifra (https://docs.arbitrum.io/node-running/node-providers).

- Пределы на пачки (batch) — **не найдено** ни у кого. Задержка провайдеров в цифрах — **не найдено** (мерить на
  сервере: `node/head_age.rs`).
- **Расчёт нагрузки:** блоков ≥ 4 в секунду (250 мс) — в ~2 раза чаще, чем на BSC (≈2,2/с по решению 68). Лента
  квитанций (3 вызова/блок) ≈ 1,04 млн вызовов в сутки, опрос головы 100 мс — 864 000 (оценка из
  `code-multinet.md` 4.2). Подписки `logs` у Alchemy платятся по байтам — при сотнях пулов на Arbitrum это может
  стоить больше, чем лента квитанций; выбирать по тарифу (`bsc.pool_events_source`, решение 68).

### 3.4 Лента секвенсора (sequencer feed) и Fast Feed

| | Стандартная лента | Fast Feed |
| --- | --- | --- |
| Что отдаёт | Готовые блоки (сообщения секвенсора), бесплатно | Каждую транзакцию сразу после того, как секвенсор её упорядочил и исполнил, **до закрытия блока**; номер блока предварительный |
| Точка | `wss://arb1-feed.arbitrum.io/feed` (https://docs.arbitrum.io/node-running/node-providers) | `wss://arb1-txfeed.arbitrum.io/` (https://docs.arbitrum.io/how-arbitrum-works/priority-gas-auction/use-fast-feed) |
| Формат | Сжатые сообщения L2 (`l2Msg`), разбор через `ParseL2Transactions` (Go, Nitro); номер блока L2 = sequenceNumber + 22207817 (https://docs.arbitrum.io/node-running/how-tos/read-sequencer-feed) | JSON: `timestamp_ms`, `pga_round`, `block_number`, `tx_index`, `raw_tx`, `tx_hash`, частичная квитанция: `status`, `gas_used`, `gas_used_for_l1`, `effective_gas_price`, `base_fee`, **`logs`** (address, topics, data) (https://docs.arbitrum.io/how-arbitrum-works/priority-gas-auction/consume-fast-feed) |
| Цена | бесплатно | Билет на раунд **24 ч**, все билеты раунда по одной цене, минимум **$17**, цель 100 билетов, максимум 200, цена по механизму EIP-4844 (до ×2 / ÷2 за раунд); оплата **USDG** (`0x004B…9bbC`), контракт билетов `0x451D…A598`; первый раунд — 23.09.2026 17:00 UTC; одно соединение на билет (https://docs.arbitrum.io/how-arbitrum-works/priority-gas-auction/fast-feed, …/use-fast-feed) |
| Ограничения | — | Нет мемпула (только уже упорядоченные), нет состояния, нет истории (отвалился — пропустил), «can be lost» при сбое секвенсора; отставание > 2048 сообщений — разрыв (код 1013) |

- Для кого Fast Feed по документации: поисковики, в том числе **арбитраж CEX–DEX**, ликвидации, бэкран; маркет-мейкеры
  (https://docs.arbitrum.io/how-arbitrum-works/priority-gas-auction/fast-feed).
- **Расчёт выгоды:** стандартная лента приходит блоком раз в 250 мс, Fast Feed — внутри раундов 125 мс. Выигрыш —
  до одного блока (≤250 мс) по сравнению с лентой, и больше — по сравнению с `eth_subscribe logs` у провайдера
  (там ещё задержка узла провайдера; в цифрах — **не найдено**). Минимальная цена $17 × 30 ≈ **$510/мес** (**расчёт**).
- Логи в Fast Feed — те же события `Swap`/`Sync`, по которым бот уже обновляет пулы (`pools/events_*`, лента
  квитанций `node/receipts_*`). Значит, Fast Feed можно подключить как ещё один источник событий пулов (наряду с
  `ws` и `receipts`, решение 68). Это новый код (свой WebSocket, свой формат, авторизация, покупка билета в USDG).
- Цены у провайдеров на «ленту секвенсора» как продукт — **не найдено**.

---

## 4. DEX сети

### 4.1 Доли объёма — DefiLlama (открыто 10.11.2026; https://defillama.com/dexs/chain/arbitrum)
Всего: **$224,2 млн за 24 ч**, **$5,869 млрд за 30 дней**.

| DEX | 24 ч | 30 дней | Доля 30 дней (**расчёт**) | Тип пула / есть ли наша математика (`bsc_dex/math`) |
| --- | --- | --- | --- | --- |
| Uniswap (V2+V3+V4) | $181,48 млн | $4,601 млрд | 78,4% | V3 — `swap_cl.rs` (как на BSC); V4 — `swap_step.rs` V4 + `abi_v4.rs` (как Uniswap V4 на BSC); V2 — `v2.rs`. **Есть** |
| — из них Uniswap V4 | $12,49 млн | $537,1 млн | 9,2% | DefiLlama, https://defillama.com/dexs/uniswap-v4 |
| PancakeSwap | $10,58 млн | $340,27 млн | 5,8% | V3 на Arbitrum (подтверждено версией `pancake_swap_v3` у Bitquery). Infinity на Arbitrum — **не найдено**. **Есть** |
| Fluid | $5,52 млн | $280,42 млн | 4,8% | Своя схема Fluid DEX. **Нет** |
| Camelot | $10,82 млн | $227,87 млн | 3,9% | V2: Uniswap V2 + «Solidly-type» стабильные пары, **направленные комиссии**; V3: **Algebra V1.9** с направленной динамической комиссией; V4: **Algebra Integral** (https://docs.camelot.exchange/protocol/amm-v2, /amm-v3, /references). Частично — см. 4.2 |
| Metric | $7,18 млн | $147,5 млн | 2,5% | Тип **не установлен** (официальную документацию не открывал) |
| Native | $1,94 млн | $79,61 млн | 1,4% | RFQ (котировки маркет-мейкера вне сети) — не пул. **Нет** |
| Curve | $1,7 млн | $47,8 млн | 0,8% | StableSwap — `curve_math.rs`, `swap_curve.rs`; CryptoSwap — **нет** |
| DODO | $1,06 млн | $39,41 млн | 0,7% | PMM — `dodo_math.rs`, `swap_dodo.rs`. **Есть** |
| Balancer | $425 тыс. | $34,87 млн | 0,6% | **Нет** |
| Sushi | $1,6 млн | $24,2 млн | 0,4% | V2/V3-форки — `v2.rs`, `swap_cl.rs` (при совпадении ABI). **Есть** |
| Maverick | $242 тыс. | $16,78 млн | 0,3% | Maverick V2 — `maverick_*.rs`. **Есть** |
| Dexalot, Integral, Alphix, WOOFi, RamsesX, GMX, LFJ и др. | < $1 млн каждый | | < 0,2% каждый | WOOFi — `abi_woo.rs` (есть на BSC); GMX — свопы по оракулу, не пул; «Integral» здесь — отдельный DEX, не Algebra Integral |

- Разбивки Uniswap V2/V3 и Camelot/PancakeSwap по версиям на странице нет. **Расчёт:** Uniswap без V4 ≈
  $4,601 − 0,537 = $4,064 млрд за 30 дней (V3 + V2 вместе).
- Bitquery (DEXrabbit) за 24 ч показывает другие цифры (Uniswap V4 $585 млн, V3 $508 млн —
  https://dexrabbit.bitquery.io/arbitrum/dex_markets); расходится с DefiLlama в разы. Причина — **не найдено**.
  Для решений беру DefiLlama, но вопрос надо держать в уме.
- **Расчёт:** доля пулов, для которых математика уже есть (Uniswap, PancakeSwap V3, Sushi, Curve StableSwap, DODO,
  Maverick) ≈ 86% объёма за 30 дней; ещё 3,9% — Camelot, если доделать направленные комиссии.

### 4.2 Что не совпадает с нашей математикой
1. **Camelot V3 = Algebra V1.9 с направленными комиссиями.** `globalState` этой версии:
   `price, tick, feeZtO, feeOtZ, timepointIndex, communityFee, unlocked`, комиссии в миллионных долях
   (https://docs.algebra.finance/algebra-integral-documentation/algebra-v1-technical-reference/contracts/api-reference-v2.0-directional-fees/v2.0-directional-fees-core/ialgebrapoolstate).
   Наш `math/swap_algebra.rs` — порт Algebra V1.9 под THENA Fusion (одна комиссия `globalState.fee`,
   `quote/pool_quote.rs:158`). Нужно: читать две комиссии и брать нужную по направлению. Что формула обмена в
   остальном совпадает — **проверить по коду пула Camelot** (не проверено).
2. **Camelot V2:** комиссия своя у пары и разная по направлению; формулы, знаменателя комиссии и стабильной кривой
   в документации нет — **не найдено** (https://docs.camelot.exchange/protocol/amm-v2/dynamic-directional-fees).
   Наш `v2.rs` берёт комиссию из настройки — нужна комиссия «из пары по направлению»; стабильная кривая — сверить с
   `solidly_math.rs` по исходникам Camelot.
3. **Camelot V4 = Algebra Integral** — у нас есть (решение 188, `abi_algebra_integral.rs`,
   `load_algebra_integral.rs`); адреса фабрики на Arbitrum — **не найдено** (страница контрактов Camelot не вывела
   адреса).
4. **Fluid, Balancer, Curve CryptoSwap, Native (RFQ)** — нет математики; вместе ≈ 7% объёма за 30 дней (**расчёт**).

### 4.3 Адреса (официальные страницы Uniswap)
- Uniswap V3: Factory `0x1F98431c8aD98523631AE4a59f267346ea31F984`, QuoterV2 `0x61fFE014bA17989E743c5F6cB21bF9697530B21e`,
  SwapRouter02 `0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45`, Multicall `0xadF885960B47eA2CD9B55E6DAc6B42b7Cb2806dB`,
  TickLens `0xbfd8137f7d1516D3ea5cA83523914859ec47F573`
  (https://developers.uniswap.org/docs/protocols/v3/deployments/v3-arbitrum-deployments.md).
- Uniswap V4: PoolManager `0x360e68faccca8ca495c1b759fd9eee466db9fb32`, StateView `0x76fd297e2d437cd7f76d50f01afe6160f86e9990`,
  Quoter `0x8928074CA1b241D8Ec02815881c1Af11E8bC5219`, Universal Router `0xa51afafe0263b40edaef0df8781ea9aa03e381a3`
  (новый 2.1.2 — `0x2d01411773c8C24805306E89A41F7855C3c4Fe65`), Permit2 `0x000000000022D473030F116dDEE9F6B43aC78BA3`
  (https://developers.uniswap.org/docs/protocols/v4/deployments.md).
- Адрес фабрики Uniswap V2 на Arbitrum — на странице V3 не указан, **не найдено**. Адреса PancakeSwap V3, Camelot,
  Sushi на Arbitrum — **не найдено** в этой сессии.
- События — те же, что у этих протоколов на BSC (тот же код контрактов): V3 `PoolCreated`/`Swap`, V4 `Initialize`/`Swap`.
  Справочник пулов строится по событиям создания (`pools/created_log.rs`) — переносится, меняются адреса фабрик в
  настройках.

---

## 5. Агрегаторы

| Агрегатор | Arbitrum | Лимиты и цена (официально) | Источник |
| --- | --- | --- | --- |
| KyberSwap (уже в боте, `best_of/`) | да, `arbitrum` (42161); `GET https://api.kyberswap.com/swap/arbitrum/api/v1/routes`, `POST …/route/build` (собрал из базы и пути — **расчёт**) | публично 3 зап/с; больше — по ключу `X-Api-Key`, через business@kyber.network; цена не указана | https://docs.kyberswap.com/developer-guide/start-here/rate-limits-and-client-id ; https://docs.kyberswap.com/kyberswap-solutions/kyberswap-aggregator/aggregator-api-specification/evm-swaps ; https://docs.kyberswap.com/getting-started/supported-exchanges-and-networks |
| 1inch (Business) | **не проверено** в этой сессии | Dev бесплатно: 1 зап/с, 100 тыс./мес; Start-Up 10/с, 1 млн; Professional 20/с, 3 млн; Business 40/с, 7 млн; цены платных не указаны; при превышении — 429 | https://help.1inch.com/en/articles/8703748-how-to-increase-your-1inch-api-rate-limits |
| 0x | **не проверено** | бесплатно ≈5 зап/с; больше — индивидуально | https://docs.0x.org/docs/developer-resources/rate-limits.md |
| Velora (ParaSwap) | **не проверено** | цифр нет; совет — не чаще 1 зап/с; есть «Professional access» | https://help.velora.xyz/en/articles/6570060-about-api-rate-limits |
| Odos | **не указано** на странице цен | бесплатно 600 запросов за 5 мин с IP; платные — «coming soon» | https://docs.odos.xyz/build/api_pricing |
| OpenOcean | — | страница открыться не успела — **не найдено** | https://docs.openocean.finance/docs/swap-api/guide |

Задержка ответа агрегаторов в цифрах — **не найдено** ни у кого.

Где агрегатор выгоднее своего поиска (**расчёт**, по фактам выше):
- Агрегаторы видят то, чего у нас нет: Fluid, Balancer, Curve CryptoSwap, RFQ (Native) — ≈7% объёма за 30 дней.
  Для котировки «сравнить с агрегатором» (как сейчас `best_of/`) это полезно.
- Для **скорости** — нет: 1–5 запросов в секунду и HTTP-путь до их серверов не успевают за раундом 125 мс.
  Арбитраж DEX ↔ DEX и продажа после перевода — своим графом пулов.
- 86% объёма — в пулах, которые мы считаем сами (п. 4.1), так что свой поиск покрывает почти всё.

---

## 6. Токены: налоги (fee-on-transfer)

- Статистики распространённости налоговых токенов на Arbitrum — **не найдено**.
- Uniswap V3: «Fee-on-transfer tokens will not function with our router contracts», поддерживать не будут
  (https://developers.uniswap.org/docs/protocols/v3/concepts/unsupported-tokens.md). Поскольку 78% объёма на
  Arbitrum — Uniswap (п. 4.1), налоговые токены в основном объёме сети, по логике, редки (**расчёт**, не факт).
- Проба налогов симуляцией (`check/simulate.rs`, `eth_simulateV1`) переносится как есть — при условии Nitro ≥ v3.12.1
  у провайдера (п. 3.2).

---

## 7. Биржи: ввод и вывод по Arbitrum One

Главный источник — ответы API бирж, записанные в репозитории (правило CLAUDE.md «данные площадок — только из API»):

| Биржа | Имя сети в API | Монета | Вывод | Мин. вывод | Подтверждений до зачисления | Ввод/вывод | Источник |
| --- | --- | --- | --- | --- | --- | --- | --- |
| BingX | `ARBITRUM` | USDT (`0xfd08…cbb9`) | 0,5 USDT | 0,5 | **3000** | открыт / открыт | `tests/data/bingx_2026-10-24/config_getall.json` (24.10.2026; в записи 20 монет — по остальным нет данных) |
| BingX, для сравнения | `BEP20` | USDT | 0,15 USDT | — | 10 | | там же |
| KuCoin | `arbitrum` | USDT | 1 | 5 | **1900** (`preConfirms` 120) | открыт / открыт | `tests/data/cex_samples_2026_11_04/kucoin_currencies_v3.json` (04.11.2026) |
| KuCoin | `arbitrum` | USDC (`0xaf88…5831`) | 1 | 5 | 1900 (120) | открыт / открыт | там же |
| KuCoin | `arbitrum` | ETH | 0,0002 | 0,0004 | 1900 (120) | открыт / открыт | там же |
| KuCoin | `arbitrum` | ARB | 4 | 8 | 1900 (120) | открыт / открыт | там же |
| Gate | `ARBEVM` | USDT, USDC, ETH, ARB | в записи нет | — | в записи нет | открыт / открыт | `tests/data/cex_samples_2026_11_04/gate_spot_currencies.json` |
| MEXC, Binance, Bybit, Bitget, OKX, XT, HTX, CoinEx, Kraken, Phemex, Poloniex | — | — | **не найдено** | — | **не найдено** | — | официальные страницы комиссий динамические, через WebFetch не читаются; в записанных ответах этих бирж сети Arbitrum нет (записи урезаны) |

**Сколько монет можно возить по Arbitrum (подсчёт по записанным ответам API):**
- Gate: сеть `ARBEVM` у **100** монет, ввод и вывод открыты у **38**; сеть `BSC` — у 1075, открыты у 346.
- KuCoin: сеть `arbitrum` у **46** монет, открыты у **19**; `bsc` — у 371, открыты у 127.
- **Расчёт:** монет для «биржа ↔ DEX» по Arbitrum в 9–11 раз меньше, чем по BSC (у этих двух бирж).

**Время зачисления (расчёт):** при блоке 250 мс 3000 подтверждений у BingX ≈ 750 с ≈ **12,5 мин**, 1900 у KuCoin ≈
475 с ≈ **8 мин** (под нагрузкой блоки чаще — время короче). Что значит `preConfirms` = 120 у KuCoin (≈30 с) — в
документации KuCoin не проверял, **не найдено**. На BSC у BingX 10 подтверждений (≈4,5 с, **расчёт** по 0,45 с).
Риск движения цены за время перевода на Arbitrum намного выше.

Что переиспользуется: имя сети у биржи — настройка `venue.bsc_network` (`docs/SETTINGS.md`), комиссии и подтверждения
адаптеры бирж читают из API. На сервере под Arbitrum — `ARBITRUM` (BingX), `arbitrum` (KuCoin), `ARBEVM` (Gate) и
т.п. Сверка адреса контракта у биржи с адресом токена в пуле (жёсткое правило CLAUDE.md) работает так же — адреса в
ответах API есть.

---

## 8. Вывод

### 8.1 Есть ли смысл
- **DEX ↔ DEX (атомарный цикл): да, сеть подходит.** Дешёвый газ, блок 250 мс, 86% объёма в пулах с нашей
  математикой, нет сэндвичей (приватный мемпул). Но исполнять можно только одной транзакцией через ArbExecutor,
  гонка идёт чаевыми PGA каждые 125 мс, а за откат платим газ (кроме `eth_sendRawTransactionConditional`).
- **Биржа ↔ DEX: слабее, чем на BSC.** Монет в 9–11 раз меньше, зачисление на биржу 8–12,5 минут против секунд на
  BSC (**расчёт**). Риск движения цены за перевод большой.
- Итог (**расчёт**): Arbitrum — кандидат прежде всего под **DEX ↔ DEX**; «биржа ↔ DEX» — на малом числе монет и
  с жёстким учётом времени в пути.

### 8.2 Что берём из bsc_dex как есть
- Математика пулов: V2, V3/CL, V4, Algebra Integral, Curve StableSwap, DODO, Maverick (`math/`).
- Справочник пулов по событиям создания, загрузка Multicall3, события `Swap`/`Sync` (`pools/`), меняются адреса и
  списки фабрик в настройках.
- Проба налогов и сделки `eth_simulateV1` (`check/`, `exec/precheck_sim.rs`, `exec/presell.rs`) — при Nitro ≥ v3.12.1.
- Цикл одной транзакцией через ArbExecutor (`exec/cycle_exec.rs`, `contracts/ArbExecutor.sol`) — пересобрать и
  задеплоить на Arbitrum; опкоды cancun поддерживаются (ArbOS 20).
- Котировка, граф маршрутов, «сухая сделка», KyberSwap для сравнения (`best_of/`, chain `arbitrum`).
- Канал `public` (`send/public.rs`, `eth_sendRawTransaction` по `channel.url`) — указать точку секвенсора.
- Квитанции, баланс кошелька, учёт.

### 8.3 Что писать заново или менять
| Часть | Что сделать | Сложность (**оценка**) |
| --- | --- | --- |
| Газ | Модель L2 + L1: брать `eth_estimateGas` (уже с частью L1) или `NodeInterface.gasEstimateComponents`; учесть `gasUsedForL1` в квитанции; цена — EIP-1559 с чаевыми | средняя |
| Чаевые PGA | Новая настройка и логика ставки `maxPriorityFeePerGas` для сделки (аналог «чаевых канала» на BSC) + учёт в стоимости | средняя |
| Отправка | Убрать билдеры/бандлы BSC (`send/blockrazor*`, `jetbuilder`, `bloxroute*`, `nodereal*`, `mev_guard`, `eth_sendBundle`) для этой сети; добавить канал `eth_sendRawTransactionConditional` с условиями `knownAccounts` по слотам пулов | средняя |
| Исполнение DEX ↔ DEX | Режим «бандл» (решение 84) не применим — только «цикл» через исполнитель | малая (выбор режима) |
| Camelot | Направленные комиссии: Algebra V1.9 (`feeZtO`/`feeOtZ`) и V2 (комиссия пары по направлению); стабильная кривая V2 — сверить по исходникам | средняя |
| Fast Feed (по желанию) | Новый источник событий пулов: WebSocket с авторизацией, JSON-формат, покупка билета USDG раз в 24 ч | средняя-высокая |
| Узел провайдера | Проверка подключения: Nitro поддерживает `eth_simulateV1`; формат `eth_syncing` Nitro; бюджет вызовов под 4–8 блоков/с | малая-средняя |
| Монеты-исключения | Нативная монета — ETH, обёртка — WETH (настройка `dex.native_wrapped`, решение 62); адрес WETH в этой сессии **не проверен** | малая |

### 8.4 Главные риски
1. **Чаевые PGA**: сколько надо платить, чтобы выиграть арбитраж, неизвестно — может съесть прибыль (п. 1.2).
2. **Откат = оплата газа**: нет бандлов «всё или ничего вне блока»; спасает только conditional-отправка — её поведение
   проверить на деле.
3. **Долгий перевод на биржу** (1900–3000 подтверждений) — для «биржа ↔ DEX».
4. **Один секвенсор**: при его сбое транзакции из Fast Feed «can be lost»; при простое сеть стоит.
5. **Провайдер**: старый Nitro (< v3.12.1) ломает `eth_simulateV1` → бот не сможет проверять налоги и сделки и, по
   правилу «не торгуем вслепую», не торгует.
6. **Цифры объёма**: DefiLlama и Bitquery расходятся в разы (п. 4.1).

---

## Что не удалось проверить (открыть страницу или найти факт)
- Официальное место секвенсора (есть только Glassnode — us-east-2).
- Замер стоимости свопа в USD с датой; порог base fee (`getMinimumGasPrice`).
- Сколько чаевых PGA платят арбитражёры; задержки стандартной ленты, Fast Feed и провайдеров в мс.
- Интервал публикации пакетов на L1; частота откатов мягкой финальности.
- `eth_simulateV1` у Infura, QuickNode (Arbitrum One), dRPC; версии Nitro у провайдеров; пределы пачек.
- Адреса фабрик PancakeSwap V3, Camelot (V2/V3/V4), Sushi, Uniswap V2 на Arbitrum; есть ли PancakeSwap Infinity на
  Arbitrum; тип пулов Metric.
- Поддержка Arbitrum у 1inch, 0x, Velora, Odos; страница OpenOcean; задержки агрегаторов.
- Комиссии и подтверждения по Arbitrum у MEXC, Binance, Bybit, Bitget, OKX, XT, HTX, CoinEx, Kraken, Phemex, Poloniex.
- Статистика налоговых токенов на Arbitrum.
- Страницы, закрытые для этой сессии: arbiscan.io/gastracker, l2fees.info, api.llama.fi, публичные узлы Arbitrum
  (curl через прокси запрещён), docs.openocean.finance.
