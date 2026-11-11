//! Файл: venues/binance/withdraw_ready.rs
//! Что делает: решение 253 — готовность вывода купленной монеты. Живой случай (Тигран 10.11): 12 сделок Binance → DEX
//! (CYBER ×11, WLFI ×1) — вывод через 0.35–0.77 с после покупки отказан «−4026 [031033] User has insufficient balance» /
//! «−4024 [031031] User does not own this currency».
//! - остатки монет из потока аккаунта (outboundAccountPosition, account_update.rs) — свободно по монете;
//! - wait (контракт withdraw_ready): свободно на споте ≥ суммы вывода — сразу; нет — ждём событие не дольше срока ядра
//!   (venue.withdraw_ready_wait_ms); не дождались — один запрос баланса REST (GET /api/v3/account, account.rs);
//! - своя сделка по монете (traded, из mod.rs trade) и сколько прошло с неё (since_trade) — окно, в котором ответы
//!   −4026 / −4024 на вывод этой монеты считаются «монета ещё не доступна для вывода» (transfers.rs, решение 253).
//!
//! Дополнение t89 (Тигран 11.11, живой случай 20261111122340181OPEN: продажа на Binance 12:24:21.980, возврат USDT
//! 12:24:21.981, отказ «−4026 [031033] User has insufficient balance» в 12:24:22.582): отметка — у ЛЮБОЙ своей
//! исполненной сделки (покупки и продажи), по монете сделки и по котируемой монете (venue.quote_asset — выручка USDT);
//! от неё же — пауза ядра venue.withdraw_min_delay_ms (контракт since_own_trade). Остаток события, полученного раньше
//! отправки своего ордера по монете, — не свежий: ждём событие после ордера (до сделки на споте могли лежать свои USDT —
//! «свободно ≥ суммы» по старому событию ничего не говорит о выручке).
//!
//! Один адаптер Binance на процесс — состояние общее (как LAST_HISTORY в transfers.rs).
//! Кто вызывает: user_stream_conn.rs (on_position), mod.rs (wait — контракт withdraw_ready; traded — после ордера;
//! since_trade — контракт since_own_trade), transfers.rs (since_trade).
//! От чего зависит: account.rs (REST), ctx.rs, core/types (WithdrawReady).
//! Настройки: venue.withdraw_wallet [binance] (ожидание — только для spot: поток аккаунта — остатки спота);
//! venue.quote_asset (через mod.rs — отметка котируемой монеты).
//!
//! Официально: user-data-stream «Account Update» — outboundAccountPosition «is sent any time an account balance has
//! changed and contains the assets that were possibly changed by the event that generated the balance change», поля
//! B[].a (монета), B[].f (свободно), B[].l (в ордерах); rest-api «Account information» — GET /api/v3/account,
//! balances[].free. Вывод POST /sapi/v1/capital/withdraw/apply с walletType 0 — со спот-счёта (transfers.rs).
//! Что вывод Binance принимает ровно при свободном споте ≥ суммы — в документации не сказано; живой записи отказа
//! −4026 вместе с кадрами потока нет — проверить на живом (вопрос в docs/TASKS.md).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use rust_decimal::Decimal;
use tokio::sync::Notify;
use tokio::time::Instant;

use crate::core::types::{Asset, WithdrawReady};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;

/// (свободно, время события E, мс; когда событие получено).
type Free = (Decimal, i64, Instant);
/// Свободный остаток монеты по последнему событию: (свободно, время события E, мс; когда событие получено).
static FREE: LazyLock<Mutex<HashMap<String, Free>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Будит ожидающих при новом событии остатков.
static WAKE: LazyLock<Notify> = LazyLock::new(Notify::new);
/// t89: (ордер отправлен, итог получен) последней своей сделки по монете.
type Marks = HashMap<String, (Instant, Instant)>;
/// t89: последняя своя сделка по монете (ордер исполнен хоть частично).
static TRADED: LazyLock<Mutex<Marks>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Что делает: запоминает остатки события outboundAccountPosition (событие старше уже записанного — пропуск) и будит
/// ожидающих. Принимает: (монета, свободно), время события E (мс). Ошибок нет.
pub fn on_position(list: &[(String, Decimal)], at: i64) {
    {
        let mut m = FREE.lock().unwrap_or_else(|p| p.into_inner());
        let rx = Instant::now();
        for (a, f) in list {
            match m.get(a) {
                Some((_, t, _)) if *t > at => {}
                _ => {
                    m.insert(a.clone(), (*f, at, rx));
                }
            }
        }
    }
    WAKE.notify_waiters();
}

