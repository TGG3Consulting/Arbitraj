//! Файл: gateway/mod.rs
//! Что делает: шлюз — единственный путь между ядром и площадками.
//! Контракт адаптера, единые ошибки, события, правила при сбоях, реестр адаптеров.
//! Кто вызывает: ядро, API, main.rs.
//! От чего зависит: core/types. Никогда — venues (адаптеры регистрируются из main.rs).
//! Настройки: правила шлюза (хранятся в SQLite, меняются из интерфейса).

pub mod addr_claim;
pub mod book_lost_log;
pub mod calc_meter;
pub mod calc_share;
pub mod contract;
pub mod core_out;
pub mod core_out_book;
pub mod core_out_item;
pub mod core_out_must;
mod core_out_must_deliver;
mod core_out_must_log;
mod core_out_must_queue;
mod core_out_reg;
pub mod core_out_side;
pub mod cpu_budget;
pub mod delist_ann;
pub mod delist_html;
pub mod delist_page;
pub mod delist_time;
pub mod deposit_min;
pub mod errors;
pub mod events;
pub mod exit_scope;
pub mod feed_health;
pub mod feed_reg;
pub mod frozen;
pub mod http;
pub mod http_bind;
pub mod keep_last;
mod keep_last_decide;
mod keep_last_deliver;
mod keep_last_kinds;
mod keep_last_state;
pub mod log_clock;
pub mod lost_log;
pub mod network_hint;
pub mod patience;
pub mod pending;
pub mod quote_purpose;
pub mod quote_stage;
pub mod rate_bound;
pub mod registry;
pub mod rest_book;
pub mod rules;
pub mod setting_cell;
pub mod step_meter;
pub mod tap;
pub mod tx_hash;
pub mod urgency;
