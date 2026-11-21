//! Файл: core/coin_ride.rs
//! Что делает: замок монеты (решение 118, Я-2) и «докупка» (решение 269 — записка договорённостей с Тиграном 14.11;
//! реализация — задача t179, решение 394). Замок в двух частях:
//! - `inner.active` — монета занята ЦЕЛИКОМ: сделка от входа исполнителя до учёта покупки (предпроверка, ордер покупки,
//!   частичная покупка, продажа здесь), цикл DEX ↔ DEX, ручное действие, продажа кнопкой «Балансов»;
//! - `inner.rides` — сделки монеты, которые уже купили, учли покупку во «вложено по монете» и везут или продают монету.
//!
//! exec.top_up не задана или «нет» (п. 15 записки: «1 в 1 как сейчас») — по монете одна сделка до самого конца:
//! пока есть запись в `rides`, новая сделка получает прежний отказ start_reason::busy, ручные действия — прежний.
//! exec.top_up = да — докупка: пока идут сделки группы (одна монета, ТО ЖЕ направление: та же площадка покупки и та
//! же площадка продажи — п. 1, 3), следующая сделка того же направления может начаться и становится сделкой группы;
//! число сделок не ограничено (п. 7) — их ограничивают деньги (money.max_per_coin_usd, «в пути», остаток, дневной
//! лимит) и порог прибыли каждой. Решение 405 (t191, ответ Тиграна 18.11 на вопрос 1 решения 394): ДРУГИЕ
//! направления монеты при идущей группе тоже торгуются — сделка другого направления начинает свою группу (Lead) или
//! входит в идущую группу своего направления (Join); у каждого направления — своя группа и свой общий возврат
//! (coin_group.rs — по ID группы). Деньги монеты общие (money.max_per_coin_usd — по монете, всех направлений), котировки
//! — после идущих сделок всех групп монеты на той же площадке (ride_quote.rs: продажа — sell_on, покупка — buy_on).
//! Встречная сделка на общем счёте (площадка покупки одной = площадка продажи другой, тот же кошелёк DEX или та же
//! биржа; ответ Тиграна на Н1) — не входит, пока идёт (coin_counter.rs).
//! Сделка, ушедшая с пути (шаг не Transfer / Selling), новых сделок монеты не пускает — ни в одном направлении. Общий
//! возврат группы уже ушёл (ревью 3, Н3) — новая сделка того же направления начинает новую группу.
//! Начата ли идущая сделка с докупкой, видно по её записи (Ride.group: None — без докупки, ставится при board из
//! Deal.group): пока по монете идёт такая сделка, очередь кандидатов (blocks_new) настройку не читает — сразу «занята».
//! Решение 405 (ответ Тиграна на ревью Н3): пока ручное действие ждёт окончания сделок монеты (manual_wait.rs) — новых
//! сделок, расчётов и действий с ней нет (admit, blocks_new, take); free_active и release будят ожидание.
//! Блокировки: сначала `active`, потом `rides`, потом `deals`; через await не держатся.
//! Кто вызывает: executor.rs (admit, board, release), candidates.rs (blocks_new), manual_halted.rs, manual_return.rs,
//! balance_sell.rs, cycle_deal.rs (take), balance_sell_check.rs, busy_note.rs, uncapped_watch.rs,
//! deposit_warm_busy.rs (held, keys), ride_quote.rs (Ride, top_up_on), costs_source.rs (group_running), coin_group.rs
//! (inner.rides — по ID группы).
//! От чего зависит: engine.rs, coin_key.rs, deal.rs, start_reason.rs, coin_counter.rs (встречные, решение 405), settings.
//! Настройки: exec.top_up.

use std::sync::atomic::Ordering;

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
/// вызовом; снимает её вызывающий (`active.remove`). Решение 405 (ответ Тиграна на ревью Н3): монету ждёт ручное
/// действие — новых действий с ней нет, кроме повтора самой ждавшей команды: он берёт монету и этим снимает паузу
/// (manual_wait::claim, ревью 4 Н3 — пауза держится до take). Ошибок нет. Замки: active, затем manual_waits.
pub fn take(core: &Core, key: &CoinKey) -> bool {
    let mut a = lk(&core.inner.active);
    if a.contains(key)
        || lk(&core.inner.rides)
            .get(key)
            .is_some_and(|v| !v.is_empty())
    {
        return false;
    }
    if super::manual_wait::paused(core, key) && !super::manual_wait::claim(core, key) {
        return false;
    }
    a.insert(key.clone())
}

/// Что делает: снять замок монеты целиком, взятый take (ручное действие, цикл, продажа «Балансов»), и разбудить
/// ожидания ручных действий (решение 405, manual_wait_run.rs). Принимает: ядро, ключ. Ошибок нет.
pub fn free_active(core: &Core, key: &CoinKey) {
    lk(&core.inner.active).remove(key);
    core.inner.coin_wake.notify_waiters();
}

/// Идущая сделка того же направления (та же площадка покупки и та же площадка продажи). Ошибок нет.
fn same_dir(r: &Ride, d: &Direction) -> bool {
    r.buy_on == d.buy_on && r.sell_on == d.sell_on
}

