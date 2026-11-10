# Другие сети-кандидаты: Base, Optimism, Polygon PoS, Avalanche C-Chain (+ коротко Linea, zkSync Era, Sonic, Mantle, Tron, TON)

> Задача Тиграна 10.11.2026: бот запускается «одна сеть — один сервер — свои кошельки», узел ОТКРЫТЫЙ или ПЛАТНЫЙ
> (не свой). Нужен разбор сетей-кандидатов, чтобы выбрать, какие брать.
>
> Что это: исследование (репозиторий не менялся, не собирался). Только факты: официальная документация, исходный код
> узлов/протоколов на GitHub, живые ответы API бирж, записанные ботом в `tests/data/`, страницы DefiLlama. У каждого
> факта — ссылка. **«не найдено»** — источника в этой сессии не нашёл. **[расчёт]** — арифметика по фактам, не факт.
>
> Ограничение среды: прямые запросы к RPC узлов (mainnet.base.org, polygon-rpc.com, api.avax.network и т. д.) и к
> публичным API бирж из этой среды запрещены прокси (403) — живых ответов узлов здесь нет. Поддержку методов узлов
> проверял по документации и по исходному коду клиентов узлов.
>
> Как устроен код бота для нескольких сетей (что зашито под BSC, что уже настройка) — отдельная записка
> `docs/notes/networks/code-multinet.md`; здесь — только то, чем сети отличаются.

---

## 0. Главное коротко

