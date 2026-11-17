//! Файл: core/coin_ride.rs
//! Что делает: замок монеты (решение 118, Я-2) и «докупка» (решение 269 — записка договорённостей с Тиграном 14.11;
//! реализация — задача t179, решение 390). Замок в двух частях:
//! - `inner.active` — монета занята ЦЕЛИКОМ: сделка от входа исполнителя до учёта покупки (предпроверка, ордер покупки,
//!   частичная покупка, продажа здесь), цикл DEX ↔ DEX, ручное действие, продажа кнопкой «Балансов»;
//! - `inner.rides` — сделки монеты, которые уже купили, учли покупку во «вложено по монете» и везут или продают монету.
//!
//! exec.top_up не задана или «нет» (п. 15 записки: «1 в 1 как сейчас») — по монете одна сделка до самого конца:
//! пока есть запись в `rides`, новая сделка получает прежний отказ start_reason::busy, ручные действия — прежний.
//! exec.top_up = да — докупка: пока идут сделки группы (одна монета, ТО ЖЕ направление: та же площадка покупки и та
//! же площадка продажи — п. 1, 3), следующая сделка того же направления может начаться и становится сделкой группы;
//! число сделок не ограничено (п. 7) — их ограничивают деньги (money.max_per_coin_usd, «в пути», остаток, дневной
//! лимит) и порог прибыли каждой. Другие направления монеты при идущей группе — прежний отказ (монета занята).
//! Сделка, ушедшая с пути (шаг не Transfer / Selling), новых докупок не пускает.
//! Значение включателя для идущей сделки запоминается при board (Ride.top_up): очередь кандидатов на каждом тике
//! настройку не читает, пока идущая сделка начата без докупки.
//! Блокировки: сначала `active`, потом `rides`, потом `deals`; через await не держатся.
//! Кто вызывает: executor.rs (admit, board, release), candidates.rs (blocks_new), manual_halted.rs, manual_return.rs,
//! balance_sell.rs, cycle_deal.rs (take), balance_sell_check.rs, busy_note.rs, uncapped_watch.rs,
//! deposit_warm_busy.rs (held, keys), ride_quote.rs и coin_group.rs (of_coin, group_of).
//! От чего зависит: engine.rs, coin_key.rs, deal.rs, start_reason.rs, settings.
//! Настройки: exec.top_up.

use crate::core::types::{Direction, VenueId};
use crate::settings::read::At;

use super::coin_key::CoinKey;
use super::deal::Step;
use super::engine::{Core, lk};

/// Имя включателя докупки (каталог — settings/catalog/trade.rs).
pub const TOP_UP: &str = "exec.top_up";

/// Идущая сделка монеты после учёта покупки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ride {
    /// ID сделки в учёте.
    pub deal_id: String,
    /// Площадка покупки.
    pub buy_on: VenueId,
    /// Площадка продажи.
    pub sell_on: VenueId,
    /// Группа докупки (ID первой сделки группы); None — сделка начата без докупки (exec.top_up выключена).
    pub group: Option<String>,
}

/// Место новой сделки по монете (admit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seat {
    /// Докупка выключена — одна сделка на монету, как раньше (группы нет).
    Alone,
    /// Докупка включена, сделок монеты нет — первая сделка группы (группа = её ID).
    Lead,
    /// Докупка: вложенная сделка группы (ID первой сделки группы).
    Join(String),
}

