//! Файл: venues/bsc_dex/trade_path.rs
//! Что делает: выбор пути исполнения сделки DEX (чистая функция): свой сборщик, KyberSwap (откат на свой при NoRoute,
//! решение 91) или «лучший из двух» (решение 123). Решение 247, п. Г: продажа выхода с условиями пере-замера
//! (TradeRequest.remeasure) — только своим сборщиком (проба перед отправкой и пере-замер выхода при откате по minOut).
//! t81 (дополнение к 247 п. Г, Тигран 10.11: согласовано): у путей, где пере-замер добавил t78 (первая продажа сделки,
//! «Вернуть боту», частичная покупка, «Ожидание» по истечении stuck.wait_ms), ядро ставит Remeasure.any_executor = да
//! (core/remeasure.rs PROFITABLE_OPEN) — такая продажа идёт как до t78: exec.via_kyber и exec.dex_best_of — «лучший из
//! двух» (свой — с пере-замером, best_of/trade.rs run_own несёт req.remeasure); только exec.via_kyber — KyberSwap (без
//! пере-замера), откат на свой при NoRoute — с пере-замером; exec.via_kyber = нет — свой с пере-замером.
//! Кто вызывает: venue.rs (trade_logs).
//! От чего зависит: core/types (TradeRequest).
//! Настройки: значения exec.via_kyber и exec.dex_best_of передаёт вызывающий (venue.rs, best_of::enabled).

use crate::core::types::TradeRequest;

/// Путь исполнения сделки DEX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path {
    /// Свой сборщик (exec/trade.rs).
    Own,
    /// KyberSwap (kyberswap/trade.rs); нет маршрута (NoRoute) — откат на свой сборщик с тем же запросом.
    Kyber,
    /// «Лучший из двух» (best_of/trade.rs).
    BestOf,
}

/// Путь по запросу ядра и настройкам. Принимает: запрос, exec.via_kyber, «лучший из двух» включён (best_of::enabled —
/// уже с exec.via_kyber). Возвращает: путь. Ошибок нет.
pub fn of(req: &TradeRequest, via_kyber: bool, best_of: bool) -> Path {
    // Решение 247, п. Г: условия пере-замера без признака «любой исполнитель» (t81) — только свой сборщик.
    let own_only = req.exit_position && req.remeasure.as_ref().is_some_and(|r| !r.any_executor);
    if own_only || !via_kyber {
        Path::Own
    } else if best_of {
        Path::BestOf
    } else {
        Path::Kyber
    }
}
