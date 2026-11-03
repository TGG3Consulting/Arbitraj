//! Файл: venues/bybit/fees.rs
//! Что делает: ставки торговой комиссии нашего аккаунта по спот-паре (taker/maker) с учётом уровня VIP и скидок —
//! такими, как их отдаёт Bybit. Ставки обновляются фоновой задачей (не в момент котировки — чтобы не упираться в лимит
//! и не задерживать расчёт); котировка берёт ставку только из памяти.
//! Кто вызывает: subs.rs (запускает фоновое обновление для подписанных пар), book.rs (читает ставку).
//! От чего зависит: rest.rs, num.rs, ctx.rs, settings/wait.rs.
//! Настройки: venue.fee_cache_s (как часто обновлять), venue.fee_requests_per_sec (частота запросов).
//!
//! Официальная документация «Get Fee Rate» GET /v5/account/fee-rate https://bybit-exchange.github.io/docs/v5/account/fee-rate
//! — category=spot, symbol; ответ list[]: symbol, takerFeeRate, makerFeeRate. Лимит — 5/s
//! (https://bybit-exchange.github.io/docs/v5/rate-limit, таблица Account). Ответ без symbol для спота в документации не
//! описан — ставка запрашивается по каждой паре.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::settings::wait::{Note, until_ok};
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
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
                format!("Bybit {symbol}: ставка комиссии ещё не получена — котировки нет"),
            ))
        })
    }

    /// Фоновое обновление ставок по списку символов: по кругу, не чаще venue.fee_requests_per_sec, круг — раз в
    /// venue.fee_cache_s. Работает, пока задачу не остановят. Ошибки — в журнал; незаданные настройки — ожидание.
    pub async fn refresh_loop(self, ctx: Ctx, symbols: Vec<String>) {
        loop {
            let read = || {
                Ok((
                    ctx.int("venue.fee_cache_s")?,
                    ctx.int("venue.fee_requests_per_sec")?,
                ))
            };
            let note = Note {
                journal: &ctx.journal,
                kind: "venue.fees",
                venue: Some(ID),
            };
            let Some((period, rate)) = until_ok(
                &ctx.settings,
                note,
                "Обновление ставок комиссии Bybit",
                read,
            )
            .await
            else {
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
                        let m = format!("Ставка комиссии Bybit {s} не получена: {}", e.message);
                        let _ = ctx
                            .journal
                            .write(Level::Warn, "venue.fees", Some(ID), m, None)
                            .await;
                    }
                }
                tokio::time::sleep(gap).await;
            }
            tokio::time::sleep(Duration::from_secs(period)).await;
        }
    }
}

/// Запрос ставок одной пары. Ошибки: ключи, связь, ответ Bybit (нет пары в list — BadResponse).
async fn fetch(ctx: &Ctx, symbol: &str) -> GatewayResult<Rates> {
    let p = vec![
        ("category", "spot".to_string()),
        ("symbol", symbol.to_string()),
    ];
    let data = rest::get(ctx, "/v5/account/fee-rate", p, Op::Other).await?;
    let row = num::arr(&data, "list")?
        .iter()
        .find(|r| num::text_opt(r, "symbol").as_deref() == Some(symbol))
        .ok_or_else(|| super::errors::bad(&format!("fee-rate без {symbol}"), &data.to_string()))?;
    Ok(Rates {
        taker: num::dec(row, "takerFeeRate")?,
        maker: num::dec(row, "makerFeeRate")?,
    })
}