/// Что делает: включена ли докупка (п. 15). Принимает: ядро. Возвращает: exec.top_up = да; не задана, «нет» или
/// неверна — false. Ошибок нет.
pub fn top_up_on(core: &Core) -> bool {
    core.inner
        .deps
        .settings
        .flag(TOP_UP, At::default())
        .unwrap_or(false)
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

/// Что делает: группа, к которой может присоединиться новая сделка направления `d` при идущих `rides` (замок целиком
/// свободен, докупка включена): все идущие — сделки группы этого же направления на шаге Transfer / Selling.
/// Принимает: ядро, направление, идущие сделки (не пусто). Возвращает: ID группы; None — нельзя. Ошибок нет.
fn joinable(core: &Core, d: &Direction, rides: &[Ride]) -> Option<String> {
    let group = rides.first()?.group.clone()?;
    let same = rides.iter().all(|r| {
        r.group.as_deref() == Some(group.as_str()) && r.buy_on == d.buy_on && r.sell_on == d.sell_on
    });
    if !same {
        return None;
    }
    let deals = lk(&core.inner.deals);
    rides
        .iter()
        .all(|r| {
            matches!(
                deals.get(&r.deal_id).map(|x| x.step),
                Some(Step::Transfer | Step::Selling)
            )
        })
        .then_some(group)
}

/// Что делает: вход исполнителя (executor::accepted) — занять монету целиком под предпроверку и покупку новой сделки.
/// Монета занята целиком или по ней идут сделки, к которым нельзя присоединиться, — прежний отказ start_reason::busy.
/// Принимает: ядро, направление. Возвращает: место сделки (монета занята этим вызовом; снимает release или board).
/// Ошибки: текст причины отказа (прежний).
pub fn admit(core: &Core, d: &Direction) -> Result<Seat, String> {
    let key = CoinKey::of(&d.asset);
    let mut a = lk(&core.inner.active);
    let busy = || super::start_reason::busy(lk(&core.inner.deals).values(), d);
    if a.contains(&key) {
        drop(a);
        return Err(busy());
    }
    let rides = of_coin(core, &key);
    let seat = if rides.is_empty() {
        if top_up_on(core) {
            Seat::Lead
        } else {
            Seat::Alone
        }
    } else if rides.iter().any(|r| r.group.is_none()) || !top_up_on(core) {
        return Err(busy());
    } else {
        match joinable(core, d, &rides) {
            Some(g) => Seat::Join(g),
            None => return Err(busy()),
        }
    };
    a.insert(key);
    Ok(seat)
}

/// Что делает: сделка учла покупку и переходит к переводу — монета из замка целиком в идущие. Принимает: ядро,
/// направление, ID сделки, группу сделки (Deal.group; None — без докупки). Ошибок нет.
pub fn board(core: &Core, d: &Direction, deal_id: &str, group: Option<String>) {
    let key = CoinKey::of(&d.asset);
    let mut a = lk(&core.inner.active);
    let mut r = lk(&core.inner.rides);
    a.remove(&key);
    r.entry(key).or_default().push(Ride {
        deal_id: deal_id.to_string(),
        buy_on: d.buy_on.clone(),
        sell_on: d.sell_on.clone(),
        group,
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
/// по ней идут сделки, к которым это направление присоединиться не может. Быстрый путь: идущие начаты без докупки —
/// ответ без чтения настройки, копий и текста (как раньше одно `contains`). Принимает: ядро, направление. Ошибок нет.
pub fn blocks_new(core: &Core, d: &Direction) -> bool {
    let key = CoinKey::of(&d.asset);
    if lk(&core.inner.active).contains(&key) {
        return true;
    }
    let rides = {
        let r = lk(&core.inner.rides);
        match r.get(&key) {
            None => return false,
            Some(v) if v.is_empty() => return false,
            Some(v) if v.iter().any(|x| x.group.is_none()) => return true,
            Some(v) => v.clone(),
        }
    };
    !top_up_on(core) || joinable(core, d, &rides).is_none()
}

/// Что делает: идёт ли группа докупки этого направления (есть идущие сделки группы того же направления) — для
/// расчёта докупки (возврат USDT без фиксированной части, п. 5). Принимает: ядро, направление. Ошибок нет.
pub fn group_running(core: &Core, d: &Direction) -> Option<String> {
    let key = CoinKey::of(&d.asset);
    let r = lk(&core.inner.rides);
    let v = r.get(&key)?;
    let first = v.first()?;
    (first.buy_on == d.buy_on && first.sell_on == d.sell_on)
        .then(|| first.group.clone())
        .flatten()
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
