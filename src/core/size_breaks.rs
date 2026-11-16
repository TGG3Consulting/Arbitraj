//! Файл: core/size_breaks.rs
//! Что делает: решение 344 (задача t159) — изломы стороны DEX в точки подбора объёма (решение 66). Сейчас изломы
//! подбора — только уровни стаканов бирж (size_curve.rs params: buy_breaks — asks A, sell_breaks — bids B); у
//! DEX-стороны изломов нет, и пик прибыли, целиком лежащий между двумя точками сетки, подбор не видит. Здесь (при
//! size.pool_breaks_max) площадка DEX через шлюз (контракт quote_breaks) отдаёт объёмы монеты, на которых у пулов
//! монеты меняется кусок кривой (тик, бин, точка лесенки), — они добавляются к изломам своей стороны: DEX на A — к
//! buy_breaks (объём покупки), DEX на B — к sell_breaks (объём продажи; в объём покупки их переводит size_points.rs по
//! приходу, как уровни bids). Ядро о пулах не знает — получает только объёмы (стык шлюза).
//! Сбой запроса изломов — подбор как без них (это только точки, не данные решения).
//! Кто вызывает: core/profit.rs.
//! От чего зависит: core/dex_account.rs (площадка на счёт направления, решение 132), core/venue_ticker.rs, gateway
//! (контракт quote_breaks), core/size.rs.
//! Настройки: size.pool_breaks_max (через profit.rs, size_extra.rs flags).

use rust_decimal::Decimal;

use crate::core::types::Side;

use super::dirctx::DirCtx;
use super::engine::Core;
use super::size::Params;

/// Что делает: добавляет изломы сторон DEX направления к изломам подбора. Принимает: ядро, контекст направления,
/// параметры поиска (изменяются), сколько изломов на пул. Наибольший объём запроса — удвоенная оценка объёма по
/// бюджету (Params.guess). Ошибок нет: сбой — без изломов этой стороны.
pub async fn add(core: &Core, cx: &DirCtx, p: &mut Params, per_pool: u32) {
    let d = &cx.dir;
    let deps = &core.inner.deps;
    let max_qty = p.guess.max(Decimal::ZERO) * Decimal::TWO;
    for (venue, buy) in [(&d.buy_on, true), (&d.sell_on, false)] {
        // Решение 132: DEX — кошелёк строки маппинга направления; у биржи контракт отдаёт пусто.
        let Ok(v) = super::dex_account::venue_dir(deps, d, venue).await else {
            continue;
        };
        let on = super::venue_ticker::on_venue(core, &d.asset, venue);
        let side = if buy { Side::Buy } else { Side::Sell };
        let Ok(breaks) = v.quote_breaks(&on, side, max_qty, per_pool).await else {
            continue;
        };
        if breaks.is_empty() {
            continue;
        }
        let target = if buy {
            &mut p.buy_breaks
        } else {
            &mut p.sell_breaks
        };
        target.extend(breaks);
        target.sort_unstable();
        target.dedup();
    }
}
