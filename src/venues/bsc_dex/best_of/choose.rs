//! Файл: venues/bsc_dex/best_of/choose.rs
//! Что делает: чистый выбор «лучший из двух» (решение 123): котировка KyberSwap против своей → кто исполняет и почему.
//! Без запросов к сети, без состояния: две котировки (или ошибки) и их затраты вне цены в USDT → решение и текст для
//! журнала, «Кандидатов» и хронологии сделки («KyberSwap 24.15 vs свой 24.21 USDT за 10 CAKE → свой (+0.06)»).
//! Правила. Своя годится, только если маршрут исполним как есть (DexRoute.executable: прямой путь, Universal Router,
//! бандл, исполнитель — решения 84, 122); на сделке обе — не хуже req.worst_price (худшая цена ядра). Ошибка одной
//! стороны (лимит 429, срок, нет маршрута, неисполнима) — берётся другая; обеих — отказ. Обе годны: сначала полнота
//! (fillable ≥ qty лучше неполной), затем итог в USDT за одно количество (меньшее из исполнимых) с затратами вне цены
//! (газ, approve, чаевые): покупка — меньше отдадим, продажа — больше получим; равенство — своя (Тигран 03.11).
//! Затраты вне цены одной стороны не переведены в USDT (нет курса BNB) — обе сравниваются без них, с пометкой.
//!
//! Кто вызывает: best_of/quote.rs (финальная котировка), best_of/trade.rs (на сделке); tests/bsc_dex_best_of.rs.
//! От чего зависит: core/types.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::{Quote, Side};

/// Кто исполняет своп.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    Kyber,
    Own,
}

impl Who {
    /// Имя для текстов: «KyberSwap» / «свой». Ошибок нет.
    pub fn label(self) -> &'static str {
        match self {
            Who::Kyber => "KyberSwap",
            Who::Own => "свой",
        }
    }
}

/// Предложение одной стороны: котировка и её затраты вне цены в USDT (None — не переведены, нет курса BNB).
#[derive(Debug, Clone, Copy)]
pub struct Offer<'a> {
    pub quote: &'a Quote,
    pub extra_usdt: Option<Decimal>,
}

/// Итог выбора.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub who: Who,
    /// Почему — текст для журнала и карточки.
    pub why: String,
    /// На сколько USDT победитель лучше (за одно количество). None — у другой стороны ошибка или неполный объём.
    pub gain_usdt: Option<Decimal>,
}

/// Годна ли своя котировка на сделку: маршрут есть и исполним как есть (решение 84). Ошибки: текст причины.
pub fn usable_own(q: &Quote) -> Result<(), String> {
    match &q.route {
        None => Err("нет маршрута DEX в котировке".to_string()),
        Some(r) if !r.executable => Err(format!(
            "маршрут не исполним как есть: {}",
            r.exec_note.as_deref().unwrap_or("нужен бандл")
        )),
        Some(_) => Ok(()),
    }
}

/// Не хуже ли средняя цена худшей цены ядра: покупка — не выше, продажа — не ниже. Ошибки: текст причины.
pub fn within_worst(q: &Quote, worst: Decimal) -> Result<(), String> {
    let bad = match q.side {
        Side::Buy => q.avg_price > worst,
        Side::Sell => q.avg_price < worst,
    };
    if bad {
        return Err(format!(
            "средняя {} хуже допустимой {worst}",
            r(q.avg_price)
        ));
    }
    Ok(())
}

