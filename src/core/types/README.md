# core/types — общие типы (стык)

| Файл | Типы |
| --- | --- |
| `account.rs` | DexAccountId — абстрактный id счёта (кошелька) DEX, решение 132: ядро знает только id, адреса и ключи — у адаптера |
| `market.rs` | VenueId, Asset (тикер + адрес контракта BSC), Direction, PriceLevel, OrderBook |
| `pool.rs` | PoolKind (V2/V3/Cl/Bin/Pmm/Solidly/StableSwap/TickBin), PoolRef (адрес или менеджер + PoolId), PoolState, HookGroup (A/B/C), HookInfo, Pool |
| `pool_tick_bin.rs` | TickBinData, TickReserves — состояние пула TickBin (решение 122 F, Maverick V2): тики около активного, комиссии по направлениям, масштабы токенов, доля протокола |
| `trade.rs` | Side, Quote (котировка на объём + затраты + маршрут DEX), TradeRequest, TradeStatus, TradeResult (комиссия — список `Fee` по монетам, решение 219; старые записи fee/fee_asset читаются; `Fee.usd` — стоимость в USDT из документации площадки у монеты без рынка, баллы HTX) |
| `fee.rs` | Fee — часть фактической комиссии ордера (монета, сумма; `usd` — стоимость в USDT из документации площадки у монеты без рынка, баллы HTX); вынесена из `trade.rs` |
| `cost.rs` | CostKind, Cost (затрата: вид, сумма, в чём, учтена ли в цене, шаг маршрута, монета скидки `paid_in` — решение 219), RouteStep (номер шага и пара токенов, решение 52), RouteMode, RouteHop, RouteLeg, DexRoute |
| `transfer.rs` | CoinNetworkInfo, TransferStatus, WithdrawRequest, Transfer, Balance |
| `hold.rs` | HoldItem, WithdrawHold — что биржа сейчас держит от вывода (решение 150: у MEXC — предварительно зачисленные депозиты, статус 9 PRE_SUCCESS); пусто — ничего не держится. Решение 166: unknown — у площадки нет официального источника («неизвестно», не «удержаний нет»). Решение 251: WithdrawReady — готов ли вывод монеты после покупки (контракт withdraw_ready): свободно ≥ суммы, источник ответа |
| `delist.rs` | DelistItem, Delistings — пары биржи в делистинге (решение 175): тикер, символ, когда прекращаются торги и закрывается вывод, источник в API; unknown — у площадки нет официального источника («неизвестно», не «делистингов нет») |
| `check.rs` | CheckState, TokenTaxes, CoinCheck, HookImpact, PoolCheck, TaxKind, BlockReason, CoinBlock |
| `send.rs` | SendMethod, SendChannelInfo, ChannelState, SendChannelStats, TxOutcome, ChainTx |
| `directory.rs` | IndexerDex, DirectoryStatus, DirectoryPool, DirectoryToken (note — «индексатор не знает пулов», решение 121), PoolDirectory, FactoryScan (поиск по фабрикам на своём узле, решение 121) — справочник пулов (решение 58) |
| `dry_swap.rs` | DexDrySwap, DrySwapPool — итог «сухой сделки DEX» (решение 121, C4): форма маршрута, пулы, выходы, минимум, газ, причина |
| `presell.rs` | DexPresell — итог симуляции продажи DEX до покупки (решение 234): кошелёк сделки, форма исполнения, выход после реального налога, ожидаемое, минимумы долей, пополнение, газ |
| `executor.rs` | ExecutorDeployQuote, ExecutorDeployed — развёртывание своего исполнителя ArbExecutor (решение 122, часть X): оценка перед подтверждением и итог |
| `pool_depth.rs` | PoolDepth, CrossCheck — пул DEX глазами адаптера для экрана «Монеты»: глубина по направлениям, вердикт, спит ли, независимая сверка расчёта (перечисление), подробности площадки строками (решение 73) |
| `dex_cycle.rs` | DexCycle, DexCycleHop — сигнал замкнутого цикла DEX ↔ DEX для «Живого экрана» (решение 85, фаза сигнала): базовая монета, вложено/получено, валовая и чистая прибыль, газ/чаевые (BNB) и их перевод в базовую, маршрут по хопам, флаг исполнимости и причина, блок |
| `cycle_trade.rs` | CycleQuote, CycleTradeRequest, CycleTradeResult — исполнение цикла DEX ↔ DEX (решение 124): расчёт на вход для подбора размера, запрос сделки (вход и граница minOut ядра), итог (своп «базовая → базовая» и курсы для учёта) |
| `control.rs` | CoinGasStats, StuckAlert, ManualHolding, ManualAction, ManualCommand, CommandSource |
| `window.rs` | TapView, TradeTick, TapDecoded — разобранный кадр окна (стакан или сделки, время площадки) для страницы окна (решение 125); outcome — «plus»/«minus»/«zero» |

Деньги — `rust_decimal::Decimal`, значения блокчейна — `alloy_primitives::U256`/`Address`/`B256`.
Карта тиков V3/CL и резервы бинов Bin в стык не входят — они живут внутри адаптера DEX (нужны только его математике).
Это стык: менять только через запрос на изменение стыка (`docs/AGENTS.md`).
