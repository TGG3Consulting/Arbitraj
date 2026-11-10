# Несколько сетей: «один экземпляр бота — одна сеть» — разбор кода (10.11.2026)

> Задача Тиграна 10.11: «взять нашего бота, отрезать часть BSC, добавить новые сети — Solana, Ethereum, Arbitrum и
> другие — с открытым или платным узлом (не своим). Один бот поддерживает все сети, но включаем так: один экземпляр —
> одна сеть — свой сервер — свои кошельки».
>
> Что это: только разбор кода (репозиторий не менялся, не собирался). Каждое утверждение — `файл:строка`.
> **[допущение]** — не проверено по коду или официальной документации; проверить до работы.
> Цифры «сколько файлов» — `grep` по `src/` на 10.11.2026.

---

## 0. Главное коротко

1. **Ядро, шлюз и биржи почти не знают о BSC в логике.** Сеть вывода/ввода биржи уже настройка
   (`venue.bsc_network`, `settings/catalog/base.rs:111`), адреса всех фабрик/роутеров/квотеров/Multicall3/WBNB —
   настройки (`settings/catalog/dex.rs:74-160`), адрес узла и ключ провайдера — настройки и секреты
   (`dex.rs:53-72`, `secrets/mod.rs:188-205`). BSC «зашита» в основном **названиями** (bsc_contract, *_bnb, «BSC» в
   текстах) и в нескольких **константах**: chainId 56, тикер газа «BNB», id площадки «bsc_dex», legacy-транзакция.
2. **Для второй EVM-сети (Arbitrum, Ethereum) ядро и биржи менять почти не нужно** — нужен «профиль сети» в
   настройках (chainId, монета газа, вид транзакции) и замена ~10 констант. Математика пулов, граф, пробы
   eth_simulateV1, исполнитель ArbExecutor, справочник пулов — переиспользуются; BSC-специфичны каналы отправки
   (14 шт.), часть видов DEX и метка нативной монеты StableSwap.
3. **Solana — это не «ещё одна сеть», а новая платформа.** Адрес монеты в ядре — `alloy_primitives::Address`
   (20 байт, `core/types/market.rs:29`), в Solana mint — 32 байта base58: сквозная смена типа (193 строки
   `bsc_contract` в 139 файлах src + 191 строка в tests, 29 файлов ядра импортируют `alloy_primitives`) + целиком
   новый адаптер `solana_dex` (узел, пулы, математика, транзакции, кошелёк ed25519).
4. **Чужой узел — главный риск по деньгам и лимитам**, не по коду: бот уже умеет `{key}` в адресе, резервный узел,
   общий темп `bsc.node_http_rps`, источник событий «квитанции блоков». Но базовая нагрузка без торговли ≈ 1,8–2,2 млн
   вызовов в сутки на BSC (раздел 4), а `eth_getCode` по старым блокам и `eth_simulateV1` у провайдера могут быть
   недоступны или платными.

---

## 1. Где в коде зашита сеть BSC

Легенда «Как сделать параметром»: **П** — профиль сети экземпляра (новые настройки `chain.*`, см. раздел 3),
**Н** — уже настройка, менять только подпись, **И** — только имя (поведение не меняется), **С** — смена типа (Solana).

### 1.1 Ядро (core/)

| Место | Что зашито | Как сделать параметром |
| --- | --- | --- |
| `core/types/market.rs:18-29` `Asset.bsc_contract: Option<Address>` | монета = адрес в BSC, тип EVM (20 байт); равенство и хэш по нему (`:33-57`, решение 211) | EVM: **И** — имя поля в Rust `contract` с `#[serde(rename = "bsc_contract")]` (JSON/SQLite/UI не меняются). Solana: **С** — тип адреса монеты сети (раздел 3.5) |
| `core/types/transfer.rs:20-21` `CoinNetworkInfo.network: String` | — (имя сети как у площадки) | уже общее |
| `core/convert.rs:18-20` `GAS_TICKER = "BNB"`, `gas_asset()` `:22-28`, `bnb_price()` `:142-145` | монета газа сети — BNB; цена газа в USDT — стакан BNB-USDT биржи | **П** `chain.gas_ticker` (BNB / ETH / SOL). Используется в `core/{precheck.rs:105,135, native.rs:114,125, balances_cache.rs:154, convert_at.rs:41, sell.rs:242, reconcile.rs:173, cycle_deal.rs:136, deal_gas.rs:102}`, `api/wallets_dex.rs:79` — все через константу, менять одно место на чтение настройки |
| `core/registry.rs:22` `Exclusion::NoBscNetwork`, `core/presence.rs:38`, `core/start_reason.rs:108` («нет сети BSC») | причина отсева названа BSC | **И** — текст из `chain.title`; имя варианта в JSON оставить (UI `ui/src/lib/coinLabels.js:70`, `coinFilter.js:42` его читают) |
| `core/transfers.rs:47-92` | сеть перевода: у биржи — из её справочника, у DEX — `venue.bsc_network`; текст «нет сети BSC» | **Н** (логика общая) + **И** текст |
| `core/native.rs:60-125, 174` | нативная монета сети ↔ обёртка по `dex.native_ticker` + `dex.native_wrapped` + `venue.bsc_network` (решение 62) | **Н** — уже общее: на Arbitrum/Ethereum те же настройки = ETH ↔ WETH |
| `core/rebuild.rs:45, 97`, `core/rebuild_build.rs:28`, `core/usdt_transfer_plan.rs:71`, `core/registry_sync.rs:87` | читают `venue.bsc_network` | **Н** (ключ можно оставить, раздел 3.4) |
| `core/deal_card.rs:25` `const DEX: &str = "bsc_dex"` | id площадки DEX зашит | **П** — брать площадку вида `VenueKind::Dex` из реестра (как уже делают 48 мест ядра/api через `kind()`) |
| поля `*_bnb`: `core/types/dex_cycle.rs:62-68`, `cycle_trade.rs:29,37,70`, `send.rs:87,121-144`, `control.rs:26-28`, `executor.rs:29`; `gateway/errors.rs:117` `approve_gas_bnb`; `core/deal_gas.rs:38-174` | «в BNB» в именах полей и текстах | **И** — смысл «в монете газа сети»; имена JSON не трогать (их хранит SQLite и читает UI), тексты «BNB» — из `chain.gas_ticker` |
| `settings/catalog/base.rs:21` `risk.min_bnb_balance` | резерв газа назван BNB | **И** (ключ оставить, подпись/единица — из `chain.gas_ticker`) |

