//! Файл: core/usdt_relay_retry.rs
//! Что делает: перегон USDT, который не срывается из-за отставания узла (решение 108). Живой случай 02.11 14:16:
//! выручка NEAR (44.07 USDT) уже была в кошельке, а перевод сразу после продажи попал на сервер публичного узла BSC,
//! который ещё видел старый баланс — откат «BEP20: transfer amount exceeds balance», перегон бросили без повтора и
//! без уведомления. Теперь: (1) перед отправкой ждём, пока площадка-отправитель покажет на свободном балансе всю
//! сумму; (2) откат (SwapReverted — денег не ушло) повторяется после паузы, снова с ожиданием баланса; (3) перегон
//! так и не состоялся — уведомление usdt.relay_failed (деньги остались на площадке-отправителе).
//! Повтор — только при откате: при «исход неизвестен» и прочих ошибках вывод мог уйти, повторять нельзя.
//! Кто вызывает: core/usdt_return.rs (relay).
//! От чего зависит: transfers.rs, limits.rs (free_now), settings.
//! Настройки: usdt_return.balance_wait_ms, usdt_return.balance_poll_ms, usdt_return.retry_attempts,
//! usdt_return.retry_pause_ms (все необязательные: не задано — без ожидания и без повтора, как раньше).

use std::time::{Duration, Instant};

use rust_decimal::Decimal;

use crate::core::types::{Asset, Transfer, VenueId};
use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;

use super::engine::Core;
use super::op_error::OpError;

/// Отправляет перегон с ожиданием баланса и повтором при откате. Принимает: ядро, ID, откуда, куда, монету, сумму.
/// Возвращает: (вывод, зачисление) как transfers::send. Ошибки: последняя ошибка перевода или «баланс не появился».
pub async fn send(
    core: &Core,
    id: &str,
    from: &VenueId,
    to: &VenueId,
    asset: &Asset,
    amount: Decimal,
) -> Result<(Transfer, Transfer), OpError> {
    let s = &core.inner.deps.settings;
    let int = |n: &str| s.integer(n, At::default()).ok();
    let attempts = int("usdt_return.retry_attempts").unwrap_or(0) + 1;
    let pause = Duration::from_millis(int("usdt_return.retry_pause_ms").unwrap_or(0));
    let mut n = 0;
    loop {
        n += 1;
        wait_balance(core, from, &asset.ticker, amount).await?;
        match super::transfers::send(core, id, from, to, asset, amount).await {
            Ok(r) => return Ok(r),
            Err(e) if e.code == Some(ErrorCode::SwapReverted) && n < attempts => {
                core.inner
                    .deps
                    .log(
                        crate::storage::journal::Level::Warn,
                        "core.usdt_return",
                        Some(&from.0),
                        format!(
                            "Перегон USDT {} → {}: откат ({e}) — повтор {}/{} через {} мс",
                            from.0,
                            to.0,
                            n,
                            attempts - 1,
                            pause.as_millis()
                        ),
                        None,
                    )
                    .await;
                tokio::time::sleep(pause).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Ждёт, пока на свободном балансе площадки появится сумма (не дольше usdt_return.balance_wait_ms, опрос раз в
/// usdt_return.balance_poll_ms). Не задано — сразу дальше. Ошибка: сумма так и не появилась (текст с балансом).
async fn wait_balance(
    core: &Core,
    venue: &VenueId,
    ticker: &str,
    amount: Decimal,
) -> Result<(), OpError> {
    let s = &core.inner.deps.settings;
    let (Ok(wait), Ok(poll)) = (
        s.integer("usdt_return.balance_wait_ms", At::default()),
        s.integer("usdt_return.balance_poll_ms", At::default()),
    ) else {
        return Ok(());
    };
    let started = Instant::now();
    loop {
        let last = match super::limits::free_now(core, venue, ticker).await {
            Ok(free) if free >= amount => return Ok(()),
            Ok(free) => format!("свободно {free} {ticker}"),
            Err(e) => e,
        };
        if started.elapsed() >= Duration::from_millis(wait) {
            return Err(OpError::from(format!(
                "{}: за {wait} мс на балансе не появилось {amount} {ticker} ({last})",
                venue.0
            )));
        }
        tokio::time::sleep(Duration::from_millis(poll)).await;
    }
}
