//! Файл: venues/binance/fees.rs
//! Что делает: ставки торговой комиссии нашего аккаунта по паре (fee_rates.rs) — фоновое обновление по кругу (не в
//! момент котировки: лимит запросов и скорость расчёта); котировка берёт ставку только из памяти.
//! Кто вызывает: subs.rs (запускает круг для подписанных пар), book.rs (читает ставку), discount_balance.rs (монета).
//! От чего зависит: rest.rs, fee_rates.rs, discount_balance.rs, ctx.rs, settings/wait.rs.
//! Настройки: venue.fee_cache_s, venue.fee_requests_per_sec, venue.fee_retry_s [binance].
//!
//! Официально: GET /api/v3/account/commission?symbol= (USER_DATA, вес 20 из REQUEST_WEIGHT по IP, rest-api «Query
//! Commission Rates»). Предел веса — rateLimits из живого exchangeInfo (в примере 6000 за минуту): 1 запрос/с = 1200
//! веса/мин — venue.fee_requests_per_sec держать малым, venue.fee_cache_s — большим.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use reqwest::Method;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::settings::wait::{Note, until_ok};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::fee_rates::Rates;
use super::rest;

/// Ставки по символу. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Fees {
    cache: Arc<RwLock<HashMap<String, Rates>>>,
    /// Баланс монеты скидки (решение 158, discount_balance.rs).
    pub discount: super::discount_balance::DiscountBalance,
}

impl Fees {
    /// Ставка из памяти. Ошибка StaleData — ставка ещё не получена (котировки нет).
    pub fn get(&self, ctx: &Ctx, symbol: &str) -> GatewayResult<Rates> {
        let cache = self.cache.read().unwrap_or_else(|p| p.into_inner());
        cache.get(symbol).cloned().ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::StaleData,
                None,
                format!("Binance {symbol}: ставка комиссии ещё не получена — котировки нет"),
            ))
        })
    }

    /// Монета скидки по полученным ставкам (discountAsset; у всех пар одна). None — ставок со скидкой нет. Ошибок нет.
    pub fn discount_asset(&self) -> Option<String> {
        let cache = self.cache.read().unwrap_or_else(|p| p.into_inner());
        cache
            .values()
            .map(|r| r.discount_asset.clone())
            .find(|a| !a.is_empty())
    }

    /// Фоновое обновление ставок по списку символов: не чаще venue.fee_requests_per_sec, круг — раз в
    /// venue.fee_cache_s; круг со сбоями — повтор через venue.fee_retry_s (решение 149). Работает, пока задачу не
    /// остановят. Ошибки — в журнал; незаданные настройки — одна запись и ожидание (settings/wait.rs).
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
                venue: Some(super::ctx::ID),
            };
            let Some((period, rate)) =
                until_ok(&ctx.settings, note, "Обновление ставок комиссии", read).await
            else {
                return;
            };
            let gap = Duration::from_secs_f64(1.0 / rate.max(1) as f64);
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

/// Запрос ставок одной пары. Ошибки: ключи, связь, ответ Binance.
async fn fetch(ctx: &Ctx, symbol: &str) -> GatewayResult<Rates> {
    let p = vec![("symbol", symbol.to_string())];
    let data = rest::signed(ctx, Method::GET, "/api/v3/account/commission", p, Op::Other).await?;
    Rates::parse(&data)
}