### 1.2 Шлюз (gateway/)

| Место | Что зашито | Как |
| --- | --- | --- |
| `gateway/contract.rs` (59 функций) | сигнатуры общие (`network: &str`, `Asset`); BSC/BNB только в комментариях (`:59, 306, 347, 503, 571`) | без изменений для EVM |
| `gateway/events.rs:37, 81` `PoolsLoading { coins: Vec<Address> }` | адрес монеты EVM | Solana: **С** |
| `gateway/network_hint.rs:21-51`, `settings/hint.rs:11` | подсказка «venue.bsc_network не задана… имя сети BSC» | **И** |

### 1.3 Адаптеры бирж (venues/<cex>)

Выбор сети вывода/ввода и разбор адреса контракта **уже параметр** `venue.bsc_network` у каждой биржи. Пример BingX:
`venues/bingx/wallet.rs:28, 45` читает настройку, `:60-85` — адрес контракта берётся только у сети с этим именем
(`network == bsc`), разбор `a.parse().ok()` в `Address`. Так же MEXC (`mexc/wallet.rs:106`, `networks_parse.rs:92`),
Gate (`gate/wallet.rs:195`, `deposit_info.rs:41`), HTX (`htx/wallet.rs:96` + `venue.contract_field`), KuCoin
(`kucoin/withdraw_quota.rs:9` — chainId сети), OKX/CoinEx (описание `catalog/base.rs:111`).

- **EVM-сеть:** кода не надо. В экземпляре Arbitrum Тигран задаёт `venue.bsc_network [bingx]` = имя сети Arbitrum у
  BingX — по живому ответу справочника (кнопка «Проверить подключение» уже показывает имена сетей, решение 200:
  `api/check.rs:79-90`). **[допущение]** у каждой биржи один и тот же адрес токена в сети Arbitrum, что в пулах —
  сверка ядра по адресу это и так проверит.
- **Solana:** `parse()` в `alloy Address` для base58 mint вернёт ошибку → `None` → все монеты «адреса контракта нет».
  Нужен разбор адреса по виду сети в каждом адаптере (~15 файлов `wallet.rs`/`networks_parse.rs`, по 1–3 строки),
  плюс у бирж Solana — свои поля (memo/tag обычно не нужны **[допущение — проверить по документации каждой биржи]**).
- Тексты «BSC»: `notify/ctl/transfer.rs:127` (строка «BSC» в сообщении), `notify/human.rs:50` (`"bsc_dex" => "DEX"`).

### 1.4 Адаптер DEX `venues/bsc_dex` (428 файлов, 56 486 строк)

