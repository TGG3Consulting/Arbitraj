//! Файл: core/coin_counter.rs
//! Что делает: решение 405 (t191, ответ Тиграна 18.11 на ревью Н1: «не пускать») — при докупке (exec.top_up = да)
//! сделка монеты НЕ входит, пока идёт ВСТРЕЧНАЯ сделка той же монеты на том же счёте: площадка покупки одной сделки =
//! площадка продажи другой, и на этой площадке у них общий баланс монеты.
//! Почему (Н1): остаток сделки вне пути ведётся через min(учёт, факт площадки) (remain_fact.rs), а факт — весь баланс
//! монеты на счёте. Пример: A BingX → DEX продаёт на кошельке W (фантомный остаток токена-рефлексии), B DEX → BingX
//! купила 5000 на том же W и ещё не вывела; продажа A откатилась → сверка min(1000, 5000) = 1000 → A продаёт монеты B.
//! Общий баланс монеты (факт кода):
//! - DEX — кошелёк сделки (Deal.dex_account, решение 132; manual_halted::free_of по виду `dex_account::venue_of`):
//!   у новой сделки — по текущему маппингу (dex_account::of_direction_now — тем же маппингом исполнитель ставит
//!   Deal.dex_account), у идущей — её Deal.dex_account. Разные кошельки — не встречные (BingX → DEX и DEX → MEXC —
//!   можно). Кошелёк неизвестен (нет строки маппинга, сделка без счёта) — считается тем же (не доказано, что другой).
//! - Биржа — один счёт площадки на все сделки (remain_fact.rs fact: все счета роли торгового и ToTrading биржи, без
//!   деления по сделкам) — встречные всегда: BingX → MEXC идёт — MEXC → BingX и MEXC → DEX не входят, BingX → DEX — можно.
//! - Вид площадки неизвестен — считается общим.
//!
//! Оба случая: новая покупает там, где идущая продаёт (монеты новой лягут к монетам идущей до продажи), и новая продаёт
//! там, где идущая купила (после board монеты идущей лежат на площадке покупки до вывода — executor.rs board →
//! transfers::send_trade). Сделки одного направления и сделки с общей только площадкой продажи (или только покупки) —
//! не встречные (их учитывает ride_quote.rs).
//! Кто вызывает: coin_ride.rs (admit).
//! От чего зависит: coin_ride.rs (Ride), dex_account.rs, trade_ops.rs (вид площадки), deal_words.rs, notify/human.rs,
//! deal.rs, engine.rs, core/types.
//! Настройки: нет (базовое поведение; маппинг кошельков — экран «Маппинг кошельков»).

use crate::core::types::{DexAccountId, Direction, VenueId};
use crate::gateway::contract::VenueKind;
use crate::notify::human as hu;

use super::coin_ride::Ride;
use super::engine::{Core, lk};

/// Что делает: общий ли счёт монеты на площадке `v` у новой сделки (счёт `mine`) и идущей (счёт `theirs`). Принимает:
/// ядро, площадку, счета DEX. Возвращает: словами, какой счёт общий (для причины); None — разные кошельки DEX.
/// Ошибок нет.
fn shared(
    core: &Core,
    v: &VenueId,
    mine: Option<&DexAccountId>,
    theirs: Option<&DexAccountId>,
) -> Option<String> {
    let name = hu::venue(&v.0);
    match super::trade_ops::kind(core, v) {
        Ok(VenueKind::Cex) => Some(format!("у них один счёт на бирже {name}")),
        Ok(VenueKind::Dex) => match (mine, theirs) {
            (Some(a), Some(b)) if a != b => None,
            (Some(a), Some(_)) => Some(format!("у них один кошелёк {a} на {name}")),
            _ => Some(format!(
                "кошелёк одной из них на {name} неизвестен — считаем, что он общий"
            )),
        },
        Err(_) => Some(format!(
            "вид площадки {name} неизвестен — считаем счёт общим"
        )),
    }
}

/// Что делает: встречная идущая сделка на общем счёте для новой сделки направления `d`. Принимает: ядро,
/// направление, идущие сделки монеты. Возвращает: причину отказа (журнал core.start_refused через Start::Busy) —
/// требование Тиграна 18.11: что за новая сделка, какая встречная (ID, откуда → куда, шаг, сколько монет и где), какой
/// счёт общий, что будет дальше; None — встречных нет. Ошибок нет. Замок: deals (коротко; вызывать без замка deals).
pub fn check(core: &Core, d: &Direction, rides: &[Ride]) -> Option<String> {
    let counter: Vec<&Ride> = rides
        .iter()
        .filter(|r| r.sell_on == d.buy_on || r.buy_on == d.sell_on)
        .collect();
    if counter.is_empty() {
        return None;
    }
    let mine = super::dex_account::of_direction_now(core, d);
    let theirs: Vec<(Option<DexAccountId>, String)> = {
        let deals = lk(&core.inner.deals);
        counter
            .iter()
            .map(|r| match deals.get(&r.deal_id) {
                Some(x) => (x.dex_account.clone(), super::deal_words::deal(core, x)),
                None => (None, format!("сделка {}", r.deal_id)),
            })
            .collect()
    };
    let t = &d.asset.ticker;
    for (r, (acc, words)) in counter.iter().zip(theirs) {
        // Обе общие площадки (точно обратное направление — обе): общий счёт хоть на одной — встречная.
        let venues = [
            (r.sell_on == d.buy_on).then(|| {
                (
                    &d.buy_on,
                    format!(
                        "купила бы {t} на {}, где встречная его продаёт",
                        hu::venue(&d.buy_on.0)
                    ),
                )
            }),
            (r.buy_on == d.sell_on).then(|| {
                (
                    &d.sell_on,
                    format!(
                        "продала бы {t} на {}, где встречная его купила",
                        hu::venue(&d.sell_on.0)
                    ),
                )
            }),
        ];
        let hit = venues
            .into_iter()
            .flatten()
            .find_map(|(v, how)| shared(core, v, mine.as_ref(), acc.as_ref()).map(|at| (how, at)));
        if let Some((how, at)) = hit {
            return Some(format!(
                "новая сделка {} не начата: идёт встречная {words}. Новая {how}, и {at} — монеты двух сделок легли бы \
                 вместе, и при сверке остатка одна могла бы продать монеты другой. Бот подождёт: когда сделка {} \
                 закончится, это направление снова торгуется. Делать ничего не нужно (решение 405)",
                super::deal_words::route(d),
                r.deal_id
            ));
        }
    }
    None
}
