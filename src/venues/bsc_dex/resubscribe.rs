//! Файл: venues/bsc_dex/resubscribe.rs
//! Что делает: подписка на рынок и её пересборка на лету: задачи текущей подписки (при новой — старые
//! останавливаются, без дублей соединений), последний список монет и канал событий ядра; изменение настроек
//! промежуточных монет (dex.intermediate_enabled, dex.intermediate_tokens — решение 52) пересобирает подписку с тем же
//! списком, чтобы новые адреса искались сразу, без перезапуска (выключение действует и без пересборки — pools/mids.rs).
//! Кто вызывает: mod.rs (subscribe_market, watch_settings).
//! От чего зависит: parts.rs, pools/market.rs, settings/live.rs (Watch).
//! При подписке очередь кошелька восстанавливается из SQLite (wallet/queue_store.rs); остановленная — событие ядру.
//! С подпиской живут задачи событий баланса — по одной на кошелёк (wallet/events.rs, решение 76; решение 132: у
//! каждого кошелька своя очередь и свои события). Изменение кошельков или маппинга пересобирает подписку.
//! Настройки: dex.intermediate_*, dex.native_ticker, dex.native_wrapped (следит за изменениями, как раньше);
//! venue.price_assets, venue.quote_asset — решение 276 (ревью): занятые тикеры подписи промежуточной без биржи
//! (pools/mid_dex.rs busy); их смена пересобирает подписку ТОЛЬКО при dex.intermediate_dex_priced = да (rebuild_on),
//! флаг нет/не задан — ни пересборки, ни записи журнала, как до решения 276. Решение 295: dex.multi_pair [DEX] — смена
//! вида ссылки многомонетных пулов (Contract ↔ Multi) пересобирает подписку (справочник, реестр, пачки).

use std::sync::{Arc, Mutex};

use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::core::types::Asset;
use crate::gateway::errors::GatewayResult;
use crate::gateway::events::MarketEvent;
use crate::settings::live::{Change, Watch};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::parts::Parts;
use super::pools;

/// Последняя подписка: монеты и канал событий ядра.
type Last = Option<(Vec<Asset>, mpsc::Sender<MarketEvent>)>;

/// Состояние подписки. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Subs {
    /// Сериализует замену лент, горячую пересборку и остановку по пустому списку.
    change: Arc<tokio::sync::Mutex<()>>,
    tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    last: Arc<Mutex<Last>>,
    /// Задачи кошельков (события баланса, решение 132) и окружение слушателей, из которого они собраны.
    pub(super) wallet_tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    pub(super) pools: Arc<Mutex<Option<super::pools::listen::Pools>>>,
}

impl Subs {
    /// Подписывает на рынок: запуск задач, остановка прежних, запоминание списка. Ошибки: как у pools/market.rs.
    pub async fn subscribe(
        &self,
        p: &Parts,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()> {
        let _change = self.change.lock().await;
        if assets.is_empty() {
            self.stop(p).await;
            return Ok(());
        }
        self.subscribe_locked(p, assets, out).await
    }

    /// Останавливает задачи до очистки токенов и пулов. Вызывается под change, запрос к RPC не требуется.
    async fn stop(&self, p: &Parts) {
        self.last.lock().unwrap_or_else(|x| x.into_inner()).take();
        self.pools.lock().unwrap_or_else(|x| x.into_inner()).take();
        let mut tasks = std::mem::take(&mut *self.tasks.lock().unwrap_or_else(|x| x.into_inner()));
        tasks.append(&mut *self.wallet_tasks.lock().unwrap_or_else(|x| x.into_inner()));
        for t in &tasks {
            t.abort();
        }
        for t in tasks {
            let _ = t.await;
        }
        p.registry.retain(|_| false);
        p.tokens.set(&[]);
        p.mids.set(vec![]);
    }

    /// Запускает подписку при удержанном change. Проверка сети предшествует первому рыночному запросу.
    async fn subscribe_locked(&self, p: &Parts, assets: Vec<Asset>, out: mpsc::Sender<MarketEvent>) -> GatewayResult<()> {
        super::node::chain_id::check_start(&p.ctx).await?;
        let pools = p.pools(out.clone());
        let new = pools::market::subscribe(&pools, &p.scanned, &p.wake, assets.clone()).await?;
        // Решение 132: очереди и события баланса — по кошелькам (resubscribe_wallets.rs).
        self.wallets(p, Some(pools.clone())).await;
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
                vec![
                    "dex.intermediate_".into(),
                    "dex.native_ticker".into(),
                    // Решение 270: смена обёртки на лету меняет, какой WETH() нативный, — список пересобирается.
                    "dex.native_wrapped".into(),
                    // Решение 276 (ревью): занятые тикеры подписи промежуточной без биржи (pools/mid_dex.rs busy).
                    "venue.price_assets".into(),
                    "venue.quote_asset".into(),
                    // Решение 295: режим «все пары многомонетного пула» — другой вид ссылки и другие записи реестра.
                    "dex.multi_pair".into(),
                ],
            );
            // Решение 132: кошельки или маппинг изменились — пересобираются только задачи кошельков (пулы не трогаем).
            let mut acc = p.ctx.settings.accounts().subscribe();
            loop {
                let c = tokio::select! {
                    c = w.next() => match c {
                        Some(c) => c,
                        None => return,
                    },
                    r = acc.changed() => match r {
                        Ok(()) => {
                            let _change = me.change.lock().await;
                            me.wallets(&p, None).await;
                            continue;
                        }
                        Err(_) => return,
                    },
                };
                let name = c.name;
                if !rebuild_on(&p.ctx, &name) {
                    continue;
                }
                let _change = me.change.lock().await;
                let last = me.last.lock().unwrap_or_else(|x| x.into_inner()).clone();
                let Some((assets, out)) = last else { continue };
                let msg = match me.subscribe_locked(&p, assets, out).await {
                    Ok(()) => (
                        Level::Info,
                        format!("{name} изменена — подписка DEX пересобрана"),
                    ),
                    Err(e) => (
                        Level::Error,
                        format!("{name} изменена, подписка не пересобрана: {}", e.message),
                    ),
                };
                p.ctx.log(msg.0, "dex.intermediate", msg.1, None).await;
            }
        })
    }
}

/// Решение 276 (ревью): настройки, нужные только подписи промежуточной без биржи (pools/mid_dex.rs busy).
const LABEL_ONLY: [&str; 2] = ["venue.price_assets", "venue.quote_asset"];

/// Пересобирать ли подписку на изменение настройки name (имя уже прошло префиксы Watch). Принимает: окружение, имя.
/// Возвращает: false — имя из LABEL_ONLY (тем же сравнением по префиксу, что у Watch), а dex.intermediate_dex_priced
/// не «да» (нет, не задана или не читается); иначе true — прежнее слежение (dex.intermediate_*, dex.native_ticker,
/// dex.native_wrapped) не меняется. Ошибок нет.
pub fn rebuild_on(ctx: &Ctx, name: &str) -> bool {
    !LABEL_ONLY.iter().any(|p| name.starts_with(p))
        || matches!(ctx.flag("dex.intermediate_dex_priced"), Ok(true))
}
