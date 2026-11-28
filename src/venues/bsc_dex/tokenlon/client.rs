//! Котировки Tokenlon PMMV5 для выбранного кошелька Ethereum. Сессия только читает публикации.
//! SDK: https://github.com/consenlabs/tokenlon-v5-sdk · методы Quoter.getRate, getNewOrder, lastOrder.
use crate::{
    core::types::Side,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy::signers::local::PrivateKeySigner;
use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use serde_json::Value;
#[path = "client_response.rs"]
mod response;
use response::{pmm_order, rate_value};

#[path = "client_auth.rs"]
mod auth;
#[path = "client_socket.rs"]
mod socket;
#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;

pub(super) struct Query {
    pub base: String,
    pub quote: String,
    pub side: Side,
    /// Количество base в обычных единицах токена, для обеих сторон.
    pub amount: Decimal,
    pub base_dec: u8,
    pub input: Address,
    pub output: Address,
}
pub(super) struct Firm {
    /// Исходный order, включая подпись maker и поля 0x. Auth и персональная подпись сюда не входят.
    pub raw_order: Value,
}

#[derive(Clone, Copy)]
enum Stage {
    Rate,
    Order,
    LastOrder,
}
impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::Rate => "rate",
            Self::Order => "order",
            Self::LastOrder => "lastOrder",
        }
    }
}
impl Query {
    fn base_units(&self) -> Option<U256> {
        crate::venues::bsc_dex::num::exact_amount(
            None,
            Some(&self.amount.normalize().to_string()),
            self.base_dec,
        )
    }
    fn topic(&self, stage: Stage, owner: Address) -> Option<String> {
        let symbol = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        };
        if !symbol(&self.base)
            || !symbol(&self.quote)
            || self.base.eq_ignore_ascii_case(&self.quote)
            || self.input == self.output
            || self.base_units().is_none()
            || owner.is_zero()
        {
            return None;
        }
        let side = match self.side {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        };
        let mut topic = format!(
            "/user/{}/{}_{}/{}/{}",
            stage.name(),
            self.base.to_ascii_uppercase(),
            self.quote.to_ascii_uppercase(),
            side,
            self.amount.normalize()
        );
        if !matches!(stage, Stage::Rate) {
            topic.push('/');
            topic.push_str(&owner.to_checksum(None));
        }
        Some(topic)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    Credentials,
    Balance,
    NoOffer,
    BadMessage,
    Transport,
    Timeout,
    Throttle,
    Endpoint,
    Query,
}
impl Failure {
    fn error(self, ctx: &Ctx) -> crate::gateway::errors::GatewayError {
        let (code, message) = match self {
            Self::Credentials => (
                ErrorCode::InvalidCredentials,
                "Tokenlon не подтвердил личную подпись кошелька",
            ),
            Self::Balance => (
                ErrorCode::InsufficientBalance,
                "Tokenlon сообщил о недостаточном балансе для заявки",
            ),
            Self::NoOffer => (
                ErrorCode::NoRoute,
                "Tokenlon не выдал подходящую котировку PMMV5",
            ),
            Self::BadMessage => (
                ErrorCode::BadResponse,
                "Неверный формат ответа Tokenlon PMMV5",
            ),
            Self::Transport => (
                ErrorCode::Unreachable,
                "Соединение с официальным сервером Tokenlon прервано",
            ),
            Self::Timeout => (ErrorCode::Timeout, "Истёк срок ожидания ответа Tokenlon"),
            Self::Throttle => (
                ErrorCode::RateLimited,
                "Пауза запросов к серверу котировок Tokenlon",
            ),
            Self::Endpoint => (
                ErrorCode::NotConfigured,
                "Для Tokenlon требуется официальный сервер котировок Ethereum",
            ),
            Self::Query => (
                ErrorCode::NoRoute,
                "Не заданы разные активы и положительное количество base для Tokenlon",
            ),
        };
        ctx.err(code, message)
    }
}
pub(super) async fn rate(ctx: &Ctx, key: &PrivateKeySigner, q: &Query) -> GatewayResult<Decimal> {
    let v = socket::request(ctx, key, q, false).await?;
    rate_value(&v, q.amount).map_err(|e| e.error(ctx))
}
pub(super) async fn firm(ctx: &Ctx, key: &PrivateKeySigner, q: &Query) -> GatewayResult<Firm> {
    let response = socket::request(ctx, key, q, true).await?;
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    pmm_order(response, key.address(), q, now, true)
        .map(|raw_order| Firm { raw_order })
        .map_err(|e| e.error(ctx))
}
