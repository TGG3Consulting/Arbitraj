# Сеть Ethereum (L1 mainnet) для бота ARB — разбор

Дата разбора: 10.11.2026. Задача Тиграна от 10.11: схема «одна сеть, один сервер, свои кошельки», узел сети не свой
(открытый или платный). Нужно понять, подходит ли Ethereum L1 боту.

Как читать: у каждого факта есть ссылка на источник. Если пометка «**расчёт**», это моя арифметика на фактах, а не факт.
«**Не найдено**» значит, что в официальных источниках я этого не нашёл или страница не открылась. Код бота я только читал
(ссылки на файлы репозитория). Ничего не менял и не собирал.

---

## 0. Коротко

- Блок раз в 12 секунд. На BSC блок 0,45 с (`src/venues/bsc_dex/README.md`, research/flow/bsc_node.md), то есть
  Ethereum примерно в 27 раз медленнее (**расчёт**). Финальность наступает через 2–3 эпохи, это 12–19 минут.
- Газ сейчас очень дешёвый. 10.11.2026 в 07:13 UTC Etherscan показывал base fee 0,04 gwei, а оценку свопа $0,03.
- Приватная отправка и бандлы есть и хорошо описаны: Flashbots, BuilderNet, Titan. Неудачный бандл в блок не попадает
  и за газ не платит.
- Биржи берут за вывод по Ethereum в 5–10 раз больше, чем по BSC, и зачисляют ввод только после **64 подтверждений**.
  Это видно в записанных ответах API BingX и KuCoin. 64 × 12 с ≈ 12,8 минуты, пока монета «в пути» (**расчёт**).
  Направление «биржа ↔ DEX» на Ethereum из-за этого рискованное.
- Направление «DEX ↔ DEX» (атомарный цикл бандлом) ложится на Ethereum хорошо. Но здесь главная конкуренция
  поисковиков MEV, а у нас блоки в 27 раз реже.
- Объём DEX на Ethereum 10.11.2026: $1,742 млрд за 24 часа, из них 60% у Uniswap (**расчёт** по DefiLlama).
  Значимая доля приходится на виды пулов, которых у нас нет: Fluid, Curve CryptoSwap, Balancer, Ekubo, а также RFQ
  (Native, Tokenlon).

---

## 1. Как устроена сеть для арбитража

### 1.1 Время блока, финальность, реорги

| Что | Факт | Источник |
| --- | --- | --- |
| Слот (место под блок) | «slots (12 seconds) and epochs (32 slots)» | https://ethereum.org/developers/docs/consensus-mechanisms/pos.md |
| Кто делает блок | «One validator is randomly selected to be a block proposer in every slot» (на каждый слот случайно выбирается один валидатор) | там же |
| Финальность | «typically takes 2 to 3 epochs (12 to 19 minutes)», в разборе примера — около 2 эпох (≈12,8 мин) | https://www.migalabs.io/eth-academy/en/consensus/finality (учебный материал MigaLabs, не ethereum.org: на ethereum.org время финальности не указано) |
| Откат финализированного блока | атакующему «commit to losing at least one-third of the total supply of staked ETH» (потерять минимум треть всего застейканного ETH) | ethereum.org pos.md |
| Короткие реорги | бывают, их смягчают «proposer boosting and attestation deadlines» | ethereum.org pos.md |

Частота и глубина реоргов на mainnet в цифрах: **не найдено**.

Что это значит для бота:
- Блок в 12 с. Окно между двумя блоками в 27 раз больше, чем на BSC (**расчёт**: 12 / 0,45). Цены пулов меняются
  реже, но и сделок у нас меньше.
- Механизм отката ленты квитанций (`node/receipts_chain.rs`, `removed: true`) переносится как есть. Нужно только
  задать порог «сколько блоков ждать» под Ethereum.

### 1.2 Кто упорядочивает транзакции, публичный мемпул

- Блок предлагает валидатор этого слота (см. выше). Сами блоки в основном собирают **билдеры** через MEV-Boost.
  Свежую долю билдеров (relayscan.io) открыть не удалось: **не найдено**.
- Публичный мемпул есть. По данным Blocknative, «approximately 30% of all transactions are private», и на них
  приходится «more than 50% of all Ethereum L1 gas used»
  (https://www.blocknative.com/blog/ethereum-private-transactions-the-flippening; даты на странице нет, последние
  данные статьи — июнь–июль 2024).
