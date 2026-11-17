//! Файл: core/price_band.rs
//! Что делает: решение 390 (задача t180) — ценовой коридор лимитного ордера биржи в расчёте сделки. Биржа отклоняет
//! ВЕСЬ лимитный ордер, если его цена за коридором (покупка — дороже верхней границы, продажа — дешевле нижней), хотя
//! ближние уровни стакана в коридор входили. Раньше коридор узнавали только отказом (PriceOutOfRange): на сверхспреде
//! подбор объёма шёл по стакану глубоко, лимит выходил за коридор — сделка закрывалась целиком. Теперь:
//! - read — перед покупкой (пересчёт предпроверки, precheck.rs) у сторон-бирж, у пары которых в справочнике есть
//!   правило коридора (SpotPairState.price_band), границы читаются у адаптера (контракт price_band — по документации
//!   биржи: опорная цена и формула у каждой своя) — обе стороны одновременно;
//! - check — в расчёте каждого объёма (costs.rs): худшая цена ордера (лимит) за коридором «дальше» — Fail::TooLarge
//!   (меньший объём даёт цену ближе — подбор берёт ту часть стакана, что внутри коридора, прибыль — на этом объёме);
//!   «не с той стороны» (покупка ниже нижней границы, продажа выше верхней) — Fail::Stop (меньший объём не поможет).
//!
//! Коридор не прочитан (сбой запроса, площадка не умеет) — расчёт как раньше, без коридора (раньше его не было вовсе;
//! отказ биржи по коридору денег не трогает), причина — строкой в журнал предпроверки и хронологию сделки.
//! Кто вызывает: core/precheck.rs (read), core/costs.rs (check).
//! От чего зависит: gateway/price_band.rs (PriceBand, Miss), dirctx.rs (DirCtx: пары, band_a / band_b), deps.rs (шлюз),
//! venue_ticker.rs (тикер площадки), size.rs (Fail).
//! Настройки: нет (границы — только из API бирж).

use rust_decimal::Decimal;

use crate::core::types::{Direction, Side, VenueId};
use crate::gateway::events::SpotPairState;
use crate::gateway::price_band::{Miss, PriceBand};

use super::dirctx::DirCtx;
use super::engine::Core;
use super::size::Fail;

/// Что делает: цена ордера стороны внутри коридора. Принимает: площадку (для текста), коридор (None — нет), сторону,
/// худшую цену ордера. Ошибки: TooLarge — «дальше» коридора (меньший объём может подойти); Stop — «не с той стороны».
pub fn check(
    venue: &str,
    band: Option<&PriceBand>,
    side: Side,
    price: Decimal,
) -> Result<(), Fail> {
    let Some(b) = band else { return Ok(()) };
    let what = match side {
        Side::Buy => "покупки",
        Side::Sell => "продажи",
    };
    match b.miss(side, price) {
        None => Ok(()),
        Some(Miss::Far(limit)) => Err(Fail::TooLarge(format!(
            "{venue}: худшая цена {what} {} за ценовым коридором биржи {} ({})",
            price.normalize(),
            limit.normalize(),
            b.note
        ))),
        Some(Miss::Wrong(limit)) => Err(Fail::Stop(format!(
            "{venue}: цена {what} {} по другую сторону ценового коридора биржи {} ({}) — меньший объём не поможет",
            price.normalize(),
            limit.normalize(),
            b.note
        ))),
    }
}

/// Прочитанные коридоры сторон и строки для журнала / хронологии.
#[derive(Default)]
pub struct Bands {
    pub a: Option<Box<PriceBand>>,
    pub b: Option<Box<PriceBand>>,
    /// «Коридор цены mexc: покупка ≤ 0.0012 (MEXC …)», «коридор цены xt не прочитан: …».
    pub lines: Vec<String>,
}

impl Bands {
    /// Есть ли хоть один коридор. Ошибок нет.
    pub fn any(&self) -> bool {
        self.a.is_some() || self.b.is_some()
    }
}

