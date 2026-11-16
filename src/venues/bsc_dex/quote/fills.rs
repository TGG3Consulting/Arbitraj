//! Файл: venues/bsc_dex/quote/fills.rs
//! Что делает: исполнимость объёма продажи без полной котировки (решение 250) — для проверки доли глубины h ядра
//! (core/costs.rs: котировка на sell_qty / h нужна ядру только ради fillable_qty). Полная котировка строит классику
//! (прямой режим: лучший одиночный пул против пары через промежуточную, решения 29, 52), а при dex.router_mode = graph
//! ещё и граф со сплитом (решение 80) — самое дорогое. Здесь — только «один пул» классики: тот же direct::best по тем же
//! пулам (с USDT, исполнимые, не форк — classic_direct::tradable). Если лучший одиночный пул исполняет объём целиком
//! (complete), полная котировка ТОЧНО дала бы fillable_qty = qty (факт кода):
//! - direct::best ставит полный обмен выше неполного (direct.rs better) → у выбранного пула монет = qty;
//! - direct_route: больший fillable из «один пул» / «через промежуточную» (pick.rs), ошибка второго — первый;
//! - classic_direct: при пулах «только сигнал» котировка-на-сделку — та же исполнимая;
//! - граф (routing.rs merge): графовый маршрут берётся только при большем или равном fillable (pick.rs), сбой графа —
//!   классика;
//!   иначе — полная котировка, как раньше. Проверки до выбора маршрута — как у Quoting::quote/priced и в том же
//!   порядке (режим, свежесть блока и ленты событий, налоги, USDT, адрес монеты, decimals монеты и USDT).
//!
//! Не входит в быстрый ответ (полная котировка как раньше): умный режим dex.swap_mode = smart (классика там — умный
//! маршрут против прямой Uniswap), покупка, нативная BNB (газ обёртки — quote/native_gas.rs). Чего быстрый ответ не
//! делает (ядру на доле глубины нужна только исполнимость): decimals промежуточных, цена газа, затраты маршрута
//! (газ разрешений, чаевые) на объёме sell_qty / h — их ошибки (узел, «газ разрешений не измерен») на этой точке
//! проявит котировка самой продажи на sell_qty, которая идёт следом (core/costs.rs) и проходит те же шаги; отличие —
//! только у ошибки, которая есть на sell_qty / h и нет на sell_qty (разрешение кошелька меньше sell_qty / h, но не
//! меньше sell_qty, и газ разрешений не измерен): раньше отказ «расчёт невозможен», теперь — нет (решение 250).
//! Решение 312: замер быстрого пути (quote/quote_timing_outer.rs begin_fast) — запись dex.quote_timing с fast = да:
//! всё время, decimals монеты и USDT, лучший одиночный пул, остаток; answered — ответил ли быстрый путь. Пишется только
//! у попытки (продажа, dex.swap_mode = direct); не ответил — следом своя запись полной котировки (priced).
//! Кто вызывает: venue.rs (контракт quote_fills).
//! От чего зависит: quote/mod.rs (Quoting), quote/direct.rs (best, taxes), quote/classic_direct.rs (tradable),
//! exec/native.rs, send/mod.rs (planned_floor), pools/registry.rs, tokens.rs, quote/quote_timing_outer.rs (замер).
//! Настройки: dex.swap_mode, venue.quote_asset [bsc_dex], feed.stale_after_ms; log.quote_timing,
//! log.quote_timing_min_ms (вся система, решение 312); через вызываемые — как у quote.

use rust_decimal::Decimal;

use crate::core::types::{Asset, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::quote_timing_outer::{Outcome, Outer};
use super::quote_timing_stages::Part;
use super::{Quoting, classic_direct, direct};

impl Quoting {
    /// Что делает: исполнимое количество объёма (решение 250): быстрый ответ fast — объём целиком; иначе fillable_qty
    /// полной котировки. Принимает: монету, сторону, количество. Возвращает: исполнимое количество.
    /// Ошибки: как у quote (режим, свежесть блока и ленты, налоги, узел, NoRoute).
    pub async fn fills(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Decimal> {
        // Порядок проверок как у quote: режим, свежесть блока и ленты событий.
        self.swap_mode()?;
        self.heads.fresh(&self.ctx)?;
        self.lag.check(&self.ctx)?;
        if fast(self, asset, side, qty).await? {
            return Ok(qty);
        }
        self.priced(asset, side, qty).await.map(|q| q.fillable_qty)
    }
}

/// Что делает: быстрый ответ (решение 250) — исполнит ли лучший одиночный пул классики объём продажи целиком.
/// Принимает: котировку, монету, сторону, количество. Возвращает: true — полная котировка дала бы fillable_qty = qty;
/// false — неизвестно (нужна полная котировка). Ошибки: те же и в том же порядке, что у Quoting::priced до выбора
/// маршрута (налоги, USDT, адрес монеты, decimals монеты и USDT).
pub async fn fast(q: &Quoting, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<bool> {
    let ctx = &q.ctx;
    if side != Side::Sell || ctx.text("dex.swap_mode").ok().as_deref() != Some("direct") {
        return Ok(false);
    }
    // Решение 312: замер попытки — одно чтение log.quote_timing; выключен — пустой, часов нет.
    let mut tm = Outer::begin_fast(ctx, &asset.ticker, side, qty);
    let out = fast_at(q, asset, side, qty, &mut tm).await;
    let mut o = Outcome::of(&out);
    o.answered = out.as_ref().ok().copied();
    tm.finish(o);
    out
}

/// Попытка быстрого ответа (продажа, прямой режим) под замером `tm` (решение 312: только часы вокруг шагов).
/// Принимает, возвращает и ошибки: как fast.
async fn fast_at(
    q: &Quoting,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    tm: &mut Outer,
) -> GatewayResult<bool> {
    let ctx = &q.ctx;
    // Предварительные шаги priced — те же и в том же порядке (их ошибки — ошибки полной котировки).
    direct::taxes(q, asset)?;
    let usdt = q.tokens.usdt(ctx)?;
    let coin = asset.bsc_contract.ok_or_else(|| {
        ctx.err(
            ErrorCode::NoRoute,
            format!("{}: нет адреса контракта в BSC", asset.ticker),
        )
    })?;
    if super::super::exec::native::is_native(ctx, asset) {
        return Ok(false);
    }
    let t = tm.now();
    let coin_dec = q.tokens.decimals(ctx, coin).await?;
    let usdt_dec = q.tokens.decimals(ctx, usdt).await?;
    tm.add(Part::Decimals, t);
    // «Один пул» классики прямого режима: те же пулы, что у classic_direct (исполнимые, не форк).
    let pools: Vec<_> = q
        .reg
        .by_pair(coin, usdt)
        .into_iter()
        .filter(classic_direct::tradable)
        .collect();
    if pools.is_empty() {
        return Ok(false);
    }
    let t = tm.now();
    let best = direct::best(q, asset, side, qty, pools, coin_dec, usdt_dec);
    tm.add(Part::OnePool, t);
    Ok(best.is_ok_and(|(c, _)| c.swapped.complete))
}
