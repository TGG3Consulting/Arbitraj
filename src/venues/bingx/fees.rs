//! Файл: venues/bingx/fees.rs
//! Что делает: ставки торговой комиссии нашего аккаунта по паре (taker/maker).
//! Ставки обновляются фоновой задачей (не в момент котировки — чтобы не упираться в лимит
//! запросов и не задерживать расчёт); котировка берёт ставку только из памяти.
//! Кто вызывает: market.rs (запускает фоновое обновление для подписанных пар), book.rs (читает ставку).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.fee_cache_s (как часто обновлять), venue.fee_requests_per_sec (частота запросов).
//!
//! Официальная документация: «Query Trading Commission Rate» GET /openApi/spot/v1/user/commissionRate
//! (подписанный, право Read, 2 запроса/с). Ответ: takerCommissionRate, makerCommissionRate.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use reqwest::Method;
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Ставки комиссии пары (доли, например 0.001 = 0,1%).
#[derive(Debug, Clone, Copy)]
pub struct Rates {
    pub taker: Decimal,
    pub maker: Decimal,
}

/// Ставки по символу. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Fees {
    cache: Arc<RwLock<HashMap<String, Rates>>>,
}

impl Fees {
    /// Ставка из памяти. Ошибка StaleData — ставка ещё не получена (котировки нет).
    pub fn get(&self, ctx: &Ctx, symbol: &str) -> GatewayResult<Rates> {
        let cache = self.cache.read().unwrap_or_else(|p| p.into_inner());
        cache.get(symbol).copied().ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::StaleData,
                None,
                format!("BingX {symbol}: ставка комиссии ещё не получена — котировки нет"),
            ))
        })
    }

    /// Фоновое обновление ставок по списку символов: по кругу, не чаще venue.fee_requests_per_sec,
    /// круг — раз в venue.fee_cache_s. Работает, пока задачу не остановят (новая подписка).
    /// Ошибки пишутся в журнал; при незаданных настройках — запись в журнал и пауза до следующего круга.
    pub async fn refresh_loop(self, ctx: Ctx, symbols: Vec<String>) {
        loop {
            let (Ok(period), Ok(rate)) = (
                ctx.int("venue.fee_cache_s"),
                ctx.int("venue.fee_requests_per_sec"),
            ) else {
                let m = "Ставки комиссии не обновляются: не заданы venue.fee_cache_s или venue.fee_requests_per_sec";
                let _ = ctx
                    .journal
                    .write(Level::Error, "venue.fees", Some(super::ctx::ID), m, None)
                    .await;
                return;
            };
            let gap = Duration::from_secs_f64(1.0 / rate.max(1) as f64);
            for s in &symbols {
                match fetch(&ctx, s).await {
                    Ok(r) => {
                        self.cache
                            .write()
                            .unwrap_or_else(|p| p.into_inner())
                            .insert(s.clone(), r);
                    }
                    Err(e) => {
                        let m = format!("Ставка комиссии {s} не получена: {}", e.message);
                        let _ = ctx
                            .journal
                            .write(Level::Warn, "venue.fees", Some(super::ctx::ID), m, None)
                            .await;
                    }
                }
                tokio::time::sleep(gap).await;
            }
            tokio::time::sleep(Duration::from_secs(period)).await;
        }
    }
}

/// Запрос ставок одной пары. Ошибки: ключи, связь, ответ BingX.
async fn fetch(ctx: &Ctx, symbol: &str) -> GatewayResult<Rates> {
    let p = vec![("symbol", symbol.to_string())];
    let data = rest::signed(
        ctx,
        Method::GET,
        "/openApi/spot/v1/user/commissionRate",
        p,
        Op::Other,
    )
    .await?;
    Ok(Rates {
        taker: num::dec(&data, "takerCommissionRate")?,
        maker: num::dec(&data, "makerCommissionRate")?,
    })
}
