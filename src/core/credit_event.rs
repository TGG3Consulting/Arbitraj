//! Файл: core/credit_event.rs
//! Что делает: сверка события изменения баланса (MarketEvent::Balance) с ожидаемыми зачислениями (решение 76):
//! - своё (own) — ничего;
//! - поступление (Deposit) — по получателю, монете и сумме (credit_match.rs, допуск transfer.credit_amount_tolerance_pct);
//!   событие с хешем подходит только к ожиданию без хеша отправителя или с тем же хешем (тот же хеш — в первую очередь);
//!   ровно одно ожидание — событие прикреплено и будит перевод (закрывает ли — credit_watch.rs); несколько — решает
//!   опрос;
//! - вывод (Withdraw) с площадки, откуда сейчас идёт перевод этой монеты, — свой;
//! - иначе — «внешнее изменение баланса» без привязки к сделке (ручной ввод и вывод): журнал core.balance_external;
//!   уведомление balance.external — если изменение не меньше transfer.external_notify_min_usd (пыль — только журнал);
//! - решение 151: у перевода сделки на DEX подошедшее поступление может сразу засчитаться зачислением по блокчейну
//!   (credit_chain.rs; transfer.credit_by_chain).
//!
//! Кто вызывает: tick.rs (Touched::Balance), transfers.rs (событие с чужим хешем — external).
//! От чего зависит: credit_watch.rs (ожидания), credit_match.rs, convert.rs (цена), settings, deps.rs (журнал).
//! Настройки: transfer.credit_amount_tolerance_pct, transfer.credit_by_event, transfer.credit_by_chain (по площадке
//! события и монете), transfer.external_notify_min_usd (вся система), venue.quote_asset.

use rust_decimal::Decimal;

use crate::core::types::{BalanceChange, BalanceKind, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::credit_chain::Verdict;
use super::credit_match::Pick;
use super::credit_watch::{State, lock, norm, same_asset};
use super::engine::{Core, rd};

/// Событие изменения баланса от адаптера. Ошибок нет: итог — в журнал.
pub async fn on_change(core: &Core, mut b: BalanceChange) {
    // Решение 211: монета события — тикер площадки; в монету ядра (адрес, показное имя) — до сверки с ожиданиями.
    // Настройки монеты — по адресу (не нашёлся адрес — тикер, как раньше).
    b.asset = super::venue_ticker::asset_back(core, &b.venue, b.asset);
    let coin = crate::settings::scope::coin_id(&b.asset.ticker, b.asset.bsc_contract.as_ref());
    let at = At {
        venue: Some(&b.venue.0),
        coin: Some(&coin),
    };
    let s = &core.inner.deps.settings;
    let tol = s.decimal("transfer.credit_amount_tolerance_pct", at);
    let close = s.flag("transfer.credit_by_event", at).unwrap_or(false);
    // Решение 151: зачисление перевода сделки на DEX по приходу в блокчейне (credit_chain.rs).
    let chain = s.flag("transfer.credit_by_chain", at).unwrap_or(false);
    let note = {
        let mut st = lock(core);
        st.seen.insert(b.venue.clone());
        if b.own {
            return;
        }
        match b.kind {
            BalanceKind::Deposit if b.delta > Decimal::ZERO => match &tol {
                Ok(tol) => deposit(&mut st, &b, *tol, close, chain),
                Err(e) => Some(format!("не сверено с переводами: {}", e.message)),
            },
            BalanceKind::Withdraw if b.delta < Decimal::ZERO => {
                let ours = st.list.values().any(|e| {
                    e.from == b.venue && same_asset(&e.asset, &b.asset) && e.same_wallet(&b)
                });
                if ours {
                    return;
                }
                None
            }
            _ => None,
        }
    };
    match note {
        None => external(core, &b).await,
        Some(t) => {
            let text = format!("{}: {t}", what(&b));
            core.inner
                .deps
                .log(
                    Level::Info,
                    "core.credit_event",
                    Some(&b.venue.0),
                    text,
                    None,
                )
                .await
        }
    }
}

/// Текст изменения для журнала. Ошибок нет.
fn what(b: &BalanceChange) -> String {
    let hash = b
        .tx_hash
        .as_deref()
        .map(|h| format!(", хеш {h}"))
        .unwrap_or_default();
    format!("{} {:+} ({}{hash})", b.asset.ticker, b.delta, b.source)
}

/// Поступление: ищет ожидание (credit_match.rs). Ровно одно — прикрепляет событие и будит перевод; у перевода сделки
/// на DEX — ещё и проверка зачисления по блокчейну (решение 151, credit_chain.rs). Принимает: состояние, событие,
/// допуск, закрывает ли событие, включено ли зачисление по блокчейну. Возвращает: запись в журнал; None — не наше.
/// Ошибок нет.
fn deposit(
    st: &mut State,
    b: &BalanceChange,
    tol: Decimal,
    close: bool,
    chain: bool,
) -> Option<String> {
    let hash = b.tx_hash.as_deref().map(norm);
    let mut open: Vec<(u64, Decimal, bool)> = st
        .list
        .iter()
        .filter(|(_, e)| e.to == b.venue && same_asset(&e.asset, &b.asset))
        // Решение 132: поступление на другой кошелёк DEX — не этого перевода.
        .filter(|(_, e)| e.same_wallet(b))
        // Уже есть своё событие — ожидание занято.
        .filter(|(_, e)| !e.hits.iter().any(|(h, _)| e.ours(h)) && e.chain_hit.is_none())
        .filter(|(_, e)| match (&hash, &e.tx) {
            (Some(h), Some(tx)) => h == tx,
            _ => true,
        })
        .map(|(id, e)| (*id, e.amount, hash.is_some() && e.tx == hash))
        .collect();
    // Тот же хеш, что у вывода, — в первую очередь.
    if open.iter().any(|(_, _, same)| *same) {
        open.retain(|(_, _, same)| *same);
    }
    let amounts: Vec<Decimal> = open.iter().map(|(_, a, _)| *a).collect();
    match super::credit_match::pick(&amounts, b.delta, tol) {
        Pick::None => None,
        Pick::Many(n) => Some(format!(
            "подходит к {n} ожидаемым зачислениям — по событию не закрываем, решает опрос статуса"
        )),
        Pick::One(i) => {
            let verdict = super::credit_chain::accept(st, open[i].0, b, tol, chain);
            let e = st.list.get_mut(&open[i].0)?;
            e.hits.push((b.clone(), close));
            e.wake.notify_one();
            let how = match verdict {
                Verdict::Taken => {
                    "зачисление по блокчейну, не дожидаясь хеша от отправителя (решение 151)"
                        .to_string()
                }
                Verdict::No(why) => {
                    format!("по блокчейну не засчитано — {why}; ждём хеш от отправителя")
                }
                Verdict::Off => how_76(close, hash.is_some(), e.tx.is_some()).to_string(),
            };
            Some(format!("{}: ожидалось {} — {how}", e.deal, e.amount))
        }
    }
}

/// Итог сверки по решению 76 для журнала. Принимает: закрывает ли событие, есть ли хеш у события, известен ли хеш
/// отправителя. Ошибок нет.
fn how_76(close: bool, hash: bool, tx: bool) -> &'static str {
    match (close, hash, tx) {
        (false, ..) => "закрытие по событию выключено (transfer.credit_by_event) — ждём опрос",
        (true, true, false) => "хеш вывода от отправителя ещё не известен — ждём его",
        (true, ..) => "зачисление по событию",
    }
}

