//! Файл: venues/bsc_dex/pools/invalidated.rs
//! Что делает: событие ядру «пулы монеты недостоверны» (MarketEvent::Invalidated) без молчаливой потери (ревью #62).
//! Кто вызывает: pools/listen_log.rs (обрыв пачки, ошибка чтения пула).
//! От чего зависит: listen.rs (Pools: канал событий), ctx.rs.
//! Настройки: нет.
//! Решение 211: монета — адрес; ядро находит её по (площадка, тикер площадки), а тикер DEX — показное имя монеты
//! ядра из подписки (tokens.rs). Тикер события берётся по адресу монеты из текущей подписки; запись пула хранит тикер
//! на момент загрузки (он мог смениться) — он только запасной.

use tokio::sync::mpsc::error::TrySendError;

use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::listen::Pools;

/// Invalidated ядру без молчаливой потери (ревью #62): реестр уже отмечен синхронно; канал полон — отправка отдельной
/// задачей, когда место появится; канал закрыт или отправка не удалась — запись в журнал. Принимает: адрес монеты,
/// запасной тикер (из записи пула). Ошибок нет.
pub fn send(p: &Pools, coin: alloy_primitives::Address, fallback: String) {
    let ticker = p.tokens.ticker_of(coin).unwrap_or(fallback);
    let ev = MarketEvent::Invalidated {
        venue: p.ctx.venue(),
        ticker: ticker.clone(),
    };
    let lost = |ctx: super::super::ctx::Ctx, ticker: String| async move {
        let msg =
            format!("{ticker}: событие «пулы недостоверны» ядру не ушло — канал событий закрыт");
        ctx.log(Level::Warn, "dex.pool", msg, None).await;
    };
    match p.out.try_send(ev) {
        Ok(()) => {}
        Err(TrySendError::Full(ev)) => {
            let (out, ctx) = (p.out.clone(), p.ctx.clone());
            tokio::spawn(async move {
                if out.send(ev).await.is_err() {
                    lost(ctx, ticker).await;
                }
            });
        }
        Err(TrySendError::Closed(_)) => {
            tokio::spawn(lost(p.ctx.clone(), ticker));
        }
    }
}
