# Переход на свой узел BSC без KyberSwap — план для Тиграна

Ветка `feat/own-bsc-node` (решения 121, 122). Ставит на сервер и меняет настройки только Тигран, в интерфейсе.
Все значения ниже — советы (как docs/FIRST_RUN.md), не значения по умолчанию движка. Текущие значения сняты с
работающего бота 03.11.

## 0. Что меняется в боте
- Узел: адрес `http://127.0.0.1:8545` / `ws://127.0.0.1:8546` теперь принимается (http/ws — только для петли).
- Здоровье узла: возраст головы (`feed.max_block_age_ms`), «Проверить подключение» показывает chainId, `eth_syncing`,
  возраст головы, Multicall3.
- Свой маршрутизатор вместо KyberSwap: сплит с учётом газа, многошаговые ноги одним Universal Router, BNB в бандле,
  свой контракт-исполнитель ArbExecutor (одна транзакция через любые DEX), новые DEX: Slipstream, THENA Fusion,
  форки V3, THENA V1 (`solidly_v1`), DODO V2, Ellipsis (StableSwap); решение 122 F, 3 — StableSwap PancakeSwap, Lista,
  Nerve (сигнал, без исполнения) и WooFi (сигнал по лесенке ответов квотера пула, dex.pmm_ladder_usd). Lista V2 / Lista V3 (форки Uniswap V2/V3) —
  кода не нужно: виды `v2_fork` / `v3_fork` с их фабриками (Lista V3 Factory 0xcb010ed373523942706F730b89792aA1C1597b20).
- Проверка цены перед сделкой — по границе сделки (`worst_price`), а не по |дрейфу|.
- «Сухая сделка DEX» (Ручное управление): собрать и просимулировать своп без отправки, в т.ч. «через исполнитель».
- DEX ↔ DEX исполняется (решение 124): циклы через исполнитель ArbExecutor одной транзакцией (или бандлом); «Сухая сделка
  DEX» — режим «цикл».
- KyberSwap не удалён: `exec.via_kyber = да` — откат на него за один клик.

## 1. Проверка узла на сервере (до переключения)
```
curl -s -XPOST 127.0.0.1:8545 -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}'
# ждём "0x38"
curl -s -XPOST 127.0.0.1:8545 -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"eth_syncing","params":[]}'
# ждём false
curl -s -XPOST 127.0.0.1:8545 -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"eth_getBlockByNumber","params":["latest",false]}' | grep -o '"timestamp":"[^"]*"'
# timestamp (hex, секунды) ≈ date +%s, разница ≤ 3 с
```
- В окружении бота `NO_PROXY=127.0.0.1,localhost` (если задан прокси) — иначе запросы к узлу уйдут в прокси.
- Узлу — приоритет CPU выше бота (например `nice`/`CPUWeight` у сервиса geth), чтобы узел не отставал под нагрузкой.