- Отправлять сделку арбитража в открытый мемпул опасно: её видят все и могут перехватить. Обычная практика —
  приватная отправка или бандл (п. 1.3).

### 1.3 MEV: приватная отправка и бандлы (официальные API)

**Flashbots Relay** — https://docs.flashbots.net/flashbots-protect/api/json-rpc
- Адрес: `https://relay.flashbots.net`.
- Методы: `eth_sendBundle`, `mev_sendBundle`, `eth_callBundle`, `mev_simBundle`, `eth_cancelBundle`,
  `eth_sendPrivateTransaction`, `eth_sendPrivateRawTransaction`, `eth_cancelPrivateTransaction`.
- `eth_sendBundle`: `txs` («A list of signed transactions to execute in an atomic bundle»), `blockNumber`,
  `minTimestamp`/`maxTimestamp`, `revertingTxHashes`, `replacementUuid`, `builders` («A list of registered block
  builder names to share the bundle with»).
- `eth_callBundle` симулирует бандл на заданном блоке. Ответ: `coinbaseDiff`, `gasFees`, `results`, `totalGasUsed`.
- Пределы: «10,000 requests per second per IP», в бандле не больше 100 транзакций и не больше 300 000 байт.
- Подпись: заголовок `X-Flashbots-Signature` = `<адрес>:<подпись>`. Подпись — EIP-191 от тела JSON, подходит любой
  ключ secp256k1. **У нас такой подписи запроса нет, её надо писать.**
