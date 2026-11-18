//! Файл: core/commands_ext.rs
//! Что делает: решение 408 (t193, Тигран 18.11: «обрабатывать тревоги прямо из Telegram») — команды человека, которые
//! раньше были только кнопками интерфейса мимо ядра, теперь — общим путём Core::command (журнал core.command: кто
//! нажал и откуда; итог — событие Command):
//! - «Снять очередь кошелька» (ReleaseWalletQueue) — контракт release_wallet_queue адаптера DEX (прежде
//!   api/wallet_queue.rs звал его сам); nonce удержанной транзакции не занят и pending_ok нет — отказ адаптера
//!   NonceConflict (второе подтверждение);
//! - «Открыть вход монеты» (OpenCoinEntry) и «Убрать из чёрного списка» (Unblacklist) — исключение из решения 157
//!   (Тигран 18.11): правка настроек coin.entry_blocked_* / coin.blocked* как у экрана «Монеты» (WithdrawBlocked.svelte,
//!   ManualBlacklist.svelte) — Store::set с автором «кто нажал» (история изменений настроек, как PUT /api/settings);
//! - «Развернуть исполнитель» (DeployExecutor) — core/executor_deploy.rs (тот же путь, что у интерфейса).
//! Кто вызывает: core/commands.rs (run_command).
//! От чего зависит: engine.rs, executor_deploy.rs, gateway (Registry, контракт Venue), settings (Store, Scope).
//! Настройки: пишет coin.entry_blocked_venues, coin.entry_blocked_note, coin.blocked, coin.blocked_note (область
//! «монета»); свои — нет.

use serde_json::Value;

use crate::core::types::{DexAccountId, ManualAction, VenueId};
use crate::settings::scope::Scope;

use super::engine::Core;

/// Что делает: выполнить команду решения 408. Принимает: ядро, действие, кто (commands::who). Возвращает: None —
/// действие не отсюда; Some(Ok(итог)) или Some(Err(причина)). Ошибок сверх итога нет.
pub async fn command(core: &Core, a: &ManualAction, who: &str) -> Option<Result<String, String>> {
    Some(match a {
        ManualAction::ReleaseWalletQueue {
            venue,
            wallet,
            pending_ok,
        } => release_queue(core, venue, wallet.as_deref(), *pending_ok, who).await,
        ManualAction::OpenCoinEntry { coin, venue } => open_entry(core, coin, venue, who).await,
        ManualAction::Unblacklist { coin } => unblacklist(core, coin, who).await,
        ManualAction::DeployExecutor {
            venue,
            wallet,
            sha256,
            wnative,
        } => {
            let d = &core.inner.deps;
            super::executor_deploy::deploy(
                &d.gw,
                &d.settings,
                &d.notifier,
                venue,
                Some(wallet.as_str()),
                sha256,
                Some(wnative.as_str()),
            )
            .await
            .map(|x| x.text)
            .map_err(|e| e.to_string())
        }
        _ => return None,
    })
}

/// «Снять очередь кошелька»: контракт release_wallet_queue (адаптер пишет журнал send.queue_released и событие).
/// Принимает: ядро, площадку, кошелёк, подтверждение незанятого nonce, кто. Ошибки: текст шлюза (NonceConflict —
/// nonce не занят, нужно второе подтверждение; NotApplicable — очередь не остановлена).
async fn release_queue(
    core: &Core,
    venue: &str,
    wallet: Option<&str>,
    pending_ok: bool,
    who: &str,
) -> Result<String, String> {
    let acc = wallet.map(|w| DexAccountId(w.to_string()));
    let v = core
        .inner
        .deps
        .gw
        .get_as(&VenueId(venue.to_string()), acc.as_ref())
        .await
        .map_err(|e| e.message)?;
    let by = format!("вручную, {who}");
    v.release_wallet_queue(&by, pending_ok)
        .await
        .map(|_| format!("очередь кошелька {} снята ({by})", wallet.unwrap_or(venue)))
        .map_err(|e| e.message)
}

/// Что делает: правка настройки области «монета» с автором «кто нажал». Принимает: ядро, имя, id монеты, значение
/// (None — очистить), кто. Ошибки: текст отказа хранилища.
async fn set(
    core: &Core,
    name: &str,
    coin: &str,
    v: Option<Value>,
    who: &str,
) -> Result<(), String> {
    core.inner
        .deps
        .settings
        .set(name, Scope::Coin(coin.to_string()), v, who)
        .await
        .map(|_| ())
        .map_err(|e| format!("{name}: {e}"))
}

/// «Открыть вход монеты» с биржи — как «Открыть» экрана «Монеты» (WithdrawBlocked.svelte open). Ошибки: биржи нет в
/// списке (уже открыто — не ошибка, итог «уже открыт»), отказ хранилища.
async fn open_entry(core: &Core, coin: &str, venue: &str, who: &str) -> Result<String, String> {
    use super::withdraw_block::{NOTE, VENUES};
    let s = &core.inner.deps.settings;
    let cur: Vec<String> = s
        .get_exact(VENUES, &Scope::Coin(coin.to_string()))
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|x| x.as_str().map(str::to_string))
        .collect();
    if !cur.iter().any(|v| v == venue) {
        return Ok(format!(
            "вход {coin} с {venue} уже открыт — команда проигнорирована"
        ));
    }
    let rest: Vec<Value> = cur
        .into_iter()
        .filter(|v| v != venue)
        .map(Value::String)
        .collect();
    if rest.is_empty() {
        set(core, VENUES, coin, None, who).await?;
        set(core, NOTE, coin, None, who).await?;
    } else {
        set(core, VENUES, coin, Some(Value::Array(rest)), who).await?;
    }
    Ok(format!("вход {coin} с {venue} открыт ({VENUES})"))
}

/// «Убрать из чёрного списка» — как «Убрать» экрана «Монеты» (ManualBlacklist.svelte remove): coin.blocked и
/// coin.blocked_note очищаются. Ошибки: отказ хранилища.
async fn unblacklist(core: &Core, coin: &str, who: &str) -> Result<String, String> {
    let s = &core.inner.deps.settings;
    let on = s
        .get_exact("coin.blocked", &Scope::Coin(coin.to_string()))
        .and_then(|v| v.as_bool())
        == Some(true);
    if !on {
        return Ok(format!(
            "{coin} не в ручном чёрном списке — команда проигнорирована"
        ));
    }
    set(core, "coin.blocked", coin, None, who).await?;
    set(core, "coin.blocked_note", coin, None, who).await?;
    Ok(format!(
        "{coin} убрана из ручного чёрного списка (coin.blocked)"
    ))
}
