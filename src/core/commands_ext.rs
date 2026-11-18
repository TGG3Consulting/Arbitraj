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
//!
//! Кто вызывает: core/commands.rs (run_command).
//! От чего зависит: engine.rs, executor_deploy.rs, gateway (Registry, контракт Venue), settings (Store, Scope).
//! Настройки: пишет coin.entry_blocked_venues, coin.entry_blocked_note, coin.blocked, coin.blocked_note (область
//! «монета»); свои — нет.

use serde_json::Value;

use crate::core::types::{DexAccountId, ManualAction, VenueId, WalletQueue, WalletQueueState};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
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
            nonce,
            hash,
        } => {
            let held = (nonce.is_some() || hash.is_some()).then_some((*nonce, hash.as_deref()));
            release_queue(core, venue, wallet.as_deref(), *pending_ok, held, who)
                .await
                .map(|_| {
                    format!(
                        "очередь кошелька {} снята (вручную, {who})",
                        wallet.as_deref().unwrap_or(venue)
                    )
                })
                .map_err(|e| e.message)
        }
        ManualAction::OpenCoinEntry { coin, venue } => open_entry(core, coin, venue, who).await,
        ManualAction::Unblacklist { coin } => unblacklist(core, coin, who).await,
        ManualAction::DeployExecutor {
            venue,
            wallet,
            sha256,
            wnative,
            executor,
        } => {
            let d = &core.inner.deps;
            let r = super::executor_deploy::Req {
                venue,
                wallet: Some(wallet.as_str()),
                sha256,
                wnative: Some(wnative.as_str()),
                executor: executor.as_deref(),
                v1_only: true,
            };
            // Ревью t193 (М4): код в сети не совпал — «не выполнено», не «выполнено».
            match super::executor_deploy::deploy(&d.gw, &d.settings, &d.notifier, &r).await {
                Ok(x) if x.code_ok => Ok(x.text),
                Ok(x) => Err(format!("код в сети не совпал с репозиторием — {}", x.text)),
                Err(e) => Err(e.to_string()),
            }
        }
        _ => return None,
    })
}

/// «Снять очередь кошелька»: контракт release_wallet_queue (адаптер пишет журнал send.queue_released и событие).
/// Ревью t193 (Н2): held — удержанная транзакция (nonce, hash 0x…) из вопроса Telegram: остановка сейчас другая (или
/// очереди уже не остановлена) — отказ, устаревшее «Да» не снимает чужую остановку. Ревью t193 (Н3): ошибка — шлюза, с
/// кодом (интерфейс отдаёт 409 NonceConflict для второго подтверждения, 404, 502). Принимает: ядро, площадку, кошелёк,
/// подтверждение незанятого nonce, удержанное из вопроса, кто. Возвращает: очередь после снятия (ответ адаптера).
/// Ошибки: шлюз (NonceConflict — nonce не занят, нужно второе подтверждение; NotApplicable — очередь не остановлена или
/// остановка другая).
pub async fn release_queue(
    core: &Core,
    venue: &str,
    wallet: Option<&str>,
    pending_ok: bool,
    held: Option<(Option<u64>, Option<&str>)>,
    who: &str,
) -> GatewayResult<WalletQueue> {
    let id = VenueId(venue.to_string());
    let acc = wallet.map(|w| DexAccountId(w.to_string()));
    let v = core.inner.deps.gw.get_as(&id, acc.as_ref()).await?;
    if let Some((n, h)) = held {
        let q = v.wallet_queue().await?;
        let now = match &q.state {
            WalletQueueState::Halted { nonce, hash, .. } => {
                Some((*nonce, (*hash).map(|x| format!("{x:#x}"))))
            }
            _ => None,
        };
        let want = (n, h.map(str::to_ascii_lowercase));
        if now.as_ref() != Some(&want) {
            let was = |x: &(Option<u64>, Option<String>)| {
                format!(
                    "nonce {}, транзакция {}",
                    x.0.map_or_else(|| "?".into(), |n| n.to_string()),
                    x.1.as_deref().unwrap_or("?")
                )
            };
            let cur = now
                .as_ref()
                .map_or_else(|| "очередь не остановлена".into(), was);
            return Err(GatewayError::new(
                ErrorCode::NotApplicable,
                Some(id),
                format!(
                    "остановка уже другая: в вопросе {}, сейчас {cur} — нажмите кнопку на новой тревоге",
                    was(&want)
                ),
            ));
        }
    }
    v.release_wallet_queue(&format!("вручную, {who}"), pending_ok)
        .await
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
