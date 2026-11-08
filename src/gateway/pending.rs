//! Файл: gateway/pending.rs
//! Что делает: решение 231 — «чем найти операцию с неизвестным исходом» (PendingRef). Адаптер прикладывает его к
//! ошибке OutcomeUnknown (GatewayError.pending) там, где ID ещё в руках: свой ID ордера (clientOrderId / clientOid /
//! orderLinkId / text), ID ордера биржи, если он уже был, свой ID вывода (withdrawOrderId / clientOid / clientOrderId /
//! withdraw_order_id), хеш и nonce транзакции DEX. Раньше ID жил только внутри функции адаптера и терялся с ошибкой —
//! ядро знало лишь номер сделки, а свой ID у каждой попытки свой (метка миллисекунд, решение 114): восстановить его
//! нельзя. Ядро (core/outcome_resolve.rs) отдаёт PendingRef обратно той же площадке — Venue::resolve_trade /
//! Venue::resolve_withdraw.
//! Дополнение решения 231 (Тигран, 08.11): поле parse — контекст разбора итога, который площадка кладёт сама и сама же
//! читает при выяснении (DEX: маршрут, токены, decimals, нативная BNB, исполнитель, подписи — venues/bsc_dex/exec/
//! swap_ctx.rs). Ядро его не разбирает — только хранит и отдаёт обратно той же площадке.
//! Кто вызывает: адаптеры (создают и прикладывают; created — найденный вывод), core/outcome_resolve.rs (читает),
//! gateway/contract.rs (тип).
//! От чего зависит: gateway/errors.rs, core/types (Transfer, WithdrawRequest).
//! Настройки: нет.
//!
//! Это стык: менять только через запрос на изменение стыка (docs/AGENTS.md); добавлен решением 231.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::{Transfer, TransferStatus, VenueId, WithdrawRequest};

use super::errors::{ErrorCode, GatewayError};

/// Чем найти операцию с неизвестным исходом. Поля, которых у площадки нет, — None.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PendingRef {
    /// Символ пары на площадке (ордер).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// Свой ID операции: ордера (clientOrderId и аналоги) или вывода (withdrawOrderId и аналоги).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// ID площадки, если уже известен: ID ордера биржи, ID вывода, хеш транзакции DEX.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub venue_ref: Option<String>,
    /// Отправленное (округлённое) количество ордера — запасное, если площадка его не вернёт.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_qty: Option<Decimal>,
    /// Когда операция отправлена (Unix, секунды) — начало окна поиска в истории (Gate: from).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at_s: Option<i64>,
    /// Монета, которую ордер получает (Bitget: разбор комиссии feeDetail).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received: Option<String>,
    /// Nonce транзакции DEX.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<u64>,
    /// Контекст разбора итога — собственность площадки (кладёт и читает только её адаптер; ядро не разбирает). None —
    /// площадке он не нужен или его нет (операция до дополнения решения 231 — тогда выясняет человек).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parse: Option<serde_json::Value>,
}

impl PendingRef {
    /// Ордер: символ, свой ID, отправленное количество. Ошибок нет.
    pub fn order(symbol: &str, client_id: &str, sent_qty: Decimal) -> Self {
        Self {
            symbol: Some(symbol.to_string()),
            client_id: Some(client_id.to_string()),
            sent_qty: Some(sent_qty),
            ..Self::default()
        }
    }

    /// Вывод по своему ID. Ошибок нет.
    pub fn withdraw(client_id: &str) -> Self {
        Self {
            client_id: Some(client_id.to_string()),
            ..Self::default()
        }
    }

    /// Монета, которую получает ордер (нужна разбору комиссии Bitget). Ошибок нет.
    pub fn received_coin(mut self, coin: &str) -> Self {
        self.received = Some(coin.to_string());
        self
    }

    /// Та же операция, ID площадки уже известен. Ошибок нет.
    pub fn with_ref(&self, venue_ref: &str) -> Self {
        Self {
            venue_ref: Some(venue_ref.to_string()),
            ..self.clone()
        }
    }

    /// Ключ операции для учёта открытых «исход неизвестен» площадки и журнала: свой ID (он есть с самого начала и не
    /// меняется), иначе ID площадки. Пустая строка — нечем найти. Ошибок нет.
    pub fn key(&self) -> String {
        self.client_id
            .clone()
            .or_else(|| self.venue_ref.clone())
            .unwrap_or_default()
    }
}

impl GatewayError {
    /// Решение 231: прикладывает «чем найти» — только к ошибке «исход неизвестен» (у прочих кодов исход известен).
    /// Принимает: PendingRef. Возвращает: ту же ошибку. Ошибок нет.
    pub fn pending(mut self, p: PendingRef) -> Self {
        if self.code == ErrorCode::OutcomeUnknown && self.pending.is_none() {
            self.pending = Some(Box::new(p));
        }
        self
    }
}

/// Вывод найден (решение 231): перевод Pending с ID площадки — как ответ на сам вывод (дальше ядро опрашивает статус).
/// Принимает: площадку, исходный запрос, ID вывода на площадке. Ошибок нет.
pub fn created(venue: VenueId, req: WithdrawRequest, venue_ref: String) -> Transfer {
    Transfer {
        venue,
        asset: req.asset,
        network: req.network,
        amount: req.amount,
        status: TransferStatus::Pending,
        venue_ref,
        tx_hash: None,
        fee: None,
        updated_at: chrono::Utc::now(),
    }
}
