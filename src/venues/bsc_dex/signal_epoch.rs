//! Файл: venues/bsc_dex/signal_epoch.rs
//! Что делает: решение 352 — «эпоха» котировки DEX для ядра: то, от чего котировка зависит, но о чём ядро не получает
//! событий. Ядро (core/book_same.rs) переиспользует прежнюю цену стороны DEX сигнала на одинаковом кадре стакана биржи,
//! только если эпоха та же (и поколение пулов монеты в ядре то же, и проверки, и настройки). Состав:
//! номер головы (котировка проверяет свежесть головы — quote/mod.rs quote; время блока — у пулов Fictive);
//! цена газа этой головы из памяти (quote/gas.rs: на голове с тем же номером котировка берёт ровно её; цены для этой
//! головы ещё нет — эпохи нет: котировка запросила бы узел или взяла бы цену прошлого блока); минимум газа готовых
//! каналов (send::planned_floor — котировка берёт max); число сборок графа и номер публикации снимка фонового владельца
//! (pools/graph_cache.rs builds, graph_snap.rs seq: граф — копии записей на момент снимка, публикация событием ядру не
//! приходит); версия реестра пулов (растёт на любую запись: состояние, сон, достоверность, новые пулы — registry.rs put /
//! update); число расчётов глубины (depth_book.rs calcs — вердикт глубины с гистерезисом считается и фоном); поколение
//! итогов проверок (check/state.rs generation: налоги монет, факты пулов — сверка с квотером, крючки, газ стороны,
//! налог пула); список найденных промежуточных (pools/mids.rs active); поколение настроек. Голова не свежая или лента
//! событий отстаёт — эпохи нет (котировка вернула бы ошибку — ядро считает заново).
//! Наборы путей графа монеты в кэше с dex.route_paths_ttl_ms (quote/route_cache.rs): момент записи и «годен ли ещё» —
//! котировка после устаревания набора перебирает пути заново (новый пул виден), поэтому устаревание меняет эпоху.
//! Строка — точная склейка чисел (не хеш): равенство строк = равенство всех частей.
//! Разделено на gate (то, что зависит от времени и узла: свежесть головы, лента, цена газа этой головы) и state
//! (остальное — проверяется тестом tests/bsc_dex_signal_epoch_352.rs на записанных пулах).
//! Кто вызывает: venue.rs (контракт signal_epoch).
//! От чего зависит: parts.rs (Parts: heads, lag, gas, registry, checks, mids), node/heads.rs, node/events_lag.rs,
//! quote/gas.rs, send/mod.rs (planned_floor), pools/{registry, depth_book, graph_cache, mids}.rs, check/state.rs,
//! settings (generation).
//! Настройки: читает feed.max_block_age_ms и настройки каналов через heads.fresh / planned_floor [bsc_dex].

use alloy_primitives::Address;

use crate::core::types::Asset;

use super::parts::Parts;

/// Что делает: эпоха котировки монеты сейчас (см. шапку). Принимает: части адаптера, монету. Возвращает: строку; None —
/// эпохи нет (gate или state не собрались). Ошибок нет.
pub fn of(p: &Parts, asset: &Asset) -> Option<String> {
    let g = gate(p)?;
    let s = state(p, asset.bsc_contract?)?;
    Some(format!("{g}|{s}"))
}

/// Что делает: часть эпохи, зависящая от узла и времени: номер свежей головы и цена газа ЭТОЙ головы из памяти
/// (quote/gas.rs known — на голове с тем же номером котировка берёт ровно её). Принимает: части. Возвращает: строку;
/// None — голова не свежая, лента отстаёт, цены газа этой головы ещё нет. Ошибок нет.
pub fn gate(p: &Parts) -> Option<String> {
    let head = p.heads.fresh(&p.ctx).ok()?;
    p.lag.check(&p.ctx).ok()?;
    let (gas_block, gas) = p.gas.known()?;
    if gas_block != head.number {
        return None;
    }
    Some(format!("{}:{gas}", head.number))
}

/// Что делает: часть эпохи без времени и узла: минимум газа каналов, версия реестра, расчёты глубины, поколение итогов
/// проверок, граф (сборки, снимок), промежуточные, поколение настроек, наборы путей монеты в кэше. Принимает: части,
/// адрес монеты. Возвращает: строку; None — не задана настройка, без которой котировка всё равно не пройдёт (минимум
/// каналов, USDT, срок набора путей задан нулём). Ошибок нет.
pub fn state(p: &Parts, coin: Address) -> Option<String> {
    let floor = super::send::planned_floor(&p.ctx).ok()?;
    let reg = &p.registry;
    let graph = reg.graph_cache();
    let seq = graph.bg().load().map(|s| s.seq);
    let mids: Vec<_> = p.mids.active(&p.ctx).iter().map(|m| m.address).collect();
    let usdt = p.tokens.usdt(&p.ctx).ok()?;
    let paths = match super::quote::route_cache::ttl(&p.ctx).ok()? {
        Some(t) => graph.paths_of(usdt, coin, t),
        None => Vec::new(),
    };
    Some(format!(
        "{floor}:{}:{}:{}:{}:{seq:?}:{}:{mids:?}:{paths:?}",
        reg.version(),
        reg.depth().calcs(),
        p.checks.generation(),
        graph.builds(),
        p.ctx.settings.generation()
    ))
}
