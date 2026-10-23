//! Файл: venues/bsc_dex/wallet/queue_store.rs
//! Что делает: хранение состояния очереди кошелька в SQLite (storage/wallet_queue.rs), чтобы оно пережило перезапуск:
//! «остановлена до ручной проверки» остаётся остановленной до кнопки «Снять очередь после ручной проверки»; «ждёт
//! итога» при запуске значит, что процесс прервался во время ожидания — исход транзакции неизвестен, очередь
//! становится «остановлена до ручной проверки» (решение 46: прерванное перезапуском само не продолжается).
//! Кто вызывает: wallet/nonce.rs (restore), send/settle.rs, wallet/deposit.rs (save).
//! От чего зависит: storage/wallet_queue.rs, ctx.rs (журнал и его база).
//! Настройки: нет.

use chrono::Utc;

use crate::core::types::WalletQueueState;
use crate::storage::journal::Level;
use crate::storage::wallet_queue;

use super::super::ctx::Ctx;

/// Сохраняет состояние. Сбой базы — в журнал (ошибка): после перезапуска состояние может быть старым. Ошибок нет.
pub async fn save(ctx: &Ctx, state: &WalletQueueState) {
    let res = match serde_json::to_string(state) {
        Ok(body) => wallet_queue::save(ctx.journal.db(), &ctx.venue().0, body)
            .await
            .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    if let Err(e) = res {
        let msg = format!("Состояние очереди кошелька не сохранено в SQLite: {e}");
        ctx.log(Level::Error, "send.queue_store", msg, None).await;
    }
}

/// Состояние после перезапуска. None — не сохранялось. «Ждёт итога» → «остановлена до ручной проверки».
/// Ошибки: Err(причина) — база не прочитана или запись не разобрана (вызывающий останавливает очередь).
pub async fn load(ctx: &Ctx) -> Result<Option<WalletQueueState>, String> {
    let body = wallet_queue::load(ctx.journal.db(), &ctx.venue().0)
        .await
        .map_err(|e| e.to_string())?;
    let Some(body) = body else { return Ok(None) };
    let st: WalletQueueState = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    Ok(Some(match st {
        WalletQueueState::Busy { nonce, hash, .. } => WalletQueueState::Halted {
            reason: format!(
                "перезапуск во время ожидания итога транзакции {hash} (nonce {nonce}) — исход неизвестен"
            ),
            nonce: Some(nonce),
            hash: Some(hash),
            since: Utc::now(),
        },
        other => other,
    }))
}