| Место | Что зашито | Как |
| --- | --- | --- |
| `bsc_dex/node/mod.rs:43` `BSC_CHAIN_ID = 56` → `send/tx.rs:24, 128` (chainId подписи), `connection.rs:202-206`, `connection_extra.rs:133-137` (проверка узла) | chainId | **П** `chain.chain_id` |
| `bsc_dex/send/tx.rs:110-135` `TxLegacy` + шапка `:11-12` «база газа в BSC всегда 0» | только legacy-транзакция, цена газа = `eth_gasPrice` (`quote/gas.rs:124`) | **П** `chain.tx_type` (legacy / eip1559). Ethereum: базовая плата ≠ 0, legacy принимается, но цена/включение хуже **[допущение — проверить по execution-apis/EIP-1559]**; Arbitrum: L1-часть газа **[допущение — проверить по docs.arbitrum.io, как она входит в gasUsed квитанции]** |
| `bsc_dex/ctx.rs:26` `ID = "bsc_dex"`, `:29` `SECRET_WALLET_KEY = "bsc_dex.wallet_key"` | id площадки (SQLite: область `venue:bsc_dex`, `venue_state`, очередь кошелька, сырые логи) | **П** id в контексте (`ctx.id`), для BSC — ровно «bsc_dex». Константа `ID` используется в 12 файлах (`errors.rs:159`, `ctx.rs:55,61,201`, `settings_wait.rs:48`, `node/ws_task.rs:139,184`, `directory/{store,gt_http}.rs`, `pools/{history_store,deploy_block}.rs`) |
| `bsc_dex/wallet/balances.rs:30` `BNB = "BNB"` → `exec/result.rs:22`, `wallet/transfer{,_cost,_status}.rs`, `resolve_withdraw.rs:38`, `best_of/extra.rs:16`, `quote/{mod,pick,native_gas,steps}.rs` | тикер газа | **П** `chain.gas_ticker` |
| `bsc_dex/pools/stable_native.rs:26-31` `0xEeee…`, 18 знаков | метка нативной монеты пулов PancakeSwap Stable / Lista (константа их контрактов) | не сеть, а вид DEX: остаётся, работает только если такие DEX включены |
| `bsc_dex/directory/mod.rs:146` «нативный BNB (адрес 0x0)» | текст | **И** |
| `bsc_dex/send/channels.rs:14-29` (14 каналов) | все каналы — BSC-билдеры/релеи (48 Club, BlockRazor, Blink, JetBuilder, NodeReal, bloXroute, MEV Guard, Flashblock) + `public` | **П** поле `chains` у `Channel` (`send/channel.rs`): канал виден/включаем только в своей сети. `public` (eth_sendRawTransaction по `channel.url`) — общий |
| `bsc_dex/send/channel.rs` `Auth::NodeKey` | ключ = `bsc.node_http_key` | **Н** |
| `bsc_dex/pools/families.rs:182-265` — виды DEX (`pancakeswap_*`, `uniswap_v2/v3/v4`, `v2_fork`, `v3_fork`, `thena_fusion`, `algebra_integral`, `solidly_*`, `curve_stableswap`, `lista_stableswap`, `woofi_v2`, `slipstream`, `maverick_v2`…), `:374-425` — какие адреса нужны каждому | только перечень видов; адреса — настройки области «DEX» | **Н**. Для новой сети — какие виды там есть и по каким адресам: по официальной документации DEX этой сети |
| `bsc_dex/node/heads.rs:2-11`, `node/rpc.rs:31` «узел BSC (HTTP)», `node/ws.rs:32` | тексты, решения 57/68 обоснованы ценами NodeReal | **И** |
| `venues/kyberswap/node.rs:29`, `connection.rs:83-85`, `signer.rs:71` `BSC_CHAIN_ID = 56`; `kyber.chain` (`catalog/kyber.rs:21`) — уже настройка | chainId | **П** (KyberSwap выключен, только для тестов — низкий приоритет) |
| `contracts/ArbExecutor.sol:16` `pragma 0.8.37`, `:72` `transient`, `:35-38` `STABLE_U256_BNB`, `NATIVE` | transient storage (EIP-1153), нативный BNB в стейблсвапах | контракт EVM-общий; сеть должна поддерживать EIP-1153 и опкоды версии компилятора **[допущение — проверить по документации Arbitrum/Ethereum]**. Развёртывается заново в каждой сети (адрес — `exec.executor_address` этого экземпляра) |

### 1.5 Настройки (settings/)

| Место | Что зашито | Как |
| --- | --- | --- |
| `catalog/dex.rs:53-72` — 20 ключей `bsc.*`, область «вся система», категория «Узел BSC» | префикс и подпись | ключи **оставить** (раздел 3.4), категория — «Узел сети» |
| `catalog/base.rs:111` `venue.bsc_network` | имя ключа | ключ оставить, подпись «Как площадка называет сеть экземпляра» |
| `catalog/send.rs:33` `channel.tip_bnb` (единица «BNB») | единица | подпись из `chain.gas_ticker` |
| `catalog/index.rs:18` `index.network` («BSC — bsc») | уже настройка (id сети у GeckoTerminal) | **Н** |
| `settings/value.rs:87-110` `Kind::Address` / `AddressList` — только `0x` + 40 hex, текст «не адрес сети BSC» | формат адреса EVM | EVM: **И**. Solana: **С** — проверка по виду сети |
| `settings/scope.rs:98-100` `is_address_id` (область «монета» = 0x-адрес) | формат | Solana: **С** |
| `secrets/mod.rs:188-205` `bsc.node_*_key` | имена секретов | оставить |

### 1.6 Интерфейс (ui/src)

Id площадки DEX в UI не зашит (поиск `bsc_dex` по `ui/src` — пусто): экраны берут площадки и их вид из API.
Зашиты подписи и формат адреса:

- «BNB»: `pages/GasStats.svelte:27-28`, `components/BalancesPanel.svelte:217`, `WalletRow.svelte:84`,
  `ExecutorDeploy.svelte:76`, `DexCycles.svelte:93-96`, `UsdtTransfer.svelte:72, 111`, `CostLines.svelte:4`.
- «BSC»: `lib/labels.js:21-22`, `components/SettingEditor.svelte:67-70`, `CoinModal.svelte:25`, `LiveCoinModal.svelte:29`,
  `DexDrySwap.svelte:74`, `lib/coinLabels.js:70-74`, `lib/coinFilter.js:18-19`.
