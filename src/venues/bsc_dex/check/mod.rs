//! Файл: venues/bsc_dex/check/mod.rs
//! Что делает: проверки монет и пулов (решения 30, 32): налоги покупки/продажи/перевода пробным обменом
//! (eth_simulateV1), газ обмена, влияние крючков V4 группы B/C (квотер против математики), циклы перепроверки.
//! Торговли нет, пока проверка не завершена.
//! Кто вызывает: pools/market.rs (запуск циклов), mod.rs (recheck_coins), quote/* (итоги).
//! От чего зависит: ctx.rs, node/*, pools/*, quote/*, exec/calls.rs.
//! Настройки: tax.*, hooks.*, check.probe_usdt, dex.quoter_check_s, dex.quoter_max_diff_pct, dex.deadline_s.

pub mod cycle;
pub mod emitter;
pub mod hooks_probe;
pub mod native_probe;
pub mod probe;
pub mod probe_calls;
pub mod quoter_cycle;
pub mod quoter_tally;
pub mod recheck;
pub mod simulate;
pub mod state;
pub mod tax_cycle;
pub mod tax_pass;
pub mod taxes;
pub mod taxes_util;
