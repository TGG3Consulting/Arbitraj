//! Файл: venues/bsc_dex/parts.rs
//! Что делает: общее состояние адаптера DEX в памяти (блоки, токены, пулы, проверки, цена газа, очередь кошелька,
//! счётчики каналов, сигнал «перепроверить») и сборка окружений для котировки, сделки и слушателей пулов.
//! Кто вызывает: mod.rs.
//! От чего зависит: ctx.rs, node/heads.rs, tokens.rs, pools/*, check/state.rs, quote/*, exec/txs.rs, send/stats.rs, wallet/nonce.rs.
//! Настройки: нет.

use std::sync::Arc;

use tokio::sync::{Notify, mpsc};

use crate::gateway::events::MarketEvent;

use super::check::state::Checks;
use super::ctx::Ctx;
use super::exec::txs::Env;
use super::node::heads::Heads;
use super::pools::discover::Scanned;
use super::pools::listen::Pools;
use super::pools::registry::Registry;
use super::quote::Quoting;
use super::quote::gas::GasPrice;
use super::send::stats::Stats;
use super::tokens::Tokens;
use super::wallet::nonce::Nonces;

/// Состояние адаптера. Дешёво клонируется.
#[derive(Clone)]
pub struct Parts {
    pub ctx: Ctx,
    pub heads: Heads,
    pub tokens: Tokens,
    pub nonces: Nonces,
    pub registry: Registry,
    pub scanned: Scanned,
    pub checks: Checks,
    pub gas: GasPrice,
    pub stats: Stats,
    pub wake: Arc<Notify>,
}

impl Parts {
    /// Пустое состояние. Ошибок нет.
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            heads: Heads::default(),
            tokens: Tokens::default(),
            nonces: Nonces::default(),
            registry: Registry::default(),
            scanned: Scanned::default(),
            checks: Checks::default(),
            gas: GasPrice::default(),
            stats: Stats::default(),
            wake: Arc::new(Notify::new()),
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
        }
    }

    /// Окружение отправки транзакций. Ошибок нет.
    pub fn env(&self) -> Env {
        Env {
            ctx: self.ctx.clone(),
            heads: self.heads.clone(),
            stats: self.stats.clone(),
            gas: self.gas.clone(),
        }
    }

    /// Окружение слушателей пулов и проверок; канал событий ядра — и для счётчиков каналов. Ошибок нет.
    pub fn pools(&self, out: mpsc::Sender<MarketEvent>) -> Pools {
        self.stats.set_out(out.clone());
        Pools {
            ctx: self.ctx.clone(),
            reg: self.registry.clone(),
            tokens: self.tokens.clone(),
            out,
            checks: self.checks.clone(),
            heads: self.heads.clone(),
        }
    }
}
