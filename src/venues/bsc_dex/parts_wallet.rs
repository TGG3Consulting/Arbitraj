//! Файл: venues/bsc_dex/parts_wallet.rs
//! Что делает: своё состояние каждого кошелька DEX (решение 132): очередь транзакций и nonce, разрешения (кэш approve),
//! ожидаемые поступления — по id счёта; и привязка состояния адаптера к счёту (Parts::bound): общие пулы, котировки,
//! токены, проверки и настройки — те же, кошельковое — своё. Деньги, nonce и approve разных кошельков не смешиваются.
//! Кто вызывает: parts.rs (поле wallets), venue_account.rs (вид адаптера на счёт), resubscribe.rs (события и очереди
//! каждого кошелька).
//! От чего зависит: wallet/{nonce,inflows,allow_cache}.rs, parts.rs, ctx_wallet.rs.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::parts::Parts;
use super::wallet::allow_cache::Allowances;
use super::wallet::inflows::Inflows;
use super::wallet::nonce::Nonces;

/// Состояние одного кошелька. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct WalletParts {
    pub nonces: Nonces,
    pub inflows: Inflows,
    pub allow: Allowances,
}

/// Состояния кошельков по id счёта. Дешёво клонируется; одно на адаптер.
#[derive(Clone, Default)]
pub struct Wallets(Arc<Mutex<HashMap<String, WalletParts>>>);

impl Wallets {
    /// Состояние кошелька (создаётся при первом обращении). Принимает: id счёта. Ошибок нет.
    pub fn of(&self, id: &str) -> WalletParts {
        let mut m = self.0.lock().unwrap_or_else(|p| p.into_inner());
        m.entry(id.to_string()).or_default().clone()
    }
}

impl Parts {
    /// Состояние адаптера, привязанное к счёту: окружение с id счёта (ключ, исполнитель, очередь — его кошелька),
    /// очередь, разрешения и ожидаемые поступления — этого кошелька. Принимает: id счёта. Ошибок нет (есть ли
    /// кошелёк — проверяет вызывающий, venue_account.rs).
    pub fn bound(&self, id: &str) -> Parts {
        let w = self.wallets.of(id);
        let mut p = self.clone();
        p.ctx = p.ctx.with_account(id);
        p.nonces = w.nonces;
        p.inflows = w.inflows;
        p.allow = w.allow;
        p
    }
}
