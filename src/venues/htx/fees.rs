//! Файл: venues/htx/fees.rs
//! Что делает: ставки торговой комиссии нашего ключа по паре (taker / maker). Обновляются фоновой задачей пачками
//! символов (не в момент котировки); котировка берёт ставку только из памяти (урок 15).
//! Кто вызывает: subs.rs (запускает фоновое обновление для подписанных пар), book.rs (читает ставку).
//! От чего зависит: rest.rs, num.rs, ctx.rs, settings/wait.rs.
//! Настройки: venue.fee_cache_s (период круга), venue.fee_retry_s (повтор после сбоя, решение 149),
//! venue.fee_symbols_per_req (символов в запросе — предел в документации не указан), venue.fee_requests_per_sec [htx].
//!
//! Официально — Get Current Fee Rate Applied to The User GET /v2/reference/transact-fee-rate?symbols=a,b («The trading
//! symbols to query, separated by comma», право Read; наибольшее число символов и лимит частоты не указаны)
//! https://huobiapi.github.io/docs/spot/v1/en/#get-current-fee-rate-applied-to-the-user: data[] {symbol, makerFeeRate,
//! takerFeeRate, actualMakerRate, actualTakerRate}.
//! Скидка HT / баллами HB (урок 23, решение 158): включается пользователем на счёте, состояние переключателя в
//! документации не читается (в SDK есть /v1/account/overview/info и /v1/account/fee/switch — в документации их нет);
//! при скидке комиссия списывается HT / баллами, монета сделки не уменьшается (FAQ: filled-fees пусто, filled-points
//! есть). Поэтому расчёт всегда по худшей ставке — большей из takerFeeRate и actualTakerRate — в монете сделки: со
//! скидкой комиссия только меньше и уходит в HT, монеты остаётся не меньше; HT кончился — сделки идут по той же ставке,
//! не останавливаются. Факт комиссии и её монета — из сделок ордера (order_fee.rs).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::settings::wait::{Note, until_ok};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::{num, rest};

/// Ставки комиссии пары (доли, например 0.002 = 0,2%).
#[derive(Debug, Clone, Copy)]
pub struct Rates {
    /// Худшая ставка тейкера (большая из takerFeeRate и actualTakerRate).
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
                format!("HTX {symbol}: ставка комиссии ещё не получена — котировки нет"),
            ))
        })
    }

    /// Фоновое обновление ставок символов списка: пачками по venue.fee_symbols_per_req, между запросами — пауза по
    /// venue.fee_requests_per_sec; круг — раз в venue.fee_cache_s, после сбоя — через venue.fee_retry_s (решение 149).
    /// Работает, пока задачу не остановят. Ошибки — в журнал; незаданная настройка — ожидание её (settings/wait.rs).
    pub async fn refresh_loop(self, ctx: Ctx, list: Vec<String>) {
        loop {
            let read = || {
                Ok::<_, GatewayError>((
                    ctx.int("venue.fee_cache_s")?,
                    ctx.int("venue.fee_symbols_per_req")?,
                    ctx.int("venue.fee_requests_per_sec")?,
                ))
            };
            let note = Note {
                journal: &ctx.journal,
                kind: "venue.fees",
                venue: Some(super::ctx::ID),
            };
            let Some((period, per_req, per_sec)) =
                until_ok(&ctx.settings, note, "Обновление ставок комиссии HTX", read).await
            else {
                return;
            };
            let gap = Duration::from_secs_f64(1.0 / per_sec.max(1) as f64);
            let mut failed = false;
            for chunk in list.chunks(per_req.max(1) as usize) {
                match fetch(&ctx, chunk).await {
                    Ok(got) => {
                        let mut c = self.cache.write().unwrap_or_else(|p| p.into_inner());
                        for s in chunk {
                            match got.get(s) {
                                Some(r) => {
                                    c.insert(s.clone(), *r);
                                }
                                None => failed = true,
                            }
                        }
                    }
                    Err(e) => {
                        failed = true;
                        let m = format!("Ставки комиссии HTX не получены: {}", e.message);
                        ctx.log(Level::Warn, "venue.fees", m).await;
                    }
                }
                tokio::time::sleep(gap).await;
            }
            if failed {
                let m = "transact-fee-rate: ставки получены не у всех пар (повтор через venue.fee_retry_s)";
                ctx.log(Level::Warn, "venue.fees", m.into()).await;
            }
            let retry = failed.then(|| ctx.int("venue.fee_retry_s").ok()).flatten();
            tokio::time::sleep(Duration::from_secs(retry.unwrap_or(period))).await;
        }
    }
}

/// Что делает: ставки пачки символов. Возвращает: символ → ставки. Ошибки: ключи, связь, ответ HTX.
async fn fetch(ctx: &Ctx, symbols: &[String]) -> GatewayResult<HashMap<String, Rates>> {
    let q = vec![("symbols", symbols.join(","))];
    let data = rest::get(ctx, "/v2/reference/transact-fee-rate", q).await?;
    parse(&data)
}

/// Разбор data transact-fee-rate. Ошибка BadResponse — не массив или поле не число.
pub fn parse(data: &serde_json::Value) -> GatewayResult<HashMap<String, Rates>> {
    let list = data.as_array().ok_or_else(|| {
        super::errors::bad("transact-fee-rate: data не массив", &data.to_string())
    })?;
    list.iter()
        .map(|x| {
            let base = num::dec(x, "takerFeeRate")?;
            let actual = num::dec_opt(x, "actualTakerRate")?.unwrap_or(base);
            Ok((
                num::text(x, "symbol")?,
                Rates {
                    taker: base.max(actual),
                    maker: num::dec(x, "makerFeeRate")?,
                },
            ))
        })
        .collect()
}