- Формат 0x: `lib/coinNames.svelte.js:78`, `SettingEditor.svelte:67-70`; поле `bsc_contract` читают 12 файлов UI.
- Как: один запрос «профиль сети экземпляра» (`GET /api/chain`: title, gas_ticker, вид адреса) и подстановка в
  подписи — ~15 файлов, по 1–3 строки; JSON-поля (`bsc_contract`, `*_bnb`) не трогать.

### 1.7 Учёт и хранение (SQLite)

- `storage/windows_db.rs:103` — `json_extract(body, '$.asset.bsc_contract')`: имя поля внутри сохранённого JSON.
- Сделки, окна, «без потолка», состояние ядра хранятся JSON-ом (`storage/db.rs:16-119`) с полями `bsc_contract`,
  `*_bnb` — поэтому имена JSON менять нельзя без миграции.
- Таблицы кошельков `dex_wallets`, `dex_wallet_map` (`storage/db.rs:97-112`) — без столбца сети: при «одна сеть на
  экземпляр» он не нужен (своя база у каждого сервера).
- Факт из старой базы Windows (`/mnt/user-data/uploads/arb-windows/data/arb.sqlite`, 24.10): область настроек
  хранится строкой `venue:bsc_dex`, `bsc.node_http_url` = адрес NodeReal с `{key}`, `bsc.node_http_rps` = 2 —
  т. е. бот уже работал на платном узле, и смена id «bsc_dex» сломала бы все настройки DEX.

---

## 2. Что общее для любой EVM-сети, а что BSC-специфично

### 2.1 Переиспользуется почти без изменений (EVM)

| Часть | Файлы | Почему общее (факт кода) |
| --- | --- | --- |
| Математика пулов | `bsc_dex/math/*` (27 файлов, 3 997 строк): V2, V3/CL (`tick_math`, `sqrt_price_math`, `swap_cl`), Algebra, Solidly, DODO, Curve/StableSwap, Maverick, Bin | формулы контрактов (Uniswap/Pancake/Algebra…), от сети не зависят; константы — протокольные (`tick_math.rs:15-17`) |
| Пулы: поиск, загрузка, события, проверка | `pools/*` (117 файлов) | адреса фабрик/менеджеров/квотеров — настройки области «DEX» (`families.rs:374-425`); Multicall3 — `dex.multicall3` |
| Граф и маршрутизатор | `quote/graph_*`, `route_*` (решение 80) | работает по реестру пулов в ОЗУ |
| Циклы DEX ↔ DEX | `quote/cycle*`, `exec/cycle_*` (решения 85, 124) | граф + исполнитель/бандл |
| Пробы налогов и крючков, сухая сделка, предпродажа | `check/simulate.rs`, `exec/dry_*`, `exec/presell.rs` — `eth_simulateV1` | метод execution-apis; **поддержка у провайдера/клиента сети — проверять (раздел 4)** |
| Исполнитель ArbExecutor | `contracts/ArbExecutor.sol`, `exec/executor/*` | EVM-общий; адрес — `exec.executor_address`, сверка кода `exec/executor/onchain.rs` |
| Universal Router / Permit2 | `exec/ur*.rs`, `swap_v4.rs`, `smart.*` | адреса — настройки (`dex.universal_router`, `smart.universal_router`, `dex.permit2`) |
| Справочник пулов (GeckoTerminal) | `directory/*` | сеть — `index.network`; id DEX — из ответа индексатора |
| Кошелёк, nonce, очередь, approve | `wallet/*` | ключ secp256k1, ERC-20, Transfer-логи — общие для EVM |
| Узел: очередь, темп, резервный узел, квитанции блоков | `node/*` | JSON-RPC execution-apis; ключ `{key}` в адресе (`ctx_url.rs`) |
| Ядро, шлюз, биржи | `core/*`, `gateway/*`, `venues/<cex>` | через `venue.bsc_network` и адрес контракта |

### 2.2 BSC-специфично (в другой сети — выключить или заменить настройками)

- **Каналы отправки** (`send/*`, 14 каналов): 48 Club (`club48.rs`, чаевые на EOA из документации 48 Club),
  BlockRazor, Blink, JetBuilder, NodeReal, bloXroute (`blxr_submit_bundle` с `blockchain_network`), MEV Guard,
  Flashblock — всё это BSC. Формат бандла `Bundle::Standard` = `eth_sendBundle {txs, maxBlockNumber}`
  (`send/channel.rs`). Ethereum — свои билдеры/релеи (у Flashbots формат с `blockNumber` **[допущение — проверить
  по docs.flashbots.net]**) → новые файлы каналов. Arbitrum — секвенсор, бандлов билдеров нет **[допущение —
  проверить по docs.arbitrum.io]** → DEX ↔ DEX там только одной транзакцией через ArbExecutor, без «бандла».
