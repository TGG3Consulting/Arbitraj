//! Файл: core/credit_event.rs
//! Что делает: сверка события изменения баланса (MarketEvent::Balance) с ожидаемыми зачислениями (решение 76):
//! - своё (own) — ничего;
//! - поступление (Deposit) — по получателю, монете и сумме (credit_match.rs, допуск transfer.credit_amount_tolerance_pct):
//!   ровно одно ожидание — оно отмечено событием и перевод разбужен; закрывает ли событие ожидание —
//!   transfer.credit_by_event получателя (нет — только журнал, решает опрос); подходят несколько — решает опрос;
//! - вывод (Withdraw) с площадки, откуда сейчас идёт перевод этой монеты, — свой;
//! - иначе — журнал core.balance_external «внешнее изменение баланса» без привязки к сделке (ручной ввод и вывод) и
//!   уведомление balance.external.
//!
//! Кто вызывает: tick.rs (Touched::Balance).
//! От чего зависит: credit_watch.rs (ожидания), credit_match.rs, settings, deps.rs (журнал).
//! Настройки: transfer.credit_amount_tolerance_pct, transfer.credit_by_event (по площадке события и монете).

use rust_decimal::Decimal;

use crate::core::types::{BalanceChange, BalanceKind};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::credit_match::Pick;
use super::credit_watch::{State, lock, same_asset};
use super::engine::Core;

/// Событие изменения баланса от адаптера. Ошибок нет: итог — в журнал.
pub async fn on_change(core: &Core, b: BalanceChange) {
    let at = At {
        venue: Some(&b.venue.0),
        coin: Some(&b.asset.ticker),
    };
    let s = &core.inner.deps.settings;
    let tol = s.decimal("transfer.credit_amount_tolerance_pct", at);
    let close = s.flag("transfer.credit_by_event", at).unwrap_or(false);
    let note = {
        let mut st = lock(core);
        st.seen.insert(b.venue.clone());
        if b.own {
            return;
        }
        match b.kind {
            BalanceKind::Deposit if b.delta > Decimal::ZERO => match &tol {
                Ok(tol) => deposit(&mut st, &b, *tol, close),
                Err(e) => Some((
                    Level::Warn,
                    format!("не сверено с переводами: {}", e.message),
                )),
            },
            BalanceKind::Withdraw if b.delta < Decimal::ZERO => {
                let ours = st
                    .list
                    .values()
                    .any(|e| e.from == b.venue && same_asset(&e.asset, &b.asset));
                if ours {
                    return;
                }
                None
            }
            _ => None,
        }
    };
    let hash = b
        .tx_hash
        .as_deref()
        .map(|h| format!(", хеш {h}"))
        .unwrap_or_default();
    let what = format!("{} {:+} ({}{hash})", b.asset.ticker, b.delta, b.source);
    let (level, kind, text) = match note {
        None => (
            Level::Warn,
            "core.balance_external",
            format!("Внешнее изменение баланса: {what}"),
        ),
        Some((l, t)) => (l, "core.credit_event", format!("{what}: {t}")),
    };
    let deps = &core.inner.deps;
    deps.log(level, kind, Some(&b.venue.0), &text, None).await;
    if kind == "core.balance_external" {
        deps.notify("balance.external", Some(&b.venue.0), &text)
            .await;
    }
}

/// Поступление: ищет ожидание по получателю, монете и сумме (credit_match.rs). Ровно одно — отмечает событием и будит
/// перевод. Принимает: состояние, событие, допуск, закрывает ли событие (transfer.credit_by_event). Возвращает: запись
/// в журнал; None — ни одно не подошло (постороннее поступление). Ошибок нет.
fn deposit(
    st: &mut State,
    b: &BalanceChange,
    tol: Decimal,
    close: bool,
) -> Option<(Level, String)> {
    let open: Vec<(u64, Decimal)> = st
        .list
        .iter()
        .filter(|(_, e)| e.to == b.venue && same_asset(&e.asset, &b.asset) && e.hit.is_none())
        .map(|(id, e)| (*id, e.amount))
        .collect();
    let amounts: Vec<Decimal> = open.iter().map(|(_, a)| *a).collect();
    match super::credit_match::pick(&amounts, b.delta, tol) {
        Pick::None => None,
        Pick::Many(n) => Some((
            Level::Warn,
            format!(
                "подходит к {n} ожидаемым зачислениям — по событию не закрываем, решает опрос статуса"
            ),
        )),
        Pick::One(i) => {
            let e = st.list.get_mut(&open[i].0)?;
            e.hit = Some((b.clone(), close));
            e.wake.notify_one();
            let how = if close {
                "зачисление по событию"
            } else {
                "событие получено, закрытие по событию выключено (transfer.credit_by_event) — ждём опрос"
            };
            Some((
                Level::Info,
                format!("{}: ожидалось {} — {how}", e.deal, e.amount),
            ))
        }
    }
}
