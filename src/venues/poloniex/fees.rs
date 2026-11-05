//! Файл: venues/poloniex/fees.rs
//! Что делает: ставки торговой комиссии нашего ключа по паре (taker/maker) и признак скидки TRX. Обновляются фоновой
//! задачей одним запросом на все пары (не в момент котировки); котировка берёт ставку только из памяти (урок 15).
//! Кто вызывает: subs.rs (запускает фоновое обновление для подписанных пар), book.rs (читает ставку), account.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs, settings/wait.rs.
//! Настройки: venue.fee_cache_s (период круга), venue.fee_retry_s (повтор после сбоя, решение 149) [poloniex].
//!
//! Официально — Fee Info GET /feeinfo (https://api-docs.poloniex.com/spot/api/private/account; «50» на VIP0 — Spot →
//! API → Rate Limits): trxDiscount «Whether a discount applies when paying fees with TRX», makerRate, takerRate,
//! volume30D, specialFeeRates[] {symbol, makerRate, takerRate} — «Custom fee rates per symbol». Ставка пары = своя из
//! specialFeeRates, иначе общая.
//!
//! Скидка TRX (урок 23, решение 158 — образец Gate GT): размер скидки, как она включается и что при нехватке TRX, в
//! документации нет. Поэтому расчёт всегда по обычной ставке ключа в монете сделки — худший случай; включённая скидка
//! — не отказ, а запись в журнал; факт комиссии и её монета — из сделок ордера (fills.rs).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
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
use super::{num, rest};

/// Ставки комиссии пары (доли, например 0.001 = 0,1%).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    pub taker: Decimal,
    pub maker: Decimal,
}

/// Ответ /feeinfo: общая ставка, ставки отдельных пар, скидка TRX.
#[derive(Debug, Clone)]
pub struct Info {
    pub general: Rates,
    pub special: HashMap<String, Rates>,
    pub trx_discount: bool,
}

impl Info {
    /// Ставка пары: своя из specialFeeRates, иначе общая. Ошибок нет.
    pub fn of(&self, symbol: &str) -> Rates {
        self.special.get(symbol).copied().unwrap_or(self.general)
    }
}

/// Ставки по символу и скидка TRX (0 — не прочитана, 1 — нет, 2 — да). Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Fees {
    cache: Arc<RwLock<HashMap<String, Rates>>>,
    trx: Arc<AtomicU8>,
}

impl Fees {
    /// Ставка из памяти. Ошибка StaleData — ставка ещё не получена (котировки нет).
    pub fn get(&self, ctx: &Ctx, symbol: &str) -> GatewayResult<Rates> {
        let cache = self.cache.read().unwrap_or_else(|p| p.into_inner());
        cache.get(symbol).copied().ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::StaleData,
                None,
                format!("Poloniex {symbol}: ставка комиссии ещё не получена — котировки нет"),
            ))
        })
    }

    /// Фоновое обновление: ставки всех пар одним запросом (в память — только символы списка) и признак скидки TRX;
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
                "Обновление ставок комиссии Poloniex",
                read,
            )
            .await
            else {
                return;
            };
            let failed = match fetch(&ctx).await {
                Ok(info) => {
                    {
                        let mut c = self.cache.write().unwrap_or_else(|p| p.into_inner());
                        for s in &list {
                            c.insert(s.clone(), info.of(s));
                        }
                    }
                    self.note_trx(&ctx, info.trx_discount).await;
                    false
                }
                Err(e) => {
                    let m = format!("Ставки комиссии Poloniex не получены: {}", e.message);
                    ctx.log(Level::Warn, "venue.fees", m).await;
                    true
                }
            };
            let retry = failed.then(|| ctx.int("venue.fee_retry_s").ok()).flatten();
            tokio::time::sleep(Duration::from_secs(retry.unwrap_or(period))).await;
        }
    }

    /// Запоминает признак скидки TRX; изменение — в журнал. Ошибок нет.
    async fn note_trx(&self, ctx: &Ctx, on: bool) {
        let new = if on { 2 } else { 1 };
        if self.trx.swap(new, Ordering::Relaxed) == new {
            return;
        }
        let m = if on {
            "Скидка TRX на счёте Poloniex включена (trxDiscount = true): расчёт по обычной ставке (худший случай), \
             факт комиссии — из сделок ордера"
        } else {
            "Скидка TRX на счёте Poloniex выключена (trxDiscount = false)"
        };
        ctx.log(Level::Info, "venue.fees", m.into()).await;
    }
}

/// Что делает: ставки ключа (/feeinfo). Возвращает: Info. Ошибки: ключи, связь, ответ Poloniex.
pub async fn fetch(ctx: &Ctx) -> GatewayResult<Info> {
    let data = rest::signed(ctx, Method::GET, "/feeinfo", vec![], None, Op::Other).await?;
    parse(&data)
}

/// Разбор ответа /feeinfo. Ошибка BadResponse — нет полей или поле не число.
pub fn parse(data: &Value) -> GatewayResult<Info> {
    let rates = |v: &Value| -> GatewayResult<Rates> {
        Ok(Rates {
            taker: num::dec(v, "takerRate")?,
            maker: num::dec(v, "makerRate")?,
        })
    };
    let mut special = HashMap::new();
    if let Some(list) = data.get("specialFeeRates").and_then(Value::as_array) {
        for x in list {
            special.insert(num::text(x, "symbol")?, rates(x)?);
        }
    }
    Ok(Info {
        general: rates(data)?,
        special,
        trx_discount: num::flag(data, "trxDiscount").unwrap_or(false),
    })
}
