//! Файл: core/manual_relay.rs
//! Что делает: решение 235 (Тигран, 08.11.2026) — кнопка «Продать и перегнать» (у алерта и у остановленной сделки):
//! «делается один запрос по всем биржам и DEX, у кого самая топовая цена с учётом всех затрат — перекидываем туда и
//! сразу продаём, даже если цена ушла, и возвращаем USDT туда, откуда началась сделка; фиксируем убыток или прибыль».
//! Один проход оценки (все цели — параллельно): «продать здесь» (trade_ops::deal_value, кошельком сделки) и перегон
//! на каждую площадку, где монету можно продать (relay_value::value_at — выручка в USDT после ВСЕХ затрат перевода:
//! комиссия вывода, налог перевода, минимумы вывода и депозита, приход-пыль = 0). Цель: у монеты там рынок, приём по
//! BSC и продажа через API (справочник монет), venue.enabled = да, площадка не на паузе; биржа-цель — только если её
//! разрешают правила биржа ↔ биржа относительно пути монеты (решение 248, cex_link::relay_block: venue.cex_cex и список
//! venue.cex_cex_deny биржи покупки); DEX — кошельком по маппингу
//! (dex_account::for_relay, решение 185), кошелёк не на паузе (решение 230). С площадки, где лежит монета, перегона нет,
//! если вывод с неё закрыт, она или её кошелёк на паузе, или она уже отказала в выводе в этой сделке — тогда только
//! «здесь». Выбор — наибольшая выручка; здесь не хуже — продажа здесь. Иначе перевод (transfers::send_trade: у DEX —
//! зачисление по блокчейну, решение 151) и сразу продажа там по рынку кнопкой (sell::market, sell::BUTTON — без порогов
//! убытка, manual_sell.rs). Перегон — один (цикла перегонов stuck.max_relays нет). Возврат USDT на площадку покупки —
//! общий шаг закрытия (ledger::finish → usdt_return_deal.rs, флаг deal.return_usdt, решения 101, 138, 227); итог сделки —
//! как обычно. Оценка каждой площадки (выручка или причина) — строкой в хронологию сделки и журнал core.manual_relay.
//! Перевод не состоялся (отказ, исход известен) — монеты на месте: продажа здесь кнопкой; монет нет или исход
//! неизвестен — сделка остановлена, итог кнопки — причина.
//! Кто вызывает: core/manual.rs (sell_now), core/manual_halted.rs (sell_halted) — при ret = «Продать и перегнать».
//! От чего зависит: relay_value.rs, trade_ops.rs, transfers.rs, fills.rs, sell.rs, dex_account.rs, wallet_pause.rs,
//! withdraw_refused.rs, relay_here.rs (place), manual_halted.rs (on_hand, set_qty),
//! manual_relay_targets.rs (цели), manual_outcome.rs, ledger.rs.
//! Настройки: через manual_relay_targets.rs — venue.enabled, venue.cex_cex; через relay_value.rs — venue.quote_asset и др.

use futures_util::future::join_all;
use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::notify::human as hu;
use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::engine::Core;
use super::sell::BUTTON;

/// Куда продавать.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// Продать здесь (здесь не хуже лучшей цели, или целей нет).
    Here,
    /// Перегнать на площадку и продать там: ожидаемая выручка USDT после затрат перевода.
    There(VenueId, Decimal),
}

/// Выбор (чистая функция): наибольшая выручка цели (> 0) против выручки здесь. Цель лучше здесь, если здесь нет
/// котировки или выручка цели больше. Принимает: выручку здесь (None — нет котировки), оценённые цели. Возвращает: Pick.
/// Ошибок нет.
pub fn pick(here: Option<Decimal>, targets: &[(VenueId, Decimal)]) -> Pick {
    let best = targets
        .iter()
        .filter(|(_, v)| *v > Decimal::ZERO)
        .max_by(|a, b| a.1.cmp(&b.1));
    match best {
        Some((to, v)) if here.is_none_or(|h| *v > h) => Pick::There(to.clone(), *v),
        _ => Pick::Here,
    }
}

