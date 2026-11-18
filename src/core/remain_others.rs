//! Файл: core/remain_others.rs
//! Что делает: решение 405 (t191, Тигран 18.11: «каждая сделка всегда должна знать своё количество монет и работать
//! только с ним»; ревью Н1′ / Н7) — сколько монет на счёте площадки числится за ДРУГИМИ сделками той же монеты, чтобы
//! сверка остатка (remain_fact.rs, решение 222) не отдала сделке чужие монеты на общем счёте / кошельке:
//! остаток = min(учёт сделки, факт счёта − учёт других сделок на этом счёте).
//! Другие сделки на счёте (по учёту в памяти): та же монета, не эта сделка, монет > 0 (Deal.qty), не закрыта (Closed),
//! и монеты на этой площадке: Deal.at = площадка (где монеты по учёту: после покупки — площадка покупки, после
//! зачисления — получатель, fills.rs transfer) или сделка на шаге «перевод» (Transfer) В эту площадку (монеты могут уже
//! лежать на счёте получателя до учёта зачисления). Остановленные и «Беру на себя» с монетами — тоже: их монеты не
//! этой сделки. Счёт: биржа — один на все сделки площадки (remain_fact.rs fact — все счета биржи); DEX — тот же
//! кошелёк (Deal.dex_account, решение 132; у одной из сделок неизвестен — считается тем же).
//! Осторожно в сторону «меньше»: перевод уже ушёл с площадки покупки, а Deal.at ещё она — его монеты вычитаются там,
//! где их уже нет (сделка видит меньше; встречные на общем счёте при докупке не сосуществуют — coin_counter.rs).
//! Факт меньше учёта других — монеты на счёте по учёту чужие: сделке не достаётся ничего, но она НЕ закрывается как
//! фантом (у кого фантом — по факту не видно), решает человек (remain_fact.rs).
//! Кто вызывает: remain_fact.rs (reconcile).
//! От чего зависит: coin_key.rs, deal.rs, trade_ops.rs (вид площадки), engine.rs.
//! Настройки: нет (базовое поведение).

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::gateway::contract::VenueKind;

use super::coin_key::CoinKey;
use super::deal::{Deal, Step};
use super::engine::{Core, lk};

/// Монеты других сделок на счёте: сумма и (сделка, монет).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Others {
    pub qty: Decimal,
    pub deals: Vec<(String, Decimal)>,
}

/// Что делает: монеты других сделок той же монеты на счёте сделки `deal` на площадке `v` (см. шапку). Принимает: ядро,
/// сделку, площадку. Возвращает: Others (пусто — нет). Ошибок нет. Замок: deals.
pub fn on_account(core: &Core, deal: &Deal, v: &VenueId) -> Others {
    let cex = super::trade_ops::kind(core, v) == Ok(VenueKind::Cex);
    let key = CoinKey::of(&deal.direction.asset);
    let deals = lk(&core.inner.deals);
    let mut out = Others::default();
    for d in deals.values() {
        if d.id == deal.id || d.qty <= Decimal::ZERO || d.step == Step::Closed {
            continue;
        }
        if CoinKey::of(&d.direction.asset) != key {
            continue;
        }
        let here = d.at == *v || (d.step == Step::Transfer && d.direction.sell_on == *v);
        let same = cex
            || d.dex_account.is_none()
            || deal.dex_account.is_none()
            || d.dex_account == deal.dex_account;
        if here && same {
            out.qty += d.qty;
            out.deals.push((d.id.clone(), d.qty));
        }
    }
    out.deals.sort();
    out
}

/// Что делает: сколько вести сделке. Принимает: учёт сделки, факт счёта, монет других сделок на счёте. Возвращает:
/// Some(min(учёт, факт − других), не меньше 0) — в том числе 0, если на счёте ничего нет (факт ≤ 0: закрыть, как
/// раньше); None — на счёте что-то есть, но по учёту всё чужое (факт − других ≤ 0): не закрывать, решает человек.
/// Других нет — то же, что remain_fact::settle_qty. Ошибок нет.
pub fn own_qty(own: Decimal, fact: Decimal, others: Decimal) -> Option<Decimal> {
    if fact <= Decimal::ZERO {
        return Some(Decimal::ZERO);
    }
    let free = fact - others;
    if free <= Decimal::ZERO {
        return None;
    }
    Some(own.min(free).max(Decimal::ZERO))
}