## 2. Настройки (Настройки → в интерфейсе, вручную)
| Настройка | Сейчас | Ставить | Зачем |
|---|---|---|---|
| `bsc.node_http_url` | https://bsc-rpc.publicnode.com | `http://127.0.0.1:8545` | свой узел |
| `bsc.node_ws_url` | wss://bsc-rpc.publicnode.com | `ws://127.0.0.1:8546` | свой узел |
| `bsc.node_http_url_backup` / `bsc.node_ws_url_backup` | публичные | очистить или оставить публичные | резерв: решите — оставить публичный узел запасным или работать только на своём |
| `bsc.receipts_node_urls` | публичные | оставить или очистить | то же |
| `bsc.node_http_rps` | 20 | ≈500 | свой узел без лимита провайдера |
| `bsc.pool_events_source` | ws | ws | без изменений |
| `dex.pools_per_sub` [bsc_dex] | 5000 | **1000** | предел своего узла: 1000 вариантов topics, больше — подписка падает |
| `feed.max_block_age_ms` [bsc_dex] | не задана | ≈3000 — **задать ДО смены адресов узла** | узел отстал > 3 с — торговли нет; при адресе узла 127.0.0.1 без этой настройки торговли нет вовсе (StaleData «задайте feed.max_block_age_ms») |
| `exec.via_kyber` [bsc_dex] | да | **нет** (после шага 6½) | свой исполнитель |
| `exec.dex_best_of` [bsc_dex] | не задана | да на шаге 6½ (при via_kyber = да) | «лучший из двух»: KyberSwap ∥ свой, исполняет лучший (решение 123) |
| `kyber.final_gate_usd` | −0.05 | можно очистить | без KyberSwap не применяется |
| `verify.max_price_drift_pct` [bsc_dex] | 50 | 10–20 | теперь только предохранитель от битых данных |
| `venue.balance_events` [bsc_dex] | не задана | да | балансы кошелька событиями своего узла |
| `index.factory_scan` [bsc_dex] | не задана | да | поиск пулов по фабрикам на своём узле |
| `dex.v3_fee_tiers` [каждая V3-DEX] | не задана | уровни комиссии DEX (Pancake V3: 100,500,2500,10000; Uniswap V3: 100,500,3000,10000) — по докам DEX | V3 по фабрике |
| `dex.cl_tick_spacings` [Slipstream] | не задана | tickSpacing фабрики Topaz — по докам DEX | Slipstream по фабрике |
| `dex.kind` [thena] | solidly_v2 | **solidly_v1** | у фабрики THENA V1 (0xAFD8…3970) другой интерфейс, сейчас её пулы не проходят проверку |
| `dex.stable_pools` [Ellipsis] | не задана | адреса базовых пулов из доков Ellipsis | подтверждение StableSwap-пулов |
| `dex.kind` [PancakeSwap Stable] | не задан | `pancake_stableswap` | решение 122 F, 3; id области — id DEX у индексатора (или любой id для поиска по фабрике) |
| `dex.factory` [PancakeSwap Stable] | не задана | фабрика V2 `0x25a55f9f2279A54951133D503490342b50E5cd15` (официальный сабграф pancake-subgraph, exchange-stableswap/bsc) | реестр getPairInfo / getThreePoolPairInfo |
| `dex.stable_pools` [PancakeSwap Stable] | не задана | пулы первого поколения (USDT-BUSD 0x169F…87EF, USDC-BUSD 0xc2F5…43Fe, USDT-USDC 0x3EFe…63DD, lisUSD-BUSD 0x4907…94eC — полные адреса из @pancakeswap/stable-swap-sdk 1.1.4) — по желанию | у фабрики V2 их нет |
| `dex.kind` / `dex.factory` [Lista Stable] | не задан | `lista_stableswap` / фабрика `0xDE9c8E1536989d8c3817afDabC37C0fb44cB49b4` (lista-dao/moolah script/smartCollateral/SCAddress.sol) | реестр getPairInfos; нужен `dex.multicall3` |
| `dex.kind` / `dex.stable_pools` [Nerve] | не задан | `saddle_stableswap` / только базовые пулы 3pool `0x1B3771a66ee31180906972580adE9b81AFc5fCDc`, BTC `0x6C341938bB75dDe823FAAfe7f446925c66E6270c`, ETH `0x146CD24dCc9f4EB224DFd010c5Bf2b0D25aFA9C0` (сверить с docs.nerve.fi/fundamentals/contract-addresses) | Nerve почти мёртв (TVL ≈ $0.9 млн) — по желанию; метапулы не подключать |
| `dex.kind` [WooFi] | не задан | `woofi_v2` | общий контракт; поиск — только по оракулу (`index.factory_scan` = да) |
| `dex.pmm_pool` / `dex.oracle` [WooFi] | не задана | `0x5520385bFcf07Ec87C4c53A7d8d65595Dff69FA4` / `0x00021b2C857A8613DAEC2fd900122833866828a0` (woonetwork/docs 68dbdad, bnb-smart-chain.md) | WooPPV2 (прокси) и оракул |
| `dex.pool_implementation` [WooFi] | не задана | `0x7b5fDb8C89A616d319202704B0eA18b168654281` (слот EIP-1967 на 03.11, сверено координатором) | сменилась реализация — пары не берутся |
| `dex.pmm_ladder_usd` [WooFi] | не задана | суммы USDT по возрастанию, например 50, 200, 1000, 5000 | лесенка цен пула; не задана — WooFi только сигнал |
| `dex.pool_enabled` [каждая новая] | не задана | да | без него DEX не подключена |
| `dex.route_gas_aware` [bsc_dex] | не задана | да | газ при выборе маршрута (как gasInclude KyberSwap) |
| `exec.executor_address` [bsc_dex] | не задана | после шага 4 | исполнитель |

Остальное без изменений: `dex.multicall3`, `dex.load_batch` (50), `dex.route_max_hops` (3), `smart.max_splits` (10),
`dex.history_scan` (нет — узел хранит ~3 суток истории).

## 3. Порядок
1. Поставить сборку ветки (CI «Linux build»), запустить в **сухом режиме**.
2. Задать `feed.max_block_age_ms` (≈3000), затем сменить адреса узла → «Проверить подключение» узла: «Подключено», chainId 56, узел не догоняет, возраст головы ≤ 3 с.
3. Остальные настройки таблицы. Смотреть журнал: нет «exceed max topics», нет «узел отстаёт», пулы и сигналы идут.
4. **Сухие сделки DEX** (Ручное управление → «Сухая сделка DEX»), по одной на форму: V2, V3 Pancake, Infinity,
   Uniswap V3/V4, сплит, маршрут через промежуточную, BNB, монета с налогом. Везде «выход ≥ минимума».
