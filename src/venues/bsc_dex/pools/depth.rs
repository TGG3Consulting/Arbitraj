//! Файл: venues/bsc_dex/pools/depth.rs
//! Что делает: делит пулы пачки на «глубокие» (все события в подписке) и «мелкие» — мельче
//! dex.min_pool_liquidity_usd в ОБА направления в полосе цены (решение 73, pools/depth_book.rs) или пустые: у них в
//! подписке только добавление ликвидности (pools/group.rs). Каждое уведомление узла по WebSocket платное, а мелкий пул
//! не котируется, не сверяется и не участвует в пробе (quote/select.rs, deep_side) — его обмены слушать незачем.
//! Ликвидность растёт только событием добавления: оно будит пул — перечитывание, и если пул стал глубоким, он
//! возвращается в полную подписку (pools/admit.rs wake); обратный переход — при следующем переподключении пачки.
//! Источник событий «квитанции блоков» (bsc.pool_events_source = receipts, решение 68): события приходят все и даром —
//! пулы НЕ спят, глубина пересчитывается на каждой смене состояния (touch). Смена источника на лету переподключает
//! пачки (pools/session.rs): на receipts — спящие пулы перечитываются и слушаются полностью, на ws — засыпают по
//! правилам.
//! Глубину не посчитать (не задана настройка, нет курса промежуточной монеты, пул не прочитан) — пул глубокий:
//! слушаем всё, как раньше, причина — одной записью в журнал.
//! Полоса цены вышла за прочитанные слова битмапа (off_map) — карта перечитывается вокруг нового тика сразу.
//! Кто вызывает: pools/admit.rs (deep, mark), pools/listen_log.rs (touch — на каждой отправке пула ядру; off_map —
//! после события).
//! От чего зависит: quote/select.rs (deep_side), quote/emptiness.rs, registry.rs, depth_book.rs, progress.rs, tokens.rs.
//! Настройки: bsc.pool_events_source [вся система]; через depth_book.rs — dex.min_pool_liquidity_usd,
//! dex.depth_band_pct, dex.depth_hysteresis_pct, dex.depth_log_min_s [bsc_dex].

use crate::core::types::PoolRef;
use crate::storage::journal::Level;

use super::super::quote::{emptiness, select};
use super::listen::Pools;

/// Глубокий ли пул (см. шапку). Принимает: окружение, ссылку на прочитанный пул. Возвращает: true — слушать всё;
/// false — только добавление ликвидности. Пул без записи в реестре — глубокий. Ошибок нет.
pub async fn deep(p: &Pools, r: &PoolRef) -> bool {
    // Квитанции блоков: события все и даром — пулы не спят (решение 73).
    if p.ctx.text("bsc.pool_events_source").ok().as_deref() == Some("receipts") {
        return true;
    }
    let Some(e) = p.reg.get(r) else {
        return true;
    };
    if emptiness::empty(&e) {
        return false;
    }
    let dec = match p.tokens.usdt(&p.ctx) {
        Ok(usdt) => p.tokens.decimals(&p.ctx, usdt).await.map_err(|e| e.message),
        Err(e) => Err(e.message),
    };
    // Мелкий пул уже спит — его порог проверяется по свежему состоянию, а не по флагу.
    let mut probe = e.clone();
    probe.asleep = false;
    let any = select::Need::Any;
    let res = dec.and_then(|d| select::deep_side(&p.ctx, &p.reg, &p.tokens, &probe, d, any));
    match res {
        Ok(None) => true,
        Ok(Some(_)) => false,
        Err(why) => {
            let msg = format!(
                "Глубина пула не считается ({why}) — такие пулы слушаются полностью (все события)"
            );
            p.signals.once(&p.ctx, Level::Warn, "dex.pool", msg).await;
            true
        }
    }
}

/// Отмечает в реестре, слушается ли пул только по добавлению ликвидности. Ошибок нет.
pub fn mark(p: &Pools, r: &PoolRef, asleep: bool) {
    p.reg.update(r, |e| e.asleep = asleep);
}

/// Глубина пула по направлениям — заново на каждой смене состояния (решение 73, pools/depth_book.rs): журнал о смене
/// «глубокий ↔ мелкий» — сразу, а не при следующей котировке; направление стало глубоким — «пул готов» (налоги и
/// сверка, ждущие глубокого пула, проверяются сразу). Знаки USDT ещё не известны или глубина не считается — без
/// пересчёта (причину покажет отбор пула). Ошибок нет.
pub fn touch(p: &Pools, r: &PoolRef) {
    let Some(e) = p.reg.get(r).filter(|e| e.valid && !e.asleep) else {
        return;
    };
    let Some(dec) = p
        .tokens
        .usdt(&p.ctx)
        .ok()
        .and_then(|u| p.tokens.decimals_known(u))
    else {
        return;
    };
    // Стало глубоким — «пул готов» подаёт сам depth_book.rs (on_rise), кто бы ни посчитал первым.
    let _ = p.reg.depth().depth(&p.ctx, &p.reg, &p.tokens, &e, dec);
}

/// Полоса цены вышла за прочитанные слова битмапа (решение 73): цена ушла от тика, вокруг которого карта читалась, и
/// глубина считалась бы не целиком — карту пора перечитать вокруг нового тика, не дожидаясь переподключения
/// (pools/listen_log.rs). Края полосы — в пределах TickMath. Не V3/CL, нет карты, полоса не задана — false. Ошибок нет.
pub fn off_map(ctx: &super::super::ctx::Ctx, e: &super::registry::Entry) -> bool {
    use super::super::math::tick_math::{MAX_TICK, MIN_TICK};
    use super::super::quote::depth_band::band_ticks;
    use crate::core::types::PoolState;
    use rust_decimal::prelude::ToPrimitive;
    let Some(map) = e.ticks.as_ref() else {
        return false;
    };
    let tick = match e.pool.state {
        PoolState::V3 { tick, .. } | PoolState::Cl { tick, .. } => tick,
        _ => return false,
    };
    let Some(b) = ctx
        .dec("dex.depth_band_pct")
        .ok()
        .and_then(|p| (p / rust_decimal::Decimal::ONE_HUNDRED).to_f64())
    else {
        return false;
    };
    let (Some(up), Some(down)) = (band_ticks(b, true), band_ticks(b, false)) else {
        return false;
    };
    let d = up.max(down);
    let lo = tick.saturating_sub(d).clamp(MIN_TICK, MAX_TICK);
    let hi = tick.saturating_add(d).clamp(MIN_TICK, MAX_TICK);
    !(map.covers(lo) && map.covers(hi))
}
