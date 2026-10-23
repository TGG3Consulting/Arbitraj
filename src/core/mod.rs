//! Файл: core/mod.rs
//! Что делает: ядро системы. Ничего не знает о конкретных площадках — работает только
//! со своими типами и с шлюзом.
//! Кто вызывает: main.rs (Core::start), api (состояние и живой поток ядра).
//! От чего зависит: gateway, settings, storage, notify. Никогда — venues.
//! Настройки: у каждого модуля ядра свои (docs/SETTINGS.md).
//!
//! Карта файлов — core/README.md и ARCHITECTURE.md («Карта ядра»).

pub mod blacklist;
pub mod candidates;
pub mod coins_view;
pub mod commands;
pub mod convert;
pub mod costs;
pub mod deal;
pub mod deps;
pub mod dirctx;
pub mod engine;
pub mod executor;
pub mod fills;
pub mod gas_stats;
pub mod inner;
pub mod ledger;
pub mod limits;
pub mod manual;
pub mod manual_return;
pub mod market_state;
pub mod partial;
pub mod persist;
pub mod precheck;
pub mod presence;
pub mod prices;
pub mod profit;
pub mod rebuild;
pub mod rebuild_report;
pub mod reconcile;
pub mod registry;
pub mod registry_sync;
pub mod relay;
pub mod risk;
pub mod rules_apply;
pub mod sell;
pub mod signal;
pub mod stuck;
pub mod subscribe;
pub mod tick;
pub mod trade_ops;
pub mod transfer_watch;
pub mod transfers;
pub mod transit;
pub mod types;
pub mod usdt_return;
pub mod view;
pub mod vol;
pub mod wait;

pub use engine::Core;
