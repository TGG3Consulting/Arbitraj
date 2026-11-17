//! Файл: core/coin_ride.rs
//! Что делает: замок монеты (решение 118, Я-2) в двух частях (задача t179, решение 390):
//! - `inner.active` — монета занята ЦЕЛИКОМ: сделка от входа исполнителя до учёта покупки (предпроверка, ордер покупки,
//!   частичная покупка, продажа здесь), цикл DEX ↔ DEX, ручное действие, продажа кнопкой «Балансов»;
//! - `inner.rides` — сделки монеты, которые уже купили, учли покупку во «вложено по монете» и везут или продают монету
//!   (шаги Transfer / Selling). Пока по монете только такие сделки, следующая сделка может начаться, если:
//!   (1) идущих меньше money.max_deals_per_coin (не задана или неверна — 1: как до t179, одна сделка на монету);
//!   (2) она не встречная ни одной идущей: не покупает там, где идущая продаёт, и не продаёт там, где идущая покупала;
//!   (3) каждая идущая — на шаге Transfer или Selling (застрявшая, остановленная, ждущая человека — новых нет).
//!
//! Деньги и ликвидность новой сделки проверяет обычный расчёт: вложенное идущих уже в учёте (limits_parts.rs вычитает
//! его из money.max_per_coin_usd и «в пути»), продажа считается по стакану / пулу ПОСЛЕ монет идущих сделок той же
//! площадки продажи, покупка — только по данным новее покупки идущих (ride_quote.rs).
//! Блокировки: сначала `active`, потом `rides`, потом `deals`; через await не держатся.
//! Кто вызывает: executor.rs (admit, board, release), candidates.rs (blocks_new), manual_halted.rs, manual_return.rs,
//! balance_sell.rs, cycle_deal.rs (take), balance_sell_check.rs, busy_note.rs, uncapped_watch.rs,
//! deposit_warm_busy.rs (held, keys), ride_quote.rs (of_coin).
//! От чего зависит: engine.rs, coin_key.rs, deal.rs, start_reason.rs, settings.
//! Настройки: money.max_deals_per_coin.

use crate::core::types::{Direction, VenueId};
use crate::settings::read::At;

use super::coin_key::CoinKey;
use super::deal::Step;
use super::engine::{Core, lk};

/// Имя настройки «сколько сделок одной монеты одновременно» (каталог — settings/catalog/base.rs).
pub const MAX_DEALS: &str = "money.max_deals_per_coin";

/// Идущая сделка монеты после учёта покупки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ride {
    /// ID сделки в учёте.
    pub deal_id: String,
    /// Площадка покупки.
    pub buy_on: VenueId,
    /// Площадка продажи.
    pub sell_on: VenueId,
}

/// Что делает: предел одновременных сделок монеты. Принимает: ядро. Возвращает: money.max_deals_per_coin; не задана,
/// неверна или 0 — 1 (как до t179). Ошибок нет.
pub fn max_deals(core: &Core) -> usize {
    match core.inner.deps.settings.integer(MAX_DEALS, At::default()) {
        Ok(n) if n >= 1 => usize::try_from(n).unwrap_or(usize::MAX),
        _ => 1,
    }
}

/// Что делает: копия идущих сделок монеты. Принимает: ядро, ключ монеты. Возвращает: список (пусто — нет). Ошибок нет.
pub fn of_coin(core: &Core, key: &CoinKey) -> Vec<Ride> {
    lk(&core.inner.rides).get(key).cloned().unwrap_or_default()
}

/// Что делает: монета занята чем-либо (замок целиком или идущие сделки) — для ручных действий, наблюдения и прогрева.
/// Принимает: ядро, ключ. Ошибок нет.
pub fn held(core: &Core, key: &CoinKey) -> bool {
    if lk(&core.inner.active).contains(key) {
        return true;
    }
    lk(&core.inner.rides)
        .get(key)
        .is_some_and(|v| !v.is_empty())
}

/// Что делает: занять монету целиком (ручное действие, цикл DEX ↔ DEX, продажа «Балансов») — только если по ней нет
/// ни замка, ни идущих сделок (как раньше `active.insert`). Принимает: ядро, ключ. Возвращает: true — занята этим
/// вызовом; снимает её вызывающий (`active.remove`). Ошибок нет.
pub fn take(core: &Core, key: &CoinKey) -> bool {
    let mut a = lk(&core.inner.active);
    if a.contains(key)
        || lk(&core.inner.rides)
            .get(key)
            .is_some_and(|v| !v.is_empty())
    {
        return false;
    }
    a.insert(key.clone())
}

