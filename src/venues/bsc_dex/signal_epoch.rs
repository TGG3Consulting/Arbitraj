//! Файл: venues/bsc_dex/signal_epoch.rs
//! Что делает: решение 352 — «эпоха» котировки DEX для ядра: то, от чего котировка зависит, но о чём ядро не получает
//! событий. Ядро (core/signal_dex.rs) переиспользует котировку стороны DEX сигнала на тот же объём, только если эпоха
//! та же (и поколение пулов монеты в ядре, и настройки). Состав:
//! номер головы (котировка проверяет свежесть головы — quote/mod.rs quote; время блока — у пулов Fictive);
//! цена газа этой головы из памяти (quote/gas.rs: на голове с тем же номером котировка берёт ровно её; цены для этой
//! головы ещё нет — эпохи нет: котировка запросила бы узел или взяла бы цену прошлого блока); минимум газа готовых
//! каналов (send::planned_floor — котировка берёт max); число сборок графа и номер публикации снимка фонового владельца
//! (pools/graph_cache.rs builds, graph_snap.rs seq: граф — копии записей на момент снимка, публикация событием ядру не
//! приходит); по каждому пулу пар из {монета, USDT, промежуточные} — поколение состояния, сон, достоверность, факты
//! проверок (сверка с квотером, крючки, газ стороны, налог пула), последняя глубина (вердикт с гистерезисом считается и
//! фоном); налоги монеты и промежуточных; список найденных промежуточных (pools/mids.rs active); поколение настроек.
//! Ревью t160 Н6: по пулам маршрутов монеты, а не по общей версии реестра (иначе любое событие любого пула меняло бы
//! эпоху и попаданий не было бы). Голова не свежая или лента событий отстаёт — эпохи нет (котировка вернула бы ошибку —
//! ядро считает заново).
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

/// Что делает: часть эпохи без времени и узла (ревью t160 Н6 — по пулам маршрутов МОНЕТЫ, а не по всему реестру):
/// минимум газа каналов; граф (сборки, номер снимка); промежуточные; поколение настроек; наборы путей монеты в кэше; по
/// каждому пулу пар из {монета, USDT, промежуточные} (обе ориентации — пулы всех путей монеты, route_paths.rs) —
/// ссылка, поколение состояния, сон, достоверность, факты проверок, последняя глубина; налоги монеты и промежуточных.
/// Принимает: части, адрес монеты. Возвращает: строку; None — не задана настройка, без которой котировка всё равно не
/// пройдёт (минимум каналов, USDT, срок набора путей задан нулём), или пул вне реестра (поколение 0). Ошибок нет.
pub fn state(p: &Parts, coin: Address) -> Option<String> {
    let floor = super::send::planned_floor(&p.ctx).ok()?;
    let reg = &p.registry;
    let graph = reg.graph_cache();
    let seq = graph.bg().load().map(|s| s.seq);
    let mids: Vec<Address> = p.mids.active(&p.ctx).iter().map(|m| m.address).collect();
    let usdt = p.tokens.usdt(&p.ctx).ok()?;
    let paths = match super::quote::route_cache::ttl(&p.ctx).ok()? {
        Some(t) => graph.paths_of(usdt, coin, t),
        None => Vec::new(),
    };
    let mut tokens = vec![coin, usdt];
    tokens.extend(mids.iter().copied());
    let mut pools = Vec::new();
    for (i, a) in tokens.iter().enumerate() {
        for b in &tokens[i + 1..] {
            // Решение 410 (t195): записи без копии (Arc) — читаются только поля для отпечатка.
            for e in reg
                .pair_arcs(*a, *b)
                .into_iter()
                .chain(reg.pair_arcs(*b, *a))
            {
                if e.state_gen == 0 {
                    return None;
                }
                let r = &e.pool.reference;
                pools.push(format!(
                    "{r:?}:{}:{}:{}:{:?}:{:?}",
                    e.state_gen,
                    e.asleep,
                    e.valid,
                    p.checks.pool(r),
                    reg.depth().last(r)
                ));
            }
        }
    }
    pools.sort();
    let taxes: Vec<String> = tokens
        .iter()
        .filter(|t| **t != usdt)
        .map(|t| format!("{:?}", p.checks.coin(t).map(|c| c.taxes)))
        .collect();
    Some(format!(
        "{floor}:{}:{seq:?}:{}:{mids:?}:{paths:?}:{pools:?}:{taxes:?}",
        graph.builds(),
        p.ctx.settings.generation()
    ))
}
