//! Файл: core/sell_turn.rs
//! Что делает: решение 415 (t200, живой LQR 19.11: две сделки одной группы докупки продавали на bsc_dex в один момент;
//! котировка B взята по пулу ДО продажи A, минимальная цена B — от неё, а пул после A уже ниже: «продажа не ушла в
//! сеть: цена в пуле хуже допустимой») — ОЧЕРЕДЬ ПРОДАЖИ СЧЁТА: продажи сделок одной монеты на одной площадке
//! (Deal.at; ключ — как у credit_lock.rs: площадка + CoinKey, у DEX — все кошельки площадки: пул у них общий) идут
//! по одной. Ход очереди (Turn) — исключительный: котировка продажи (sell_turn_fresh.rs — данные не раньше прошлой
//! продажи на ключе) и ордер продажи этой котировки — под одним ходом; следующая сделка берёт котировку только после
//! итога ордера предыдущей и обновления её учёта в памяти (credit_lock::selling обновляет его до своего возврата).
//! Ход держится ТОЛЬКО на «котировка → итог ордера» и снимается сразу после итога (done) — до любых продолжений
//! (режим застрявшей монеты, повтор, перегон, алерт, ожидания идут без очереди): минуты ожиданий никого не держат.
//! Котировки-решения (режим застрявшей монеты, перегон, продажа здесь) берут ход только на время котировки.
//! Разные монеты и разные площадки — разные ключи: их продажи по-прежнему идут параллельно.
//!
//! Порядок замков (взаимной блокировки нет):
//! 1. sell_fact::before (чтение факта — credit_lock::fact, исключительный) — ДО хода; ход под ним не берётся.
//! 2. ход очереди (этот файл) — внутри него котировка и credit_lock::selling (общий доступ).
//! 3. Чтение факта (credit_lock::fact / fact_try — remain_read.rs, credit_base.rs) ход НЕ берёт: держатель хода ждёт
//!    общего доступа только до конца чтения факта, а чтение не ждёт хода. Все credit_lock::selling одного ключа — под
//!    ходом этого же ключа (sell.rs, sell_range_run.rs), поэтому держатель хода — единственный продавец ключа.
//! 4. Ход не вкладывается сам в себя: держатель хода не вызывает путей, которые берут ход (продолжения — после done).
//!
//! След прошлой продажи ключа (Last: конец, блок, пулы, известен ли маршрут) хранится в самом замке: им пользуется
//! проверка свежести следующей котировки; прошла (или вышел срок у продажи) — след снят. Замки и следы — только в памяти.
//! Ревью t200 (С-2): исход ордера «неизвестен» тоже оставляет след (конец — сейчас, блок неизвестен, пулы — из
//! запроса): транзакция могла попасть в блок. С-3: маршрут исполнения может отличаться от запроса, когда у площадки
//! включён внешний исполнитель шлюза (gateway/quote_stage.rs final_external) — маршрут следа «неизвестен».
//! Время последней продажи ключа (sold_after) — для котировки, взятой до чтения факта (relay.rs, М-3).
//! Оценки НЕ под ходом (М-4: к ордеру не ведут — только текст и решение человека): manual_relay.rs (оценка «продать
//! здесь» «Продать и перегнать»), sell_unsent.rs (убыток в алерте), manual_sell.rs (убыток кнопки), manual_sold_check.rs
//! (проверка «Продано вручную»).
//! Кто вызывает: core/sell.rs (sell_or_stuck_guarded, market_guarded → sell), core/sell_range_run.rs (branch),
//! core/sell_turn_fresh.rs (котировка под ходом; once — stuck.rs, relay.rs, relay_here.rs).
//! От чего зависит: sell_turn_last.rs (Last, route_known), coin_key.rs, op_error.rs, deal.rs, core/types, tokio::sync::Mutex.
//! Настройки: своих нет (базовое поведение, как credit_lock.rs).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use chrono::{DateTime, Utc};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::core::types::{Asset, DexRoute, TradeResult, VenueId};

use super::coin_key::CoinKey;
use super::deal::Deal;
use super::engine::Core;
use super::op_error::OpError;

pub use super::sell_turn_last::{Last, route_known};

/// Время последней исполненной (или с неизвестным исходом) продажи по ключу (М-3).
type Sold = HashMap<(VenueId, CoinKey), DateTime<Utc>>;

static SOLD: LazyLock<Mutex<Sold>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Что делает: была ли продажа по ключу позже `t` (М-3: котировка, взятая до ожидания, устарела). Принимает: площадку,
/// монету, время. Ошибок нет.
pub fn sold_after(v: &VenueId, a: &Asset, t: DateTime<Utc>) -> bool {
    SOLD.lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&(v.clone(), CoinKey::of(a)))
        .is_some_and(|x| *x > t)
}

/// Замок ключа очереди со следом прошлой продажи.
type Slot = Arc<AsyncMutex<Option<Last>>>;

/// Замки по (площадка, ключ монеты).
type Turns = HashMap<(VenueId, CoinKey), Slot>;

static TURNS: LazyLock<Mutex<Turns>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Ход очереди продажи ключа: держится от котировки до итога ордера (снимается done или drop).
pub struct Turn {
    g: OwnedMutexGuard<Option<Last>>,
}

/// Что делает: взять ход очереди продажи (ждёт, пока ход держит другая продажа этого ключа). Принимает: площадку,
/// монету. Возвращает: ход. Ошибок нет.
pub async fn take(v: &VenueId, a: &Asset) -> Turn {
    let slot = TURNS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .entry((v.clone(), CoinKey::of(a)))
        .or_default()
        .clone();
    Turn {
        g: slot.lock_owned().await,
    }
}

/// Что делает: ход свободен прямо сейчас (для тестов и проверок — без ожидания). Принимает: площадку, монету.
/// Возвращает: ход или None — ход держит другая продажа. Ошибок нет.
pub fn try_take(v: &VenueId, a: &Asset) -> Option<Turn> {
    let slot = TURNS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .entry((v.clone(), CoinKey::of(a)))
        .or_default()
        .clone();
    slot.try_lock_owned().ok().map(|g| Turn { g })
}

impl Turn {
    /// Что делает: след прошлой продажи ключа, ещё не проверенный свежей котировкой. Ошибок нет.
    pub fn last(&self) -> Option<&Last> {
        self.g.as_ref()
    }

    /// Что делает: след снят — котировка после него проверена (или срок ожидания вышел). Ошибок нет.
    pub fn seen(&mut self) {
        *self.g = None;
    }

    /// Что делает: итог ордера продажи под ходом — исполнено (в том числе частично) или исход «неизвестен» (С-2) —
    /// след для следующей котировки; ход снимается сразу (до продолжений). Принимает: ядро, сделку, маршрут запроса
    /// (котировки), итог ордера. Ошибок нет.
    pub fn done(
        mut self,
        core: &Core,
        deal: &Deal,
        route: Option<&DexRoute>,
        r: &Result<TradeResult, OpError>,
    ) {
        let known = route_known(core, &deal.at);
        let last = match r {
            Ok(x) if x.filled_qty > rust_decimal::Decimal::ZERO => Last::of(x, known),
            Err(e) if e.outcome_unknown() => Last::unknown(&deal.id, route, known),
            _ => return,
        };
        let key = (deal.at.clone(), CoinKey::of(&deal.direction.asset));
        SOLD.lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, last.at);
        *self.g = Some(last);
    }
}
