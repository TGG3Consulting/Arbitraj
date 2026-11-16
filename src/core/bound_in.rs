//! Файл: core/bound_in.rs
//! Что делает: решение 361 — сборка границы прибыли (bound.rs) из тех же входов, что полный расчёт (costs.rs), без
//! котировок. Сверка статей с кодом — docs/notes/t162-proven-sieve-2026-11-17.md, раздел 1:
//! - стакан биржи и план комиссии тейкера — из снимка адаптера (контракт quote_snapshot: тот же стакан и та же формула
//!   FrozenBook::price, что у живой котировки); ставка — uncapped_replay_plan.rs fee_of (как расчёт «без потолка»):
//!   комиссия в монете — от количества (уменьшает вывод, costs.rs to_send), в котируемой — от суммы (затрата в USDT;
//!   котируемые монеты convert.rs считает равными USDT); в иной валюте — затрата ≥ 0 при ставке ≥ 0 (не вычитается),
//!   ставка < 0 — граница не строится;
//! - вывод с биржи A: фикс + доля (CoinNetworkInfo::fee_for), налог перевода t (DirCtx.transfer_tax, налог с (A − комиссия)),
//!   поправка по фактам (withdraw_seen.rs model: фикс → фикс + F − fee_for(S) при тех же условиях, недостача ≥ 0 только
//!   уменьшает приход) ⇒ приход ≤ min(A, A(1 − pct) − W_eff)·(1 − t/100); у DEX на A налог перевода монеты ≥ 0
//!   (проверено адаптером) ⇒ приход ≤ q;
//! - запас: reserve.rs amount ≥ k·σ·√T·выручка (доля закрытия только увеличивает) ⇒ κ_B = max(0, 1 − k·σ·√T − r_B);
//! - возврат USDT с биржи B (usdt_return.mode = per_trade): fee_for(сумма) ≥ фикс при доле ≥ 0 ⇒ F = фикс; с DEX — газ ≥ 0.
//!
//! Газ, разрешения, чаевые, комиссия продажи в монете, остаток шага, доля глубины h и минимумы — не вычитаются (≥ 0 или
//! только сужают допустимые объёмы).
//! Кто вызывает: core/sieve.rs, тесты.
//! От чего зависит: bound.rs, dirctx.rs, uncapped_replay_plan.rs (fee_of), withdraw_seen.rs, core/types, gateway/rate_bound.rs.
//! Настройки: нет своих (transfer.seen_cost читает вызывающий — признак see в Withdraw).

use rust_decimal::{Decimal, MathematicalOps};

use crate::core::types::{OrderBook, PriceLevel, Quote};
use crate::gateway::rate_bound::DexBound;

use super::bound::{Arrive, Bound, Buy, Sell};
use super::dirctx::DirCtx;

/// Вход одной стороны.
pub enum SideIn<'a> {
    /// Биржа: стакан снимка, котировка снимка (для ставки комиссии), тикер монеты на площадке.
    Book {
        book: &'a OrderBook,
        sample: &'a Quote,
        ticker: &'a str,
    },
    /// DEX: верх курса.
    Dex(&'a DexBound),
}

/// Факт прошлого перевода для поправки вывода (withdraw_seen.rs): фактическая комиссия F и отправлено S. None — поправки
/// нет (transfer.seen_cost не «да», фактов нет или F не сообщена).
pub struct Seen {
    pub fee: Decimal,
    pub sent: Decimal,
}

/// Что делает: граница направления из входов (см. шапку). Принимает: контекст направления, сторону A (покупка) и B
/// (продажа), факт перевода для поправки вывода. Возвращает: Bound. Ошибки: текст — входа нет или он вне допустимого
/// (комиссия вывода неизвестна, доля ≥ 1, налог > 100 %, ставка < 0 в иной валюте, уровень с ценой ≤ 0, обе стороны DEX).
pub fn assemble(cx: &DirCtx, a: &SideIn, b: &SideIn, seen: Option<&Seen>) -> Result<Bound, String> {
    let buy = match a {
        SideIn::Book {
            book,
            sample,
            ticker,
        } => {
            let f = super::uncapped_replay_plan::fee_of(sample, ticker, &cx.quote[0])?;
            let (q, c) = split_fee(cx, f.rate, f.per_coin, &f.asset, ticker)?;
            Buy::Book {
                levels: levels(&book.asks)?,
                fee_quote: q,
                fee_coin: c,
            }
        }
        SideIn::Dex(d) => Buy::Dex {
            rho: positive(d.rho)?,
        },
    };
    let (sell, r_b) = match b {
        SideIn::Book {
            book,
            sample,
            ticker,
        } => {
            let f = super::uncapped_replay_plan::fee_of(sample, ticker, &cx.quote[1])?;
            let (q, _) = split_fee(cx, f.rate, f.per_coin, &f.asset, ticker)?;
            (
                Sell::Book {
                    levels: levels(&book.bids)?,
                    fee_quote: q,
                },
                q,
            )
        }
        SideIn::Dex(d) => (
            Sell::Dex {
                rho: positive(d.rho)?,
            },
            Decimal::ZERO,
        ),
    };
    if matches!((&buy, &sell), (Buy::Dex { .. }, Sell::Dex { .. })) {
        return Err("обе стороны DEX — направления с переводом нет".into());
    }
    let sqrt_t = cx.transit_s.sqrt().ok_or("время в пути не вычисляется")?;
    let c = cx.k * cx.sigma * sqrt_t;
    let kappa = (Decimal::ONE - c - r_b).max(Decimal::ZERO);
    Ok(Bound {
        buy,
        sell,
        arrive: arrive(cx, seen)?,
        kappa,
        fixed: fixed(cx)?,
    })
}

/// Ставка комиссии → (в котируемой от суммы, в монете от количества). Иная валюта: ставка ≥ 0 — не вычитается.
/// Ошибки: ставка < 0 в иной валюте, или «в монете» не от количества.
fn split_fee(
    cx: &DirCtx,
    rate: Decimal,
    per_coin: bool,
    asset: &str,
    ticker: &str,
) -> Result<(Decimal, Decimal), String> {
    if asset == ticker {
        if !per_coin {
            return Err("комиссия в монете не от количества".into());
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

/// Уровни стакана: цена > 0 и количество ≥ 0. Ошибки: текст.
fn levels(l: &[PriceLevel]) -> Result<Vec<PriceLevel>, String> {
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

/// Верх курса DEX > 0. Ошибки: текст.
fn positive(r: Decimal) -> Result<Decimal, String> {
    if r <= Decimal::ZERO {
        return Err("верх курса DEX ≤ 0".into());
    }
    Ok(r)
}

/// Верх прихода (см. шапку): биржа A — фикс (с поправкой по фактам), доля, налог перевода; DEX A — приход ≤ q.
fn arrive(cx: &DirCtx, seen: Option<&Seen>) -> Result<Arrive, String> {
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
    // withdraw_seen.rs model: поправка — только если комиссия справочника на S известна и ожидание на S > 0.
    let w = match seen.and_then(|s| info.fee_for(s.sent).map(|d| (s, d))) {
        Some((s, dir_s)) if (s.sent - s.fee) * keep > Decimal::ZERO => fix + s.fee - dir_s,
        _ => fix,
    };
    Ok(Arrive { pct, fix: w, keep })
}

/// Постоянные затраты: фикс возврата USDT с биржи B в котируемой (режим per_trade); иначе 0. Ошибки: возврат с биржи
/// без сети USDT, комиссии или с долей < 0 (полный расчёт тоже не пройдёт).
fn fixed(cx: &DirCtx) -> Result<Decimal, String> {
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
