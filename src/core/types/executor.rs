//! Файл: core/types/executor.rs
//! Что делает: развёртывание своего контракта-исполнителя ArbExecutor (решение 122, часть X) глазами интерфейса —
//! оценка перед подтверждением человеком (газ, цена газа, стоимость в BNB и USDT, кошелёк, nonce, будущий адрес, хеш
//! байткода, версия компилятора) и итог отправки (хеш транзакции, адрес, статус, совпал ли код в сети с репозиторием).
//! Кто вызывает: адаптер DEX (Venue::dex_executor_quote / dex_executor_deploy), api/dex_executor.rs, интерфейс.
//! От чего зависит: core/types/market.rs (VenueId), core/types/send.rs (ChainTx), alloy-primitives, rust_decimal.
//! Настройки: нет.

use alloy_primitives::Address;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::market::VenueId;
use super::send::ChainTx;

/// Оценка развёртывания (ничего не отправлено).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutorDeployQuote {
    pub venue: VenueId,
    /// Кошелёк бота — он и станет владельцем исполнителя.
    pub wallet: Address,
    /// Nonce кошелька сейчас и адрес, который получит контракт при этом nonce (CREATE). Если до отправки уйдёт другая
    /// транзакция кошелька — адрес будет другим; точный адрес — в итоге отправки.
    pub nonce: u64,
    pub address: Address,
    /// Лимит газа (eth_estimateGas + запас send.gas_limit_margin_pct) и цена газа (wei, как у сделки).
    pub gas: u64,
    pub gas_price_wei: String,
    pub cost_bnb: Decimal,
    /// По лучшему пулу WBNB — USDT реестра; None — пула нет (см. note).
    pub cost_usdt: Option<Decimal>,
    /// SHA-256 кода создания (contracts/out/ArbExecutor.bin) — подтверждение отправляет его обратно.
    pub bytecode_sha256: String,
    pub solc: String,
    /// Байт кода создания.
    pub bytecode_len: usize,
    pub note: Option<String>,
}

/// Итог развёртывания.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorDeployed {
    pub venue: VenueId,
    /// Адрес контракта (CREATE: кошелёк + nonce транзакции) — его Тигран вписывает в exec.executor_address.
    pub address: Address,
    pub chain: ChainTx,
    /// Код по адресу (eth_getCode) совпал с contracts/out/ArbExecutor.bin-runtime.
    pub code_ok: bool,
    /// Текст итога для человека.
    pub text: String,
}
