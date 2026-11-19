# Две сети, шаг 3: биржи по сетям — инвентарь по коду и план (решение 431, 19.11.2026)

Основа: ветка `t215/arb-step3-cex-nets` от `feat/arbitrum` `1bbe7864` (шаги 1 = 416 и 2 = 430 сделаны). План всего пути —
`docs/notes/arbitrum/multichain-plan-2026-11-19.md`, шаг 3: «Биржи: адрес контракта и имя сети по каждой сети; исправить
4 места, где сеть зашита настройкой». Главное условие: **BSC после шага работает точно как раньше**, существующие тесты —
без изменения смысла. Источник — только факт кода (номера строк — на `1bbe7864`); cargo локально не запускался.

## 1. Факты кода: где у бирж одна сеть

### 1.1 Адрес контракта — только у сети `venue.bsc_network` (условие `is_bsc` / `== bsc`)
| Биржа | Разбор (адрес и проверки сети) | Чтение настройки |
|---|---|---|
| Binance | networks_parse.rs:63 (code == bsc), :77-93 (denomination, contractAddress) | wallet.rs:71 |
| BingX | wallet.rs:76 (network == bsc) | wallet.rs:28, :45 |
| Bitget | networks_parse.rs:64-76 (журнал needTag/extra/congestion), :93-97 (адрес) | wallet.rs:62, :86 |
| Bybit | networks_parse.rs:50-52 (журнал процента), :69-72 (адрес) | wallet.rs:59, :78 |
| CoinEx | networks_parse.rs:88-89 (адрес из /assets/info), :107 (журнал); contracts.rs:41-60 (карта «тикер → адрес» ОДНОЙ цепи venue.bsc_contract_chain) | wallet.rs:81-82 |
| Gate | networks_parse.rs:92-100 (адрес addr, журнал процента) | wallet.rs:212 |
| HTX | networks_parse.rs:81-97 (bsc_count: несколько chain с тем же baseChainProtocol — сеть пропускается), :106 (журнал) | wallet.rs:96 |
| Kraken | networks_parse.rs:99, :112 (адрес) | wallet.rs:66 |
| KuCoin | networks_parse.rs:66-72 (журнал needTag/rate), :94-98 (адрес) | wallet.rs:57, :78 |
| MEXC | networks_parse.rs:77-84 (адрес, журнал шага) | wallet.rs:106 |
| OKX | networks_parse.rs:70-73 (журнал needTag), :93-97 (ctAddr) | wallet.rs:64, :95 |
| Phemex | networks_parse.rs:72 (chain == bsc), :91-97 (адрес) | wallet.rs:97 |
| Poloniex | networks_parse.rs:73-76 (журнал memo), :93-98 (адрес) | wallet.rs:67, :90 |
| XT | networks_parse.rs:99 (chain == bsc), :122-127 (адрес) | wallet.rs:69 |

Записи сети ядро получает с `chain: None` у всех сетей (литерал в каждом разборе).

### 1.2 Сеть зашита настройкой, а не из запроса
| Место | Что | Решение шага |
|---|---|---|
| src/venues/kucoin/fund_move.rs:49 | deposit_to_trading: адрес депозита — по venue.bsc_network | сеть — по Asset.chain запроса (имя сети биржи для сети монеты) |
| src/venues/okx/fund_move.rs:50 | то же у OKX | то же |
| src/venues/xt/deposits.rs:32 | deposit_status: история депозитов — chain = venue.bsc_network | то же |
| src/venues/kucoin/withdraw_quota.rs:38 | withdraw_hold(quote): квота вывода USDT по chain = venue.bsc_network | **не меняется**: в контракте withdraw_hold нет ни монеты, ни сети (стык, docs/AGENTS.md) — вопрос Тиграну |

Ещё по сети BSC (не «зашито вместо запроса», в шаге не меняется — TASKS): gate/deposit_plan.rs:44-61 (круг минимумов
депозита — только монеты с открытым вводом в BSC), gate/deposit_min.rs:156, kraken/account.rs:150 («Проверить
подключение» — вывод USDT по сети BSC).

### 1.3 Ядро (TASKS 430, пп. ж, к)
| Место | Что предполагает |
|---|---|
| src/core/presence.rs:27-60 `cex` | запись сети монеты без адреса ищется по `venue.bsc_network` (bsc), для любой группы сетей |
| src/core/rebuild.rs:89-99 | в справочник передаётся только `venue.bsc_network` бирж |
| src/core/rebuild.rs:105 | USDT сети по умолчанию — по ВСЕМ записям справочника (адрес USDT Arbitrum дал бы «разные адреса USDT») |
| src/core/rebuild_build.rs:76, registry_chains.rs:83 | USDT другой сети — по копии отфильтрованного справочника |
| src/core/precheck.rs:83-86 | флаг ввода/вывода — первая запись справочника с этим адресом, сеть не сверяется |
| src/core/precheck_nets.rs:77 | «запись в памяти есть» — по адресу без сети |
| src/core/transfers.rs:100-108 `own_name` | у биржи имени другой сети нет (None) |
| src/core/intermediate.rs:52-58 `ticker_of`, subscribe.rs:236-245 | тикер промежуточной монеты — по адресу без сети |
| src/core/subscribe.rs:28-104 `wanted` | бирже — монета с одним тикером в двух сетях — две записи на один стакан (TASKS 430 и) |
| src/core/coins_view.rs:56 `coin_json` | монета экрана по адресу без сети (API GET /api/coins/{address}) |
| src/core/tick.rs:157-162 `assets_by_tokens` | событие пула DEX одной сети будит монету с тем же адресом в другой сети |
| src/core/registry_sync.rs:74, precheck.rs:80, deposit_min_ask.rs:163 | справочник биржи → `native::map` без привязки записей к сети ядра |
| src/core/rebuild.rs:36-49 REGISTRY, :67 kick_networks | смена имени сети биржи толкает пересборку и справочник сетей — только venue.bsc_network |