1. **Base — самый большой рынок из четырёх и ближе всех к тому, что бот уже умеет.** Объём DEX за 24 ч
   $854,78 млн (BSC — $1,214 млрд) ([DefiLlama Base](https://defillama.com/dexs/chain/base),
   [DefiLlama BSC](https://defillama.com/dexs/chain/bsc), снимок 10.11.2026). Главные DEX — Aerodrome (пулы Solidly +
   Slipstream), Uniswap (V2/V3/V4), PancakeSwap (V2/V3/Infinity CL и Bin) — **все эти виды пулов уже есть в коде бота**.
   `eth_simulateV1` в документации Base есть. Минусы: нет публичного мемпула и бандлов; блок каждые 200 мс (с 05.11.2026);
   к газу добавляется плата L1; публичный узел без WebSocket — нужен платный провайдер; USDT на Base у Gate и KuCoin
   **нет** (есть USDC) — вопрос «за что покупаем/продаём» (раздел 1.7).
2. **Polygon PoS — второй.** $96,74 млн за 24 ч; Uniswap, QuickSwap (Algebra), DODO, Curve, WOOFi — виды, которые
   бот знает. Узел Bor (код) умеет `eth_simulateV1` и `eth_call` с подменой состояния. Публичный мемпул + официальный
   приватный мемпул Polygon (бесплатный уровень). USDT нативный, биржи выводят. Блок 1 с.
3. **Avalanche C-Chain — третий, но дороже в переделке.** $44–62 млн за 24 ч (две страницы DefiLlama дали разные
   снимки). Главные DEX — Pharaoh, Blackhole, LFJ (Liquidity Book), Uniswap; Liquidity Book у бота нет (нужна новая
   математика). **`eth_simulateV1` в списке методов C-Chain нет** — пробы налогов и сухие сделки бота
   (`check/simulate.rs`) переделывать. С 22.09.2026 (Helicon) — отложенное исполнение блоков и газ
   `max(gasUsed, gasLimit/2)`.
4. **Optimism — последний из четырёх.** $12,21 млн за 24 ч (в 70 раз меньше Base), 76 % — Velodrome. Ввод/вывод USDT
   по сети OPTIMISM выключен у BingX (24.10.2026) и у KuCoin (04.11.2026) — по записанным ответам API.
5. **Короткие:** Mantle ($3,37 млн), Sonic ($1,61 млн), Linea ($0,35 млн), zkSync Era ($0,33 млн) — объёмы малы.
   Tron — EVM-подобная TVM, но свой формат адресов, своя модель газа и свой API (не наш стек). TON — не EVM,
   атомарного цикла по нескольким пулам в одной транзакции нет по устройству сети.

---

## 1. Base (chainId 8453, OP Stack, газ — ETH)

### 1.1 Устройство
- **Блок каждые 200 мс** — изменение Denim, Base Mainnet 05.11.2026: «Moved to a canonical block every 200ms»,
  «retired Flashblocks» ([configuration changelog](https://docs.base.org/base-chain/network-information/configuration-changelog)).
  Страница FAQ про Flashblocks (2 с блок + 10 «флэшблоков» по 200 мс) ещё висит — она устарела относительно changelog
  ([FAQ](https://docs.base.org/base-chain/flashblocks/faq.md)).
- **Кто упорядочивает:** один секвенсор; блоки строит `base-reth-node`, «transactions within each block are ordered by
  priority fee» ([transaction ordering](https://docs.base.org/specifications/transactions/transaction-ordering.md)).
- **Публичного мемпула нет:** «OP Stack chains do not have a public mempool. The OP Stack mempool is currently only
  visible to the Sequencer» (OP docs, `op-stack/protocol/differences.mdx`, репозиторий
  [ethereum-optimism/docs](https://github.com/ethereum-optimism/docs) коммит 8a4bf24). Base — OP Stack-сеть.
- **MEV / приватная отправка / бандлы:** официального канала бандлов нет — в документации Base про бандлы ничего
  (FAQ, block building, RPC overview — «не найдено»). Есть новый вид отправки **Validity Transactions**
  (`base_sendRawTransactionValidity`): транзакция участвует в борьбе за место, только когда совпали все условия
  (баланс, nonce, хранилище, номер блока); «does not reserve block space or guarantee inclusion»; платится ли газ при
  неисполнении — не сказано ([validity transactions](https://docs.base.org/specifications/build-transaction/validity-transactions.md)).
- **Атомарность:** бандлов нет, значит DEX ↔ DEX — только одной транзакцией через наш `ArbExecutor` (все шаги в
  одной транзакции и откат, если выход < minOut — `contracts/ArbExecutor.sol:1-6`). Откат стоит газа.
- **Конкуренция (первоисточник — исследование, не документация):** на L2 доля откатившихся транзакций 5–40 % в день
  против ~1 % на Ethereum; >80 % откатов — свопы; на Base найдены «спамеры конца блока» (≈196 откатов на блок)
  ([arXiv 2506.01462](https://arxiv.org/pdf/2506.01462), данные январь–июль 2025, до перехода на 200 мс).

### 1.2 Газ
- Модель: `totalFee = gasUsed × (baseFee + priorityFee) + l1Fee + operatorFee`; L1-плата — по `GasPriceOracle`
  `0x420000000000000000000000000000000000000F` (`getL1Fee(bytes)`, `getL1FeeUpperBound(size)`, `getOperatorFee(gasUsed)`)
  ([Base fees](https://docs.base.org/chain/fees)). Формула Fjord для L1-платы — в OP docs
  ([fees](https://docs.optimism.io/stack/transactions/fees)).
- **Минимальная baseFee — 5 000 000 wei (0,005 gwei)** с 19.02.2026 ([changelog](https://docs.base.org/base-chain/network-information/configuration-changelog)).
- Лимит газа одной транзакции 16 777 216 (EIP-7825, с 28.05.2026); лимит блока 40 млн
  ([throughput and limits](https://docs.base.org/base-chain/network-information/throughput-and-limits.md)).
- Типичная стоимость свопа в USD — **не найдено** (живой цены газа и курса ETH из этой среды не получить).
  [расчёт] на минимуме: 200 000 газа × 0,005 gwei = 0,000001 ETH + L1-плата (её даёт `getL1Fee` на конкретную
  транзакцию). 200 000 — число для примера, не замер.

### 1.3 Узлы
- Публичный `https://mainnet.base.org` — с ограничением частоты, «not suitable for production»; **публичного
  WebSocket нет**: «WebSocket RPC connections (eth_subscribe, newHeads, logs) are not available on public Base endpoints»
  ([RPC overview](https://docs.base.org/base-chain/api-reference/rpc-overview.md),
  [throughput](https://docs.base.org/base-chain/network-information/throughput-and-limits.md)).
- Платные провайдеры из официального списка: Coinbase Developer Platform, Alchemy, Blockmachine, GetBlock, QuickNode,
  Validation Cloud; цены/лимиты на странице — только стартовые скидки (QuickNode $300 разово и т. п.)
  ([node providers](https://docs.base.org/base-chain/node-operators/node-providers.md)). Цены тарифов — не найдено.
- Методы: `eth_simulateV1` — **есть** («Simulate transaction bundles against pre-confirmed state»); `debug_traceTransaction`,
  `debug_traceBlock*` — есть; **`debug_traceCall` в списке нет**; подмена состояния в `eth_call` — в документации не
  описана ([RPC overview](https://docs.base.org/base-chain/api-reference/rpc-overview.md)). У конкретного провайдера —
  проверять «Проверить подключение».
- Что из нужного боту есть: `eth_simulateV1` (пробы налогов, сухая сделка), `eth_subscribe logs/newHeads` (у
  провайдера), `eth_getBlockReceipts`/квитанции. При 5 блоках/с лента квитанций бота (2 вызова на блок —
  `node/receipts_budget.rs`) = 10 вызовов/с только на ленту [расчёт].

### 1.4 DEX (DefiLlama, снимок 10.11.2026; [страница](https://defillama.com/dexs/chain/base))
Всего: 24 ч $854,78 млн, 30 дн. $33,25 млрд.

| DEX | 24 ч | 30 дн. | Доля 30 дн. [расчёт] | Вид пулов | Есть у бота? |
| --- | --- | --- | --- | --- | --- |
| Aerodrome | $451,65 млн | $17,018 млрд | 51 % | Solidly V2 (`Pool.sol`, «inspired by Solidly», [README](https://github.com/aerodrome-finance/contracts)) + Slipstream (форк Uniswap V3 core, «preserved the UniswapV3 interface», [SPECIFICATION](https://github.com/velodrome-finance/slipstream)) | **Да:** `Proto::Solidly` (математика — порт velodrome-finance `Pool.sol`, `math/solidly_math.rs`), `Proto::Slipstream` (`pools/families.rs`) |
| Uniswap | $328,12 млн | $10,329 млрд | 31 % | V2, V3, V4 (V4 на Base: PoolManager `0x4985…2b2b`, UR `0x6ff5…9b43`, [deployments](https://developers.uniswap.org/docs/protocols/v4/deployments.md)) | **Да:** V2/V3/V4 |
| PancakeSwap | $51,73 млн | $3,796 млрд | 11 % | V2, V3, Infinity CL + Bin (Infinity на Base с 22.07.2025, [блог](https://blog.pancakeswap.finance/articles/baseinfinity)) | **Да** |
| Metric | $7,24 млн | $957,54 млн | 3 % | не проверено | нет данных |
| Tessera V | $3,75 млн | $514,05 млн | 1,5 % | не проверено (есть и на BSC — $322 млн/30 дн.) | нет данных |
| Curve | $2,3 млн | $72,85 млн | 0,2 % | StableSwap / крипто-пулы | только StableSwap |
| Quickswap | $1 млн | $62,79 млн | 0,2 % | Algebra | Да (Algebra/Integral) |
| Maverick | $0,38 млн | $18,71 млн | — | Maverick V2 | Да |

### 1.5 Агрегаторы
KyberSwap Aggregator поддерживает Base ([supported networks](https://docs.kyberswap.com/getting-started/supported-exchanges-and-networks)).
У бота адаптер KyberSwap есть, но выключен (решение Тиграна, 09.11). На Base 93 % объёма [расчёт] — в видах пулов,
которые бот считает сам; агрегатор нужен только для «хвоста» (Metric, Tessera и др. — их устройство не проверено).

### 1.6 Токены с налогами
Статистики распространённости — не найдено. Бот меряет налог пробой `eth_simulateV1` — на Base метод есть.

### 1.7 Биржи (сеть Base)
| Биржа | Факт | Источник |
| --- | --- | --- |
| Gate | сеть `BASEEVM`: 161 монета, из них 84 без выключенного ввода/вывода; **USDT — нет сети Base**, USDC — есть | `tests/data/cex_samples_2026_11_04/gate_spot_currencies.json` (живой ответ API 04.11.2026) |
| KuCoin | сеть `Base`: 84 монеты (53 с включёнными вводом и выводом); **USDT — нет Base**; USDC Base: комиссия 0,5, 240 подтверждений | `tests/data/cex_samples_2026_11_04/kucoin_currencies_v3.json` |
| MEXC | поддерживает Base (USDC on Base) | [MEXC: USDC on Base](https://mexc.com/support/articles/17827791523950) |
| Binance | интегрирует токены на Base (пример — BIO) | [пост Binance](https://www.binance.com/en/square/post/05-21-2025-binance-completes-integration-of-bio-protocol-bio-on-base-network-enables-deposits-and-withdrawals-24528386090041) |
| BingX | в записанном ответе `config/getall` (20 монет, 24.10.2026) у USDT сети Base нет; другие монеты — не найдено | `tests/data/bingx_2026-10-24/config_getall.json` |
| Bybit, Bitget, OKX, XT, HTX, CoinEx, Kraken, Phemex, Poloniex | не найдено (нет записанного ответа API; живой запрос отсюда недоступен) | — |

**Вопрос к Тиграну:** бот покупает/продаёт за `venue.quote_asset` (USDT). На Base USDT у Gate/KuCoin нет — значит,
монету купили за USDT на бирже, на Base продаём в пул X/USDC или X/WETH, а USDC/ETH надо возвращать на биржу и менять
в USDT. Нужен ли режим «котируемая монета сети = USDC» (или USDC как промежуточная, решение 52)?

### 1.8 Вывод по Base
- **Смысл:** самый большой объём из кандидатов, и 93 % [расчёт] объёма — в видах пулов, которые у бота уже есть.
- **Переиспользуем:** ядро, шлюз, биржи; математику V2/V3/V4/Infinity CL+Bin/Solidly/Slipstream/Algebra/Maverick/
  Curve-stable; граф, пробы `eth_simulateV1`, `ArbExecutor` (развернуть в сети; transient storage — нужен Cancun:
  в OP Stack есть с Ecotone — проверить по адресу узла перед развёртыванием).
- **Заново:** газ = L2 + L1-плата (`getL1Fee`) и EIP-1559 вместо legacy с базой 0 (`send/tx.rs:2-11`); каналы отправки
  — все 14 BSC-каналов не применимы, остаётся открытая отправка через провайдера (+ возможно Validity Transactions);
  темп ленты при 200 мс блоках; котируемая монета USDC.
- **Сложность:** средняя (сеть по устройству проще BSC — нет бандлов и билдеров; главное — газ L1 и USDC).

---

## 2. OP Mainnet / Optimism (chainId 10, OP Stack, газ — ETH)

### 2.1 Устройство
- Блок **каждые 2 с** («In OP Mainnet, blocks are produced every two seconds»), мемпул приватный у секвенсора
  (`op-stack/protocol/overview.mdx:35-36`, [ethereum-optimism/docs](https://github.com/ethereum-optimism/docs) 8a4bf24).
  Flashblocks — 250 мс на OP Mainnet (`op-stack/transactions/flashblocks.mdx:5`).
- Порядок — по priority fee; с 26.05.2026 шёл ограниченный эксперимент «stake-based priority ordering» до 4 недель,
  после — снова priority fee ([fees](https://docs.optimism.io/stack/transactions/fees)).
- Бандлов нет. `eth_sendRawTransactionConditional` есть, но proxy секвенсора пропускает его **только для транзакций в
  4337 EntryPoint** (`chain-operators/tools/op-txproxy.mdx:25-40`) — арбитражу не подходит.
- Атомарность DEX ↔ DEX — одной транзакцией `ArbExecutor`, как на Base.

### 2.2 Газ
Та же формула OP Stack: execution + L1 data fee (Fjord) + operator fee ([fees](https://docs.optimism.io/stack/transactions/fees)).
Минимальная baseFee OP Mainnet и типичная стоимость свопа в USD — не найдено.

### 2.3 Узлы
- Публичный `https://mainnet.optimism.io` — с лимитом, **без WebSocket** («rate limited and do not support websocket
  connections»), секвенсор `https://mainnet-sequencer.optimism.io` — только запись
  (`op-mainnet/network-information/connecting-to-op.mdx`).
- Платные провайдеры (официальный список, 20 шт.): 1RPC, Ankr, Alchemy, All That Node, Blast, Blockdaemon, BlockPI,
  Chainstack, dRPC, GetBlock, Grove, Infura, Moralis, Nodies, NOWNodes, OnFinality, QuickNode, RockX, Tenderly,
  Validation Cloud (`app-developers/reference/rpc-providers.mdx`). Цены — не найдено.
- `eth_simulateV1` и `debug_traceCall`: в op-geth есть `internal/ethapi/simulate.go` и `eth/tracers/api.go`
  ([op-geth](https://github.com/ethereum-optimism/op-geth) коммит 79e9d24). Включены ли у провайдера — проверять.

### 2.4 DEX (DefiLlama 10.11.2026, [страница](https://defillama.com/dexs/chains/optimism))
Всего 24 ч $12,21 млн, 30 дн. $564,08 млн. Velodrome $314,99 млн/30 дн. (56 %), Uniswap $230,05 млн (41 %), остальное —
копейки (Balancer, Curve, Sushi). Velodrome = тот же код, что Aerodrome (Solidly V2 + Slipstream) — **у бота есть**.

### 2.5 Агрегаторы
KyberSwap — поддерживает Optimism ([список](https://docs.kyberswap.com/getting-started/supported-exchanges-and-networks)).
При двух DEX на 97 % объёма [расчёт] свой поиск пулов достаточен.

### 2.6 Налоги — статистики не найдено.

### 2.7 Биржи
| Биржа | Факт | Источник |
| --- | --- | --- |
| BingX | USDT сеть `OPTIMISM`: **вывод и ввод выключены**, комиссия 1, minConfirm 600 | `tests/data/bingx_2026-10-24/config_getall.json` (24.10.2026) |
| KuCoin | USDT `OPTIMISM`: **ввод и вывод выключены**; USDC `OPTIMISM`: вкл., комиссия 1, 500 подтв.; монет с сетью — 8 (6 активных) | `kucoin_currencies_v3.json` (04.11.2026) |
| Gate | `OPETH`: 14 монет (9 активных); USDT и USDC — вкл. | `gate_spot_currencies.json` (04.11.2026) |
| MEXC | поддерживает обновления сети Optimism | [MEXC](https://www.mexc.com/support/articles/17827791523154) |
| Остальные | не найдено | — |

### 2.8 Вывод
Устройство и код — как Base (та же работа), но рынок в 70 раз меньше и USDT по сети у двух бирж выключен.
**Брать только «в нагрузку» к Base**, когда профиль OP Stack уже сделан: переделок сверх Base почти нет.

---

## 3. Polygon PoS (chainId 137, газ — POL)

### 3.1 Устройство
- **Блок 1 с** с блока 80 084 800 (обновление Madhugiri), до того 2 с: `Period: {"0": 2, "80084800": 1}` в
  `BorMainnetChainConfig` ([bor params/config.go](https://github.com/0xPolygon/bor) коммит 3823bb8).
- **Кто производит блоки:** после Rio (08.10.2025) валидаторы выбирают небольшой пул производителей, активен один,
  есть резервные; «Rio brings reorgs to an end» ([блог Rio](https://polygon.technology/blog/polygon-launches-major-payments-upgrade-with-rio-faster-lighter-and-easier-to-build)).
  По блогу 02.04.2026 — «a known set of Polygon-operated producers».
- Финальность 2–5 с (milestones, Heimdall v2) ([finality](https://docs.polygon.technology/pos/concepts/finality/finality.md)).
- **Мемпул публичный** + **официальный приватный мемпул** (02.04.2026): транзакции идут прямо выбранным производителям
  блоков, мимо публичного мемпула; бесплатный уровень для всех, платные — выше лимиты; адрес и поддержка бандлов в
  статье не указаны ([блог](https://polygon.technology/blog/polygon-launches-private-mempool-mev-protection-is-now-a-one-line-integration)).
- Атомарность DEX ↔ DEX — одной транзакцией `ArbExecutor`; официальных бандлов — не найдено.

### 3.2 Газ
- EIP-1559, baseFee сжигается ([EIP-1559 на Polygon](https://docs.polygon.technology/pos/concepts/transactions/eip-1559.md)).
- **Минимальный priority fee — 25 gwei** («On Polygon Chain mainnet, a minimum priority fee of 25 gWei is required»);
  живые значения — Gas Station `https://gasstation.polygon.technology/v2`
  ([gas station](https://docs.polygon.technology/tools/gas/polygon-gas-station.md)).
- Стоимость свопа в USD — не найдено (курса POL и живой цены газа здесь нет). [расчёт] на минимуме чаевых:
  200 000 газа × 25 gwei = 0,005 POL + baseFee.

### 3.3 Узлы
- В примерах документации — `https://polygon-rpc.com`; лимиты — не найдено. Платные — Alchemy, QuickNode, Infura и
  др. (цены — не найдено).
- Bor (код): `eth_call` с `StateOverride` и `BlockOverrides` (`internal/ethapi/api.go:1039`), **`eth_simulateV1`**
  (`api.go:1084`, `simulate.go`), трассировщики `eth/tracers/api.go` ([bor](https://github.com/0xPolygon/bor) 3823bb8).
  Всё нужное боту в клиенте есть; у провайдера — проверять.

### 3.4 DEX (DefiLlama 10.11.2026, [страница](https://defillama.com/dexs/chain/polygon))
Всего 24 ч $96,74 млн, 30 дн. $3,837 млрд.

| DEX | 24 ч | 30 дн. | Вид | Есть у бота? |
| --- | --- | --- | --- | --- |
| Uniswap | $64,58 млн | $2,17 млрд | V2/V3/V4 (V4 PoolManager `0x6736…e5cd6`, [deployments](https://developers.uniswap.org/docs/protocols/v4/deployments.md)) | Да |
| QuickSwap | $5,67 млн | $1,235 млрд | V2 + V3 на Algebra («exclusive license to operate Algebra's V3», [блог QuickSwap](https://quickswap-blog-qs.pages.dev/posts/quickswaps-v3-on-polygon.html)); версия Algebra — не указана | Algebra V1/V1.9 и Integral — да; какая версия у QuickSwap — проверить на узле |
| RamsesX | $10,77 млн | $155,03 млн | не проверено | ? |
| DODO | $5,28 млн | $100,81 млн | PMM (DVM/DPP/DSP) | Да |
| Balancer | $0,26 млн | $24,2 млн | взвешенные/стабильные пулы Balancer | **Нет** |
| Curve | $0,93 млн | $13,9 млн | StableSwap и др. | Только StableSwap |
| Sushi | $0,78 млн | $13,63 млн | V2/V3-форк | Да (форки) |
| WOOFi | $0,46 млн | $8,22 млн | WooPPV2 | Да |

### 3.5 Агрегаторы
KyberSwap — поддерживает Polygon POS ([список](https://docs.kyberswap.com/getting-started/supported-exchanges-and-networks)).
Свой поиск закрывает Uniswap + QuickSwap + DODO = ~91 % 30-дневного объёма [расчёт].

### 3.6 Налоги — статистики не найдено. Проба `eth_simulateV1` в Bor есть.

### 3.7 Биржи
| Биржа | Факт | Источник |
| --- | --- | --- |
| BingX | USDT сеть `POLYGON`: ввод/вывод вкл., комиссия 0,8 USDT, minConfirm 450 | `config_getall.json` (24.10.2026) |
| KuCoin | USDT `Polygon POS`: вкл., комиссия 0,8, 300 подтв. (pre 200); монет с сетью 38 (7 с обоими вкл.) | `kucoin_currencies_v3.json` (04.11.2026) |
| Gate | `MATIC`: 159 монет (46 активных), USDT — вкл. | `gate_spot_currencies.json` |
| Остальные | не найдено | — |

Время зачисления: [расчёт] 450 подтв. × 1 с ≈ 7,5 мин у BingX; 300 × 1 с = 5 мин у KuCoin (по числу подтверждений
из API и блоку 1 с; реальное время — проверять сделкой).

### 3.8 Вывод
- **Смысл:** средний рынок, USDT нативный и выводится, публичный + официальный приватный мемпул, все методы узла есть.
- **Переиспользуем:** почти всё (как Base) + легче: нет L1-платы.
- **Заново:** EIP-1559 и минимум чаевых 25 gwei; канал отправки — открытый + приватный мемпул Polygon (по его
  документации, когда найдём адрес); проверка версии Algebra у QuickSwap; Balancer — нет.
- **Сложность:** низкая–средняя.

---

## 4. Avalanche C-Chain (chainId 43114, газ — AVAX)

### 4.1 Устройство
- Блок: ~1 с с января 2025 (заметка в официальном блоге: «as of January 2025, C-Chain block time has been reduced to
  ~1s», `content/blog/cortina-x-chain-linearization.mdx`, [ava-labs/builders-hub](https://github.com/ava-labs/builders-hub)
  2a3c9f7); с Granite (19.11.2025) минимальное время блока голосуют валидаторы (ACP-226), текущее значение — не найдено
  ([Granite](https://docs.avax.network/blog/granite-upgrade), [ACP-226](https://docs.avax.network/docs/acps/226-dynamic-minimum-block-times)).
- Упорядочивают валидаторы (Snowman); порядок в блоке — «by the priority fee, then the timestamp (oldest first)»
  (`content/docs/rpcs/other/guides/txn-fees.mdx:48`).
- **Мемпул публичный:** API `txpool` и подписка `newPendingTransactions` включены по умолчанию
  (`vms/evm/imp/cchain/config.md:70-84`, [avalanchego](https://github.com/ava-labs/avalanchego) ff584dc).
  Официальной приватной отправки/бандлов — не найдено.
- **Helicon, 22.09.2026 — отложенное исполнение (ACP-194):** консенсус принимает блок, исполнение идёт позже. Для
  бота: `latest` = последний ИСПОЛНЕННЫЙ блок; `eth_getLogs` и фильтры по ещё не исполненному блоку **возвращают
  пусто**; `baseFeePerGas` в заголовке — верхняя граница; `gasUsed` заголовка = сумма лимитов
  (`content/docs/rpcs/c-chain/index.mdx:55-63`). Лента квитанций бота (`node/receipts_*.rs`) должна это учитывать.
- **Газ берётся `max(gasUsed, ceil(gasLimit/2))`** — лимит газа выше 2× реального расхода стоит денег
  (`content/docs/primary-network/exchange-integration.mdx:107-113`). Бот ставит лимит `eth_estimateGas + запас`
  (`send/tx.rs`) — запас должен быть < 100 %.
- Транзакции: legacy, EIP-2930, EIP-1559; blob и EIP-7702 — нет (там же).

### 4.2 Газ
- С Helicon минимальную baseFee голосуют валидаторы (ACP-283); «In late September 2026, Mainnet validators raised the
  minimum from 1 wei to 5 nAVAX»; 03.10.2026 baseFee была на этом минимуме; и baseFee, и чаевые **сжигаются**
  (`txn-fees.mdx:54-75`, [ACP-283](https://docs.avax.network/docs/acps/283-dynamic-minimum-gas-price)).
- Стоимость свопа в USD — не найдено. [расчёт] 200 000 газа × 5 nAVAX = 0,001 AVAX.

### 4.3 Узлы
- Публичные `https://api.avax.network/ext/bc/C/rpc` и **WebSocket `wss://api.avax.network/ext/bc/C/ws`**
  (`content/docs/rpcs/c-chain/index.mdx:13-35`). Лимиты публичного API — не найдено.
- Методы C-Chain (полный список по API): `eth_call`, `eth_estimateGas`, `eth_getBlockReceipts`, `eth_subscribe`
  (logs, newHeads, newPendingTransactions), `debug_traceCall` — **есть**; **`eth_simulateV1` в списке нет**
  (`vms/evm/imp/cchain/config.md:70-84`). Поддержка подмены состояния в `eth_call` — в документации не описана
  (в geth-коде `debug_traceCall` принимает stateOverrides — проверять на узле).
- Пачка JSON-RPC по умолчанию ≤ 1000 запросов (`batch-request-limit`, там же:65).

### 4.4 DEX (DefiLlama, 10.11.2026)
Два снимка разошлись: [dexs/chain/avalanche](https://defillama.com/dexs/chain/avalanche) — 24 ч $44,31 млн, 30 дн.
$2,674 млрд (имя лидера не отдано); [protocols/dexs/avalanche](https://defillama.com/protocols/dexs/avalanche) — 24 ч
$61,72 млн, 30 дн. $2,374 млрд:

| DEX | 24 ч | 30 дн. | Вид | Есть у бота? |
| --- | --- | --- | --- | --- |
| Pharaoh Exchange | $22,2 млн | $715,44 млн | не проверено по коду | ? |
| Blackhole | $15,71 млн | $666,83 млн | не проверено по коду | ? |
| LFJ (Trader Joe) | $3,15 млн | $469,98 млн | **Liquidity Book** (`LBPair`, [joe-v2](https://github.com/lfj-gg/joe-v2)) | **Нет.** Бины как у Infinity Bin, но у LB переменная комиссия от волатильности (`getVariableFee`, `PairParameterHelper.sol:238`) — в Infinity Bin её нет; нужен порт |
| Uniswap | $9,19 млн | $236,4 млн | V2/V3/V4 (V4 PoolManager `0x0638…bc85`) | Да |
| Pangolin | $2,42 млн | $68,77 млн | V2-форк | Да (форк V2) |
| WOOFi | $1,46 млн | $19,48 млн | WooPPV2 | Да |
| Dexalot | $0,45 млн | $39,71 млн | не проверено | ? |

### 4.5 Агрегаторы
KyberSwap — поддерживает Avalanche ([список](https://docs.kyberswap.com/getting-started/supported-exchanges-and-networks)).
Здесь агрегатор полезнее, чем на Base: ~78 % 30-дневного объёма [расчёт] — Pharaoh/Blackhole (вид не проверен) и LFJ
(нет математики).

### 4.6 Налоги — статистики не найдено. Пробу через `eth_simulateV1` на C-Chain делать нельзя (метода нет).

### 4.7 Биржи
| Биржа | Факт | Источник |
| --- | --- | --- |
| KuCoin | USDT `AVAX C-Chain`: вкл., комиссия 1, 32 подтв.; монет с сетью 16 (9 активных) | `kucoin_currencies_v3.json` |
| Gate | `AVAX_C`: 49 монет (15 активных), USDT — вкл. | `gate_spot_currencies.json` |
| BingX | у USDT сети AVAX в записанном ответе нет | `config_getall.json` (24.10.2026) |
| Остальные | не найдено | — |

### 4.8 Вывод
- **Смысл:** рынок меньше Polygon, мало монет у бирж (Gate 15, KuCoin 9 активных).
- **Заново:** пробы налогов и сухая сделка без `eth_simulateV1` (`check/simulate.rs` и всё, что на нём); учёт
  отложенного исполнения в ленте; газ «половина лимита»; Liquidity Book; разбор Pharaoh/Blackhole.
- **Сложность:** высокая.

---

## 5. Короткие сети

**Linea.** DEX за 24 ч $351 808, за 30 дн. $37,36 млн; Lynex, Etherex, BrownFi, PancakeSwap, Uniswap
([DefiLlama](https://defillama.com/dexs/chain/linea), 10.11.2026). Uniswap V4 на Linea в списке развёртываний нет
([deployments](https://developers.uniswap.org/docs/protocols/v4/deployments.md)). KyberSwap поддерживает. Биржи: Gate
`LINEAETH` 8 монет (6 активных), KuCoin `Linea` 3 (1 активная). Устройство сети (время блока, мемпул) — не найдено:
страницы docs.linea.build в этой сессии не отдали текст. **Объём мал — не брать сейчас.**

**zkSync Era.** DEX 24 ч $331 945, 30 дн. $15,76 млн (Maverick, SyncSwap, PancakeSwap, iZUMi, zkSwap)
([DefiLlama](https://defillama.com/dexs/chain/zksync-era)). Обычный EVM-байткод теперь разворачивается через
интерпретатор, но газ «1.5x to 4x higher», нет `CALLCODE`/`SELFDESTRUCT`, `delegatecall` между EVM и EraVM нет
([EVM interpreter](https://docs.zksync.io/zksync-protocol/evm-interpreter/overview)). KyberSwap Aggregator — нет (только
cross-chain). Gate `ZKSERA` 12 монет (6 активных). **Объём мал + другая VM — не брать.**

**Sonic** (chainId 146, газ S). Блок «<1s», финальность меньше секунды ([Alchemy](https://www.alchemy.com/rpc/sonic.md);
официальная docs.soniclabs.com в этой сессии не читалась). DEX 24 ч $1,61 млн, 30 дн. $41,78 млн (Shadow Exchange,
SwapX, Metropolis, Beets) ([DefiLlama](https://defillama.com/dexs/chain/sonic)). KyberSwap поддерживает. Gate `S` 4 монеты
(3 активных), KuCoin `Sonic` 2 активных (USDC Sonic: комиссия 0,21, 10 подтв.). **Объём мал — не брать.**

**Mantle** (газ MNT). По docs-v2: дизайн OP Stack, блок 2 с, данные — Mantle DA на EigenDA
([FAQ v2](https://docs-v2.mantle.xyz/intro/more/faqs)); старая страница FAQ говорит другое (блок на каждую транзакцию,
FIFO, без EIP-1559 — [FAQ](https://docs.mantle.xyz/network/introduction/faqs)) — **противоречие, проверять на узле**.
DEX 24 ч $3,37 млн, 30 дн. $228,39 млн (Merchant Moe, Agni, Fluxion — виды пулов не проверены)
([DefiLlama](https://defillama.com/dexs/chain/mantle)). KyberSwap Aggregator — нет. Gate `MNT` 7 монет (2 активных).
**Не брать.**

**Tron — не полностью EVM.** TVM «largely EVM-compatible», но блок 3 с, DPoS 27 Super Representatives, плата —
Bandwidth + Energy (не газ), адреса `T…` Base58Check / `41…` hex ([TRON vs Ethereum](https://developers.tron.network/docs/tron-vs-ethereum)).
Для бота это другой формат адреса (ядро — 20-байтный `Address`, см. code-multinet.md), другая модель платы, свой API
узла. Объём DEX — не получен (страница DefiLlama не открылась). Биржи: KuCoin `TRC20` 18 монет, Gate `TRX` 34 (17
активных); USDT TRC20 у BingX комиссия 1,5, 2 подтв. **Только если нужен именно USDT-рынок Tron; работа — как новая
платформа.**

**TON — не EVM.** Сообщения асинхронные: одна транзакция меняет один аккаунт; «Ethereum can run multiple contract calls
atomically in one transaction. TON has no equivalent» ([TON from Ethereum](https://docs.ton.org/from-ethereum)) —
атомарного цикла DEX ↔ DEX как у бота быть не может. DEX 24 ч $5,13 млн, 30 дн. $277,96 млн (STON.fi, DeDust, TONCO)
([DefiLlama](https://defillama.com/dexs/chain/ton)). Gate `TON` 45 монет, активных 0 (по флагам ответа 04.11);
KuCoin `TON` 54 (8 активных). **Новая платформа; не брать.**

---

## 6. Сравнительная таблица (снимки 04–10.11.2026)

| Сеть | DEX 24 ч / 30 дн. | Блок | Мемпул / приватная отправка | `eth_simulateV1` | Виды пулов у бота | Монет у Gate / KuCoin (активных) | USDT по сети | Сложность |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| BSC (для сравнения) | $1,214 млрд / $21,4 млрд | 0,45 с (bsc_node.md) | публичный / 14 каналов, бандлы | есть | все | 346 / 127 | да | — |
| **Base** | $854,8 млн / $33,25 млрд | 200 мс | нет публичного / Validity Tx, бандлов нет | есть (док.) | ~93 % объёма | 84 / 53 | **нет у Gate/KuCoin** (USDC есть) | средняя |
| **Polygon PoS** | $96,7 млн / $3,84 млрд | 1 с | публичный + приватный Polygon | есть (код Bor) | ~91 % | 46 / 7 | да | низкая–средняя |
| **Avalanche C** | $44–62 млн / $2,4–2,7 млрд | ~1 с, голосуемое | публичный / не найдено | **нет** | ~14 % (Uniswap, Pangolin, Woo) | 15 / 9 | да | высокая |
| **Optimism** | $12,2 млн / $0,56 млрд | 2 с | нет публичного / нет | есть (op-geth) | ~97 % | 9 / 6 | выкл. у BingX и KuCoin | как Base (после Base — дёшево) |
| Mantle | $3,4 млн / $228 млн | 2 с (docs-v2) | не найдено | не найдено | не проверено | 2 / 1 | не найдено | — |
| TON | $5,1 млн / $278 млн | — | — | — (не EVM) | 0 | 0 / 8 | да (TON) | новая платформа |
| Sonic | $1,6 млн / $41,8 млн | <1 с | не найдено | не найдено | не проверено | 3 / 2 | не найдено | — |
| Linea | $0,35 млн / $37,4 млн | не найдено | не найдено | не найдено | частично | 6 / 1 | не найдено | — |
| zkSync Era | $0,33 млн / $15,8 млн | не найдено | не найдено | не найдено | — (другая VM) | 6 / 1 | не найдено | высокая |
| Tron | не получено | 3 с | не найдено | — (свой API) | 0 | 17 / 11 | да (TRC20) | новая платформа |

«Монет у Gate / KuCoin» — число монет с сетью, у которых ввод и вывод не выключены, по ответам API 04.11.2026
(`tests/data/cex_samples_2026_11_04/`). «Виды пулов у бота» — доля 30-дневного объёма DefiLlama у DEX, чьи виды пулов
есть в `pools/families.rs` [расчёт].

---

## 7. С чего начинать (рейтинг)

1. **Base.** Факты: объём DEX 70 % от BSC за 24 ч; 3 главных DEX (93 % объёма) — виды, уже написанные в боте (Solidly и
   Slipstream портированы как раз из кода Velodrome/Aerodrome); `eth_simulateV1` есть; у Gate 84, у KuCoin 53 активных
   монеты в сети — больше всех кандидатов. Перед стартом решить: USDC вместо USDT как котируемая монета; газ L1;
   платный провайдер с WebSocket; отправка без бандлов (конкуренция приоритетной платой).
2. **Polygon PoS.** Факты: объём в 9 раз меньше Base, но USDT по сети включён у всех трёх бирж с записанными ответами
   (BingX и KuCoin — комиссия 0,8 USDT, Gate — вкл.); в Bor есть всё (`eth_simulateV1`, overrides); есть официальный
   приватный мемпул. Переделок меньше всех (нет L1-платы).
3. **Optimism** — сразу после Base, если профиль OP Stack готов: код тот же, но рынок маленький и USDT по сети у
   BingX/KuCoin выключен.
4. **Avalanche C-Chain** — позже: нет `eth_simulateV1`, отложенное исполнение (Helicon), Liquidity Book без математики,
   мало монет у бирж.
5. **Не брать сейчас:** Mantle, Sonic, Linea, zkSync Era (объём < $4 млн/сутки); Tron и TON — новые платформы
   (не EVM-стек бота).

---

## 8. Открытые вопросы к Тиграну

1. **Base / Optimism без USDT.** Gate и KuCoin не выводят USDT по Base. Делать котируемую монету сети = USDC (новая
   настройка на экземпляр), или USDC — промежуточная (решение 52) с обменом USDC ↔ USDT на бирже?
2. **Avalanche: пробы без `eth_simulateV1`.** Переписывать пробы на `eth_call`/`debug_traceCall` с подменой состояния
   (надо проверить на узле, что overrides работают), или Avalanche отложить?
3. **Какие ещё биржи проверить по сетям.** У Bybit, Bitget, OKX, XT, HTX, CoinEx, Kraken, Phemex, Poloniex, Binance,
   MEXC нет записанных ответов со справочником сетей. Снять живые ответы (с ключом бота) — тогда таблица 7 будет полной.

## Источники, недоступные в этой сессии
- Прямые RPC-запросы к узлам и публичные API бирж — 403 прокси среды.
- `https://gasstation.polygon.technology/v2`, `https://docs.polygon.technology/_llms/polygon-chain.txt`,
  страницы DefiLlama по Tron — не открылись.
