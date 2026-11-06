//! Файл: venues/bingx/fees.rs
//! Что делает: ставки торговой комиссии нашего аккаунта по паре (taker/maker).
//! Ставки обновляются фоновой задачей (не в момент котировки — чтобы не упираться в лимит
//! запросов и не задерживать расчёт); котировка берёт ставку только из памяти.
//! Решение 204: код 100410 (эндпоинт заблокирован до метки в ответе) — пауза запросов ставок до разблокировки
//! (fees_pause.rs), одна запись в журнал на паузу; ставки, полученные раньше, остаются в памяти.
//! Кто вызывает: market.rs (запускает фоновое обновление для подписанных пар), book.rs (читает ставку).
//! От чего зависит: rest.rs, num.rs, ctx.rs, settings/wait.rs, fees_pause.rs.
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
                format!("BingX {symbol}: ставка комиссии ещё не получена — котировки нет"),
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
            let mut i = 0;
            while let Some(s) = symbols.get(i) {
                match fetch(&ctx, s).await {
                    Ok(r) => {
                        self.cache
                            .write()
                            .unwrap_or_else(|p| p.into_inner())
                            .insert(s.clone(), r);
                    }
                    // Решение 204: эндпоинт заблокирован — ждать разблокировки и повторить ту же пару.
                    // Метки нет и venue.fee_retry_s не задана — круг прерывается, повтор — через venue.fee_cache_s.
                    Err(e) if e.code == ErrorCode::RateLimited => {
                        if pause(&ctx, &e.message).await {
                            continue;
                        }
                        failed = true;
                        break;
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
                i += 1;
            }
            let retry = failed.then(|| ctx.int("venue.fee_retry_s").ok()).flatten();
            tokio::time::sleep(Duration::from_secs(retry.unwrap_or(period))).await;
        }
    }
}

/// Решение 204: пауза запросов ставок после 100410 — до момента разблокировки из ответа; метки нет — на
/// venue.fee_retry_s. Одна запись в журнал на паузу. Принимает: окружение, текст ошибки. Возвращает: true — подождали
/// (повторить ту же пару), false — ждать нечем (метки нет, venue.fee_retry_s не задана — круг прерывается). Ошибок нет.
async fn pause(ctx: &Ctx, msg: &str) -> bool {
    let now = chrono::Utc::now();
    let got = match super::fees_pause::unblock_at(msg).filter(|t| *t > now) {
        Some(t) => Some((
            (t - now).to_std().unwrap_or_default(),
            format!(
                "до {} UTC (метка разблокировки из ответа)",
                t.format("%H:%M:%S")
            ),
        )),
        None => ctx.int("venue.fee_retry_s").ok().map(|s| {
            (
                Duration::from_secs(s),
                format!("на venue.fee_retry_s = {s} с (метки в ответе нет)"),
            )
        }),
    };
    let m = match &got {
        Some((_, why)) => format!(
            "Ставки комиссии: эндпоинт commissionRate на паузе {why} — запросы ставок остановлены, полученные \
             ставки действуют. Ответ: {msg}"
        ),
        None => format!(
            "Ставки комиссии: лимит частоты, метки разблокировки нет и venue.fee_retry_s не задана — круг \
             прерван до venue.fee_cache_s. Ответ: {msg}"
        ),
    };
    let _ = ctx
        .journal
        .write(Level::Warn, "venue.fees", Some(super::ctx::ID), m, None)
        .await;
    match got {
        Some((wait, _)) => {
            tokio::time::sleep(wait).await;
            true
        }
        None => false,
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
