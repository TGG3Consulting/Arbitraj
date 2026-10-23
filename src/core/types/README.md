# core/types — общие типы (стык)

| Файл | Типы |
| --- | --- |
| `market.rs` | VenueId, Asset (тикер + адрес контракта BSC), Direction, PriceLevel, OrderBook, PoolState (V2/V3), Pool |
| `trade.rs` | Side, Quote (котировка на объём), TradeRequest, TradeStatus, TradeResult |
| `transfer.rs` | CoinNetworkInfo, TransferStatus, WithdrawRequest, Transfer, Balance |

Деньги — `rust_decimal::Decimal`, значения блокчейна — `alloy_primitives::U256`/`Address`.
Это стык: менять только через запрос на изменение стыка (`docs/AGENTS.md`).
