//! Файл: core/sieve.rs
//! Что делает: решение 372 — отсев кандидата по ДОКАЗАННОЙ верхней границе прибыли перед тяжёлым расчётом (подбор
//! объёма с котировками DEX). Граница (bound.rs, сборка — bound_in.rs) строится только из памяти: стаканы бирж — снимок
//! адаптера (quote_snapshot), сторона DEX — строгий верх курса адаптера (контракт rate_bound, без котировок),
//! вывод/возврат/запас — из контекста направления (dirctx.rs, те же функции, что расчёт); стороны — sieve_side.rs.
//! Отсекает, только если доказано, что НИ ОДИН объём не пройдёт пороги: max U < money.min_profit_usd или (порог % ≥ 0 и
//! max(U − порог%·вложение) < 0). Метод не действует (кандидат идёт в расчёт как сейчас): внешний исполнитель
//! (exec.via_kyber), проверки до расчёта не пройдены (делистинг, закрытые входы, свежесть налогов — ревью t162, Н9:
//! полный расчёт откажет с НАСТОЯЩЕЙ причиной, отсев её не подменяет), сторона DEX без строгого верха, нет снимка
//! стакана, вход вне допустимого, любой сбой счёта.
//! Ревью t162, Н7: построенный контекст направления (DirCtx — с замером σ) отдаётся дальше (Checked.cx) — полный расчёт
//! прошедшего кандидата не строит его второй раз.
//! Не в потоке тика: вызывается задачей расчёта кандидата (sieve_gate.rs из candidates.rs calc_one); замки рынка не
//! держит (снимки — копии адаптеров; контекст направления — те же короткие чтения, что у расчёта).
//! Кто вызывает: core/sieve_gate.rs, core/sieve_audit.rs (пересчёт границы при принятии аудитом).
//! От чего зависит: bound.rs, bound_best.rs, bound_in.rs, sieve_side.rs, dirctx.rs, venue_dir.rs, profit.rs
//! (final_external), delist_check.rs, withdraw_block.rs, tax_age.rs.
//! Настройки: money.min_profit_usd, money.min_profit_pct (как расчёт, venue_dir.rs), transfer.seen_cost [площадка A];
//! через dirctx.rs — profit.*, usdt_return.mode; через проверки — delist.*, tax.max_age_s.

use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::{Direction, Side};

use super::bound_in::assemble_for_return;
use super::dirctx::DirCtx;
use super::engine::Core;
use super::sieve_side::{seen, side};

/// Итог отсева.
#[derive(Debug, Clone)]
pub enum Sieve {
    /// Доказано: сделки на этих данных нет. Причина с числами и данные (граница, порог, из чего сложилась).
    Cut { reason: String, data: Value },
    /// Граница не исключает сделку — в расчёт.
    Pass,
    /// Метод не действует: класс (ключ счётчика) и причина — в расчёт как сейчас.
    Na { class: &'static str, why: String },
}

/// Итог отсева и построенный контекст направления (None — не строился или не построен).
pub struct Checked {
    pub sieve: Sieve,
    pub cx: Option<Box<DirCtx>>,
}

/// Что делает: отсев кандидата (см. шапку). Принимает: ядро, направление. Возвращает: Checked. Ошибок нет.
pub async fn check(core: &Core, d: &Direction) -> Checked {
    let na = |class: &'static str, why: String| Checked {
        sieve: Sieve::Na { class, why },
        cx: None,
    };
    if super::profit::final_external(core, d) {
        return na(
            "kyber",
            "финальная котировка — у внешнего исполнителя (exec.via_kyber)".into(),
        );
    }
    // Ревью t162, Н9: проверки до расчёта (только память) — первыми; не пройдены — расчёт откажет с их причиной.
    let now = chrono::Utc::now();
    let pre = super::delist_check::check(core, d, now)
        .await
        .and_then(|()| super::withdraw_block::check(core, d))
        .and_then(|()| super::tax_age::check(core, d, now));
    if let Err(e) = pre {
        return na("checks", e);
    }
    let cx = match DirCtx::build(core, d) {
        Ok(c) => Box::new(c),
        Err(e) => return na("no_data", e),
    };
    let sieve = verdict(core, d, &cx).await;
    Checked {
        sieve,
        cx: Some(cx),
    }
}

/// Что делает: граница и решение по готовому контексту (см. шапку). Принимает: ядро, направление, контекст.
/// Возвращает: Sieve. Ошибок нет.
async fn verdict(core: &Core, d: &Direction, cx: &DirCtx) -> Sieve {
    let na = |class: &'static str, why: String| Sieve::Na { class, why };
    if !cx.units_b_per_a.is_identity() {
        return na(
            "settlement_units",
            super::bound_in::UNIT_BOUND_UNAVAILABLE.into(),
        );
    }
    let s = &core.inner.deps.settings;
    let (min_usd, min_pct) = match super::venue_dir::thresholds(s, super::venue_dir::of(core, d)) {
        Ok(t) => t,
        Err(e) => return na("no_data", e),
    };
    let a = match side(core, d, &d.buy_on, Side::Buy).await {
        Ok(x) => x,
        Err((c, w)) => return na(c, w),
    };
    let b = match side(core, d, &d.sell_on, Side::Sell).await {
        Ok(x) => x,
        Err((c, w)) => return na(c, w),
    };
    let (Some(ai), Some(bi)) = (a.as_in(), b.as_in()) else {
        return na("no_data", "снимок без стакана".into());
    };
    let seen = seen(core, cx);
    let may_share_return =
        cx.return_per_trade && cx.b.info.is_some() && super::coin_ride::top_up_on(core);
    let bound = match assemble_for_return(cx, &ai, &bi, seen.as_ref(), may_share_return) {
        Ok(x) => x,
        Err(e) => return na("inputs", e),
    };
    let Some(best) = bound.best(Some(min_pct)) else {
        return na(
            "overflow",
            "граница не вычисляется (переполнение или приход не обратить)".into(),
        );
    };
    let r = |v: Decimal| v.round_dp(6).normalize();
    let mut data = json!({
        "best": best, "min_usd": min_usd, "min_pct": min_pct, "kappa": bound.kappa, "fixed": bound.fixed,
        "arrive": { "pct": bound.arrive.pct, "fix": bound.arrive.fix, "keep": bound.arrive.keep },
        "a": a.describe(), "b": b.describe(),
    });
    let how = format!(
        "κ = {}, фикс вывода {}, постоянные {}; A: {}; B: {}; изломов {}, объём до {}",
        r(bound.kappa),
        r(bound.arrive.fix),
        r(bound.fixed),
        a.describe(),
        b.describe(),
        best.points,
        r(best.q_hi)
    );
    if best.below_usd(min_usd) {
        let reason = format!(
            "отсев по границе: прибыль не больше {} USDT (на {} {}) < порога {min_usd} ({how})",
            r(best.max_u),
            r(best.q_u),
            d.asset.ticker
        );
        data["by"] = json!("usd");
        return Sieve::Cut { reason, data };
    }
    if best.below_pct()
        && let Some(v) = best.max_v
    {
        let reason = format!(
            "отсев по границе: прибыль ниже порога {min_pct}% на любом объёме (запас до порога {} USDT < 0; {how})",
            r(v)
        );
        data["by"] = json!("pct");
        return Sieve::Cut { reason, data };
    }
    Sieve::Pass
}
