# core/types — общие типы (стык)

| Файл | Типы |
| --- | --- |
| `market.rs` | VenueId, Asset (тикер + адрес контракта BSC), Direction, PriceLevel, OrderBook |
| `pool.rs` | PoolKind (V2/V3/Cl/Bin), PoolRef (адрес или менеджер + PoolId), PoolState, HookGroup (A/B/C), HookInfo, Pool |
| `trade.rs` | Side, Quote (котировка на объём + затраты + маршрут DEX), TradeRequest, TradeStatus, TradeResult |
| `cost.rs` | CostKind, Cost (затрата: вид, сумма, в чём, учтена ли в цене, шаг маршрута), RouteStep (номер шага и пара токенов, решение 52), RouteMode, RouteHop, RouteLeg, DexRoute |
| `transfer.rs` | CoinNetworkInfo, TransferStatus, WithdrawRequest, Transfer, Balance |
| `check.rs` | CheckState, TokenTaxes, CoinCheck, HookImpact, PoolCheck, TaxKind, BlockReason, CoinBlock |
| `send.rs` | SendMethod, SendChannelInfo, ChannelState, SendChannelStats, TxOutcome, ChainTx |
| `directory.rs` | IndexerDex, DirectoryStatus, DirectoryPool, DirectoryToken, PoolDirectory — справочник пулов от индексатора (решение 58) |
| `control.rs` | CoinGasStats, StuckAlert, ManualHolding, ManualAction, ManualCommand, CommandSource |

Деньги — `rust_decimal::Decimal`, значения блокчейна — `alloy_primitives::U256`/`Address`/`B256`.
Карта тиков V3/CL и резервы бинов Bin в стык не входят — они живут внутри адаптера DEX (нужны только его математике).
Это стык: менять только через запрос на изменение стыка (`docs/AGENTS.md`).
