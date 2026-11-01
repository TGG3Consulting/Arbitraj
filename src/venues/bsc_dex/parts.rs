//! Файл: venues/bsc_dex/parts.rs
//! Что делает: общее состояние адаптера DEX в памяти (блоки, токены, пулы, проверки, цена газа, очередь кошелька,
//! ожидаемые поступления, счётчики каналов, сигнал «перепроверить») и сборка окружений для котировки, сделки и слушателей пулов.
//! Кто вызывает: mod.rs.
//! От чего зависит: ctx.rs, node/heads.rs, tokens.rs, pools/*, check/state.rs, quote/*, exec/txs.rs, send/stats.rs, wallet/nonce.rs,
//! wallet/inflows.rs.
//! Настройки: нет.

use tokio::sync::mpsc;

use crate::gateway::events::MarketEvent;

use super::check::state::Checks;
use super::ctx::Ctx;
use super::exec::txs::Env;
use super::node::heads::Heads;
use super::pools::discover::Scanned;
use super::pools::listen::Pools;
use super::pools::mids::Mids;
use super::pools::registry::Registry;
use super::quote::Quoting;
use super::quote::gas::GasPrice;
use super::send::stats::Stats;
use super::tokens::Tokens;
use super::wallet::inflows::Inflows;
use super::wallet::nonce::Nonces;

/// Состояние адаптера. Дешёво клонируется.
#[derive(Clone)]
pub struct Parts {
    pub ctx: Ctx,
    pub heads: Heads,
    pub tokens: Tokens,
    pub nonces: Nonces,
    pub inflows: Inflows,
    pub registry: Registry,
    /// Промежуточные монеты (решение 52).
    pub mids: Mids,
    pub scanned: Scanned,
    pub checks: Checks,
    pub gas: GasPrice,
    pub stats: Stats,
    /// Разрешения кошелька для котировки (wallet/allow_cache.rs).
    pub allow: super::wallet::allow_cache::Allowances,
    /// Очереди «Перепроверить» и «новые пулы» для циклов проверок (check/recheck.rs).
    pub wake: super::check::recheck::Recheck,
    /// «Данные пулов отстают на N блоков» — лента квитанций догоняет без сброса (решение 72).
    pub lag: super::node::events_lag::EventsLag,
}

impl Parts {
    /// Пустое состояние. Ошибок нет.
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            heads: Heads::default(),
            tokens: Tokens::default(),
            nonces: Nonces::default(),
            inflows: Inflows::default(),
            registry: Registry::default(),
            mids: Mids::default(),
            scanned: Scanned::default(),
            checks: Checks::default(),
            gas: GasPrice::default(),
            stats: Stats::default(),
            allow: Default::default(),
            wake: Default::default(),
            lag: Default::default(),
        }
    }

    /// Окружение котировки. Ошибок нет.
    pub fn quoting(&self) -> Quoting {
        Quoting {
            ctx: self.ctx.clone(),
            reg: self.registry.clone(),
            checks: self.checks.clone(),
            tokens: self.tokens.clone(),
            heads: self.heads.clone(),
            gas: self.gas.clone(),
            mids: self.mids.clone(),
            allow: self.allow.clone(),
            lag: self.lag.clone(),
        }
    }

    /// Окружение отправки транзакций. Ошибок нет.
    pub fn env(&self) -> Env {
        Env {
            ctx: self.ctx.clone(),
            heads: self.heads.clone(),
            stats: self.stats.clone(),
            gas: self.gas.clone(),
            inflows: self.inflows.clone(),
            allow: self.allow.clone(),
        }
    }

    /// Окружение слушателей пулов и проверок; канал событий ядра — и для счётчиков каналов. Ошибок нет.
    pub fn pools(&self, out: mpsc::Sender<MarketEvent>) -> Pools {
        self.stats.set_out(out.clone());
        let signals = super::pools::progress::Signals::default();
        // Отметка «данные пулов отстают» снята — циклы проверок перепроверяют (решение 72).
        self.lag.on_clear(signals.ready.clone());
        // Направление пула стало глубоким — «пул готов» (решение 73, pools/depth_book.rs).
        self.registry.depth().on_rise(signals.ready.clone());
        Pools {
            ctx: self.ctx.clone(),
            reg: self.registry.clone(),
            tokens: self.tokens.clone(),
            out,
            checks: self.checks.clone(),
            heads: self.heads.clone(),
            mids: self.mids.clone(),
            signals,
            loading: super::pools::loading::Loading::default(),
            feed: super::node::receipts::Feed::with_lag(self.lag.clone()),
            lag: self.lag.clone(),
            gas: self.gas.clone(),
        }
    }
}