5. Те же сухие сделки «через исполнитель» (код подставляется в симуляцию, контракт ещё не развёрнут). Особо: Slipstream,
   THENA Fusion, THENA V1, DODO, Ellipsis, а также PancakeSwap Stable Swap и Lista (шаг STABLE_U256) и Nerve (шаг
   SADDLE) — эти шаги ещё ни разу не исполнялись. Для новых стейблсвапов: в итоге сухой сделки — успех пробы,
   «получено» ≈ котировке (разница — только округление), газ шага; Lista — пара, где цена близка к оракулу (у пула
   checkPriceDiff: котировка за порогом сделку не даёт); пул на паузе (Nerve / Lista paused, Pancake is_killed) —
   маршрута через него нет. Нативный BNB пула (шаг STABLE_U256_BNB, «122 X (нативный BNB в стейблсвапах)»): сухая
   сделка через Lista slisBNB/BNB 0x3DcE…6131 в обе стороны (нужны `dex.native_wrapped` = WBNB и WBNB промежуточной
   монетой): WBNB → slisBNB (WBNB.withdraw → exchange с value) и slisBNB → WBNB (BNB от пула с `bnb_gas` → deposit);
   в итоге — успех пробы, «получено» ≈ котировке, у исполнителя после симуляции нет ни BNB, ни WBNB, ни разрешений;
   откат «BNB transfer failed» — receive не принял BNB (сообщить). После развёртывания — «Проверить подключение»
   исполнителя: код совпал (новый байткод).
5а. **Сухая сделка цикла DEX ↔ DEX** (решение 124): задать `dex_dex.enabled` = да, `dex_dex.base_tokens`,
   `dex_dex.max_cycle_len`, `dex_dex.max_cycles`, `dex_dex.probe_input_base`, `dex_dex.min_profit_usd`,
   `dex_dex.size_step_base` (`dex_dex.execute` НЕ включать). Ручное управление → «Сухая сделка DEX» → «Что проверить:
   цикл DEX ↔ DEX» → «Загрузить циклы» → выбрать цикл (лучше «исполнимо», чистая > 0) → бюджет (малый, например
   рабочий `money.max_per_trade_usd`) → режим «через исполнитель (код подставлен)» → «Проверить цикл». Смотреть:
   «Годится»; заметка «план ядра» (вход ≤ бюджета и баланса кошелька, граница = вход + затраты + порог); форма
   «цикл: через исполнитель …»; выход по симуляции ≥ границы; заметка «чистая прибыль по симуляции» ≥ порога; газ
   обмена и разрешения. Отказ «ниже границы» / «газ съел прибыль» — нормально для неприбыльного цикла (денег нет).
   Повторить для цикла с базовой WBNB. После развёртывания (шаг 6) — то же в режиме «как пойдёт сделка»: форма
   «цикл: исполнитель 0x…». В сухом режиме бота (`dry.enabled` = да) с `dex_dex.execute` = да то же делается само на
   каждом прибыльном цикле — журнал `core.cycle_dry`, уведомление `cycle.dry`.
6. Площадки → DEX → «Развернуть исполнитель»: газ ≈1 млн единиц (≈0.07 USDT при текущей цене газа), один раз.
   Адрес — в `exec.executor_address`. Повторить 2–3 сухие сделки — теперь против развёрнутого контракта.
6½. **Промежуточный шаг — «лучший из двух» (решение 123):** `exec.via_kyber = да` + `exec.dex_best_of = да`. KyberSwap
   спрашивается всегда, но своп идёт своим сборщиком, если свой маршрут лучше (или KyberSwap ответил 429/сроком).
   Сначала — сухой режим: в «Кандидатах» строки «Исполнитель DEX: KyberSwap X vs свой Y → …» показывают, как часто
   и на сколько свой лучше; затем малые суммы — в «Сделках» хронология «исполнил: свой сборщик … / KyberSwap; лучший
   из двух: …», журнал `dex.best_of`. Когда свой стабильно выигрывает и исполняется без отказов — следующий шаг.
7. Выйти из сухого режима на малых суммах (`exec.via_kyber = нет`); циклы — `dex_dex.execute` = да только после 5а. Первые сделки — сверить «Сделки»: исполнитель
   «свой сборщик…», план ↔ факт.
8. Откат в любой момент: `exec.via_kyber = да` (и `exec.dex_best_of = нет` — только KyberSwap) и/или старые адреса узла.

## 4. Критерии «можно на деньги»
- Шаги 2–5 без отказов; сухая сделка каждой формы проходит.
- Сутки в сухом режиме: узел не отстаёт, подписки без ошибок, сигналы есть.
- Независимая проверка ветки и контракта — пройдена (результат — в docs/notes).

## 5. Что не сделано (техдолг — docs/ROADMAP.md)
- Maverick V2 (ждём официальный исходник пула), KyberSwap Elastic (без исполнителя), RFQ (не делаем).
- PancakeSwap Stable, Lista Stable, Nerve (решение 122 F, 3) — в графе и сигналах, исполнения нет: у ArbExecutor нет
  шага exchange(uint256,…) и swap(uint8,…,deadline). WooFi — котировка по лесенке ответов квотера пула (свой расчёт невозможен — развёрнутый
  код закрыт); исполнения нет до шага WOO (сухая сделка).
- Тесты THENA V1, Ellipsis, PancakeSwap Stable, Lista Stable, Nerve ждут записей с узла (tests/README.md).