/// Нужно ли читать коридор у стороны: у пары биржи есть правило коридора. Принимает: правила пары (None — DEX).
/// Ошибок нет.
pub fn wanted(pair: Option<&SpotPairState>) -> bool {
    pair.is_some_and(|p| p.price_band.is_some())
}

/// Что делает: коридоры обеих сторон сейчас — запросы к биржам одновременно, только у сторон с правилом коридора.
/// Принимает: ядро, направление, контекст (правила пар A и B). Возвращает: Bands (без коридора — пусто). Ошибок нет:
/// сбой чтения — строка в lines, у стороны коридора нет (расчёт как раньше).
pub async fn read(core: &Core, d: &Direction, cx: &DirCtx) -> Bands {
    let a = side(core, d, &d.buy_on, Side::Buy, wanted(cx.pair_a.as_ref()));
    let b = side(core, d, &d.sell_on, Side::Sell, wanted(cx.pair_b.as_ref()));
    let ((a, la), (b, lb)) = tokio::join!(a, b);
    Bands {
        a,
        b,
        lines: la.into_iter().chain(lb).collect(),
    }
}

/// Коридор одной стороны. Принимает: ядро, направление, площадку, сторону сделки на ней, нужно ли читать.
/// Возвращает: (коридор, строка для журнала). Ошибок нет.
async fn side(
    core: &Core,
    d: &Direction,
    v: &VenueId,
    s: Side,
    want: bool,
) -> (Option<Box<PriceBand>>, Option<String>) {
    if !want {
        return (None, None);
    }
    let on = super::venue_ticker::on_venue(core, &d.asset, v);
    let got = match core.inner.deps.gw.get(v).await {
        Ok(gw) => gw.price_band(&on).await,
        Err(e) => Err(e),
    };
    match got {
        Ok(Some(b)) if !b.is_empty() => {
            let line = line(v, s, &b);
            (Some(Box::new(b)), Some(line))
        }
        Ok(_) => (
            None,
            Some(format!("коридор цены {}: сейчас не действует", v.0)),
        ),
        Err(e) => (
            None,
            Some(format!(
                "коридор цены {} не прочитан ({:?}: {}) — расчёт без коридора",
                v.0, e.code, e.message
            )),
        ),
    }
}

/// Строка коридора стороны для журнала. Принимает: площадку, сторону сделки, коридор. Ошибок нет.
pub fn line(v: &VenueId, s: Side, b: &PriceBand) -> String {
    let (lo, hi, what) = match s {
        Side::Buy => (b.buy_min, b.buy_max, "покупка"),
        Side::Sell => (b.sell_min, b.sell_max, "продажа"),
    };
    let f = |x: Option<Decimal>| x.map_or("—".to_string(), |x| x.normalize().to_string());
    format!(
        "коридор цены {}: {what} от {} до {} ({})",
        v.0,
        f(lo),
        f(hi),
        b.note
    )
}

/// Что делает: контекст направления для пересчёта предпроверки с коридорами (precheck.rs). Читает коридоры только у
/// сторон с правилом коридора; ни одного не прочитано — контекста нет (None: пересчёт соберёт его сам, как раньше).
/// Принимает: ядро, направление. Возвращает: (контекст с band_a / band_b или None, прочитанное). Ошибок нет: контекст
/// не собран — None (ту же причину отказа покажет пересчёт).
pub async fn context(core: &Core, d: &Direction) -> (Option<Box<DirCtx>>, Bands) {
    let Ok(mut cx) = DirCtx::build(core, d) else {
        return (None, Bands::default());
    };
    if !wanted(cx.pair_a.as_ref()) && !wanted(cx.pair_b.as_ref()) {
        return (None, Bands::default());
    }
    let mut bands = Box::pin(read(core, d, &cx)).await;
    if !bands.any() {
        return (None, bands);
    }
    cx.band_a = bands.a.take();
    cx.band_b = bands.b.take();
    (Some(Box::new(cx)), bands)
}
