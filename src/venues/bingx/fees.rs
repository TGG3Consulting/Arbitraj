//! Файл: venues/bingx/fees.rs
//! Что делает: ставки торговой комиссии нашего аккаунта по паре (taker/maker) с кэшем на venue.fee_cache_s.
//! Кто вызывает: book.rs (котировка на объём учитывает комиссию).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.fee_cache_s.
//!
//! Официальная документация: «Query Trading Commission Rate» GET /openApi/spot/v1/user/commissionRate
//! (подписанный, право Read, 2 запроса/с). Ответ: takerCommissionRate, makerCommissionRate.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use reqwest::Method;
use rust_decimal::Decimal;

use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Ставки комиссии пары (доли, например 0.001 = 0,1%).
#[derive(Debug, Clone, Copy)]
pub struct Rates {
    pub taker: Decimal,
    pub maker: Decimal,
}

/// Кэш ставок по символу. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Fees {
    cache: Arc<RwLock<HashMap<String, (Rates, Instant)>>>,
}

impl Fees {
    /// Ставки по символу (например CAKE-USDT): из кэша, если моложе venue.fee_cache_s, иначе запрос.
    /// Ошибки: настройки, ключи, ответ BingX.
    pub async fn get(&self, ctx: &Ctx, symbol: &str) -> GatewayResult<Rates> {
        let ttl = Duration::from_secs(ctx.int("venue.fee_cache_s")?);
        if let Some((r, at)) = self
            .cache
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(symbol)
            && at.elapsed() < ttl
        {
            return Ok(*r);
        }
        let data = rest::signed(
            ctx,
            Method::GET,
            "/openApi/spot/v1/user/commissionRate",
            vec![("symbol", symbol.to_string())],
            Op::Other,
        )
        .await?;
        let rates = Rates {
            taker: num::dec(&data, "takerCommissionRate")?,
            maker: num::dec(&data, "makerCommissionRate")?,
        };
        self.cache
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(symbol.to_string(), (rates, Instant::now()));
        Ok(rates)
    }
}
