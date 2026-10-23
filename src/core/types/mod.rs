//! Файл: core/types/mod.rs
//! Что делает: общие типы ядра — единый язык данных между ядром, шлюзом, адаптерами и интерфейсом.
//! Кто вызывает: весь проект.
//! От чего зависит: market.rs, pool.rs, trade.rs, cost.rs, transfer.rs, check.rs, send.rs, control.rs.
//! Настройки: нет.
//!
//! Это стык: менять только через запрос на изменение стыка (docs/AGENTS.md).

pub mod check;
pub mod control;
pub mod cost;
pub mod market;
pub mod pool;
pub mod send;
pub mod trade;
pub mod transfer;

pub use check::{
    BlockReason, CheckState, CoinBlock, CoinCheck, HookImpact, PoolCheck, TaxKind, TokenTaxes,
};
pub use control::{
    CoinGasStats, CommandSource, ManualAction, ManualCommand, ManualHolding, StuckAlert,
};
pub use cost::{Cost, CostKind, DexRoute, RouteHop, RouteLeg, RouteMode};
pub use market::{Asset, Direction, OrderBook, PriceLevel, VenueId};
pub use pool::{HookGroup, HookInfo, Pool, PoolKind, PoolRef, PoolState};
pub use send::{
    ChainTx, ChannelState, SendChannelInfo, SendChannelStats, SendMethod, TxOutcome, WalletQueue,
    WalletQueueState,
};
pub use trade::{Quote, Side, TradeRequest, TradeResult, TradeStatus};
pub use transfer::{Balance, CoinNetworkInfo, Transfer, TransferStatus, WithdrawRequest};