/// Что делает: место новой сделки направления `d` при идущих `rides` (замок целиком свободен, докупка включена): все
/// идущие — сделки групп (любых направлений, решение 405) на шаге Transfer / Selling. Есть группа ЭТОГО направления,
/// чей общий возврат ещё не ушёл, — вложенная в неё (Join); групп этого направления нет или у всех общий возврат уже
/// ушёл (ревью 3, Н3: coin_group::flushed) — первая сделка новой группы (Lead: фиксированная часть возврата — в её
/// плане). Две открытые группы одного направления — нельзя. Принимает: ядро, направление, идущие сделки (не пусто).
/// Возвращает: место; None — нельзя. Ошибок нет. Замки: deals, затем groups — по очереди, не вложенно.
fn joinable(core: &Core, d: &Direction, rides: &[Ride]) -> Option<Seat> {
    if rides.iter().any(|r| r.group.is_none()) {
        return None;
    }
    let on_path = {
        let deals = lk(&core.inner.deals);
        rides.iter().all(|r| {
            matches!(
                deals.get(&r.deal_id).map(|x| x.step),
                Some(Step::Transfer | Step::Selling)
            )
        })
    };
    if !on_path {
        return None;
    }
    let mut open: Vec<&str> = rides
        .iter()
        .filter(|r| same_dir(r, d))
        .filter_map(|r| r.group.as_deref())
        .filter(|g| !super::coin_group::flushed(core, g))
        .collect();
    open.sort_unstable();
    open.dedup();
    match open.as_slice() {
        [] => Some(Seat::Lead),
        [g] => Some(Seat::Join(g.to_string())),
        _ => None,
    }
}

/// Что делает: вход исполнителя (executor::accepted) — занять монету целиком под предпроверку и покупку новой сделки.
/// Монета занята целиком или по ней идут сделки, к которым нельзя присоединиться, — прежний отказ start_reason::busy.
/// Решение 405: встречная идущая сделка на общем счёте (coin_counter.rs) — отказ её причиной (журнал core.start_refused).
/// Вложенная (Join) сразу входит в ожидание общего возврата группы (coin_group::join — под замком `active`, ревью 3,
/// Н1: порядок active → groups; закрытие последней идущей не уведёт общий возврат без неё).
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
    // Решение 405 (ответ Тиграна на ревью Н3): ручное действие ждёт окончания сделок монеты — новых сделок нет.
    if let Some(why) = super::manual_wait::paused_why(core, &key) {
        return Err(why);
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
            // Решение 405 (ответ Тиграна на Н1): встречная идущая сделка на общем счёте — отказ своей причиной.
            Some(s) => match super::coin_counter::check(core, d, &rides) {
                Some(why) => return Err(why),
                None => s,
            },
            None => return Err(busy()),
        }
    };
    if let Seat::Join(g) = &seat {
        super::coin_group::join(core, g);
    }
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
    if group.is_some() {
        core.inner.group_live.fetch_add(1, Ordering::Relaxed);
    }
    // Решение 403 (ревью t188): счётчик записей — под замками `active` и `rides` (blocks_new читает его после `active`).
    core.inner.rides_live.fetch_add(1, Ordering::AcqRel);
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
        free_active(core, key);
        return;
    };
    let mut r = lk(&core.inner.rides);
    if let Some(v) = r.get_mut(key) {
        let grouped = v
            .iter()
            .filter(|x| x.deal_id == id && x.group.is_some())
            .count();
        if grouped > 0 {
            core.inner.group_live.fetch_sub(grouped, Ordering::Relaxed);
        }
        let before = v.len();
        v.retain(|x| x.deal_id != id);
        core.inner
            .rides_live
            .fetch_sub(before - v.len(), Ordering::AcqRel);
        if v.is_empty() {
            r.remove(key);
        }
    }
    drop(r);
    // Решение 405: запись снята — ожидания ручных действий проверяют монету (manual_wait_run.rs).
    core.inner.coin_wake.notify_waiters();
}

/// Что делает: очередь расчёта (candidates::enqueue) — новый расчёт направления не нужен: монета занята целиком или
/// по ней идут сделки, к которым это направление присоединиться не может. Решение 403 (ревью t188): идущих сделок нет
/// (inner.rides_live = 0) — без замка `rides`. Быстрый путь: идущие начаты без докупки —
/// ответ без чтения настройки, копий и текста (как раньше одно `contains`). Решение 405: встречная сделка на общем
/// счёте (coin_counter.rs) расчёт НЕ блокирует — отказ даёт admit с причиной в журнал core.start_refused (повторы —
/// счётчиком, reject_log.rs). Принимает: ядро, направление. Ошибок нет.
pub fn blocks_new(core: &Core, d: &Direction) -> bool {
    let key = CoinKey::of(&d.asset);
    if lk(&core.inner.active).contains(&key) {
        return true;
    }
    // Решение 405: ручное действие ждёт окончания сделок монеты — новых расчётов нет (атомарный быстрый путь).
    if super::manual_wait::paused(core, &key) {
        return true;
    }
    // Решение 403 (ревью t188): идущих сделок нет ни по одной монете — замок `rides` не берётся (атомарное чтение).
    if core.inner.rides_live.load(Ordering::Acquire) == 0 {
        return false;
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

/// Что делает: идёт ли группа докупки этого направления, в которую войдёт новая сделка (есть идущие сделки группы того
/// же направления, и её общий возврат ещё не ушёл — ревью 3, Н3) — для расчёта докупки (возврат USDT без
/// фиксированной части, п. 5). Решение 405: сделки других направлений монеты (их группы) не в счёт — прежде смотрелась
/// только первая запись монеты. Принимает: ядро, направление. Возвращает: ID группы; None — новая сделка будет первой
/// своей группы (фиксированная часть — в её плане). Ошибок нет.
pub fn group_running(core: &Core, d: &Direction) -> Option<String> {
    let key = CoinKey::of(&d.asset);
    let groups: Vec<String> = {
        let r = lk(&core.inner.rides);
        r.get(&key)?
            .iter()
            .filter(|x| same_dir(x, d))
            .filter_map(|x| x.group.clone())
            .collect()
    };
    groups
        .into_iter()
        .find(|g| !super::coin_group::flushed(core, g))
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
