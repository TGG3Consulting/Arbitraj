//! Файл: venues/phemex/fees.rs
//! Что делает: ставки торговой комиссии нашего ключа по паре (taker/maker). Обновляются фоновой задачей одним запросом
//! на все пары котируемой монеты (не в момент котировки); котировка берёт ставку только из памяти (урок 15).
//! Кто вызывает: subs.rs (запускает фоновое обновление для подписанных пар), book.rs (читает ставку), account.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs, settings/wait.rs.
//! Настройки: venue.quote_asset, venue.fee_cache_s (период круга), venue.fee_retry_s (повтор после сбоя, решение 149)
//! [phemex].
//!
//! Официально — Query fee rate by quote currency GET /api-data/spots/fee-rate?quoteCurrency=<quoteCurrency>
//! (https://phemex-docs.github.io/#query-fee-rate-by-quote-currency): {"symbolFeeRates": [{"takerFeeRateEr",
//! "makerFeeRateEr", "symbol"}]}; Er — доля × 10^8 («ratioScale is always scaled 8»). Противоречие: в строке запроса
//! параметр quoteCurrency, в таблице — currency (и параметры страниц start/end/offset/limit, «default 20, max 200»):
//! берётся строка запроса без страниц; пар без ставки — запись в журнал (урок 2, проверить на живом ответе).
//! Скидки своей монетой (урок 23) в документации Phemex нет: расчёт по ставке ключа, монета комиссии — по факту ордера
//! (feeCurrency, order_state.rs).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::settings::wait::{Note, until_ok};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::num::{self, RATIO_SCALE};
use super::rest;

/// Путь ставок комиссии.
pub const PATH: &str = "/api-data/spots/fee-rate";

/// Ставки комиссии пары (доли, например 0.001 = 0,1%).
#[derive(Debug, Clone, Copy, PartialEq)]
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
                format!("Phemex {symbol}: ставка комиссии ещё не получена — котировки нет"),
            ))
        })
    }

    /// Фоновое обновление: ставки пар котируемой монеты одним запросом (в память — только символы списка); круг — раз в
    /// venue.fee_cache_s, после сбоя — через venue.fee_retry_s (решение 149). Работает, пока задачу не остановят.
    /// Ошибки — в журнал; незаданный период — одна запись и ожидание настройки (settings/wait.rs).
    pub async fn refresh_loop(self, ctx: Ctx, list: Vec<String>) {
        loop {
            let read = || ctx.int("venue.fee_cache_s");
            let note = Note {
                journal: &ctx.journal,
                kind: "venue.fees",
                venue: Some(super::ctx::ID),
            };
            let Some(period) = until_ok(
                &ctx.settings,
                note,
                "Обновление ставок комиссии Phemex",
                read,
            )
            .await
            else {
                return;
            };
            let mut failed = false;
            match fetch(&ctx).await {
                Ok(got) => {
                    {
                        let mut c = self.cache.write().unwrap_or_else(|p| p.into_inner());
                        for s in &list {
                            if let Some(r) = got.get(s) {
                                c.insert(s.clone(), *r);
                            }
                        }
                    }
                    let missing: Vec<&String> =
                        list.iter().filter(|s| !got.contains_key(*s)).collect();
                    if !missing.is_empty() {
                        failed = true;
                        let m = format!(
                            "fee-rate: нет ставок {} пар (котировок по ним нет): {missing:?}",
                            missing.len()
                        );
                        ctx.log(Level::Warn, "venue.fees", m).await;
                    }
                }
                Err(e) => {
                    failed = true;
                    let m = format!("Ставки комиссии Phemex не получены: {}", e.message);
                    ctx.log(Level::Warn, "venue.fees", m).await;
                }
            }
            let retry = failed.then(|| ctx.int("venue.fee_retry_s").ok()).flatten();
            tokio::time::sleep(Duration::from_secs(retry.unwrap_or(period))).await;
        }
    }
}

/// Что делает: ставки всех пар котируемой монеты (fee-rate). Возвращает: символ → ставки.
/// Ошибки: ключи, связь, ответ Phemex (нет symbolFeeRates, поле не целое).
pub async fn fetch(ctx: &Ctx) -> GatewayResult<HashMap<String, Rates>> {
    let q = vec![("quoteCurrency", ctx.text("venue.quote_asset")?)];
    let data = rest::signed(ctx, Method::GET, PATH, q, None, Op::Other).await?;
    parse(&data)
}

/// Разбор data fee-rate ({symbolFeeRates: [{symbol, takerFeeRateEr, makerFeeRateEr}]}). Ошибка BadResponse.
pub fn parse(data: &Value) -> GatewayResult<HashMap<String, Rates>> {
    num::arr(data, "symbolFeeRates")?
        .iter()
        .map(|x| {
            Ok((
                num::text(x, "symbol")?,
                Rates {
                    taker: num::scaled(x, "takerFeeRateEr", RATIO_SCALE)?,
                    maker: num::scaled(x, "makerFeeRateEr", RATIO_SCALE)?,
                },
            ))
        })
        .collect()
}