/// Выбор. Принимает: сторону, худшую цену ядра (на сделке; None — на котировке), предложение KyberSwap и своё (или
/// текст ошибки). Возвращает: Choice. Ошибки: текст — обе стороны не годятся («KyberSwap: …; свой: …»).
pub fn choose(
    side: Side,
    worst: Option<Decimal>,
    kyber: Result<Offer<'_>, String>,
    own: Result<Offer<'_>, String>,
) -> Result<Choice, String> {
    fn price_ok(worst: Option<Decimal>, o: Offer<'_>) -> Result<Offer<'_>, String> {
        match worst {
            Some(w) => within_worst(o.quote, w).map(|_| o),
            None => Ok(o),
        }
    }
    let kyber = kyber.and_then(|o| price_ok(worst, o));
    let own = own
        .and_then(|o| usable_own(o.quote).map(|_| o))
        .and_then(|o| price_ok(worst, o));
    match (kyber, own) {
        (Err(k), Err(o)) => Err(format!("KyberSwap: {k}; свой: {o}")),
        (Ok(_), Err(o)) => Ok(Choice {
            who: Who::Kyber,
            why: format!("свой не годится ({o}) → KyberSwap"),
            gain_usdt: None,
        }),
        (Err(k), Ok(_)) => Ok(Choice {
            who: Who::Own,
            why: format!("KyberSwap не годится ({k}) → свой"),
            gain_usdt: None,
        }),
        (Ok(k), Ok(o)) => Ok(compare(side, k, o)),
    }
}

/// Обе годны: полнота, затем итог в USDT за одно количество. Ошибок нет.
fn compare(side: Side, k: Offer<'_>, o: Offer<'_>) -> Choice {
    let full = |x: &Offer<'_>| x.quote.fillable_qty >= x.quote.qty;
    let ticker = &o.quote.asset.ticker;
    let partial = |who: Who, x: &Offer<'_>, other: Who| Choice {
        who,
        why: format!(
            "{} исполняет только {} из {} {ticker} → {}",
            other.label(),
            x.quote.fillable_qty.normalize(),
            x.quote.qty.normalize(),
            who.label()
        ),
        gain_usdt: None,
    };
    match (full(&k), full(&o)) {
        (true, false) => return partial(Who::Kyber, &o, Who::Own),
        (false, true) => return partial(Who::Own, &k, Who::Kyber),
        _ => {}
    }
    let vol = k.quote.fillable_qty.min(o.quote.fillable_qty);
    if vol <= Decimal::ZERO {
        let who = if o.quote.fillable_qty >= k.quote.fillable_qty {
            Who::Own
        } else {
            Who::Kyber
        };
        return Choice {
            who,
            why: format!("исполнимый объём у одной стороны 0 → {}", who.label()),
            gain_usdt: None,
        };
    }
    // Затраты вне цены — только если переведены у обеих (иначе сравнение нечестное — обе без них).
    let with_extra = k.extra_usdt.is_some() && o.extra_usdt.is_some();
    let total = |x: &Offer<'_>| {
        let q = x.quote;
        let extra = if with_extra {
            x.extra_usdt.unwrap_or_default()
        } else {
            Decimal::ZERO
        };
        let gross = q.avg_price * q.fillable_qty;
        let net = match side {
            Side::Buy => gross + extra,
            Side::Sell => gross - extra,
        };
        net / q.fillable_qty * vol
    };
    let (tk, to) = (total(&k), total(&o));
    // Равенство — своя (Тигран 03.11: без лимита запросов, копим живые свопы своего сборщика).
    let own_better = match side {
        Side::Buy => to <= tk,
        Side::Sell => to >= tk,
    };
    let who = if own_better { Who::Own } else { Who::Kyber };
    let gain = (tk - to).abs();
    let gas = if with_extra {
        "с газом"
    } else {
        "без газа: курс BNB не найден"
    };
    Choice {
        who,
        why: format!(
            "KyberSwap {} vs свой {} USDT за {} {ticker} ({gas}) → {} (+{})",
            r(tk),
            r(to),
            vol.normalize(),
            who.label(),
            r(gain)
        ),
        gain_usdt: Some(gain),
    }
}

/// Округление для текстов: 4 знака. Ошибок нет.
fn r(v: Decimal) -> Decimal {
    v.round_dp(4).normalize()
}