- **«BNB-48»/чаевые в BNB** — `channel.tip_bnb`, `channel.tip_address` — уже настройки канала, привязаны к BSC-каналам.
- **Виды DEX, которых нет вне BSC** — PancakeSwap Infinity CL/Bin/Stable, THENA (`thena_fusion`, `solidly_v1`),
  Lista, Nerve (Saddle), WooFi-версия, Ellipsis… — код вида общий, но в другой сети не включаются (нет адресов).
  Есть ли на Arbitrum/Ethereum Uniswap V2/V3/V4, Curve, Algebra-форки (Camelot и т. п.) с теми же интерфейсами —
  **[допущение — проверить по официальной документации каждого DEX]**.
- **Метка 0xEeee… пулов Stable** (`pools/stable_native.rs:26`, `ArbExecutor.sol:38`) — свойство DEX PancakeSwap/Lista.
- **Справочник пулов**: сам индексатор общий; `index.network` для Arbitrum/Ethereum — по `GET /networks`
  (`catalog/index.rs:18`) **[допущение: id сетей у GeckoTerminal — проверить живым ответом]**.
- **Адреса фабрик/роутеров/квотеров/Multicall3/WBNB** — уже в настройках (`catalog/dex.rs:74-160`), в коде зашитых
  адресов сети нет (поиск литералов `0x…40` в src — только тесты и протокольная метка 0xEeee).
- **Решения 57/68** (опрос блока, лента квитанций) обоснованы ценой NodeReal для BSC — механизмы общие, выбор
  источника — по тарифу провайдера новой сети.

---

## 3. Архитектура «одна сеть на экземпляр»

### 3.1 Профиль сети экземпляра (новые настройки, область «вся система»)

| Ключ | Пример BSC | Где читается вместо константы |
| --- | --- | --- |
| `chain.platform` | `evm` | `main.rs:257-292` — какой адаптер DEX создать (`evm` → нынешний `bsc_dex`, `solana` → `solana_dex`) |
| `chain.id` | `bsc` | id площадки DEX = `bsc_dex` (для BSC — ровно как сейчас), `<id>_dex` для остальных; тексты |
| `chain.title` | `BSC` | тексты ядра, уведомлений, UI («нет сети BSC») |
| `chain.chain_id` | `56` | `send/tx.rs:128`, `connection.rs:202`, `connection_extra.rs:133`, `kyberswap/*` |
| `chain.gas_ticker` | `BNB` | `core/convert.rs:20`, `bsc_dex/wallet/balances.rs:30`, `kyberswap/balances.rs:23`, UI |
| `chain.tx_type` | `legacy` | `send/tx.rs:110-135` (для BSC — как сейчас; `eip1559` — новый путь сборки) |

- Смена профиля — только с перезапуском (адаптер создаётся в `main.rs`). Это исключение из «меняется на лету» —
  **вопрос Тиграну** (вариант Б: `chain.platform` и `chain.id` — аргумент запуска `--chain`, `cli.rs`; остальное —
  настройки).
- «Проверить подключение» DEX сверяет `eth_chainId` узла с `chain.chain_id` (сейчас — с 56,
  `connection.rs:202-206`): чужой узел другой сети → «узел другой сети».
- Профиль не задан → DEX не запускается (правило «не задан параметр — функция не работает»).

### 3.2 Адаптер DEX: «evm_dex» = нынешний `bsc_dex`, параметризованный профилем

- **Не переименовывать папку и не переписывать** (предпочтение «только точечные правки»): `venues/bsc_dex` остаётся,
  меняются ~10 мест из таблицы 1.4 (chainId, тикер газа, id, вид транзакции, тексты). Переименование папки в
  `evm_dex` — косметика, отдельным коммитом в конце или никогда.
- `ctx.rs:26` `ID` → поле `ctx.id` (12 файлов). `SECRET_WALLET_KEY`, имена секретов `bsc.node_*_key` — оставить.
- `send/channel.rs` — поле `chains: &[&str]` у канала + отказ `channel.enabled` в чужой сети. Новые каналы Ethereum
  (Flashbots и др.) — новые файлы по их документации.
- Ethereum/Arbitrum: `chain.tx_type = eip1559` — `eth_feeHistory`/`eth_maxPriorityFeePerGas` (сейчас «не
  вызываются», `own-node-connect-audit-2026-11-03.md`, таблица 2) + `TxEip1559` в `send/tx.rs`, `wallet/signer.rs`.
- Arbitrum: учёт L1-части газа в затратах сделки (`quote/gas.rs`, `deal_gas.rs`) — по документации Arbitrum
  **[допущение: в Arbitrum L1-часть входит в gasUsed/estimateGas — проверить]**; у OP-сетей (Base, Optimism) она
  отдельно (поле квитанции) **[допущение — проверить]**.

### 3.3 Как биржи узнают сеть вывода/ввода

Уже сейчас: `venue.bsc_network` у каждой биржи (`catalog/base.rs:111`) — имя сети по справочнику этой биржи; ядро
сравнивает с ним `CoinNetworkInfo.network` (`core/transfers.rs:47-92`, `core/native.rs:97`), адаптер берёт адрес
контракта только у этой сети (`bingx/wallet.rs:76-80`). В экземпляре другой сети Тигран задаёт своё имя сети у
каждой биржи (подсказка решения 200 — «Проверить подключение» показывает список имён). **Кода не надо.**
Санкционные пары бирж (`venue.cex_cex_deny`, `catalog/trade.rs`) — те же настройки в каждом экземпляре.

