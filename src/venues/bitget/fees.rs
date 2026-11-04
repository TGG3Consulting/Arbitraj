//! Файл: venues/bitget/fees.rs
//! Что делает: ставки торговой комиссии нашего ключа по паре (taker/maker) и состояние скидки BGB. Обновляются фоновой
//! задачей одним запросом на все пары (не в момент котировки); котировка берёт ставку только из памяти (урок 15).
//! Кто вызывает: subs.rs (запускает фоновое обновление для подписанных пар), book.rs (читает ставку), account.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs, settings/wait.rs.
//! Настройки: venue.fee_cache_s (период круга), venue.fee_retry_s (повтор после сбоя, решение 149) [bitget].
//!
//! Официально:
//! - Get Business Line All Symbol Trade Rate GET /api/v2/common/all-trade-rate?businessType=spot («10 times/1s (UID)»)
//!   https://www.bitget.com/docs/catalog/classic-common-public/classic-common#get-trade-rate — data: массив {symbol,
//!   makerFeeRate, takerFeeRate} (в схеме data — объект, в примере — массив: разбор принимает массив, урок 2).
//! - Get BGB Deduct Info GET /api/v2/spot/account/deduct-info https://www.bitget.com/docs/catalog/classic-spot-account/
//!   classic-spot-account#get-bgb-deduct-info — data.deduct «on / off».
//!
//! Скидка BGB (урок 23, решение 158 — образец Gate GT): размер скидки и учтена ли она в trade-rate, в документации нет;
//! известно только, что при включённой скидке комиссия списывается в BGB, а при нехватке BGB — остаток из получаемой
//! монеты (feeDetail.newFees: d — «deducted by BGB», r — «remaining … when BGB is insufficient»). Поэтому расчёт всегда
//! по обычной ставке ключа в монете сделки — худший случай: со скидкой комиссия только меньше и уходит в BGB, монеты
//! остаётся не меньше. Расхождение скидки на счёте с расчётом — не отказ, а запись в журнал; факт комиссии и её монета —
//! из ордера (order_fee.rs).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
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

/// Ставки по символу и скидка BGB (0 — не прочитана, 1 — off, 2 — on). Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Fees {
    cache: Arc<RwLock<HashMap<String, Rates>>>,
    deduct: Arc<AtomicU8>,
}

impl Fees {
    /// Ставка из памяти. Ошибка StaleData — ставка ещё не получена (котировки нет).
    pub fn get(&self, ctx: &Ctx, symbol: &str) -> GatewayResult<Rates> {
        let cache = self.cache.read().unwrap_or_else(|p| p.into_inner());
        cache.get(symbol).copied().ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::StaleData,
                None,
                format!("Bitget {symbol}: ставка комиссии ещё не получена — котировки нет"),
            ))
        })
    }

    /// Скидка BGB на счёте по последнему кругу: Some(true) — on, Some(false) — off, None — не прочитана. Ошибок нет.
    pub fn bgb_deduct(&self) -> Option<bool> {
        match self.deduct.load(Ordering::Relaxed) {
            1 => Some(false),
            2 => Some(true),
            _ => None,
        }
    }

    /// Фоновое обновление: ставки всех пар одним запросом (в память — только символы списка) и состояние скидки BGB;
    /// круг — раз в venue.fee_cache_s, после сбоя — через venue.fee_retry_s (решение 149). Работает, пока задачу не
    /// остановят. Ошибки — в журнал; незаданный период — одна запись и ожидание настройки (settings/wait.rs).
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
                "Обновление ставок комиссии Bitget",
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
                        let m = format!("all-trade-rate: нет ставок пар {missing:?}");
                        ctx.log(Level::Warn, "venue.fees", m).await;
                    }
                }
                Err(e) => {
                    failed = true;
                    let m = format!("Ставки комиссии Bitget не получены: {}", e.message);
                    ctx.log(Level::Warn, "venue.fees", m).await;
                }
            }
            self.read_deduct(&ctx).await;
            let retry = failed.then(|| ctx.int("venue.fee_retry_s").ok()).flatten();
            tokio::time::sleep(Duration::from_secs(retry.unwrap_or(period))).await;
        }
    }

    /// Читает скидку BGB; изменение (и включённая скидка) — в журнал. Сбой — в журнал, прежнее значение остаётся.
    async fn read_deduct(&self, ctx: &Ctx) {
        let got = deduct_info(ctx).await;
        let new = match &got {
            Ok(true) => 2,
            Ok(false) => 1,
            Err(_) => self.deduct.load(Ordering::Relaxed),
        };
        let old = self.deduct.swap(new, Ordering::Relaxed);
        let m = match got {
            Err(e) => format!("Скидка BGB Bitget не прочитана: {}", e.message),
            Ok(true) if old != new => {
                "Скидка BGB на счёте Bitget включена (deduct = on): расчёт по обычной ставке \
                                       (худший случай), факт комиссии — из ордера"
                    .into()
            }
            Ok(false) if old != new => "Скидка BGB на счёте Bitget выключена (deduct = off)".into(),
            Ok(_) => return,
        };
        ctx.log(Level::Info, "venue.fees", m).await;
    }
}

/// Что делает: ставки всех спот-пар ключа (all-trade-rate). Возвращает: символ → ставки.
/// Ошибки: ключи, связь, ответ Bitget (data не массив, поле не число).
async fn fetch(ctx: &Ctx) -> GatewayResult<HashMap<String, Rates>> {
    let q = vec![("businessType", "spot".to_string())];
    let path = "/api/v2/common/all-trade-rate";
    let data = rest::signed(ctx, Method::GET, path, q, None, Op::Other).await?;
    parse_all(&data)
}

/// Разбор data all-trade-rate (массив {symbol, makerFeeRate, takerFeeRate}). Ошибка BadResponse.
pub fn parse_all(data: &serde_json::Value) -> GatewayResult<HashMap<String, Rates>> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("all-trade-rate: data не массив", &data.to_string()))?;
    list.iter()
        .map(|x| {
            Ok((
                num::text(x, "symbol")?,
                Rates {
                    taker: num::dec(x, "takerFeeRate")?,
                    maker: num::dec(x, "makerFeeRate")?,
                },
            ))
        })
        .collect()
}

/// Что делает: скидка BGB на счёте (deduct-info). Возвращает: true — on. Ошибки: ключи, связь, значение не on/off.
pub async fn deduct_info(ctx: &Ctx) -> GatewayResult<bool> {
    let path = "/api/v2/spot/account/deduct-info";
    let data = rest::signed(ctx, Method::GET, path, vec![], None, Op::Other).await?;
    match num::text(&data, "deduct")?.as_str() {
        "on" => Ok(true),
        "off" => Ok(false),
        other => Err(super::errors::bad(
            &format!("deduct-info: deduct = {other} (по документации on / off)"),
            &data.to_string(),
        )),
    }
}
