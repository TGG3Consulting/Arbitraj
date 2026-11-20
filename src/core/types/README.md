# core/types — общие типы (стык)

| Файл | Типы |
| --- | --- |
| `chain.rs` | ChainId — id сети (решение 416): строка только из настроек (dex.chain площадки DEX), ядро сравнивает на равенство |
| `account.rs` | DexAccountId — абстрактный id счёта (кошелька) DEX, решение 132: ядро знает только id, адреса и ключи — у адаптера |
| `market.rs` | VenueId, Asset (тикер + адрес контракта BSC), Direction, PriceLevel, OrderBook |
| `pool.rs` | PoolKind (V2/V3/Cl/Bin/Pmm/Solidly/StableSwap/TickBin), PoolRef (адрес или менеджер + PoolId), PoolState, HookGroup (A/B/C), HookInfo, Pool |
| `pool_solidly.rs` | SolidlyFee, FeeRule, SolidlyGate — комиссия пула Solidly со своим правилом (числитель над делителем / делитель) и ограничения обмена пары (пауза фабрики, предел цены газа), решение 273 |
| `pool_tick_bin.rs` | TickBinData, TickReserves — состояние пула TickBin (решение 122 F, Maverick V2): тики около активного, комиссии по направлениям, масштабы токенов, доля протокола |
| `pool_amp.rs` | AmpV2Data — состояние пула «V2 с усилением» (решение 287, KyberSwap Classic DMM): реальные и виртуальные резервы и комиссия блока над 1e18; вид ядра PoolKind::AmpV2 / PoolState::AmpV2 |
| `pool_fictive.rs` | FictiveData — состояние пары с фиктивными резервами и средней ценой (решение 288, SmarDex): резервы, фиктивные резервы, средняя цена и её время, комиссии, время блока чтения; вид ядра PoolKind::Fictive / PoolState::Fictive |
| `pool_vault.rs` | VaultPoolData, VaultCurve — состояние пула в общем хранилище (решение 289, Balancer V2 / ACSI Finance): id пула, балансы и множители точности токенов, комиссия, кривая (веса / усиление A), пауза, индексы пары; вид ядра PoolKind::VaultPool / PoolState::VaultPool; решение 441 — часть Balancer V3 `VaultPoolData.v3` (VaultV3: живые балансы, курсы, наименьшая сумма обмена, правило фабрики VaultV3Rule) |
| `pool_vault_v2.rs` | V2Curve — решение 473: кривые пулов Balancer V2 других фабрик (стейблсвап с курсами — composable stable / meta-stable / stable phantom, managed с пределами цены BPT, linear с целями) — VaultCurve::V2 |
| `pool_crypto.rs` | CryptoData — состояние crypto-пула Curve (решение 442, Twocrypto-NG / Tricrypto-NG): балансы, множители точности, price_scale, D, A, gamma, mid_fee / out_fee / fee_gamma, остановка, индексы пары; вид ядра PoolKind::Crypto / PoolState::Crypto |
| `pool_lb.rs` | LbBinData — состояние пула из бинов с изменчивой комиссией (решение 290, Liquidity Book V2.1 / LFJ): активный бин, шаг бина, порядок X/Y, постоянные и изменчивые параметры комиссии, время блока чтения; вид ядра PoolKind::LbBin / PoolState::LbBin; решение 445 — признак v20 (Liquidity Book v2.0, LFJ V2 Arbitrum: правила математики v2.0) |
| `pool_smart.rs` | SmartColDebtData, ColReserves, DebtReserves — состояние пула со смарт-залогом и смарт-долгом (решение 440, Fluid DEX T1): token0 пула, нативная монета, части залог/долг, пауза, комиссия и доля протокола, центральная цена, реальные и мнимые резервы (приведённые единицы), пределы снятия/займа, множители точности, слово цен, версия, слой ликвидности, время блока; вид ядра PoolKind::SmartColDebt / PoolState::SmartColDebt |
| `pool_ekubo.rs` | EkuboData — состояние пула синглтон-ядра Ekubo (решение 447): config ключа пула, сжатая цена sqrtRatio, тик, ликвидность, прочитанные слова битмапа и тики (liquidityNet, liquidityDelta), состояние MEVCapture, время блока чтения; вид ядра PoolKind::Ekubo / PoolState::Ekubo |
| `pool_basin.rs` | BasinWellData, BasinFunction — состояние скважины Basin (решение 485): токены и резервы по порядку скважины, число LP-токенов (totalSupply — входит в формулу обмена), функция скважины ConstantProduct2 или Stable2 (A, десятичные знаки); вид ядра PoolKind::BasinWell / PoolState::BasinWell |
| `trade.rs` | Side, Quote (котировка на объём + затраты + маршрут DEX), TradeRequest, TradeStatus, TradeResult (комиссия — список `Fee` по монетам, решение 219; старые записи fee/fee_asset читаются; `Fee.usd` — стоимость в USDT из документации площадки у монеты без рынка, баллы HTX) |
| `fee.rs` | Fee — часть фактической комиссии ордера (монета, сумма; `usd` — стоимость в USDT из документации площадки у монеты без рынка, баллы HTX); вынесена из `trade.rs` |
| `cost.rs` | CostKind, Cost (затрата: вид, сумма, в чём, учтена ли в цене, шаг маршрута, монета скидки `paid_in` — решение 219), RouteStep (номер шага и пара токенов, решение 52), RouteMode, RouteHop, RouteLeg, DexRoute |
| `transfer.rs` | CoinNetworkInfo, TransferStatus, WithdrawRequest, Transfer, Balance |
| `hold.rs` | HoldItem, WithdrawHold — что биржа сейчас держит от вывода (решение 150: у MEXC — предварительно зачисленные депозиты, статус 9 PRE_SUCCESS); пусто — ничего не держится. Решение 166: unknown — у площадки нет официального источника («неизвестно», не «удержаний нет»). Решение 253: WithdrawReady — готов ли вывод монеты после покупки (контракт withdraw_ready): свободно ≥ суммы, источник ответа |
| `delist.rs` | DelistItem, Delistings — пары биржи в делистинге (решение 175): тикер, символ, когда прекращаются торги и закрывается вывод, источник в API; unknown — у площадки нет официального источника («неизвестно», не «делистингов нет») |
| `check.rs` | CheckState, TokenTaxes, CoinCheck, HookImpact (лесенки HookLadder — общие, Arc), HookStep, PoolCheck, TaxKind, BlockReason, CoinBlock |
| `send.rs` | SendMethod, SendChannelInfo, ChannelState, SendChannelStats, TxOutcome, ChainTx |
| `tip_share.rs` | Решение 437 (3): TipShare (доля прибыли на чаевые площадки DEX, счётчики, последний проигрыш), TipLoss, TipOutcome |
| `pool_cond_stat.rs` | Решение 439: PoolCondStat (статистика сделок цикла через исполнитель по режиму send.pool_cond_mode на момент отправки: off / pools), PoolCondCounts (отправки, победы, откаты и их газ в нативной монете, отказы по условию пулов и по окну времени) |
| `directory.rs` | IndexerDex, DirectoryStatus, DirectoryPool, DirectoryToken (note — «индексатор не знает пулов», решение 121), PoolDirectory, FactoryScan (поиск по фабрикам на своём узле, решение 121) — справочник пулов (решение 58) |
| `dry_swap.rs` | DexDrySwap, DrySwapPool — итог «сухой сделки DEX» (решение 121, C4): форма маршрута, пулы, выходы, минимум, газ, причина |
| `presell.rs` | DexPresell — итог симуляции продажи DEX до покупки (решение 234): кошелёк сделки, форма исполнения, выход после реального налога, ожидаемое, минимумы долей, пополнение, газ |
| `executor.rs` | ExecutorDeployQuote, ExecutorDeployed — развёртывание своего исполнителя ArbExecutor (решение 122, часть X): оценка перед подтверждением и итог |
| `pool_depth.rs` | PoolDepth, CrossCheck — пул DEX глазами адаптера для экрана «Монеты»: глубина по направлениям, вердикт, спит ли, независимая сверка расчёта (перечисление), подробности площадки строками (решение 73) |
| `dex_cycle.rs` | DexCycle, DexCycleHop — сигнал замкнутого цикла DEX ↔ DEX для «Живого экрана» (решение 85, фаза сигнала): базовая монета, вложено/получено, валовая и чистая прибыль, газ/чаевые (BNB) и их перевод в базовую, маршрут по хопам, флаг исполнимости и причина, блок |
| `dex_graph.rs` | Решение 308: DexGraphState, DexGraphRound, DexGraphFull — состояние фонового владельца графа маршрутов DEX для экрана «Площадки» (снимок, отставание, круг, полная пересборка со сверкой, расхождения, предохранитель за час/сутки, перезапуски) |
| `cycle_trade.rs` | CycleQuote, CycleTradeRequest, CycleTradeResult — исполнение цикла DEX ↔ DEX (решение 124): расчёт на вход для подбора размера, запрос сделки (вход и граница minOut ядра), итог (своп «базовая → базовая» и курсы для учёта) |
| `control.rs` | CoinGasStats, StuckAlert, ManualHolding, ManualAction, ManualCommand, CommandSource |
| `window.rs` | TapView, TradeTick, TapDecoded — разобранный кадр окна (стакан или сделки, время площадки) для страницы окна (решение 125); outcome — «plus»/«minus»/«zero» |

Деньги — `rust_decimal::Decimal`, значения блокчейна — `alloy_primitives::U256`/`Address`/`B256`.
Карта тиков V3/CL и резервы бинов Bin в стык не входят — они живут внутри адаптера DEX (нужны только его математике).
Это стык: менять только через запрос на изменение стыка (`docs/AGENTS.md`).
