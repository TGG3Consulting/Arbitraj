//! Файл: core/types/mod.rs
//! Что делает: общие типы ядра — единый язык данных между ядром, шлюзом, адаптерами и интерфейсом.
//! Кто вызывает: весь проект.
//! От чего зависит: balance.rs, market.rs, pool.rs, trade.rs, cost.rs, transfer.rs, check.rs, send.rs, control.rs, directory.rs, dex_cycle.rs, cycle_trade.rs, dry_swap.rs, executor.rs.
//! Настройки: нет.
//!
//! Это стык: менять только через запрос на изменение стыка (docs/AGENTS.md).

pub mod balance;
pub mod check;
pub mod control;
pub mod cost;
pub mod cycle_trade;
pub mod dex_cycle;
pub mod directory;
pub mod dry_swap;
pub mod executor;
pub mod market;
pub mod pool;
pub mod pool_depth;
pub mod pool_tick_bin;
pub mod send;
pub mod trade;
pub mod transfer;

pub use balance::{BalanceChange, BalanceKind};
pub use check::{
    BlockReason, CheckState, CoinBlock, CoinCheck, HookImpact, PoolCheck, TaxKind, TokenTaxes,
};
pub use control::{
    CoinGasStats, CommandSource, CommandTx, ManualAction, ManualCommand, ManualHolding, StuckAlert,
};
pub use cost::{Cost, CostKind, DexRoute, RouteHop, RouteLeg, RouteMode, RouteStep};
pub use cycle_trade::{CycleQuote, CycleTradeRequest, CycleTradeResult};
pub use dex_cycle::{DexCycle, DexCycleHop};
pub use directory::{
    DirectoryPool, DirectoryStatus, DirectoryToken, FactoryScan, IndexerDex, PoolDirectory,
};
pub use dry_swap::{DexDrySwap, DrySwapPool};
pub use executor::{ExecutorDeployQuote, ExecutorDeployed};
pub use market::{Asset, Direction, OrderBook, PriceLevel, VenueId};
pub use pool::{
    HookGroup, HookInfo, PmmData, Pool, PoolKind, PoolRef, PoolState, StableData, TickBinData,
    TickReserves,
};
pub use pool_depth::{CrossCheck, PoolDepth, SideCheck};
pub use send::{
    ChainTx, ChannelState, SendChannelInfo, SendChannelStats, SendMethod, TxOutcome, WalletQueue,
    WalletQueueState,
};
pub use trade::{Quote, Side, TradeRequest, TradeResult, TradeStatus};
pub use transfer::{Balance, CoinNetworkInfo, Transfer, TransferStatus, WithdrawRequest};
