//! Файл: core/bound_in_parts.rs
//! Что делает: решение 361 — разбор отдельных входов границы прибыли для сборки (bound_in.rs assemble): ставка комиссии
//! тейкера по монете списания, уровни стакана, верх курса DEX, верх прихода на B (вывод с A с поправкой по фактам и
//! налогом перевода), постоянные затраты (возврат USDT). Правило для всех: вход, на котором граница могла бы оказаться
//! НИЖЕ полного расчёта (отрицательная затрата, которую расчёт вычел бы как доход, а граница не учитывает), — ошибка:
//! метод не действует, кандидат идёт в полный расчёт (ревью t162, Н1, Н2; разбор — docs/notes/t162-review-fixes-2026-11-17.md).
//! Выделено из bound_in.rs (ревью t162, Н10: файл ≤ 200 строк).
//! Кто вызывает: core/bound_in.rs.
//! От чего зависит: bound.rs (Arrive), bound_in.rs (Seen), dirctx.rs, core/types.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::PriceLevel;

use super::bound::Arrive;
use super::bound_in::Seen;
use super::dirctx::DirCtx;

/// Что делает: ставка комиссии → (в котируемой от суммы, в монете от количества). Принимает: контекст, ставку, «от
/// количества», монету списания, тикер монеты, сторону (true — продажа на B). Иная валюта: ставка ≥ 0 — не вычитается.
/// Продажа в монете (costs.rs: затрата стадии B в USDT по цене A): ставка ≥ 0 — не вычитается (вызывающий берёт только
/// котируемую часть). Ошибки: ставка < 0 в иной валюте или у продажи в монете; «в монете» не от количества.
pub(super) fn split_fee(
    cx: &DirCtx,
    rate: Decimal,
    per_coin: bool,
    asset: &str,
    ticker: &str,
    sell: bool,
) -> Result<(Decimal, Decimal), String> {
    if asset == ticker {
        if !per_coin {
            return Err("комиссия в монете не от количества".into());
        }
        // Ревью t162, Н2: у продажи отрицательная комиссия в монете уменьшила бы затраты расчёта, а граница её не видит.
        if sell && rate < Decimal::ZERO {
            return Err(format!("ставка комиссии продажи в монете {rate} < 0"));
        }
        return Ok((Decimal::ZERO, rate));
    }
    if cx.quote.iter().any(|q| q == asset) {
        return Ok((rate, Decimal::ZERO));
    }
    if rate.is_sign_negative() {
        return Err(format!("ставка комиссии < 0 в {asset}"));
    }
    Ok((Decimal::ZERO, Decimal::ZERO))
}

/// Что делает: уровни стакана: цена > 0 и количество ≥ 0. Принимает: уровни. Возвращает: копию. Ошибки: текст.
pub(super) fn levels(l: &[PriceLevel]) -> Result<Vec<PriceLevel>, String> {
    if l.iter()
        .any(|x| x.price <= Decimal::ZERO || x.qty.is_sign_negative())
    {
        return Err("уровень стакана с ценой ≤ 0 или отрицательным количеством".into());
    }
    if l.is_empty() {
        return Err("стакан пуст".into());
    }
    Ok(l.to_vec())
}

/// Что делает: верх курса DEX > 0. Принимает: верх. Возвращает: его же. Ошибки: текст.
pub(super) fn positive(r: Decimal) -> Result<Decimal, String> {
    if r <= Decimal::ZERO {
        return Err("верх курса DEX ≤ 0".into());
    }
    Ok(r)
}

/// Что делает: верх прихода (шапка bound_in.rs): биржа A — фикс (с поправкой по фактам), доля, налог перевода; DEX A —
/// приход ≤ q. Принимает: контекст, факт перевода. Возвращает: Arrive. Ошибки: комиссия вывода неизвестна, доля вне
/// [0, 1), налог > 100 %, фикс < 0 без поправки по фактам (ревью t162, Н1).
pub(super) fn arrive(cx: &DirCtx, seen: Option<&Seen>) -> Result<Arrive, String> {
    let Some(info) = &cx.a.info else {
        return Ok(Arrive {
            pct: Decimal::ZERO,
            fix: Decimal::ZERO,
            keep: Decimal::ONE,
        });
    };
    let fix = info.withdraw_fee.ok_or("комиссия вывода A неизвестна")?;
    let pct = info.withdraw_fee_pct.unwrap_or_default();
    if pct.is_sign_negative() || pct >= Decimal::ONE {
        return Err(format!("доля комиссии вывода {pct} вне [0, 1)"));
    }
    let t = cx.transfer_tax.unwrap_or_default();
    let keep = Decimal::ONE - t / Decimal::ONE_HUNDRED;
    if keep.is_sign_negative() {
        return Err(format!("налог перевода {t}% больше 100%"));
    }
    // withdraw_seen.rs model: поправка — только если комиссия справочника на S известна и ожидание на S > 0; там
    // комиссия на A = max(0, fee_for(A) + F − fee_for(S)) — обрезка нулём повторяется минимумом в arr⁺.
    let adjusted = match seen.and_then(|s| info.fee_for(s.sent).map(|d| (s, d))) {
        Some((s, dir_s)) if (s.sent - s.fee) * keep > Decimal::ZERO => Some(fix + s.fee - dir_s),
        _ => None,
    };
    let w = match adjusted {
        Some(w) => w,
        // Ревью t162, Н1: без поправки fee_for = фикс + доля·A не обрезается нулём — при фиксе < 0 приход расчёта
        // больше A, а arr⁺ режет его до A: граница была бы ниже расчёта.
        None if fix < Decimal::ZERO => {
            return Err(format!(
                "фикс комиссии вывода A {fix} < 0 без поправки по фактам"
            ));
        }
        None => fix,
    };
    Ok(Arrive { pct, fix: w, keep })
}

/// Что делает: постоянные затраты: фикс возврата USDT с биржи B в котируемой (режим per_trade); иначе 0. Принимает:
/// контекст. Возвращает: USDT. Ошибки: возврат с биржи без сети USDT, комиссии или с долей < 0 (полный расчёт тоже не
/// пройдёт), фикс < 0 в иной валюте. Фикс < 0 в котируемой — строго: расчёт вычтет фикс + доля·сумма ≥ фикс.
pub(super) fn fixed(cx: &DirCtx) -> Result<Decimal, String> {
    if !cx.return_per_trade || cx.b.info.is_none() {
        return Ok(Decimal::ZERO);
    }
    let info = cx.usdt_b.as_ref().ok_or("нет сети возврата USDT у B")?;
    let fix = info
        .withdraw_fee
        .ok_or("комиссия возврата USDT неизвестна")?;
    if info.withdraw_fee_pct.is_some_and(|p| p.is_sign_negative()) {
        return Err("доля комиссии возврата < 0".into());
    }
    if cx.quote.iter().any(|q| q == &info.asset.ticker) {
        return Ok(fix);
    }
    // Иная валюта — затрата по живой цене ≥ 0 при фиксе ≥ 0: не вычитается.
    if fix.is_sign_negative() {
        return Err("фикс возврата < 0 в иной валюте".into());
    }
    Ok(Decimal::ZERO)
}
