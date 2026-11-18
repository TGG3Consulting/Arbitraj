//! Файл: core/remain_others.rs
//! Что делает: решение 405 (t191, Тигран 18.11: «каждая сделка всегда должна знать своё количество монет и работать
//! только с ним»; ревью Н1′ / Н7) — сколько монет на счёте площадки ДОКАЗУЕМО числится за другими сделками той же
//! монеты, чтобы сверка остатка (remain_fact.rs, решение 222) не отдала сделке чужие монеты на общем счёте / кошельке:
//! остаток = min(учёт сделки, факт счёта − учёт других сделок на этом счёте).
//!
//! Другие сделки (по набору unsettled.rs — без прохода по истории): та же монета, не эта сделка, монет > 0
//! (Deal.qty), шаг не «закрыта» и не «Беру на себя» (ревью 3, Н1: монеты у человека — balance_sell_check.rs их не
//! считает ботовыми; manual.rs qty не обнуляет), счёт тот же (биржа — один на все сделки площадки, remain_fact.rs fact;
//! DEX — тот же кошелёк Deal.dex_account, неизвестен — тот же). Остановленные (Halted) и ждущие человека — считаются
//! (монеты бота).
//!
//! Где монеты другой сделки (ревью 3, Б1):
//! - шаг не «перевод» — на Deal.at (после покупки — площадка покупки, после зачисления — получатель: fills.rs
//!   transfer ставит at и qty = «сколько дошло», комиссия вывода в монете уже вычтена — ревью 3, М3);
//! - шаг «перевод» (Transfer) — не доказано: шаг ставится ДО запроса вывода (executor.rs, relay.rs, manual_relay.rs),
//!   признака «вывод запрошен» в сделке нет, at и qty меняются только после зачисления (qty в пути — купленное, до
//!   комиссии вывода). Такая сделка на площадке покупки или получателя этого счёта — «неясно» (Others::unclear).
//!
//! «Неясно» — сверка НЕ вычитает других и не поднимает алерт: остаток = min(учёт, факт) — ровно решение 222, как до
//! решения 405 (учёт не урезается из-за чужих монет, которые могли уже уйти или ещё не прийти); в хронологию — какая
//! сделка в пути.
//! Факт меньше учёта других (без «неясно») — монеты на счёте по учёту чужие: сделке не достаётся ничего, но она НЕ
//! закрывается как фантом (у кого фантом — по факту не видно), решает человек (remain_fact.rs all_foreign).
//! Кто вызывает: remain_fact.rs (reconcile).
//! От чего зависит: unsettled.rs, coin_key.rs, deal.rs, trade_ops.rs (вид площадки), engine.rs.
//! Настройки: нет (базовое поведение).

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::gateway::contract::VenueKind;

use super::coin_key::CoinKey;
use super::deal::{Deal, Step};
use super::engine::{Core, lk};

/// Монеты других сделок на счёте: сумма доказуемо лежащих, (сделка, монет), сделки в пути через этот счёт.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Others {
    pub qty: Decimal,
    pub deals: Vec<(String, Decimal)>,
    /// Сделки на шаге «перевод» с площадкой покупки или получателем = эта площадка (где их монеты — не доказано).
    pub unclear: Vec<String>,
}

/// Что делает: монеты других сделок той же монеты на счёте сделки `deal` на площадке `v` (см. шапку). Принимает: ядро,
/// сделку, площадку. Возвращает: Others. Ошибок нет. Замок: deals (по набору unsettled).
pub fn on_account(core: &Core, deal: &Deal, v: &VenueId) -> Others {
    let cex = super::trade_ops::kind(core, v) == Ok(VenueKind::Cex);
    let key = CoinKey::of(&deal.direction.asset);
    let ids = super::unsettled::ids(core);
    let deals = lk(&core.inner.deals);
    let mut out = Others::default();
    for d in ids.iter().filter_map(|id| deals.get(id)) {
        if d.id == deal.id
            || d.qty <= Decimal::ZERO
            || matches!(d.step, Step::Closed | Step::TakenOver)
            || CoinKey::of(&d.direction.asset) != key
        {
            continue;
        }
        let same = cex
            || d.dex_account.is_none()
            || deal.dex_account.is_none()
            || d.dex_account == deal.dex_account;
        if !same {
            continue;
        }
        if d.step == Step::Transfer {
            if d.direction.buy_on == *v || d.direction.sell_on == *v || d.at == *v {
                out.unclear.push(d.id.clone());
            }
        } else if d.at == *v {
            out.qty += d.qty;
            out.deals.push((d.id.clone(), d.qty));
        }
    }
    out.deals.sort();
    out.unclear.sort();
    out
}

/// Итог сверки остатка.
#[derive(Debug, Clone, PartialEq)]
pub enum Own {
    /// Вести столько (0 — на счёте ничего нет: закрыть, как раньше).
    Qty(Decimal),
    /// На счёте что-то есть, но по учёту всё чужое: не закрывать, решает человек.
    Human,
}

/// Что делает: сколько вести сделке. Принимает: учёт сделки, факт счёта, других (Others). Возвращает: «неясно» —
/// Qty(min(учёт, факт)) (решение 222, других не вычитаем); иначе факт ≤ 0 — Qty(0); факт − других ≤ 0 — Human;
/// иначе Qty(min(учёт, факт − других)). Других нет — то же, что remain_fact::settle_qty. Ошибок нет.
pub fn own_qty(own: Decimal, fact: Decimal, o: &Others) -> Own {
    if fact <= Decimal::ZERO {
        return Own::Qty(Decimal::ZERO);
    }
    if !o.unclear.is_empty() {
        return Own::Qty(own.min(fact).max(Decimal::ZERO));
    }
    let free = fact - o.qty;
    if free <= Decimal::ZERO {
        return Own::Human;
    }
    Own::Qty(own.min(free).max(Decimal::ZERO))
}