/// «Продать и перегнать» кнопкой: проход оценки, выбор, перегон (если лучше) и продажа по рынку кнопкой. Принимает:
/// ядро, сделку (шаг Selling, количество — по реальному балансу, deal.return_usdt = true). Ошибок нет: итог — событием
/// кнопки (manual_outcome.rs) и в хронологии сделки.
pub async fn run(core: &Core, mut deal: Deal) {
    let a = deal.direction.asset.clone();
    let here = super::trade_ops::deal_value(core, &deal).await.map(|x| x.0);
    let (list, skipped) = super::manual_relay_targets::targets(core, &deal);
    let (d, a) = (&deal, &a);
    let vals = join_all(list.iter().map(|to| async move {
        let acc = super::dex_account::for_relay(core, d, to).ok().flatten();
        let r = super::relay_value::value_at(core, &d.at, to, a, d.qty, acc.as_ref()).await;
        (to.clone(), r)
    }))
    .await;
    // Решение 247, п. Д: продажа здесь уже откатилась по minOut — оценка «здесь» из той же модели завышена; помечаем
    // (сама продажа здесь пойдёт кнопкой с пере-замером выхода, sell::market → remeasure::Guard::Any).
    let failed = hu::min_out_failed(deal.alert_was.as_ref().and_then(|a| a.error.as_deref()));
    let mark = if failed {
        " (по модели — продажа по ней откатилась по minOut; продажа здесь — с пере-замером)"
    } else {
        ""
    };
    let mut lines = vec![match &here {
        Ok(v) => format!("здесь ({}): {}{mark}", deal.at.0, hu::usd(*v)),
        Err(e) => format!("здесь ({}): нет котировки — {e}", deal.at.0),
    }];
    for (to, r) in &vals {
        lines.push(match r {
            Ok(v) => format!("{}: {} после затрат перевода", to.0, hu::usd(*v)),
            Err(e) => format!("{}: не оценена — {e}", to.0),
        });
    }
    lines.extend(skipped);
    let ok: Vec<(VenueId, Decimal)> = vals
        .into_iter()
        .filter_map(|(t, r)| r.ok().map(|v| (t, v)))
        .collect();
    let choice = pick(here.as_ref().ok().copied(), &ok);
    let text = format!(
        "«Продать и перегнать»: оценка площадок — {}; выбор — {}",
        lines.join("; "),
        match &choice {
            Pick::Here => format!("продать здесь ({})", deal.at.0),
            Pick::There(to, v) => format!("перегон на {} ({})", to.0, hu::usd(*v)),
        }
    );
    deal.note(text.clone());
    super::ledger::save(core, &deal).await;
    core.inner
        .deps
        .log(
            Level::Info,
            "core.manual_relay",
            Some(&deal.at.0),
            format!("{}: {text}", deal.id),
            None,
        )
        .await;
    match choice {
        Pick::Here => super::sell::market(core, deal, BUTTON).await,
        Pick::There(to, _) => relay_to(core, deal, to).await,
    }
}

/// Перегон на выбранную площадку и сразу продажа там по рынку кнопкой. Принимает: ядро, сделку, цель. Ошибок нет.
async fn relay_to(core: &Core, mut deal: Deal, to: VenueId) {
    let (from, a) = (deal.at.clone(), deal.direction.asset.clone());
    // Решение 185: сделка без счёта DEX перегоняется на DEX кошельком строки биржи, где монета, — счёт до перевода.
    if deal.dex_account.is_none()
        && let Ok(Some(acc)) = super::dex_account::for_relay(core, &deal, &to)
    {
        deal.note(format!(
            "Перегон на DEX — кошелёк {} (строка {} в маппинге)",
            acc.0, from.0
        ));
        deal.dex_account = Some(acc);
    }
    deal.step = Step::Transfer;
    super::ledger::save(core, &deal).await;
    let price = deal.buy.as_ref().map(|b| b.avg_price).unwrap_or_default();
    let mut gas = Vec::new();
    let res =
        super::transfers::send_trade(core, &deal.id, (&from, &to), &a, deal.qty, &mut gas).await;
    // Решение 236, дополнение: газ неудачных переводов из кошелька DEX (откат, отмена) — в затраты сделки.
    super::deal_gas::charge(core, &mut deal, &from, &mut gas).await;
    match res {
        Ok((sent, credited, chain)) => {
            if let Some(line) = chain {
                deal.note(line);
            }
            super::fills::transfer(core, &mut deal, &sent, &credited, &to, price).await;
            deal.relays += 1;
            let tg = hu::Msg::new(hu::INFO, "Монета перегнана кнопкой «Продать и перегнать»")
                .line(format!(
                    "{} {} {}",
                    hu::qty(deal.qty),
                    a.ticker,
                    hu::route(&from.0, &to.0)
                ))
                .kv(
                    "Дальше",
                    "продажа там по рынку, затем USDT — на площадку покупки",
                )
                .id(&deal.id)
                .render();
            core.inner
                .deps
                .notify("stuck.relayed", Some(&to.0), &tg)
                .await;
            deal.step = Step::Selling;
            super::ledger::save(core, &deal).await;
            super::sell::market(core, deal, BUTTON).await
        }
        Err(e) if e.outcome_unknown() => {
            let text = format!("перегон не состоялся, исход неизвестен: {e} — проверьте вручную");
            super::ledger::finish(core, &mut deal, Step::Halted, &text).await;
            super::manual_outcome::failed(core, &deal, BUTTON, &text).await;
        }
        Err(e) => {
            super::withdraw_refused::mark_at(core, &mut deal, &from, &e);
            let why = format!("перегон на {} не состоялся: {e}", to.0);
            here_after(core, deal, &why).await
        }
    }
}

/// Перевод не состоялся (исход известен): монеты на месте — продажа здесь кнопкой; нет — сделка остановлена.
/// Принимает: ядро, сделку, причину. Ошибок нет.
async fn here_after(core: &Core, mut deal: Deal, why: &str) {
    let (at, a) = (deal.at.clone(), deal.direction.asset.clone());
    let hand = super::manual_halted::on_hand(core, &deal, &at, &a, deal.qty).await;
    match super::relay_here::place(why, hand) {
        super::relay_here::Place::SellHere(q, note) => {
            super::manual_halted::set_qty(&mut deal, q, note);
            deal.step = Step::Selling;
            deal.note(format!("{why} — продажа здесь по рынку кнопкой"));
            super::ledger::save(core, &deal).await;
            super::sell::market(core, deal, BUTTON).await
        }
        super::relay_here::Place::Halt(text) => {
            super::ledger::finish(core, &mut deal, Step::Halted, &text).await;
            super::manual_outcome::failed(core, &deal, BUTTON, &text).await;
        }
    }
}
