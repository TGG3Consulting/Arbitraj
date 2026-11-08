//! Файл: core/types/mod.rs
//! Что делает: общие типы ядра — единый язык данных между ядром, шлюзом, адаптерами и интерфейсом.
//! Кто вызывает: весь проект.
//! От чего зависит: account.rs (id счёта DEX, решение 132), balance.rs, market.rs, pool.rs, trade.rs, cost.rs, transfer.rs, check.rs, send.rs, control.rs, directory.rs, dex_cycle.rs, cycle_trade.rs, dry_swap.rs, executor.rs, hold.rs (удержания вывода, решение 150), presell.rs (симуляция продажи DEX до покупки, решение 234), delist.rs (делистинги, решение 175), manual_feed.rs (записи экрана «Ручное управление» с кнопками, решение 156), window.rs.
//! Настройки: нет.
//!
//! Это стык: менять только через запрос на изменение стыка (docs/AGENTS.md).

pub mod account;
pub mod balance;
pub mod check;
pub mod control;
pub mod cost;
pub mod cycle_trade;
pub mod delist;
pub mod dex_cycle;
pub mod directory;
pub mod dry_swap;
pub mod executor;
pub mod hold;
pub mod manual_feed;
pub mod market;
pub mod pool;
pub mod pool_depth;
pub mod pool_tick_bin;
pub mod presell;
pub mod send;
pub mod trade;
pub mod transfer;
pub mod window;

pub use account::DexAccountId;
pub use balance::{BalanceChange, BalanceKind};
pub use check::{
    BlockReason, CheckState, CoinBlock, CoinCheck, HookImpact, PoolCheck, TaxKind, TokenTaxes,
};
pub use control::{
    CoinGasStats, CommandSource, ManualAction, ManualCommand, ManualHolding, StuckAlert,
};
pub use cost::{Cost, CostKind, DexRoute, RouteHop, RouteLeg, RouteMode, RouteStep};
pub use cycle_trade::{CycleQuote, CycleTradeRequest, CycleTradeResult};
pub use delist::{DelistItem, Delistings};
pub use dex_cycle::{DexCycle, DexCycleHop};
pub use directory::{
    DirectoryPool, DirectoryStatus, DirectoryToken, FactoryScan, IndexerDex, PoolDirectory,
};
pub use dry_swap::{DexDrySwap, DrySwapPool};
pub use executor::{ExecutorDeployQuote, ExecutorDeployed};
pub use hold::{HoldItem, WithdrawHold};
pub use manual_feed::{ManualButton, ManualRecord, RecordKind};
pub use market::{Asset, Direction, OrderBook, PriceLevel, VenueId};
pub use pool::{
    HookGroup, HookInfo, LadderPoint, OraclePmmData, PmmData, Pool, PoolKind, PoolRef, PoolState,
    StableData, StableGuard, StableNg, TickBinData, TickReserves,
};
pub use pool_depth::{CrossCheck, PoolDepth, SideCheck};
pub use presell::DexPresell;
pub use send::{
    ChainTx, ChannelState, SendChannelInfo, SendChannelStats, SendMethod, TxOutcome, WalletQueue,
    WalletQueueState,
};
pub use trade::{Fee, Quote, Side, TradeRequest, TradeResult, TradeStatus};
pub use transfer::{
    AccountBalances, Balance, CoinNetworkInfo, Transfer, TransferStatus, WithdrawRequest,
};
pub use window::{TapDecoded, TapView, TradeTick};
