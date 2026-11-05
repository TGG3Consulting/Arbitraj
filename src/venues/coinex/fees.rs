//! Файл: venues/coinex/fees.rs
//! Что делает: ставки торговой комиссии нашего ключа по паре (taker/maker). Обновляются фоновой задачей по одной паре
//! на запрос (запроса на все пары у CoinEx нет) в темпе venue.fee_requests_per_sec; котировка берёт ставку только из
//! памяти (урок 15).
//! Кто вызывает: subs.rs (запускает фоновое обновление для подписанных пар), book.rs (читает ставку), account.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs, settings/wait.rs.
//! Настройки: venue.fee_cache_s (период круга), venue.fee_retry_s (повтор после сбоя, решение 149),
//! venue.fee_requests_per_sec, venue.fee_rate_percent [coinex].
//!
//! Официально — Get Account Trading Fee Rate GET /account/trade-fee-rate?market_type=SPOT&market= (подпись; группа
//! «Query spot accounts (10r/1s)», Rate Limit) https://docs.coinex.com/api/v2/account/fees/http/get-account-trade-fees:
//! data {market, taker_rate, maker_rate}; «Currently, there is no endpoint provided to obtain the account fee rate of all
//! markets». ЕДИНИЦЫ СТАВКИ НЕ ОПИСАНЫ: в примере taker_rate "0.03", а у Get Market Status ставка по умолчанию "0.002" —
//! 0.03 это 3 % или 0,03 %, не ясно (урок 2). Поэтому делитель — решение человека по живому ответу:
//! venue.fee_rate_percent = да — значение в процентах (÷ 100), нет — доля; не задана — ставок нет, котировок CoinEx нет.
//!
//! Скидка CET (урок 23): POST /account/settings {cet_discount_enabled} — только глобальный выключатель, прочитать
//! текущее состояние нельзя (GET нет), размер скидки не описан. Расчёт всегда по обычной ставке ключа (худший случай);
//! факт комиссии и её монета — из ордера (discount_fee — «Fee charged in the deduction currency», order_fee.rs).

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

/// Ставки комиссии пары (доли, например 0.002 = 0,2%).
#[derive(Debug, Clone, Copy)]
pub struct Rates {
    pub taker: Decimal,
    pub maker: Decimal,
}

/// Ставки по рынку. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Fees {
    cache: Arc<RwLock<HashMap<String, Rates>>>,
}

impl Fees {
    /// Ставка из памяти. Ошибка StaleData — ставка ещё не получена (котировки нет).
    pub fn get(&self, ctx: &Ctx, market: &str) -> GatewayResult<Rates> {
        let cache = self.cache.read().unwrap_or_else(|p| p.into_inner());
        cache.get(market).copied().ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::StaleData,
                None,
                format!("CoinEx {market}: ставка комиссии ещё не получена — котировки нет"),
            ))
        })
    }

    /// Фоновое обновление: ставки рынков списка по одному запросу в темпе venue.fee_requests_per_sec; круг — раз в
    /// venue.fee_cache_s, после сбоя — через venue.fee_retry_s (решение 149). Работает, пока задачу не остановят.
    /// Ошибки — в журнал; незаданный период — одна запись и ожидание настройки (settings/wait.rs).
    pub async fn refresh_loop(self, ctx: Ctx, list: Vec<String>) {
        loop {
            let read = || {
                let period = ctx.int("venue.fee_cache_s")?;
                let per_sec = ctx.int("venue.fee_requests_per_sec")?;
                let percent = ctx.flag("venue.fee_rate_percent")?;
                Ok::<_, GatewayError>((period, per_sec.max(1), percent))
            };
            let note = Note {
                journal: &ctx.journal,
                kind: "venue.fees",
                venue: Some(super::ctx::ID),
            };
            let Some((period, per_sec, percent)) = until_ok(
                &ctx.settings,
                note,
                "Обновление ставок комиссии CoinEx",
                read,
            )
            .await
            else {
                return;
            };
            let gap = Duration::from_secs_f64(1.0 / per_sec as f64);
            let mut failed = Vec::new();
            for m in &list {
                match fetch(&ctx, m, percent).await {
                    Ok(r) => {
                        let mut c = self.cache.write().unwrap_or_else(|p| p.into_inner());
                        c.insert(m.clone(), r);
                    }
                    Err(e) => failed.push(format!("{m}: {}", e.message)),
                }
                tokio::time::sleep(gap).await;
            }
            if !failed.is_empty() {
                let m = format!(
                    "Ставки комиссии CoinEx не получены у {} пар: {}",
                    failed.len(),
                    failed.join("; ")
                );
                ctx.log(Level::Warn, "venue.fees", m).await;
            }
            let retry = (!failed.is_empty())
                .then(|| ctx.int("venue.fee_retry_s").ok())
                .flatten();
            tokio::time::sleep(Duration::from_secs(retry.unwrap_or(period))).await;
        }
    }
}

/// Что делает: ставки ключа по одному рынку. Принимает: рынок, «значение в процентах».
/// Возвращает: Rates (доли). Ошибки: ключи, связь, ответ CoinEx (поле не число, рынок не тот).
pub async fn fetch(ctx: &Ctx, market: &str, percent: bool) -> GatewayResult<Rates> {
    let q = vec![
        ("market_type", "SPOT".to_string()),
        ("market", market.to_string()),
    ];
    let path = "/v2/account/trade-fee-rate";
    let data = rest::signed(ctx, Method::GET, path, q, None, Op::Other).await?;
    parse(&data, market, percent)
}

/// Разбор data trade-fee-rate. Принимает: data, ожидаемый рынок, «значение в процентах». Ошибка BadResponse.
pub fn parse(data: &serde_json::Value, market: &str, percent: bool) -> GatewayResult<Rates> {
    if let Some(m) = num::text_opt(data, "market")
        && m != market
    {
        return Err(super::errors::bad(
            &format!("trade-fee-rate: market {m}, а запрошен {market}"),
            &data.to_string(),
        ));
    }
    let div = if percent {
        Decimal::ONE_HUNDRED
    } else {
        Decimal::ONE
    };
    Ok(Rates {
        taker: num::dec(data, "taker_rate")? / div,
        maker: num::dec(data, "maker_rate")? / div,
    })
}
