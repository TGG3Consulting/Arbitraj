//! Файл: venues/bsc_dex/pools/depth.rs
//! Что делает: делит пулы пачки на «глубокие» (все события в подписке) и «мелкие» — мельче
//! dex.min_pool_liquidity_usd или пустые: у них в подписке только добавление ликвидности (pools/group.rs).
//! Каждое уведомление узла BSC платное, а мелкий пул не котируется, не сверяется и не участвует в пробе
//! (quote/select.rs, deep_enough) — его обмены слушать незачем. Ликвидность растёт только событием добавления:
//! оно будит пул — перечитывание, и если пул стал глубоким, он возвращается в полную подписку (pools/admit.rs).
//! Цена без добавления ликвидности глубину USDT-пула заметно не меняет; обратный переход (глубокий → мелкий)
//! — при следующем переподключении пачки, без отдельного опроса.
//! Глубину не посчитать (не задана настройка, нет курса промежуточной монеты, пул не прочитан) — пул глубокий:
//! слушаем всё, как раньше, причина — одной записью в журнал.
//! Кто вызывает: pools/admit.rs.
//! От чего зависит: quote/select.rs (deep_enough), quote/emptiness.rs, registry.rs, progress.rs (once), tokens.rs.
//! Настройки: dex.min_pool_liquidity_usd [bsc_dex].

use crate::core::types::PoolRef;
use crate::storage::journal::Level;

use super::super::quote::{emptiness, select};
use super::listen::Pools;

/// Глубокий ли пул (см. шапку). Принимает: окружение, ссылку на прочитанный пул. Возвращает: true — слушать всё;
/// false — только добавление ликвидности. Пул без записи в реестре — глубокий. Ошибок нет.
pub async fn deep(p: &Pools, r: &PoolRef) -> bool {
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
    let res = dec.and_then(|d| select::deep_enough(&p.ctx, &p.reg, &p.tokens, &probe, d));
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