### 3.4 Как не сломать живой BSC-бот (обратная совместимость)

1. **Id площадки DEX для BSC — «bsc_dex» навсегда**: на нём области настроек (`venue:bsc_dex` в SQLite), `venue_state`
   (`directory/store.rs`, `pools/history_store.rs`, `pools/deploy_block.rs`), очередь кошелька (`main.rs:279`),
   маппинг кошельков, сырые логи.
2. **Ключи настроек не переименовывать** (`bsc.*`, `venue.bsc_network`, `risk.min_bnb_balance`, `channel.tip_bnb`) —
   меняются только подписи и категории в каталоге. Переименование = миграция `settings` + `settings_history` +
   выгрузка настроек (`api/settings_io*.rs`) — выгоды нет.
3. **Имена полей JSON не менять** (`bsc_contract`, `*_bnb`): их хранит SQLite (`storage/windows_db.rs:103` читает
   `$.asset.bsc_contract`) и читает UI. В Rust можно переименовать поле с `#[serde(rename = "bsc_contract")]`.
4. **Профиль сети для уже работающей базы** — один раз записать значения BSC при первом запуске новой версии, если
   в базе есть настройки области `venue:bsc_dex`. Прецедент: перенос кошелька при решении 132
   (`main.rs:270-288`, `settings::accounts::migrate`). Без этого обновлённый бот на BSC остановится («профиль не
   задан»). **Вопрос Тиграну:** разрешить такой разовый перенос (это перенос существующего, а не значение по умолчанию).
5. **Поведение BSC не меняется**: legacy-транзакция, chainId 56, BNB — те же значения, только из профиля. Проверка —
   все тесты на записанных данных BSC (`tests/`, 271 файл) проходят без правок, кроме тех, что сами задают настройки.

### 3.5 Solana — отдельный адаптер `solana_dex` и сквозной тип адреса

- **Ядро:** адрес монеты → свой тип ядра (например `TokenAddr`), который у EVM сериализуется так же, как сейчас
  `alloy Address` (иначе не прочитаются старые записи; **[допущение: формат сериализации Address — проверить тестом
  на записанной сделке]**), у Solana — base58. Затрагивает: `core/types/market.rs:29`, `cost.rs:89-90`,
  `dex_cycle.rs:32-50`, `directory.rs:43-62`, `pool.rs`, `executor.rs`, `gateway/events.rs:37,81`, 29 файлов ядра с
  `alloy_primitives`, 193 строки `bsc_contract` в 139 файлах, `settings/value.rs:87-110`, `settings/scope.rs:98`,
  разбор адреса в ~15 адаптерах бирж, UI `coinNames.svelte.js:78`. Это самая большая и рискованная часть — делать
  после второй EVM-сети и только если Тигран подтвердит Solana.
- **Пулы в ядре** (`core/types/pool.rs`: V2/V3/CL/Bin/StableSwap… с EVM-адресами) используют 8 файлов ядра
  (`market_state.rs:79`, `market_read.rs`, `card*.rs`, `window_dex.rs`, `window_deal.rs`, `episode_snap.rs`,
  `touched.rs`). Вопрос дизайна: Solana-пулы (Raydium, Orca, Meteora…) отдавать ядру новыми видами `PoolKind` или
  держать внутри адаптера, а ядру — только котировки контракта. **Вопрос Тиграну/архитектору.**
- **Адаптер** — с нуля по официальной документации Solana и каждого DEX: RPC/WebSocket Solana (подписки на аккаунты
  пулов вместо логов), программы пулов и их математика, сборка и подпись транзакций (ed25519), приоритетная плата,
  атомарность (бандлы — по документации провайдера бандлов **[допущение]**), токены SPL/Token-2022 (налог перевода —
  аналог налога токена), кошелёк и маппинг кошельков. Контракт шлюза (`gateway/contract.rs`) подходит: EVM-функции
  (`dex_executor_*`, `dex_dry_swap_executor`) по умолчанию `NotApplicable`.

---

## 4. Открытый/платный узел вместо своего

### 4.1 Что сейчас опирается на свой узел (факты кода)

