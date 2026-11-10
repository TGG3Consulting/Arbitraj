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
//! Кто вызывает: venue.rs (контракт quote_fills).
//! От чего зависит: quote/mod.rs (Quoting), quote/direct.rs (best, taxes), quote/classic_direct.rs (tradable),
//! exec/native.rs, send/mod.rs (planned_floor), pools/registry.rs, tokens.rs.
//! Настройки: dex.swap_mode, venue.quote_asset [bsc_dex], feed.stale_after_ms; через вызываемые — как у quote.

use rust_decimal::Decimal;

use crate::core::types::{Asset, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};

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
    let coin_dec = q.tokens.decimals(ctx, coin).await?;
    let usdt_dec = q.tokens.decimals(ctx, usdt).await?;
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
    Ok(direct::best(q, asset, side, qty, pools, coin_dec, usdt_dec)
        .is_ok_and(|(c, _)| c.swapped.complete))
}