/// Что делает: почему новая сделка направления `d` не может начаться при идущих сделках `rides` (замок `active` уже
/// свободен). Принимает: ядро, направление, идущие сделки монеты. Возвращает: None — может; Some(причина). Ошибок нет.
/// При пределе 1 (money.max_deals_per_coin не задана) причина — прежний текст start_reason::busy.
pub fn refuse(core: &Core, d: &Direction, rides: &[Ride]) -> Option<String> {
    if rides.is_empty() {
        return None;
    }
    let max = max_deals(core);
    if rides.len() >= max {
        if max == 1 {
            return Some(super::start_reason::busy(lk(&core.inner.deals).values(), d));
        }
        return Some(format!(
            "по монете идут {} сделки — предел {MAX_DEALS} = {max}",
            rides.len()
        ));
    }
    if let Some(r) = rides
        .iter()
        .find(|r| r.buy_on == d.sell_on || r.sell_on == d.buy_on)
    {
        return Some(format!(
            "встречная сделка: идёт {} ({} → {}) — новая {} → {} купила бы или продала на её площадке",
            r.deal_id, r.buy_on.0, r.sell_on.0, d.buy_on.0, d.sell_on.0
        ));
    }
    let deals = lk(&core.inner.deals);
    for r in rides {
        match deals.get(&r.deal_id).map(|x| x.step) {
            Some(Step::Transfer | Step::Selling) => {}
            step => {
                return Some(format!(
                    "по монете идёт сделка {} не на пути «перевод / продажа» (шаг {step:?}) — новой нет",
                    r.deal_id
                ));
            }
        }
    }
    None
}

/// Что делает: вход исполнителя (executor::accepted) — занять монету целиком под предпроверку и покупку новой сделки.
/// Монета занята целиком (другая сделка до учёта покупки, цикл, ручное действие) — прежний отказ; по монете только
/// идущие сделки — refuse. Принимает: ядро, направление. Возвращает: Ok — монета занята этим вызовом (снимает release
/// или board). Ошибки: текст причины отказа.
pub fn admit(core: &Core, d: &Direction) -> Result<(), String> {
    let key = CoinKey::of(&d.asset);
    let mut a = lk(&core.inner.active);
    if a.contains(&key) {
        drop(a);
        return Err(super::start_reason::busy(lk(&core.inner.deals).values(), d));
    }
    let rides = of_coin(core, &key);
    if let Some(why) = refuse(core, d, &rides) {
        return Err(why);
    }
    a.insert(key);
    Ok(())
}

/// Что делает: сделка учла покупку и переходит к переводу — монета из замка целиком в идущие (следующая сделка
/// монеты может начаться по admit). Принимает: ядро, направление, ID сделки. Ошибок нет.
pub fn board(core: &Core, d: &Direction, deal_id: &str) {
    let key = CoinKey::of(&d.asset);
    let mut a = lk(&core.inner.active);
    let mut r = lk(&core.inner.rides);
    a.remove(&key);
    r.entry(key).or_default().push(Ride {
        deal_id: deal_id.to_string(),
        buy_on: d.buy_on.clone(),
        sell_on: d.sell_on.clone(),
    });
}

/// Что делает: задача сделки кончилась — снять то, что она держит: до board — замок целиком; после — свою запись в
/// идущих (замок целиком может держать уже другая сделка — его не трогаем). Принимает: ядро, ключ, ID сделки, если
/// board был (None — не был). Ошибок нет.
pub fn release(core: &Core, key: &CoinKey, boarded: Option<&str>) {
    let Some(id) = boarded else {
        lk(&core.inner.active).remove(key);
        return;
    };
    let mut r = lk(&core.inner.rides);
    if let Some(v) = r.get_mut(key) {
        v.retain(|x| x.deal_id != id);
        if v.is_empty() {
            r.remove(key);
        }
    }
}

/// Что делает: очередь расчёта (candidates::enqueue) — новый расчёт направления не нужен: монета занята целиком или
/// новая сделка не может начаться при идущих (refuse). Принимает: ядро, направление. Ошибок нет.
pub fn blocks_new(core: &Core, d: &Direction) -> bool {
    let key = CoinKey::of(&d.asset);
    if lk(&core.inner.active).contains(&key) {
        return true;
    }
    let rides = of_coin(core, &key);
    refuse(core, d, &rides).is_some()
}

/// Что делает: ключи всех занятых монет (замок целиком и идущие) — для прогрева адресов депозита. Принимает: ядро.
/// Ошибок нет.
pub fn keys(core: &Core) -> std::collections::HashSet<CoinKey> {
    let mut all = lk(&core.inner.active).clone();
    all.extend(
        lk(&core.inner.rides)
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, _)| k.clone()),
    );
    all
}
