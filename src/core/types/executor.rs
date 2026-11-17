//! Файл: core/types/executor.rs
//! Что делает: развёртывание своего контракта-исполнителя ArbExecutor (решение 122, часть X) глазами интерфейса —
//! оценка перед подтверждением человеком (газ, цена газа, стоимость в BNB и USDT, кошелёк, nonce, будущий адрес, хеш
//! байткода, версия компилятора) и итог отправки (хеш транзакции, адрес, статус, совпал ли код в сети с репозиторием).
//! Решение 400: приёмка классов шага исполнителя v3 по записи симуляции на живых пулах (ExecutorClassProof).
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
    /// Байт данных транзакции создания (код создания + аргумент конструктора — адрес обёртки, ревью 272 А1).
    pub bytecode_len: usize,
    /// Ревью 272 (А1): адрес обёртки нативной монеты (dex.native_wrapped), который уйдёт аргументом конструктора и
    /// навсегда запишется в исполнитель (сеттера нет) — показывается в окне подтверждения.
    pub wnative: Address,
    pub note: Option<String>,
    /// Ревью t181 (Н8), решения 399 / 400: причина запрета развёртывания — в записи симуляции на живых пулах сети
    /// (tests/data) у текущей сборки не принят ни один класс шага (exec/executor/deploy_gate.rs). None — разрешено.
    #[serde(default)]
    pub warning: Option<String>,
    /// Решение 400: классы шага исполнителя v3 и их приёмка по записи симуляции (exec/executor/live_proof.rs).
    #[serde(default)]
    pub classes: Vec<ExecutorClassProof>,
}

/// Решение 400: приёмка одного класса шага исполнителя v3 (вид шага × крючок) по записи симуляции на живых пулах.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutorClassProof {
    /// Имя класса, например «Uniswap V4 (вид 19) без крючка».
    pub class: String,
    /// Пулов класса в одиночных вызовах записи и сколько из них доказано правилом.
    pub pools: usize,
    pub proven: usize,
    /// Принят: шаг этого класса исполним исполнителем v3.
    pub accepted: bool,
    /// Почему принят / не принят.
    pub why: String,
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
