//! Файл: venues/binance/withdraw_ready.rs
//! Что делает: решение 253 — готовность вывода купленной монеты. Живой случай (Тигран 10.11): 12 сделок Binance → DEX
//! (CYBER ×11, WLFI ×1) — вывод через 0.35–0.77 с после покупки отказан «−4026 [031033] User has insufficient balance» /
//! «−4024 [031031] User does not own this currency».
//! - остатки монет из потока аккаунта (outboundAccountPosition, account_update.rs) — свободно по монете;
//! - wait (контракт withdraw_ready): свободно на споте ≥ суммы вывода — сразу; нет — ждём событие не дольше срока ядра
//!   (venue.withdraw_ready_wait_ms); не дождались — один запрос баланса REST (GET /api/v3/account, account.rs);
//! - своя покупка монеты (bought, из mod.rs trade) и сколько прошло с неё (since_buy) — окно, в котором ответы
//!   −4026 / −4024 на вывод этой монеты считаются «монета ещё не доступна для вывода» (transfers.rs, решение 253).
//!
//! Один адаптер Binance на процесс — состояние общее (как LAST_HISTORY в transfers.rs).
//! Кто вызывает: user_stream_conn.rs (on_position), mod.rs (wait — контракт withdraw_ready; bought — после ордера),
//! transfers.rs (since_buy).
//! От чего зависит: account.rs (REST), ctx.rs, core/types (WithdrawReady).
//! Настройки: venue.withdraw_wallet [binance] (ожидание — только для spot: поток аккаунта — остатки спота).
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

/// Свободный остаток монеты по последнему событию: (свободно, время события E, мс).
static FREE: LazyLock<Mutex<HashMap<String, (Decimal, i64)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// Будит ожидающих при новом событии остатков.
static WAKE: LazyLock<Notify> = LazyLock::new(Notify::new);
/// Когда адаптер последний раз купил монету (ордер исполнен хоть частично).
static BOUGHT: LazyLock<Mutex<HashMap<String, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Что делает: запоминает остатки события outboundAccountPosition (событие старше уже записанного — пропуск) и будит
/// ожидающих. Принимает: (монета, свободно), время события E (мс). Ошибок нет.
pub fn on_position(list: &[(String, Decimal)], at: i64) {
    {
        let mut m = FREE.lock().unwrap_or_else(|p| p.into_inner());
        for (a, f) in list {
            match m.get(a) {
                Some((_, t)) if *t > at => {}
                _ => {
                    m.insert(a.clone(), (*f, at));
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
        .map(|(f, _)| *f)
}

/// Что делает: отмечает свою покупку монеты (mod.rs после ордера BUY с исполненным количеством > 0). Ошибок нет.
pub fn bought(ticker: &str) {
    BOUGHT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(ticker.to_string(), Instant::now());
}

/// Сколько прошло с последней своей покупки монеты. None — покупок не было (с запуска). Ошибок нет.
pub fn since_buy(ticker: &str) -> Option<Duration> {
    BOUGHT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(ticker)
        .map(Instant::elapsed)
}

/// Что делает: ждёт, пока свободно на споте станет ≥ amount: событие потока аккаунта, не дольше wait; не дождались —
/// один запрос баланса REST. Принимает: окружение, монету (тикер Binance), сумму вывода, срок ожидания.
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
        if let Some(f) = free(ticker).filter(|f| *f >= amount) {
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
