//! Файл: core/bound_in.rs
//! Что делает: решение 361 — сборка границы прибыли (bound.rs) из тех же входов, что полный расчёт (costs.rs), без
//! котировок. Сверка статей с кодом — docs/notes/t162-proven-sieve-2026-11-17.md, раздел 1:
//! - стакан биржи и план комиссии тейкера — из снимка адаптера (контракт quote_snapshot: тот же стакан и та же формула
//!   FrozenBook::price, что у живой котировки); ставка — uncapped_replay_plan.rs fee_of (как расчёт «без потолка»):
//!   комиссия в монете — от количества (уменьшает вывод, costs.rs to_send), в котируемой — от суммы (затрата в USDT;
//!   котируемые монеты convert.rs считает равными USDT); в иной валюте — затрата ≥ 0 при ставке ≥ 0 (не вычитается),
//!   ставка < 0 — граница не строится; комиссия ПРОДАЖИ в монете — затрата ≥ 0 при ставке ≥ 0 (не вычитается), ставка < 0
//!   — граница не строится (ревью t162, Н2: расчёт вычел бы отрицательную затрату — доход);
//! - вывод с биржи A: фикс + доля (CoinNetworkInfo::fee_for), налог перевода t (DirCtx.transfer_tax, налог с (A − комиссия)),
//!   поправка по фактам (withdraw_seen.rs model: фикс → фикс + F − fee_for(S) при тех же условиях, недостача ≥ 0 только
//!   уменьшает приход) ⇒ приход ≤ min(A, A(1 − pct) − W_eff)·(1 − t/100); фикс < 0 без поправки по фактам — граница не
//!   строится (ревью t162, Н1: fee_for без поправки не обрезает комиссию нулём — приход больше A); у DEX на A налог
//!   перевода монеты ≥ 0 (проверено адаптером) ⇒ приход ≤ q;
//! - запас: reserve.rs amount ≥ k·σ·√T·выручка (доля закрытия только увеличивает) ⇒ κ_B = max(0, 1 − k·σ·√T − r_B);
//! - возврат USDT с биржи B (usdt_return.mode = per_trade): fee_for(сумма) ≥ фикс при доле ≥ 0 ⇒ F = фикс; с DEX — газ ≥ 0.
//!
//! Газ, разрешения, чаевые, остаток шага, доля глубины h и минимумы — не вычитаются (≥ 0 или только сужают допустимые
//! объёмы). Разбор отдельных входов (ставки, стакан, приход, постоянные) — bound_in_parts.rs.
//! Кто вызывает: core/sieve.rs, тесты.
//! От чего зависит: bound.rs, bound_in_parts.rs, dirctx.rs, uncapped_replay_plan.rs (fee_of), core/types,
//! gateway/rate_bound.rs.
//! Настройки: нет своих (transfer.seen_cost читает вызывающий — признак see в Withdraw).

use rust_decimal::{Decimal, MathematicalOps};

use crate::core::types::{OrderBook, Quote};
use crate::gateway::rate_bound::DexBound;

use super::bound::{Bound, Buy, Sell};
use super::bound_in_parts::{arrive, fixed, levels, positive, split_fee};
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
/// (комиссия вывода неизвестна, фикс вывода < 0 без поправки по фактам, доля ≥ 1, налог > 100 %, ставка < 0 в иной
/// валюте или у продажи в монете, уровень с ценой ≤ 0, обе стороны DEX).
pub fn assemble(cx: &DirCtx, a: &SideIn, b: &SideIn, seen: Option<&Seen>) -> Result<Bound, String> {
    let buy = match a {
        SideIn::Book {
            book,
            sample,
            ticker,
        } => {
            let f = super::uncapped_replay_plan::fee_of(sample, ticker, &cx.quote[0])?;
            let (q, c) = split_fee(cx, f.rate, f.per_coin, &f.asset, ticker, false)?;
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
            let (q, _) = split_fee(cx, f.rate, f.per_coin, &f.asset, ticker, true)?;
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
