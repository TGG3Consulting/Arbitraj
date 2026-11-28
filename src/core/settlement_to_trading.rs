//! Файл: core/settlement_to_trading.rs
//! Что делает: денежный круг Base (решения Тиграна 28.11.2026) — после КАЖДОГО подтверждённого зачисления на биржу и
//! ДО ордера на бирже тот же перевод зачисленного на торговый счёт, что в обычной сделке (transfer_credit::to_trading:
//! решение 102, Venue::deposit_to_trading, правила шлюза на сбой, решение 413 «мало средств» → один повтор по факту
//! счёта, журнал и уведомление transfer.to_trading_failed). Логика перевода не дублируется — вызывается та же функция.
//! Два места: монета сделки перед продажей (settlement_cycle.rs) и расчётная монета (USDC) перед IOC USDC → USDT
//! (settlement_path.rs, Leg::Return).
//! Отметка шага привязана к хешу зачисления и хранится там, где этот путь хранит своё состояние (Store): в плане сделки
//! или в сохранённом денежном пути. Повтор после перезапуска:
//! - «ready» — монета на торговом счёте (или переводить нечего): шаг пропускается целиком, без запросов;
//! - «started» — перевод начат, итог не записан: сначала факт торгового счёта; хватает — шаг закрыт без перевода
//!   (перевод уже прошёл, второй дал бы «мало средств» и ложную тревогу); не хватает — перевод ещё раз;
//! - «failed» — перевод не удался (журнал и уведомление уже ушли): нового перевода нет, только факт торгового счёта —
//!   человек перевёл вручную, шаг закрыт; нет — ордер не ставится.
//!
//! Перевод не удался и на торговом счёте меньше зачисленного — ошибка до ордера: ставить ордер вслепую на сумму больше,
//! чем есть на торговом счёте, нельзя (отказ биржи после записи заявки оставил бы заявку без исполнения).
//! Кто вызывает: core/settlement_cycle.rs (advance), core/settlement_path.rs (run, Leg::Return).
//! От чего зависит: transfer_credit.rs (to_trading, Leg), limits.rs (free_now — торговый счёт, Venue::balance),
//! settlement_cycle.rs (save сделки), settlement_recovery.rs (save пути), шлюз (get_as).
//! Настройки: своих нет; у адаптеров — venue.deposit_to_spot (как в обычной сделке).

use rust_decimal::Decimal;
use serde_json::{Value, json};

use super::deal::Deal;
use super::engine::Core;
use super::settlement_state::Path;
use super::transfer_credit::Leg;
use super::types::{Asset, Transfer, VenueId};

/// Поле плана сделки с отметкой шага (монета сделки).
pub const FIELD: &str = "settlement_to_trading";
const STARTED: &str = "started";
const FAILED: &str = "failed";
const READY: &str = "ready";

/// Где хранится отметка шага: сделка круга (монета сделки) или сохранённый денежный путь (USDC возврата).
pub(super) enum Store<'a> {
    Deal(&'a mut Deal),
    Path(&'a crate::storage::db::Db, &'a mut Path),
}

impl Store<'_> {
    /// Что делает: ID сделки. Ошибок нет.
    fn deal(&self) -> String {
        match self {
            Store::Deal(d) => d.id.clone(),
            Store::Path(_, p) => p.plan.deal.clone(),
        }
    }

    /// Что делает: сохранённая отметка. Возвращает: значение поля или None. Ошибок нет.
    fn saved(&self) -> Option<&Value> {
        match self {
            Store::Deal(d) => d.plan.get(FIELD),
            Store::Path(_, p) => p.to_trading.as_ref(),
        }
    }

    /// Что делает: пишет отметку и сохраняет её владельца. Ошибки: сбой записи (settlement_cycle::save — сделка;
    /// settlement_recovery::save — путь, с проверкой прежней версии).
    async fn mark(&mut self, core: &Core, tx: &str, s: &str) -> Result<(), String> {
        let v = json!({ "tx": tx, "state": s });
        match self {
            Store::Deal(d) => {
                d.plan[FIELD] = v;
                super::settlement_cycle::save(core, &**d).await
            }
            Store::Path(db, p) => {
                let previous = (**p).clone();
                p.to_trading = Some(v);
                super::settlement_recovery::save(*db, &**p, Some(&previous)).await
            }
        }
    }
}

/// Что делает: переводит зачисленное на торговый счёт биржи и проверяет, что ордеру есть что тратить (шапка).
/// Принимает: ядро, хранилище отметки, (отправитель, биржа-получатель), монету для вызова перевода, тикер монеты на
/// бирже (баланс торгового счёта), подтверждённое зачисление. Возвращает: Ok — ордер можно ставить. Ошибки: нет хеша
/// зачисления, сбой записи или чтения баланса, перевод не удался и на торговом счёте меньше зачисленного.
pub(super) async fn ensure(
    core: &Core,
    mut store: Store<'_>,
    (from, to): (&VenueId, &VenueId),
    asset: &Asset,
    ticker: &str,
    credit: &Transfer,
) -> Result<(), String> {
    let tx = credit
        .tx_hash
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or("У зачисления на биржу нет хеша")?;
    let deal = store.deal();
    let prior = state(store.saved(), &tx);
    if prior.as_deref() == Some(READY) {
        return Ok(());
    }
    if prior.is_some() {
        // Повтор после перезапуска: перевод мог уже пройти — сначала факт торгового счёта, без нового перевода.
        let free = super::limits::free_now(core, to, None, ticker).await?;
        if free >= credit.amount {
            return store.mark(core, &tx, READY).await;
        }
        if prior.as_deref() == Some(FAILED) {
            return Err(short(&deal, credit, asset, to, free));
        }
    }
    store.mark(core, &tx, STARTED).await?;
    let gw = core
        .inner
        .deps
        .gw
        .get_as(to, None)
        .await
        .map_err(|e| e.message)?;
    let leg = Leg {
        deal: &deal,
        from,
        to,
        asset,
        network: credit.network.clone(),
        tx: tx.clone(),
        started: credit.updated_at,
    };
    if super::transfer_credit::to_trading(core, &leg, &gw, credit).await {
        return store.mark(core, &tx, READY).await;
    }
    store.mark(core, &tx, FAILED).await?;
    let free = super::limits::free_now(core, to, None, ticker).await?;
    if free >= credit.amount {
        return store.mark(core, &tx, READY).await;
    }
    Err(short(&deal, credit, asset, to, free))
}

/// Что делает: состояние отметки для этого зачисления. Принимает: сохранённую отметку, хеш зачисления.
/// Возвращает: состояние; None — для этого хеша шаг ещё не начинался. Ошибок нет.
fn state(saved: Option<&Value>, tx: &str) -> Option<String> {
    let v = saved?;
    if v.get("tx").and_then(Value::as_str) != Some(tx) {
        return None;
    }
    v.get("state").and_then(Value::as_str).map(str::to_owned)
}

/// Что делает: текст остановки до ордера. Принимает: сделку, зачисление, монету, биржу, свободно на торговом.
/// Возвращает: текст. Ошибок нет.
fn short(deal: &str, credit: &Transfer, asset: &Asset, to: &VenueId, free: Decimal) -> String {
    format!(
        "{deal}: {} {} зачислено на {}, но не переведено на торговый счёт (на торговом {free}) — ордер не ставится; \
         переведите вручную, сделка продолжится при следующей сверке",
        credit.amount, asset.ticker, to.0
    )
}
