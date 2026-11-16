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
//! Строка — точная склейка чисел (не хеш): равенство строк = равенство всех частей.
//! Кто вызывает: venue.rs (контракт signal_epoch).
//! От чего зависит: parts.rs (Parts: heads, lag, gas, registry, checks, mids), node/heads.rs, node/events_lag.rs,
//! quote/gas.rs, send/mod.rs (planned_floor), pools/{registry, depth_book, graph_cache, mids}.rs, check/state.rs,
//! settings (generation).
//! Настройки: читает feed.max_block_age_ms и настройки каналов через heads.fresh / planned_floor [bsc_dex].

use super::parts::Parts;

/// Что делает: эпоха котировки DEX сейчас (см. шапку). Принимает: части адаптера. Возвращает: строку; None — эпохи нет
/// (голова не свежая, лента отстаёт, цены газа этой головы в памяти нет, минимум каналов не считается). Ошибок нет.
pub fn of(p: &Parts) -> Option<String> {
    let head = p.heads.fresh(&p.ctx).ok()?;
    p.lag.check(&p.ctx).ok()?;
    let (gas_block, gas) = p.gas.known()?;
    if gas_block != head.number {
        return None;
    }
    let floor = super::send::planned_floor(&p.ctx).ok()?;
    let reg = &p.registry;
    let graph = reg.graph_cache();
    let seq = graph.bg().load().map(|s| s.seq);
    let mids: Vec<_> = p.mids.active(&p.ctx).iter().map(|m| m.address).collect();
    Some(format!(
        "{}:{gas}:{floor}:{}:{}:{}:{}:{seq:?}:{}:{mids:?}",
        head.number,
        reg.version(),
        reg.depth().calcs(),
        p.checks.generation(),
        graph.builds(),
        p.ctx.settings.generation()
    ))
}