## 2. Решение шага

**2.1 Имя сети биржи по сети ядра — одна функция шлюза** (`src/gateway/own_nets.rs`, новый). Правило имени настройки:
сеть по умолчанию (`None`) — `venue.bsc_network` (как раньше); другая сеть `c` — `venue.<c>_network` (id сети — dex.chain
площадки DEX). Для Arbitrum (dex.chain = `arbitrum`) — новая настройка **`venue.arbitrum_network`** (по площадке,
необязательная). Адаптер читает имена своих сетей (`OwnNets`: BSC обязательно, как раньше; Arbitrum — если задана) и
ставит адрес контракта у записи каждой своей сети. CoinEx: адрес — из /assets/info по своему имени цепи, для Arbitrum —
новая **`venue.arbitrum_contract_chain`** [coinex] (по образцу venue.bsc_contract_chain). Значения — только из живых
ответов API: записаны BingX `ARBITRUM` (tests/data/bingx_2026-10-24/config_getall.json), KuCoin `arbitrum`
(tests/data/cex_samples_2026_11_04/kucoin_currencies_v3.json), Gate `ARBEVM` (gate_spot_currencies.json); остальным — не
задавать, совет FIRST_RUN «по живому ответу API».

Разборы бирж принимают `impl Into<OwnNets>` вместо `&str`: вызовы тестов с «BEP20» компилируются без правки. Проверки,
от которых зависит пригодность сети (Binance denomination, HTX неоднозначный baseChainProtocol), — у каждой своей сети;
журнальные пометки «сети BSC …» — как раньше, только у BSC.

**2.2 Привязка записи к сети ядра — только ядро** (`src/core/net_chain.rs`, новый; вызывается перед `native::map` в
registry_sync.rs:74, precheck.rs:80, deposit_min_ask.rs:163). Запись с именем `venue.bsc_network` — без сети (как раньше);
с именем сети `c` площадки DEX не по умолчанию (`venue.<c>_network` биржи) — `chain = core/chain.rs tag(c)`; адрес у
записи любой другой сети снимается (как было: адрес был только у BSC). Площадки DEX другой сети нет — ни одна запись
сети не получает, адрес Arbitrum снимается: справочник BSC байт в байт прежний, даже если venue.arbitrum_network задана.

**2.3 Ядро по записи сети (сеть + адрес)**:
- presence: имя сети группы — `rebuild_build.rs` передаёт в справочник имена сети группы (None — venue.bsc_network, как
  раньше; сеть c — venue.<c>_network бирж);
- USDT: `registry_build::usdt_address_in(сеть)` — записи только этой сети; сеть по умолчанию — записи без сети;
- precheck.rs / precheck_nets.rs (`recent_in`): запись — по (сеть, адрес);
- transfers::own_name: у биржи для другой сети — `venue.<c>_network`;
- intermediate/subscribe: тикер промежуточной — по записи её сети; подписка биржи — монета другой сети с тикером, который
  уже есть у монеты без сети, отдельной записью не идёт (один стакан);
- coins_view::coin_json: монета сети по умолчанию (API по адресу; ключ с сетью — вопрос Тиграну);
- tick assets_by_tokens: монеты сети площадки DEX события (все монеты без сети — без проверки, как раньше);
- rebuild REGISTRY и толчок справочника сетей — по `own_nets::affects` (venue.bsc_network и имена других сетей).

**2.4 Сеть из запроса**: kucoin/okx fund_move, xt deposits — имя сети по `Asset.chain` (`own_nets::name_of`); монета без
сети — venue.bsc_network, как раньше.

## 3. Не в шаге (docs/TASKS.md, блок 431)
- Ключи второй сети «bingx:cex_dex@arbitrum» / «arbitrum:0x…» — вопрос Тиграну (не решается здесь).
- core/coin_account.rs remember_saved — не пополняется: запись для монеты BSC сменила бы ключ счёта у живых сделок BSC
  (ключ CoinKey::of → CoinKey::Venue) — вопрос Тиграну, как делить остаток одной монеты биржи между сетями.
- ticker_index (TASKS 430 г): (биржа, тикер) → одна монета — монета Arbitrum с тем же тикером не будится стаканом биржи.
- KuCoin withdraw_hold — сеть BSC (контракт без сети); Gate круг минимумов депозита — по BSC; Kraken проверка — BSC.

## 4. Проверка «BSC как раньше»
Одна площадка DEX (bsc_dex): других сетей нет → net_chain снимает адреса не-BSC записей (их и не было без
venue.arbitrum_network) — справочник прежний; presence получает те же имена (venue.bsc_network); usdt_address_in(None) =
все записи (все без сети); precheck/recent_in(None) = прежнее сравнение; own_name(None) = venue.bsc_network; подписка —
нет монет с сетью → без изменений; tick — все без сети → без фильтра; fund_move/xt — Asset.chain None → venue.bsc_network.
Разборы бирж с одним именем — прежние результаты (тесты на записанных данных — без правки).

## 5. Порядок коммитов
1. Этот план. 2. Шлюз own_nets + настройки (каталог, SETTINGS, SETTINGS_CASES, FIRST_RUN). 3. Биржи (14 разборов,
fund_move, xt). 4. Ядро (net_chain, presence, USDT, precheck, transfers, subscribe, coins_view, tick, rebuild).
5. Тесты на записанных данных + DECISIONS 431, TASKS, ROADMAP, ARCHITECTURE, README.