- `eth_sendPrivateTransaction`: Flashbots пытается включить транзакцию «for the next 25 blocks».
- Рассылка нескольким билдерам: параметр `builders` у `eth_sendBundle`, `privacy.builders` у `mev_sendBundle`
  (https://docs.flashbots.net/flashbots-auction/advanced/multiplexing). Список имён билдеров на странице не вывелся:
  **не найдено**.

**Атомарность и откат** — https://docs.flashbots.net/flashbots-auction/advanced/troubleshooting
- Бандл не включают, если в нём откатилась транзакция, которой нет в `revertingTxHashes`. Это «всё или ничего».
  Неудачный бандл в цепь не попадает, газ за него не платим.
- «Each bundle submission is designed to target a specific block number»: каждый бандл — на один блок. Не успел —
  не включён.
- Конкуренты могут перебить ставку. Аукцион слепой: чужие ставки видны только после блока.
- После PoS бандл может стоять в любом месте блока. Внутрь бандла чужие транзакции не встают
  (https://docs.flashbots.net/flashbots-auction/advanced/bundle-pricing).

**Flashbots Protect** (одиночные транзакции) — https://docs.flashbots.net/flashbots-protect/settings-guide
- `https://rpc.flashbots.net`. С `/fast` транзакция уходит «all registered builders».
- Откатившиеся транзакции по умолчанию не включаются («does not land reverted transactions»).
- Повторы идут 25 блоков, потом транзакция отбрасывается.

**BuilderNet** — https://buildernet.org/docs/api
- `https://rpc.buildernet.org`. Методы: `eth_sendBundle` (с `replacementUuid`, `revertingTxHashes`,
  `droppingTxHashes`, `refundPercent` и др.) и `eth_sendRawTransaction`. Про одиночную транзакцию сказано: не идёт
  в мемпул, пробуется до 5 блоков.
- Подпись: `X-Flashbots-Signature` (v1) или многоразовый `X-Flashbots-Auth` (v2, живёт до 24 ч).
- Предел запросов на странице: **не найдено**.

**Titan Builder** — https://docs.titanbuilder.xyz/llms-full.txt
- `https://rpc.titanbuilder.xyz` и региональные `eu.` / `us.` / `ap.rpc.titanbuilder.xyz`. Для сервера в Европе
  нужен `eu.`.
- `eth_sendBundle` (`txs`, `blockNumber`, `revertingTxHashes`, `droppingTxHashes`, `replacementUuid`,
  `refundPercent`…), `eth_cancelBundle`, `eth_sendRawTransaction`, `eth_sendPrivateTransaction` (без
  `maxBlockNumber`).
- Предел 50 запросов/с. Ключ для этих методов в документации не указан.
- «will never unbundle a bundle»; бандлы и приватные транзакции в публичный мемпул не уходят.

bloXroute работает с «more than twenty external builders», в примере — titan, beaverbuild, rsync-builder
(https://docs.bloxroute.com/eth/submit-bundles/list-of-external-builders.md).

### 1.4 Атомарность нескольких свопов
- В одной транзакции: свой контракт-исполнитель (как наш `contracts/ArbExecutor.sol`) или Universal Router Uniswap.
  Откат любого шага откатывает всю транзакцию (это свойство EVM; наш исполнитель так и устроен, см.
  `contracts/README.md`).
- Несколькими транзакциями: бандл Flashbots, BuilderNet или Titan, всё или ничего (п. 1.3).

---

## 2. Газ

### 2.1 Модель (EIP-1559) — https://eips.ethereum.org/EIPS/eip-1559
- Транзакция типа 2 («TransactionType 2»). В ней задаются `max_fee_per_gas` и `max_priority_fee_per_gas`.
- Base fee сжигается («The base fee per gas is burned»), строителю блока достаются только чаевые.
- `BASE_FEE_MAX_CHANGE_DENOMINATOR = 8`, `ELASTICITY_MULTIPLIER = 2`: base fee меняется за блок не больше чем на 1/8
  (12,5%). Цель по газу — половина лимита блока.
- **Отличие от нашего кода.** `src/venues/bsc_dex/send/tx.rs` собирает legacy-транзакцию (EIP-155), в шапке
  написано «база газа в BSC всегда 0». Цена газа берётся из `eth_gasPrice` (`quote/gas.rs`). На Ethereum base fee не
  0. Нужны транзакция типа 2 (`TxEip1559` в alloy) и учёт base fee следующего блока в котировке.

### 2.2 Сколько стоит своп сейчас
Etherscan Gas Tracker (https://etherscan.io/gastracker), снимок 10.11.2026 07:13:56 UTC:
- ETH = $2 115,13. Base fee 0,04 gwei, чаевые 0 / 0,001 / 0,002 gwei.
- Оценка «Swap»: $0,030 / $0,031 / $0,032. Сколько газа Etherscan закладывает в «Swap», на странице не сказано.

**Расчёт** по этим числам: $0,03 / (0,04 gwei × $2 115) ≈ 355 000 газа на своп в оценке Etherscan.

Газ меняется. Если base fee поднимется до 1 gwei, тот же своп будет стоить ≈ $0,75, а при 10 gwei ≈ $7,5
(**расчёт**, без чаевых).

### 2.3 Как это влияет на минимальный выгодный объём (**расчёт**, пример)
- DEX ↔ DEX, цикл из 3 свопов, ≈ 400 000–600 000 газа при 0,04 gwei: ≈ $0,03–0,05 плюс чаевые билдеру. Чаевые в
  бандле — это ставка на аукционе. Сколько обычно платят победители, в официальных источниках **не найдено**.
- Биржа ↔ DEX: газ мелкий, главная затрата — вывод с биржи (п. 7). Пример: USDT по ERC20 у KuCoin стоит 5,5 USDT.
  При пороге прибыли 1% только чтобы окупить вывод, нужна сделка от ≈ $550, без учёта остальных затрат.
- Формулы бота (газ × цена газа против прибыли) переносятся. Меняются только источник цены газа и тип транзакции.

---

## 3. Узлы (не свой)

### 3.1 Что из узла нужно боту (по коду)
Методы JSON-RPC в `src/venues` (подсчёт grep): `eth_blockNumber`, `eth_call` (в том числе Multicall3), `eth_chainId`,
`eth_estimateGas`, `eth_gasPrice`, `eth_getBalance`, `eth_getBlockByNumber`, `eth_getBlockReceipts`, `eth_getCode`,
`eth_getLogs`, `eth_getStorageAt`, `eth_getTransactionByHash`, `eth_getTransactionCount`,
`eth_getTransactionReceipt`, `eth_sendRawTransaction`, `eth_sendBundle`, `eth_sendPrivateTransaction`,
**`eth_simulateV1`** (с `stateOverrides` и `blockOverrides`, файлы `check/simulate.rs`, `check/sim_base.rs`),
`eth_subscribe` (`logs`, проверка `newHeads`), `eth_syncing`. `debug_traceCall` и `trace_*` в коде не используются.

Важно: `eth_simulateV1` нужен для пробы налогов (`check/`), сухой сделки и пробы бандла (`exec/`), а виртуальный
квотер работает через подмену кода в `eth_call` (`contracts/VirtualQuoter.sol`). Без `eth_simulateV1` у провайдера
не работают проверка монет и проба сделки.

### 3.2 Стандарт
- `eth_simulateV1` описан в спецификации execution-apis
  (https://ethereum.github.io/execution-apis/docs/reference/eth_simulatev1).
- В geth: «The total number of blocks for a single request is limited to 256», общий предел газа задаёт
  `--rpc.gascap` (https://geth.ethereum.org/docs/rpc/ns-eth).

### 3.3 Платные провайдеры

| Провайдер | Цена и лимиты (официальная страница) | eth_simulateV1 на Ethereum | debug/trace | Архив |
| --- | --- | --- | --- | --- |
| Alchemy (https://www.alchemy.com/docs/reference/pricing-plans.md) | Free: 30 млн CU, 300 CU/с. PAYG: $0,525 за 1 млн CU, 10 000 CU/с. Enterprise — договор | **не найдено** (страница для Ethereum не открылась; для других сетей у Alchemy метод описан) | Free — нет; PAYG, Enterprise — есть | на всех планах |
| Infura / MetaMask (https://docs.infura.io/get-started/pricing.md) | Free: 3 млн кредитов в сутки, 500 кредитов/с. Developer: 15 млн/сут, 4 000/с. Team: 75 млн/сут, 40 000/с. Цены в $ на странице нет | **есть**: «only supported on Mainnet and Sepolia», до 16 blockStateCalls за запрос (https://docs.metamask.io/services/reference/ethereum/json-rpc-methods/eth_simulatev1) | на странице цен не сказано | архив (старше 128 блоков) по той же цене кредитов |
| QuickNode (https://www.quicknode.com/pricing.md) | Free trial: 10 млн кредитов, 15 зап/с. Build $49: 80 млн, 50 зап/с. Accelerate $249: 450 млн, 125 зап/с. Scale $499: 950 млн, 250 зап/с. Business $999: 2 млрд, 500 зап/с. Перерасход $0,50–0,62 за 1 млн | **не найдено** (страница для Ethereum не открылась; для других сетей метод описан) | с Build | на всех |
| Chainstack (https://chainstack.com/pricing/) | Developer (бесплатно): 3 млн RU/мес, 25 зап/с. Growth $49: 20 млн, 250 зап/с. Pro $199: 80 млн, 400 зап/с. Business $499: 200 млн, 600 зап/с. Enterprise от $990: 400 млн, без предела зап/с. «Unlimited Node» $149/мес при 25 зап/с. Выделенные узлы — с Pro, от $0,50/час | **есть** (https://docs.chainstack.com/reference/ethereum-simulatev1.md; в примере `stateOverrides` с `balance`, `blockOverrides.baseFeePerGas`) | с Growth | с Growth |
| dRPC (https://drpc.org/docs/pricing/requests) | Free: только публичные узлы, 210 млн CU за 30 дней. PAYG: $6 за 1 млн запросов (20 CU × $0,30 за 1 млн CU), новые аккаунты до 100 зап/с. Pro $49: 163 млн CU, 300 зап/с. Scale $199: 664 млн CU, 600 зап/с | **не найдено** | **не найдено** | **не найдено** |

- WebSocket (`eth_subscribe`): Alchemy пишет «Smart WebSockets» на всех планах. У Infura WebSocket обрывается, когда
  кончился суточный лимит кредитов. Отдельные пределы WebSocket у провайдеров: **не найдено**.
- Задержка: официальных цифр задержки провайдеров до нашего дата-центра **не найдено**. Её надо мерить на сервере
  (у бота уже есть возраст головы `node/head_age.rs` и «Проверить подключение»).
- Пределы на пачки (batch): **не найдено** ни у кого.

### 3.4 Открытые (бесплатные) узлы
- PublicNode: «serves free Ethereum RPC endpoints that need no API key» (https://ethereum.org/tr/developers/tools/publicnode-allnodes/).
  Официальных цифр лимитов **не найдено**.
- Вывод: для котировки и сделки бесплатных узлов не хватит. Подойдут только для резерва (`node/failover.rs`,
  `receipts_nodes.rs`).

### 3.5 Сколько запросов нужно (**расчёт**)
Блок раз в 12 с, это ≈ 7 200 блоков в сутки. Лента квитанций бота делает 2 вызова на блок
(`node/receipts_budget.rs`), то есть ≈ 14 400 вызовов в сутки. Это мало. Основная нагрузка — загрузка пулов
(Multicall3) и `eth_simulateV1` на пробы. Сколько CU или кредитов стоит `eth_simulateV1` у каждого провайдера,
**не найдено**.

---

## 4. DEX сети

### 4.1 Доли объёма — DefiLlama, https://defillama.com/dexs/chain/ethereum (открыто 10.11.2026)
Всего по Ethereum: $1,742 млрд за 24 ч, $35,231 млрд за 30 дней.

| DEX | 24 ч | 30 дней | Доля 30 д (**расчёт**) |
| --- | --- | --- | --- |
| Uniswap (все версии вместе) | $1,04 млрд | $20,222 млрд | 57% |
| Fluid | $237,55 млн | $5 млрд | 14% |
| Curve | $205,32 млн | $3,969 млрд | 11% |
| Native (RFQ) | $96,61 млн | $1,511 млрд | 4% |
| DODO | $21,04 млн | $1,186 млрд | 3% |
| Ekubo | $18,9 млн | $802,51 млн | 2% |
| Metric | $26,11 млн | $553,4 млн | 2% |
| Balancer | $15,13 млн | $501,26 млн | 1,4% |
| PancakeSwap | $11,07 млн | $361,77 млн | 1% |
| Angstrom | $15,77 млн | $272,5 млн | 0,8% |
| Tokenlon (RFQ) | $20,52 млн | $230,79 млн | 0,7% |
| Sushi | $7,04 млн | $53,1 млн | 0,15% |
| Maverick | $128 537 | $2,89 млн | <0,01% |

Разбивка Uniswap по V2/V3/V4 только для Ethereum: **не найдено** (на странице одна строка «Uniswap»).

### 4.2 Виды пулов против нашей математики (`src/venues/bsc_dex/math/`, `pools/families.rs`)

| DEX | Вид пула | Есть у нас? | Реестр и события |
| --- | --- | --- | --- |
| Uniswap V2 | x·y=k | **да** (`math/v2.rs`) | Factory `0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f`, Router02 `0x7a250d5630B4cF539739dF2C5dAcb4c659F2488D` (https://docs.uniswap.org/contracts/v2/reference/smart-contracts/v2-deployments). События — Sync/Swap, как у нас |
| Uniswap V3 | концентрированная ликвидность | **да** (`swap_cl.rs`, ABI Uniswap V3 — `Proto::UniswapV3`) | адреса Ethereum на странице не вывелись (https://developers.uniswap.org/docs/protocols/v3/deployments.md отсылает на отдельную страницу) — **не найдено** здесь |
| Uniswap V4 | синглтон PoolManager, крючки | **да** (`Proto::UniswapV4`, `swap_step.rs` V4, проверка крючков `check/hooks_probe.rs`) | PoolManager `0x000000000004444c5dc75cB358380D2e3dE08A90`, StateView `0x7ffe42c4…7227`, Quoter `0x99bd42B2…A9D1`, Universal Router `0x66a9893c…a8af` (есть ещё 2.1.1 и 2.1.2), Permit2 `0x000000000022D473030F116dDEE9F6B43aC78BA3` (https://developers.uniswap.org/docs/protocols/v4/deployments.md) |
| Angstrom | официально не проверено (по памяти — крючок Uniswap V4) | **не проверено** | **не найдено** |
| Curve StableSwap / StableSwap-NG | stableswap | **частично**: `curve_math.rs`, `swap_curve.rs`, `stableswap_ng.rs` (порт NG для Infinity Stable) | фабрики StableSwap-NG, Twocrypto-NG, Tricrypto-NG (https://docs.curve.finance/factory/overview). Фабрики пополняют MetaRegistry. Адреса на странице не даны |
| Curve Twocrypto-NG / Tricrypto-NG | cryptoswap (другой инвариант) | **нет** | там же |
| Fluid DEX | свой (smart collateral / debt) | **нет** | есть `FluidDexResolver`, `FluidDexReservesResolver` (оглавление https://docs.fluid.instadapp.io/integrate/dex-v2-swaps.html). Тело руководства не вывелось: **не найдено** |
| Balancer (V2 и V3) | Vault; weighted, stable и др. | **нет** | на странице V2 (https://docs.balancer.fi/concepts/overview/basics.html) — единый Vault и виды пулов. Адреса V3: **не найдено** |
| Ekubo | синглтон («till», `ICore#lock`), concentrated / full-range / stableswap / TWAMM и др. | **нет** | https://docs.ekubo.org/integration-guides/aggregators: агрегаторы считают по журналам Core и расширений, при сомнениях перечитывают из цепи |
| DODO | PMM | **да** (`dodo_math.rs`, `swap_dodo.rs`) | адреса на Ethereum не искал: **не найдено** |
| PancakeSwap на Ethereum | V2/V3 (Infinity на Ethereum в блоге только «scheduled»: https://blog.pancakeswap.finance/articles/introducing-pancake-swap-for-developers-traders-and-every-user) | V2/V3 — **да** | адреса Ethereum: **не найдено** |
| Sushi V2/V3 | форки Uniswap | **да** (вид «форк V2/V3») | **не найдено** |
| Maverick V2 | бины | **да** (`maverick_*`) | объём на Ethereum ничтожный |
| Native, Tokenlon, Metric | RFQ / маркет-мейкеры вне цепи | **нет** и не подходит: котировка у их API, не из пула | — |

**Расчёт** по таблице 4.1: виды, которые у нас уже есть (Uniswap, DODO, PancakeSwap, Sushi, Maverick, часть
Curve-stable), закрывают примерно 61–70% объёма за 30 дней. Неточность из-за того, что у DefiLlama Curve одной
строкой, без деления stable и crypto. Fluid, Curve-crypto, Balancer и Ekubo — новая математика.

Справочник пулов: бот берёт список у GeckoTerminal/CoinGecko (`directory/gt_http.rs`, настройка `index.network`).
Ethereum у этого индексатора есть, точный id сети я не проверял.

---

## 5. Агрегаторы

| Агрегатор | Пределы (официально) | Цена |
| --- | --- | --- |
| 1inch (https://business.1inch.com/portal/documentation/payment-methods/subscriptions) | Free: 100 000 вызовов в месяц, 1 зап/с. Startup: 1 млн, 10/с. Pro: 3 млн, 20/с. Business: 7 млн, 40/с | на странице нет, отсылка на business.1inch.com/pricing |
| 0x (https://docs.0x.org/docs/developer-resources/rate-limits.md) | Free: «approximately 5 Requests Per Second» на все методы вместе | на странице нет |
| KyberSwap (https://docs.kyberswap.com/developer-guide/start-here/rate-limits-and-client-id) | публично 3 зап/с на Aggregator API; больше — с `X-Api-Key` | на странице нет |
| Odos (https://docs.odos.xyz/build/api_pricing) | «600 requests per 5 minutes (Per IP Address)» | платных планов пока нет (бета) |
| Velora / ParaSwap | страница лимитов не открылась | **не найдено** |
| CoW Protocol (https://docs.cow.fi/cow-protocol/concepts/introduction/solvers) | не мгновенный обмен: подписанное намерение → пакетный аукцион → решатель | — |

Задержка ответа ни у одного агрегатора официально не указана: **не найдено**.

Где агрегатор выгоднее своего поиска, а где нет (выводы из фактов выше):
- **Свой поиск выгоднее** для сигнала и котировки на каждый блок. У агрегаторов 1–5 запросов/с на бесплатных
  планах, а нам нужно котировать сотни монет. Плюс CLAUDE.md: «цены только со своего узла». Плюс по памяти Тиграна:
  KyberSwap в боте выключен, план — свои свопы.
- **Агрегатор полезен** как сверка или «второе мнение» (у нас уже есть `best_of/`). Ещё он полезен для DEX, чьей
  математики у нас нет (Fluid, Balancer, Ekubo, Curve-crypto, RFQ Native): агрегатор их видит, мы — нет.
- **CoW не подходит** для арбитража: нет атомарности под нашим контролем и нет мгновенного исполнения.

---

## 6. Токены: налоги, ребейз, блэклисты
- Dedaub, 09.09.2025, выборка из 100 новых токенов Ethereum за 24 часа
  (https://dedaub.com/blog/erc-20-on-ethereum-growth-risks-and-hidden-traps-in-2025/): налог у «nearly 16%» (в другом
  месте статьи — 26%, цифры в статье не сведены), блэклист/вайтлист ≈ 8%, ограничение торговли 3%, mint/burn 30%.
  Это только новые токены, на монеты бирж переносить нельзя.
- Доля ребейз-токенов и общая доля налоговых среди монет, которые торгуются на биржах: **не найдено**.
- У USDT на Ethereum есть блэклист, это известно из контракта Tether. Официальную ссылку я не открывал: **не
  проверено**.
- Для бота: проба налогов через `eth_simulateV1` (`check/`) переносится как есть, если у провайдера есть
  `eth_simulateV1` (п. 3.3).

---

## 7. Биржи: ввод и вывод по Ethereum (ERC20)

Главный источник — ответы API самих бирж, которые записаны в репозитории (правило CLAUDE.md «данные площадок —
только из API»):

| Биржа | Монета | Вывод | Мин. вывод | Подтверждений | Источник |
| --- | --- | --- | --- | --- | --- |
| BingX | USDT ERC20 | 1 USDT | 5 | 64 | `tests/data/bingx_2026-10-24/config_getall.json` (GET /openApi/wallets/v1/capital/config/getall, 24.10.2026) |
| KuCoin | USDT ERC20 | 5,5 USDT | 30 | 64 (`preConfirms` 12) | `tests/data/cex_samples_2026_11_04/kucoin_currencies_v3.json` (04.11.2026) |
| KuCoin | USDC ERC20 | 5,5 USDC | 1 | 64 (12) | там же |
| KuCoin | ETH | 0,0015 ETH | 0,003 | 64 (12) | там же |
| Gate | USDT, сеть «ETH» | в записи нет | — | в записи нет | `tests/data/cex_samples_2026_11_04/gate_spot_currencies.json`: сеть есть, ввод и вывод открыты |
| MEXC, Binance, Bybit, Bitget, OKX, XT, HTX, CoinEx, Kraken, Phemex, Poloniex | — | **не найдено** | — | **не найдено** | официальные страницы комиссий собираются скриптом и через WebFetch не читаются. У Kraken страница подтверждений открылась (обновлена 22.10.2026), но таблица не вывелась |

**Расчёт**: 64 подтверждения × 12 с ≈ 12,8 мин до зачисления на BingX и KuCoin. Столько монета висит «в пути»,
и цена за это время может уйти. По памяти Тиграна, на BSC перевод кошелёк → BingX зачисляется «почти моментально».

Что переиспользуется: имя сети у биржи — настройка `venue.bsc_network` (`docs/SETTINGS.md`, стр. 169), комиссия и
подтверждения адаптеры бирж читают из API. На отдельном сервере под Ethereum достаточно задать `ERC20` / `ETH` и т.п.
по каждой бирже. Но имена настроек и модулей содержат «bsc», это вопрос к структуре (п. 8).

---

## 8. Вывод

### 8.1 Есть ли смысл
- **DEX ↔ DEX (атомарный цикл): да, смысл есть.** Газ сейчас копеечный, бандлы с «всё или ничего» есть у трёх и
  больше билдеров, и наш исполнитель и бандл ложатся на это напрямую. Минусы: блок раз в 12 с (сигналов меньше),
  самая сильная конкуренция MEV в индустрии, слепой аукцион ставок. Сколько прибыли реально остаётся после чаевых
  билдеру, по фактам не установлено (**не найдено**).
- **Биржа ↔ DEX: под большим вопросом.** Вывод дороже (1–5,5 USDT против BSC) и около 12,8 мин до зачисления на
  биржу (64 подтверждения). Риск движения цены за перевод большой, и минимальная выгодная сделка растёт до сотен
  долларов (п. 2.3).
- Если выбирать «следующую сеть», Ethereum по скорости зачисления на биржи хуже BSC. Сравнение с Arbitrum (его
  Тигран называл следующим) нужно делать отдельным разбором.

### 8.2 Что берём из bsc_dex как есть
- Математика: V2, V3 (ABI Uniswap), V4 + крючки, DODO, Maverick V2, StableSwap (`math/`).
- Связь с узлом: JSON-RPC, пачки, Multicall3 (адрес Multicall3 на Ethereum задать настройкой; одинаков ли он, я не
  проверял), WebSocket `logs`, лента квитанций с реоргами, резервный узел, темп запросов.
- Проверки: проба налогов и «кто меняет» через `eth_simulateV1`, сверка с квотером, виртуальный квотер подменой
  кода (если у провайдера есть `eth_simulateV1` и state override в `eth_call`).
- Исполнение: Universal Router + Permit2 (у Uniswap V4 на Ethereum они есть), ArbExecutor (развернуть заново в сети Ethereum;
  приём нативной монеты в контракте рассчитан на BNB/WBNB (`contracts/README.md`) — под ETH/WETH проверить), бандлы цикла, трекер nonce и отмен.
- Справочник пулов от индексатора (другая сеть в `index.network`).

### 8.3 Что писать заново или менять
| Часть | Что | Сложность |
| --- | --- | --- |
| Транзакции | тип 2 (EIP-1559) вместо legacy; цена = base fee следующего блока + чаевые; котировка газа и отмена (`send/cancel.rs`, «газ выше на %») — по правилам EIP-1559 | средняя |
| Каналы отправки | Flashbots Relay / BuilderNet / Titan: подпись `X-Flashbots-Signature` (EIP-191), `eth_sendBundle` с `blockNumber`, `replacementUuid`/`eth_cancelBundle`, рассылка `builders`; одиночная — Protect/BuilderNet. Каналы BSC (48 Club, BlockRazor, NodeReal…) не нужны | средняя |
| Ставка билдеру | правило размера чаевых или coinbase-перевода в бандле (у нас «чаевые канала» для BSC) | средняя |
| Новые виды пулов | Fluid, Curve Twocrypto/Tricrypto-NG, Balancer V2/V3, Ekubo — каждый своя математика, события, исполнение в ArbExecutor | большая (каждый — отдельная фича) |
| Сеть как настройка | chainId 56 записан в `node/mod.rs`, имена `bsc.*` / `bsc_dex` по всему коду, нативная монета BNB/WBNB. При «одна сеть — один сервер» нужен либо параметр сети, либо копия адаптера. Это решение Тиграна | средняя–большая |
| Узел | выбрать провайдера с `eth_simulateV1` на Ethereum (по фактам — Infura, Chainstack) и замерить задержку с сервера | малая |
| Биржи | сеть ERC20 по каждой бирже через настройку `venue.bsc_network` + проверка по API, что подтверждения и комиссии читаются | малая |

### 8.4 Главные риски
1. Биржа ↔ DEX: 64 подтверждения (≈12,8 мин) — монета долго «в пути» (п. 7).
2. Конкуренция MEV и слепой аукцион: бандл могут перебить, и каждый бандл действует только на один блок (п. 1.3).
3. Газ сейчас 0,04 gwei, но base fee может вырасти на 12,5% за блок. Порог прибыли должен считаться по живой цене
   газа (п. 2.1).
4. Около трети объёма приходится на DEX, которые мы не видим (Fluid, Curve-crypto, Balancer, Ekubo, RFQ) (п. 4.2).
5. У двух из пяти проверенных провайдеров `eth_simulateV1` на Ethereum подтверждён (Infura, Chainstack), у остальных
   **не найдено**. Без него проверки монет не работают.

---

## Что не удалось проверить (открыть страницу или найти факт)
- Доли билдеров (relayscan), частота реоргов, лимит газа блока сейчас.
- `eth_simulateV1` для Ethereum у Alchemy, QuickNode, dRPC: страницы закрыты для загрузки или не найдены.
- Адреса Uniswap V3, Curve, Balancer V3, Fluid, Ekubo, DODO, Sushi, PancakeSwap на Ethereum.
- Комиссии вывода и подтверждения по ERC20 у MEXC, Binance, Bybit, Bitget, OKX, XT, HTX, CoinEx, Kraken, Phemex,
  Poloniex. Правильный путь — живые ответы их API (адаптеры уже умеют их читать).
- Velora (ParaSwap): лимиты API.