/// Свободно по последнему событию. Принимает: монету (тикер Binance). Возвращает: None — событий по монете не было.
/// Ошибок нет.
pub fn free(ticker: &str) -> Option<Decimal> {
    FREE.lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(ticker)
        .map(|(f, ..)| *f)
}

/// t89: свободно по событию, полученному ПОСЛЕ отправки своего последнего ордера по монете (своей сделки не было —
/// любое событие). Принимает: монету. Возвращает: None — свежего события нет. Ошибок нет.
fn fresh_free(ticker: &str) -> Option<Decimal> {
    let sent = TRADED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(ticker)
        .map(|(s, _)| *s);
    FREE.lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(ticker)
        .filter(|(_, _, rx)| sent.is_none_or(|s| *rx >= s))
        .map(|(f, ..)| *f)
}

/// Что делает: отмечает свою сделку по монете (mod.rs после ордера покупки или продажи с исполненным количеством > 0 —
/// по монете сделки и по котируемой монете). Принимает: монету, когда ордер отправлен. Ошибок нет.
pub fn traded(ticker: &str, sent: Instant) {
    TRADED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(ticker.to_string(), (sent, Instant::now()));
}

/// Сколько прошло с последней своей сделки по монете (с получения итога ордера). None — сделок не было (с запуска).
/// Ошибок нет.
pub fn since_trade(ticker: &str) -> Option<Duration> {
    TRADED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(ticker)
        .map(|(_, done)| done.elapsed())
}

/// Что делает: ждёт, пока свободно на споте станет ≥ amount: событие потока аккаунта (t89: полученное после своего
/// последнего ордера по монете), не дольше wait; не дождались — один запрос баланса REST. Принимает: окружение, монету (тикер Binance), сумму вывода, срок ожидания.
/// Возвращает: WithdrawReady (ready None — счёт вывода не spot: поток даёт остатки спота, сверять нечего).
/// Ошибки: настройка venue.withdraw_wallet, REST баланса (ключи, связь, ответ).
pub async fn wait(
    ctx: &Ctx,
    asset: &Asset,
    amount: Decimal,
    wait: Duration,
) -> GatewayResult<WithdrawReady> {
    if ctx.text("venue.withdraw_wallet")? != "spot" {
        return Ok(WithdrawReady {
            ready: None,
            free: None,
            source: "вывод с funding — остатки потока аккаунта (спот) не сверяются".into(),
        });
    }
    let ticker = asset.ticker.as_str();
    let end = Instant::now() + wait;
    loop {
        // Подписка на пробуждение — до проверки: событие между проверкой и ожиданием не теряется.
        let woke = WAKE.notified();
        tokio::pin!(woke);
        woke.as_mut().enable();
        // t89: только событие после своего ордера по монете (выручка USDT, купленная монета).
        if let Some(f) = fresh_free(ticker).filter(|f| *f >= amount) {
            return Ok(WithdrawReady {
                ready: Some(true),
                free: Some(f),
                source: "событие баланса outboundAccountPosition".into(),
            });
        }
        tokio::select! {
            _ = woke => {}
            _ = tokio::time::sleep_until(end) => break,
        }
    }
    let list = super::account::balances(ctx).await?;
    let f = list
        .iter()
        .find(|b| b.asset == ticker)
        .map_or(Decimal::ZERO, |b| b.free);
    Ok(WithdrawReady {
        ready: Some(f >= amount),
        free: Some(f),
        source: "REST GET /api/v3/account (события не было за срок)".into(),
    })
}