| Что | Где | Чем грозит у провайдера |
| --- | --- | --- |
| Темп HTTP `bsc.node_http_rps` (живой — 500, `docs/notes/calc-timeout-2026-11-03.md:6`) | `node/pace.rs`, `node/queue.rs` | лимит провайдера; ответ 429 — правило шлюза `venue.rate_limited` (`pace.rs:4-7`) |
| Опрос головы `eth_blockNumber` раз в `bsc.blocks_poll_ms`, **мимо очереди** | `node/heads.rs:2-6`, `rpc.rs:54-63` | 100 мс = 10 вызовов/с = 864 000/сутки только на голову |
| Прогрев на каждый блок: `eth_gasPrice` + `eth_getBlockByNumber` | `quote/warm.rs:3-4`, `heads.rs:9-10` | 2 вызова × блоков/с |
| Подписки `logs` по пулам, «новые пулы», Transfer кошелька | `node/ws_task.rs:118`, `pools/group.rs:82`, `pools/created.rs:100`, `wallet/events.rs:96` | у NodeReal WS считается по байтам; ~2850 пулов ≈ 2,4 млн CU/час, бесплатный лимит кончился (решение 68) |
| Лента квитанций (`bsc.pool_events_source = receipts`): 3 вызова на блок | `node/receipts_budget.rs:24`, `receipts_fetch.rs:48-50` | `eth_getBlockReceipts` 0,4–0,7 МБ на блок BSC (решение 68) — трафик и цена; метод есть не у всех **[допущение — проверить у провайдера]** |
| Перечитывание пулов без событий на каждом блоке (WooFi, Lista, Infinity Stable), лесенка WooFi | `pools/per_block.rs:27-30`, решение 122 F3 | Multicall3 `eth_call` × блоков/с |
| Загрузка пулов пачками Multicall3 `dex.load_batch` | `pools/load_batch*.rs`, `node/multicall.rs:94` | газ-лимит `eth_call` у провайдера меньше своего узла (у своего `RPCGasCap=100M`) **[допущение — проверить]** → уменьшить `dex.load_batch` |
| JSON-RPC пачки (batch) | `node/rpc.rs:68-104`, `receipts_fetch.rs` | провайдер считает каждый вызов (`pace.rs:3`); предел размера пачки — свой у провайдера **[проверить]** |
| `eth_simulateV1` (пробы налогов, предпродажа, сухие сделки, проверка подключения) | `check/simulate.rs:110`, `connection.rs:123` | поддержка метода и его цена у провайдера и клиента сети **[проверить]**; без него — нет проб налогов → «не торгуем вслепую» |
| `eth_call` на номере блока (не `latest`) | `load.rs:51`, `load_batch.rs:69`, `listen_log.rs:148` | у неархивного провайдера — только свежие блоки (у своего geth ~128, аудит узла) |
| `eth_getCode` по **старым** блокам (бинарный поиск блока развёртывания) | `pools/deploy_block.rs:55, 109` | нужен архивный доступ — у провайдеров обычно отдельный/платный тариф **[проверить]**; результат уже кэшируется в `venue_state` |
| `eth_getLogs` истории (Initialize V4) кусками `dex.logs_block_batch` | `pools/scan.rs:81`, при `dex.history_scan = да` | предел диапазона блоков у провайдера; у публичного узла BNB Chain `eth_getLogs` выключен (решение 68) |
| История «600000 блоков» | флаги своего geth (`own-node-connect-audit-2026-11-03.md:180, 284`) | боту нужна только для квитанций свежих блоков и своих транзакций — у провайдера обычно есть **[проверить]** |
| Квитанция своей транзакции — на каждую голову | `send/receipt.rs:49`, аудит узла (3) | 1 вызов × блоков/с на каждую транзакцию в полёте |
| Отправка: каналы `send/*`, `public` — `eth_sendRawTransaction` по `channel.url` | `send/ready.rs:130` | общая для EVM |

### 4.2 Оценка нагрузки (по коду; без торговли и фоновых проверок)

Формула на блок (HTTP): прогрев 2 + (receipts: 3) + перечитывание «пулов без событий» ≥1 (если включены) + квитанции
своих транзакций. Плюс опрос головы: 1000 / `bsc.blocks_poll_ms` в секунду.

| Сеть | Блоков/с | Опрос головы 100 мс | Прогрев 2/блок | Лента receipts 3/блок | Итого в сутки (receipts) |
| --- | --- | --- | --- | --- | --- |
| BSC | ≈2,2 (решение 68) | 864 000 | ≈380 000 | ≈570 000 | **≈1,8–2,0 млн** (+ перечитывание пулов) |
| Arbitrum | ≈4 **[допущение — проверить]** | 864 000 | ≈690 000 | ≈1 040 000 | ≈2,6 млн |
| Ethereum | ≈1/12 **[допущение]** | 864 000 | ≈14 400 | ≈21 600 | ≈0,9 млн (почти всё — опрос головы) |

Сверху: фоновые циклы (сверка с квотером `dex.quoter_check_s`, крючки `hooks.recheck_interval_s`, налоги
`tax.recheck_interval_s` — eth_simulateV1 по монетам), старт (загрузка всех пулов + первый проход проб налогов
~200 монет — аудит узла (3)), торговля. В режиме `ws` вместо ленты — подписки logs, цена по тарифу провайдера
(NodeReal: по байтам, решение 57).

### 4.3 Что ограничить / кэшировать

1. **Опрос головы** — главный постоянный расход: для провайдера — `bsc.blocks_poll_ms` под тариф; или вернуть
   подписку `newHeads`, если провайдер считает WS по сообщениям, а не по байтам (решение 57 выбрало опрос из-за
   цены NodeReal) — настройка-выбор, без смены текущего поведения BSC.
2. **Источник событий пулов** — `ws` или `receipts` под тариф (уже настройка, решение 68).
3. **`eth_getCode` по старым блокам** — не звать у неархивного провайдера: блок развёртывания уже хранится в
   `venue_state` (`deploy_block.rs:38`); нет архива — ясный отказ с причиной, а не бинарный поиск по ошибкам.
4. **`dex.load_batch`, `dex.pools_per_sub`, `dex.logs_block_batch`** — под пределы провайдера (уже настройки).
5. **Проверка «Проверить подключение» DEX** дополнить: поддерживает ли узел `eth_simulateV1`, `eth_getBlockReceipts`,
   `eth_feeHistory` (для eip1559) — точная причина, если нет.