/// Внешнее изменение баланса: журнал; уведомление — если не пыль (transfer.external_notify_min_usd; не задана —
/// уведомляется всё; цена неизвестна — уведомляется). Ошибок нет.
pub async fn external(core: &Core, b: &BalanceChange) {
    let deps = &core.inner.deps;
    let text = format!("Внешнее изменение баланса: {}", what(b));
    let min = deps.settings.decimal(
        "transfer.external_notify_min_usd",
        At {
            venue: None,
            coin: None,
        },
    );
    let dust = match min {
        Ok(min) => usd(
            core,
            &b.venue,
            &super::venue_ticker::ticker_on(core, &b.asset, &b.venue),
            b.delta.abs(),
        )
        .await
        .is_some_and(|v| v < min),
        Err(_) => false,
    };
    let level = if dust { Level::Info } else { Level::Warn };
    deps.log(
        level,
        "core.balance_external",
        Some(&b.venue.0),
        &text,
        None,
    )
    .await;
    if !dust {
        let tg =
            crate::notify::human::Msg::new(crate::notify::human::WARN, "Баланс изменился не ботом")
                .line(format!(
                    "{} {}{} на {}",
                    b.asset.ticker,
                    if b.delta.is_sign_negative() { "" } else { "+" },
                    crate::notify::human::qty(b.delta),
                    crate::notify::human::venue(&b.venue.0)
                ))
                .line(
                    b.tx_hash
                        .as_deref()
                        .map(|h| format!("Транзакция: {}", crate::notify::human::addr(h)))
                        .unwrap_or_default(),
                )
                .todo("если это не вы — проверьте площадку")
                .render();
        deps.notify("balance.external", Some(&b.venue.0), &tg).await;
    }
}

/// Стоимость в USDT: котируемая монета площадки — как есть; иначе — по живому стакану биржи (convert_at::price_at).
/// Принимает: ядро, площадку, тикер ЭТОЙ площадки, количество. None — не известна.
async fn usd(core: &Core, venue: &VenueId, ticker: &str, qty: Decimal) -> Option<Decimal> {
    let at = At {
        venue: Some(&venue.0),
        coin: None,
    };
    if core
        .inner
        .deps
        .settings
        .text("venue.quote_asset", at)
        .ok()
        .as_deref()
        == Some(ticker)
    {
        return Some(qty);
    }
    let cex: Vec<VenueId> = rd(&core.inner.kinds)
        .iter()
        .filter(|(_, k)| *k == VenueKind::Cex)
        .map(|(v, _)| v.clone())
        .collect();
    // Решение 211 (ревью В7): тикер — площадки события; другие биржи — по их тикеру того же адреса (convert_at.rs).
    let p = super::convert_at::price_at(core, &cex, venue, ticker, qty)
        .await
        .ok()?;
    Some(p * qty)
}
