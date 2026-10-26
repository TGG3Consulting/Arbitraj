//! Файл: venues/bsc_dex/resubscribe.rs
//! Что делает: подписка на рынок и её пересборка на лету: задачи текущей подписки (при новой — старые
//! останавливаются, без дублей соединений), последний список монет и канал событий ядра; изменение настроек
//! промежуточных монет (dex.intermediate_enabled, dex.intermediate_tokens — решение 52) пересобирает подписку с тем же
//! списком, чтобы новые адреса искались сразу, без перезапуска (выключение действует и без пересборки — pools/mids.rs).
//! Кто вызывает: mod.rs (subscribe_market, watch_settings).
//! От чего зависит: parts.rs, pools/market.rs, settings/live.rs (Watch).
//! При подписке очередь кошелька восстанавливается из SQLite (wallet/queue_store.rs); остановленная — событие ядру.
//! С подпиской живёт и задача событий баланса кошелька (wallet/events.rs, решение 76).
//! Настройки: dex.intermediate_*, dex.native_ticker (следит за изменениями).

use std::sync::{Arc, Mutex};

use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::core::types::Asset;
use crate::gateway::errors::GatewayResult;
use crate::gateway::events::MarketEvent;
use crate::settings::live::{Change, Watch};
use crate::storage::journal::Level;

use super::parts::Parts;
use super::pools;

/// Последняя подписка: монеты и канал событий ядра.
type Last = Option<(Vec<Asset>, mpsc::Sender<MarketEvent>)>;

/// Состояние подписки. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Subs {
    tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    last: Arc<Mutex<Last>>,
}

impl Subs {
    /// Подписывает на рынок: запуск задач, остановка прежних, запоминание списка. Ошибки: как у pools/market.rs.
    pub async fn subscribe(
        &self,
        p: &Parts,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()> {
        // Очередь кошелька после запуска — из SQLite; остановленная — сразу ядру (уведомление, экран).
        p.nonces.restore(&p.ctx).await;
        let pools = p.pools(out.clone());
        if let st @ crate::core::types::WalletQueueState::Halted { .. } = p.nonces.state() {
            let q = crate::core::types::WalletQueue {
                venue: p.ctx.venue(),
                state: st,
                nonce_taken: None,
            };
            let _ = out.try_send(MarketEvent::WalletQueue(q));
        }
        let mut new = pools::market::subscribe(&pools, &p.scanned, &p.wake, assets.clone()).await?;
        // Изменения баланса кошелька событиями — для зачисления переводов без опроса (решение 76).
        new.push(tokio::spawn(super::wallet::events::run(
            pools.clone(),
            p.inflows.clone(),
        )));
        let mut tasks = self.tasks.lock().unwrap_or_else(|x| x.into_inner());
        for old in tasks.drain(..) {
            old.abort();
        }
        *tasks = new;
        *self.last.lock().unwrap_or_else(|x| x.into_inner()) = Some((assets, out));
        Ok(())
    }

    /// Следит за изменениями dex.intermediate_* и пересобирает подписку тем же списком. Работает, пока жива рассылка
    /// изменений. Ошибки пересборки — в журнал.
    pub fn watch(&self, p: Parts, rx: broadcast::Receiver<Change>) -> JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move {
            let mut w = Watch::new(
                rx,
                vec!["dex.intermediate_".into(), "dex.native_ticker".into()],
            );
            while let Some(c) = w.next().await {
                let last = me.last.lock().unwrap_or_else(|x| x.into_inner()).clone();
                let Some((assets, out)) = last else { continue };
                let msg = match me.subscribe(&p, assets, out).await {
                    Ok(()) => (
                        Level::Info,
                        format!("{} изменена — подписка DEX пересобрана", c.name),
                    ),
                    Err(e) => (
                        Level::Error,
                        format!(
                            "{} изменена, подписка не пересобрана: {}",
                            c.name, e.message
                        ),
                    ),
                };
                p.ctx.log(msg.0, "dex.intermediate", msg.1, None).await;
            }
        })
    }
}
