//! Файл: venues/mexc/fees.rs
//! Что делает: ставки торговой комиссии нашего аккаунта по паре (taker/maker).
//! Ставки обновляются фоновой задачей (не в момент котировки — чтобы не упираться в лимит
//! запросов и не задерживать расчёт); котировка берёт ставку только из памяти.
//! Кто вызывает: market.rs (запускает фоновое обновление для подписанных пар), book.rs (читает ставку).
//! От чего зависит: rest.rs, num.rs, ctx.rs, settings/wait.rs.
//! Настройки: venue.fee_cache_s (как часто обновлять), venue.fee_requests_per_sec (частота запросов).
//!
//! Официальная документация: «Query Symbol Commission» GET /api/v3/tradeFee?symbol=
//! https://www.mexc.com/api-docs/spot-v3/spot-account-trade/query-symbol-commission (подписанный, SPOT_ACCOUNT_READ,
//! «Weight: 20 (IP)» из корзины IP 300 за 10 с — решение 118, А-🟠1: 1 запрос/с — уже 200 из 300, а в той же корзине
//! опрос ордера, истории переводов, getall, exchangeInfo; поэтому venue.fee_requests_per_sec держать низким, а
//! venue.fee_cache_s — большим). Ответ: data.makerCommission,
//! data.takerCommission (числа: «0E-18», «0.000500000000000000» — читаются через текст JSON, без f64).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use reqwest::Method;
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::settings::wait::{Note, until_ok};
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
                format!("MEXC {symbol}: ставка комиссии ещё не получена — котировки нет"),
            ))
        })
    }

    /// Фоновое обновление ставок по списку символов: по кругу, не чаще venue.fee_requests_per_sec,
    /// круг — раз в venue.fee_cache_s. Работает, пока задачу не остановят (новая подписка).
    /// Ошибки пишутся в журнал; при незаданных настройках — одна запись и ожидание их задания (settings/wait.rs).
    pub async fn refresh_loop(self, ctx: Ctx, symbols: Vec<String>) {
        loop {
            // Не заданы — обновление ждёт настройку и продолжает само (settings/wait.rs, решение 75), а не
            // останавливается до пересборки подписки.
            let read = || {
                Ok((
                    ctx.int("venue.fee_cache_s")?,
                    ctx.int("venue.fee_requests_per_sec")?,
                ))
            };
            let note = Note {
                journal: &ctx.journal,
                kind: "venue.fees",
                venue: Some(super::ctx::ID),
            };
            let Some((period, rate)) =
                until_ok(&ctx.settings, note, "Обновление ставок комиссии", read).await
            else {
                return;
            };
            let gap = Duration::from_secs_f64(1.0 / rate.max(1) as f64);
            // Решение 149: круг со сбоями (например, секреты ещё не разблокированы при старте) — повтор через
            // venue.fee_retry_s, а не через полный venue.fee_cache_s (пары без ставки иначе стоят без котировки весь срок).
            let mut failed = false;
            for s in &symbols {
                match fetch(&ctx, s).await {
                    Ok(r) => {
                        self.cache
                            .write()
                            .unwrap_or_else(|p| p.into_inner())
                            .insert(s.clone(), r);
                    }
                    Err(e) => {
                        failed = true;
                        let m = format!("Ставка комиссии {s} не получена: {}", e.message);
                        let _ = ctx
                            .journal
                            .write(Level::Warn, "venue.fees", Some(super::ctx::ID), m, None)
                            .await;
                    }
                }
                tokio::time::sleep(gap).await;
            }
            let retry = failed.then(|| ctx.int("venue.fee_retry_s").ok()).flatten();
            tokio::time::sleep(Duration::from_secs(retry.unwrap_or(period))).await;
        }
    }
}

/// Запрос ставок одной пары. Ошибки: ключи, связь, ответ MEXC.
async fn fetch(ctx: &Ctx, symbol: &str) -> GatewayResult<Rates> {
    let p = vec![("symbol", symbol.to_string())];
    let data = rest::signed(ctx, Method::GET, "/api/v3/tradeFee", p, Op::Other).await?;
    Ok(Rates {
        taker: num::dec(&data, "takerCommission")?,
        maker: num::dec(&data, "makerCommission")?,
    })
}
