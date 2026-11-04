//! Файл: venues/okx/fees.rs
//! Что делает: ставки торговой комиссии нашего аккаунта по паре (taker/maker). Обновляются фоновой задачей по одной паре
//! на запрос (не в момент котировки); котировка берёт ставку только из памяти (урок 15).
//! Кто вызывает: subs.rs (запускает фоновое обновление для подписанных пар), book.rs (читает ставку).
//! От чего зависит: rest.rs, num.rs, ctx.rs, settings/wait.rs.
//! Настройки: venue.fee_cache_s (период круга), venue.fee_requests_per_sec (частота запросов; у OKX не больше 2 —
//! «5 requests per 2 seconds»), venue.fee_retry_s (повтор круга со сбоями, решение 149) [okx].
//!
//! Официально — «Trading Account → REST API → Get fee rates» GET /api/v5/account/trade-fee (5 запросов / 2 с, User ID)
//! https://www.okx.com/docs-v5/en/: instType SPOT, instId — «Specifying this parameter returns the correct applicable
//! fee rates»; taker / maker — «positive number, which means the rate of rebate; negative number, which means the rate
//! of commission» → ставка комиссии бота = −taker (комиссия — положительная доля, скидка — отрицательная).
//! Скидки за удержание OKB у OKX нет (справка «Trading Fee Rules FAQ»: «The platform currently doesn't offer a feature
//! that lets you offset trading fees by holding OKB») — урок 23 (скидка своей монетой, решение 158) неприменим.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::settings::wait::{Note, until_ok};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::{num, rest};

/// Запросов в секунду — потолок официального «5 requests per 2 seconds» (целое вниз).
pub const MAX_REQUESTS_PER_SEC: u64 = 2;

/// Ставки комиссии пары (доли, например 0.001 = 0,1%): комиссия — больше нуля, скидка — меньше.
#[derive(Debug, Clone, Copy)]
pub struct Rates {
    pub taker: Decimal,
    pub maker: Decimal,
}

/// Ставки по instId. Дешёво клонируется.
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
                format!("OKX {symbol}: ставка комиссии ещё не получена — котировки нет"),
            ))
        })
    }

    /// Фоновое обновление ставок по списку instId: по одной паре, не чаще venue.fee_requests_per_sec, круг — раз в
    /// venue.fee_cache_s; круг со сбоями — повтор через venue.fee_retry_s (решение 149). Работает, пока задачу не
    /// остановят. Ошибки — в журнал; незаданные настройки — одна запись и ожидание их задания (settings/wait.rs).
    pub async fn refresh_loop(
        self,
        ctx: Ctx,
        _symbols: super::symbols::Symbols,
        list: Vec<String>,
    ) {
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
                venue: Some(super::ctx::ID),
            };
            let Some((period, rate)) =
                until_ok(&ctx.settings, note, "Обновление ставок комиссии OKX", read).await
            else {
                return;
            };
            let rate = rate.clamp(1, MAX_REQUESTS_PER_SEC);
            let gap = Duration::from_secs_f64(1.0 / rate as f64);
            let mut failed = false;
            for sym in &list {
                match fetch(&ctx, sym).await {
                    Ok(r) => {
                        let mut c = self.cache.write().unwrap_or_else(|p| p.into_inner());
                        c.insert(sym.clone(), r);
                    }
                    Err(e) => {
                        failed = true;
                        let m = format!("Ставка комиссии {sym} не получена: {}", e.message);
                        ctx.log(Level::Warn, "venue.fees", m).await;
                    }
                }
                tokio::time::sleep(gap).await;
            }
            let retry = failed.then(|| ctx.int("venue.fee_retry_s").ok()).flatten();
            tokio::time::sleep(Duration::from_secs(retry.unwrap_or(period))).await;
        }
    }
}

/// Запрос ставки одной пары. Возвращает: ставки (комиссия > 0). Ошибки: ключи, связь, ответ OKX.
pub async fn fetch(ctx: &Ctx, inst: &str) -> GatewayResult<Rates> {
    let q = vec![
        ("instType", "SPOT".to_string()),
        ("instId", inst.to_string()),
    ];
    let data = rest::get(ctx, "/api/v5/account/trade-fee", q).await?;
    parse(rest::first(&data, "trade-fee")?)
}

/// Разбор строки trade-fee: ставка бота = −taker, −maker (знак — в шапке). Ошибка BadResponse — нет поля.
pub fn parse(row: &serde_json::Value) -> GatewayResult<Rates> {
    Ok(Rates {
        taker: -num::dec(row, "taker")?,
        maker: -num::dec(row, "maker")?,
    })
}
