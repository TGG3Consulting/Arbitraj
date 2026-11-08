//! Файл: venues/bsc_dex/exec/swap_ctx.rs
//! Что делает: решение 231, дополнение (Тигран, 08.11: «своп DEX по квитанции — восстанавливать итог полностью») —
//! контекст разбора итога свопа в «чем найти» (PendingRef.parse, gateway/pending.rs). Итог свопа строится в пути
//! исполнения из того, чего в квитанции нет: кошелёк, токены входа и выхода, decimals, роутеры нативной BNB (решение 62),
//! адрес исполнителя (его возврат остатка, решение 122 X), пометка «продажа по квотеру» (решение 177), подпись
//! исполнителя (решение 121 B7), причина «лучший из двух» (решение 123), чаевые и каналы транзакции. Путь кладёт это в
//! ошибку «исход неизвестен» (attach), позднее выяснение (resolve.rs → exec/swap_rebuild.rs) разбирает квитанцию теми же
//! функциями (exec/result.rs, executor/fact.rs) и даёт тот же TradeResult; решение 222 (баланс до/после) — venue.rs.
//! Разрешение (approve) с неизвестным исходом — отметка approve: обмен после него не отправлялся.
//! Ошибки с другим кодом или без факта транзакции не трогаются. Контекста нет (wallet пуст и не approve) — выясняет
//! человек (операции до дополнения, KyberSwap, бандл — см. resolve.rs).
//! Кто вызывает: exec/txs.rs (seed — чаевые), exec/swap_send.rs (approve), exec/trade_direct.rs, exec/trade_smart.rs,
//! exec/executor/trade.rs (attach), exec/trade.rs (подпись), best_of/trade.rs (причина), exec/swap_rebuild.rs, resolve.rs.
//! От чего зависит: gateway/pending.rs, gateway/errors.rs, core/types (ChainTx).
//! Настройки: нет.

use alloy_primitives::Address;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::ChainTx;
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::gateway::pending::PendingRef;

/// Контекст разбора итога свопа (или отметка разрешения). Все поля необязательны: по шагам их дополняют разные места.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SwapCtx {
    /// Транзакция — разрешение (approve): обмен после неё не отправлялся.
    #[serde(default)]
    pub approve: bool,
    /// Кошелёк сделки. None — контекста свопа нет.
    #[serde(default)]
    pub wallet: Option<Address>,
    /// Токен входа и токен выхода свопа.
    #[serde(default)]
    pub tokens: Option<(Address, Address)>,
    /// Decimals монеты и USDT.
    #[serde(default)]
    pub decs: Option<(u8, u8)>,
    /// Роутеры нативной BNB (монеты — по обёртке / развороту WBNB ими); пусто — монета не нативная.
    #[serde(default)]
    pub native_routers: Vec<Address>,
    /// Адрес своего исполнителя (его возврат остатка входа — executor/fact.rs); None — не исполнитель.
    #[serde(default)]
    pub executor: Option<Address>,
    /// Пометка исполнения («— продажа по квотеру», решение 177).
    #[serde(default)]
    pub note: Option<String>,
    /// Подпись исполнителя («свой сборщик: …», решение 121 B7).
    #[serde(default)]
    pub who: Option<String>,
    /// Причина выбора «лучший из двух» (решение 123).
    #[serde(default)]
    pub best_of: Option<String>,
    /// Чаевые, отправленные с транзакцией (BNB).
    #[serde(default)]
    pub tip_bnb: Option<Decimal>,
    /// Факт транзакции на момент ошибки (каналы, способ отправки, хеш, nonce).
    #[serde(default)]
    pub chain: Option<ChainTx>,
}

impl SwapCtx {
    /// Контекст из «чем найти». Принимает: PendingRef. Возвращает: None — parse нет или не разобран. Ошибок нет.
    pub fn of(p: &PendingRef) -> Option<Self> {
        serde_json::from_value(p.parse.clone()?).ok()
    }

    /// Хватает ли контекста, чтобы восстановить итог: разрешение — факт транзакции; своп — кошелёк, токены, decimals и
    /// факт транзакции. Ошибок нет.
    pub fn complete(&self) -> bool {
        self.chain.is_some()
            && (self.approve
                || (self.wallet.is_some() && self.tokens.is_some() && self.decs.is_some()))
    }

    /// Контекст свопа пути исполнения. Принимает: кошелёк, (вход, выход), decimals, роутеры нативной BNB, исполнитель,
    /// пометку «по квотеру». Ошибок нет.
    pub fn swap(
        wallet: Address,
        tokens: (Address, Address),
        decs: (u8, u8),
        native_routers: &[Address],
        executor: Option<Address>,
        note: Option<String>,
    ) -> Self {
        Self {
            wallet: Some(wallet),
            tokens: Some(tokens),
            decs: Some(decs),
            native_routers: native_routers.to_vec(),
            executor,
            note,
            ..Self::default()
        }
    }

    /// Прикладывает контекст к ошибке «исход неизвестен» с фактом транзакции (чаевые из прежнего «чем найти» — txs.rs
    /// seed — сохраняются). Принимает: ошибку. Возвращает: ту же ошибку. Ошибок нет.
    pub fn attach(&self, e: GatewayError) -> GatewayError {
        let mine = self.clone();
        merge(e, move |c| {
            let tip = c.tip_bnb;
            *c = mine;
            c.tip_bnb = tip;
        })
    }
}

/// «Чем найти» транзакции с неизвестным исходом: хеш, nonce и чаевые (txs.rs — до того, как путь приложит контекст).
/// Принимает: факт транзакции, чаевые. Ошибок нет.
pub fn seed(chain: &ChainTx, tip_bnb: Option<Decimal>) -> PendingRef {
    let c = SwapCtx {
        tip_bnb,
        chain: Some(chain.clone()),
        ..SwapCtx::default()
    };
    PendingRef {
        venue_ref: Some(chain.hash.to_string()),
        nonce: Some(chain.nonce),
        parse: serde_json::to_value(c).ok(),
        ..PendingRef::default()
    }
}

/// Отметка «разрешение (approve)»: обмен после этой транзакции не отправлялся. Ошибок нет.
pub fn approve(e: GatewayError) -> GatewayError {
    merge(e, |c| c.approve = true)
}

/// Подпись исполнителя (решение 121 B7) — та же, что exec/result.rs by ставит обычному итогу. Ошибок нет.
pub fn who(e: GatewayError, who: &str) -> GatewayError {
    merge(e, |c| c.who = Some(who.to_string()))
}

/// Причина выбора «лучший из двух» (решение 123, best_of/trade.rs tag). Ошибок нет.
pub fn best_of(e: GatewayError, why: &str) -> GatewayError {
    merge(e, |c| c.best_of = Some(why.to_string()))
}

/// Меняет контекст в «чем найти» ошибки «исход неизвестен» с фактом транзакции: прежний контекст (или пустой) →
/// правка → обратно; хеш и nonce — из факта транзакции, факт — в контекст. Другие ошибки — без изменений. Ошибок нет.
fn merge(mut e: GatewayError, f: impl FnOnce(&mut SwapCtx)) -> GatewayError {
    if e.code != ErrorCode::OutcomeUnknown {
        return e;
    }
    let Some(tx) = e.tx.as_deref().cloned() else {
        return e;
    };
    let mut c = e
        .pending
        .as_deref()
        .and_then(SwapCtx::of)
        .unwrap_or_default();
    f(&mut c);
    c.chain = Some(tx.clone());
    let mut p = seed(&tx, c.tip_bnb);
    p.parse = serde_json::to_value(c).ok();
    e.pending = Some(Box::new(p));
    e
}