6. **Счётчик вызовов в сутки** по методам в интерфейсе (сейчас есть только запись «нужно N вызовов/с»,
   `receipts_budget.rs`) — чтобы видеть расход тарифа.
7. Отставание узла: проверка возраста головы уже есть (`feed.max_block_age_ms`, `node/head_age.rs`, решение 121) —
   у провайдера обязательна.

---

## 5. План работ по этапам

### Этап 1. «Сеть — параметр», поведение BSC не меняется

| Шаг | Файлы | Объём |
| --- | --- | --- |
| Профиль сети `chain.*` в каталоге + разовая запись значений BSC в существующую базу | `settings/catalog/` (новый `chain.rs`), `main.rs`, `settings/accounts/migrate.rs` (образец) | ~3 файла |
| chainId из профиля | `bsc_dex/node/mod.rs:43`, `send/tx.rs:128`, `connection.rs:202`, `connection_extra.rs:133`, `kyberswap/{node,connection,signer}.rs` | 7 файлов, по 1–5 строк |
| Тикер газа из профиля | `core/convert.rs:20`, `bsc_dex/wallet/balances.rs:30`, `kyberswap/balances.rs:23` (+ вызывающие уже через константу) | 3 + ~15 вызывающих |
| Id площадки DEX из профиля (для BSC — «bsc_dex») | `bsc_dex/ctx.rs:26` + 12 файлов с `ID`, `main.rs:279-282`, `core/deal_card.rs:25`, `notify/human.rs:50` | ~16 файлов |
| Тексты «BSC/BNB» из профиля | ядро, уведомления (`notify/ctl/transfer.rs:127`), `settings/value.rs:96,110`, каталог (подписи) | ~20 файлов, только строки |
| `GET /api/chain` + подписи UI | `api/` (новый маршрут), ~15 файлов `ui/src` | |
| `Channel.chains` + отказ канала чужой сети | `send/channel.rs`, 14 файлов каналов | по 1 строке |
| Документы | `ARCHITECTURE.md`, `docs/DECISIONS.md`, `docs/SETTINGS.md`, `docs/FIRST_RUN.md`, `docs/UI.md` | пять частей готовой фичи |

Проверка: все тесты на записанных данных BSC без изменений; на живом BSC после обновления — тот же chainId, газ BNB,
id `bsc_dex`, все настройки на месте.

### Этап 2. Вторая EVM-сеть (Arbitrum — следующая по плану Тиграна, память проекта 10.11)

1. Сбор фактов по официальной документации: Arbitrum (chainId, газ и L1-часть, вид транзакции, время блока,
   отправка через секвенсор), DEX сети (виды, фабрики, роутеры, квотеры), провайдер узла (методы, лимиты, цены,
   `eth_simulateV1`, `eth_getBlockReceipts`, архив), имена сети Arbitrum у каждой биржи (живые справочники).
2. Код: EIP-1559 (`send/tx.rs`, `wallet/signer.rs`, `quote/gas.rs`) — если документация сети требует/советует;
   L1-газ в затратах — если не входит в `gasUsed`; каналы отправки сети (минимум `public`); виды DEX, которых нет в
   `families.rs`, — по их документации, как раньше (решения 122 F).
3. Развёртывание ArbExecutor в новой сети (кнопка уже есть, решение 122 X) после проверки поддержки EIP-1153.
4. Отдельный сервер, своя папка данных и база, свои кошельки (маппинг кошельков — в базе этого экземпляра),
   замер нагрузки при «Стоп» (решение 109) и расхода тарифа узла.
5. Объём кода (оценка): 5–10 файлов + новые каналы; основное время — факты и настройки.

### Этап 3. Solana

1. Подтверждение Тиграна и решение по пулам в ядре (раздел 3.5).
2. Сквозной тип адреса монеты (ядро, шлюз, настройки, адаптеры бирж, UI, тесты) — с совместимой сериализацией EVM.
3. Новый адаптер `venues/solana_dex` по шаблону DEX (`docs/ADAPTER_TEMPLATE.md`): узел, пулы и их математика,
   транзакции, кошелёк, налоги Token-2022, каналы отправки — каждый пункт по официальной документации.
4. Оценка объёма: сопоставима с нынешним `bsc_dex` по пулам/математике/исполнению (сейчас 428 файлов) — самый
   большой этап; сквозная смена типа — порядка 300 файлов src+tests с правкой 1–3 строк.

---

## 6. Вопросы Тиграну

1. Профиль сети: настройки с перезапуском или аргумент запуска `--chain` (раздел 3.1)?
2. Разовая запись профиля BSC в существующую базу при обновлении (раздел 3.4, п. 4) — разрешить?
3. Ключи настроек `bsc.*`, `venue.bsc_network` оставить как есть (меняются только подписи) — согласен?
4. Solana: делать ли вообще и как отдавать её пулы ядру (раздел 3.5)?
5. Провайдер узла для Arbitrum/Ethereum — какой (от него зависят источник событий, темп, архив, `eth_simulateV1`)?
