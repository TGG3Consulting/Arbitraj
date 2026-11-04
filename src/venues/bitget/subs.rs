//! Файл: venues/bitget/subs.rs
//! Что делает: текущая подписка адаптера Bitget на рынок — задачи соединений стаканов и обновления комиссий
//! плюс фоновое обновление справочника пар (pairs_watch.rs). Новая подписка останавливает старые задачи (без
//! дублей соединений); подписка из ядра и пересподписка из pairs_watch.rs идут строго по одной (замок gate),
//! поэтому задачи не теряются и не запускаются дважды. Обновление ставок комиссии (fees.rs) перезапускается только при
//! смене списка символов (решение 131). Потока аккаунта (решение 76) у Bitget пока нет: страница приватного канала
//! баланса спота Classic в официальной документации не прочитана (README.md адаптера, вопрос Тиграну).
//! Кто вызывает: mod.rs (subscribe_market из контракта), pairs_watch.rs (resubscribe при смене пар).
//! От чего зависит: market.rs, pairs.rs, pairs_watch.rs, symbols.rs, book.rs, fees.rs, ctx.rs.
//! Настройки: нет своих (venue.symbols_refresh_s читает pairs_watch.rs).

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::core::types::Asset;
use crate::gateway::errors::GatewayResult;
use crate::gateway::events::MarketEvent;

use super::book::Books;
use super::ctx::Ctx;
use super::fees::Fees;
use super::pairs::Snapshot;
use super::symbols::Symbols;

/// Всё, что нужно для подписки. Дешёво клонируется.
#[derive(Clone)]
pub struct Parts {
    pub ctx: Ctx,
    pub symbols: Symbols,
    pub books: Books,
    pub fees: Fees,
}

/// Текущая подписка. Дешёво клонируется (общее состояние).
#[derive(Clone, Default)]
pub struct Subs {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Подписка и пересподписка — по одной.
    gate: tokio::sync::Mutex<()>,
    /// Задачи соединений стаканов и обновления комиссий.
    tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Фоновое обновление справочника пар текущей подписки.
    watch: Mutex<Option<JoinHandle<()>>>,
    /// Обновление ставок комиссии и его список символов (решение 131): тот же список — задача не перезапускается.
    fees: Mutex<Option<(Vec<String>, JoinHandle<()>)>>,
}

impl Subs {
    /// Подписка из ядра: подписывается заново на список монет, останавливает старые задачи и прежнее обновление
    /// справочника пар, запускает новое (pairs_watch.rs) для этого списка. Ошибка — прежняя подписка остаётся.
    /// Принимает: части адаптера, монеты, канал событий. Ошибки: market::subscribe (настройки, справочник пар).
    pub async fn subscribe(
        &self,
        p: &Parts,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()> {
        let _g = self.inner.gate.lock().await;
        let snap = self.start(p, assets.clone(), out.clone()).await?;
        let mut watch = self.inner.watch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = watch.take() {
            old.abort();
        }
        *watch = Some(tokio::spawn(super::pairs_watch::run(
            self.clone(),
            p.clone(),
            assets,
            out,
            snap,
        )));
        Ok(())
    }

    /// Пересподписка тем же списком монет (справочник пар изменил набор стаканов). Фоновое обновление
    /// справочника не трогает (его и вызывает). Возвращает: новый снимок пар. Ошибки: как у subscribe.
    pub async fn resubscribe(
        &self,
        p: &Parts,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<Snapshot> {
        let _g = self.inner.gate.lock().await;
        self.start(p, assets, out).await
    }

    /// Запускает задачи подписки и останавливает прежние (вызывать под замком gate).
    /// Возвращает: снимок пар. Ошибки: market::subscribe — тогда прежние задачи остаются.
    async fn start(
        &self,
        p: &Parts,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<Snapshot> {
        let (new, snap, syms) =
            super::market::subscribe(&p.ctx, &p.symbols, &p.books, assets, out).await?;
        let mut tasks = self.inner.tasks.lock().unwrap_or_else(|e| e.into_inner());
        for old in tasks.drain(..) {
            old.abort();
        }
        *tasks = new;
        // Решение 131: круг ставок комиссии начинается заново только при новом списке символов или упавшей задаче.
        let mut fees = self.inner.fees.lock().unwrap_or_else(|e| e.into_inner());
        let same = fees
            .as_ref()
            .is_some_and(|(list, h)| *list == syms && !h.is_finished());
        if !same {
            if let Some((_, old)) = fees.take() {
                old.abort();
            }
            let h = tokio::spawn(p.fees.clone().refresh_loop(p.ctx.clone(), syms.clone()));
            *fees = Some((syms, h));
        }
        Ok(snap)
    }
}
