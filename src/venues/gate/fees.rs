//! Файл: venues/gate/fees.rs
//! Что делает: ставки торговой комиссии нашего аккаунта по паре (taker/maker).
//! Ставки обновляются фоновой задачей (не в момент котировки — чтобы не упираться в лимит
//! запросов и не задерживать расчёт); котировка берёт ставку только из памяти.
//! Кто вызывает: subs.rs (запускает фоновое обновление для подписанных пар), book.rs (читает ставку).
//! От чего зависит: rest.rs, num.rs, ctx.rs, settings/wait.rs.
//! Настройки: venue.fee_cache_s (как часто обновлять), venue.fee_requests_per_sec (частота запросов).
//!
//! Официальная документация: API v4 → Wallet → «Query personal trading fees» GET /wallet/fee?currency_pair=
//! https://www.gate.com/docs/developers/apiv4/en/ (SDK: docs/WalletApi.md#get_trade_fee, модель docs/TradeFee.md) —
//! taker_fee «spot taker fee rate», maker_fee, gt_discount; «Usually fee settings are the same for all currency pairs»,
//! но currency_pair уточняет («Specify currency pair to get more accurate fee settings») — запрос по паре.
//! GET /spot/fee того же смысла помечен «This API is deprecated» — не используется. Пакетный GET /spot/batch_fee
//! («Maximum 50 currency pairs per request») не используется: формат списка пар в параметре не описан (вопрос
//! Тиграну, docs/TASKS.md). Лимит — приватные «200r/10s» на конечную точку: venue.fee_requests_per_sec — меньше 20.
//! Скидка GT (gt_discount, gt_taker_fee) не учитывается: при ней комиссия списывается в GT — другой монетой; котировка
//! берёт taker_fee (без скидки — не меньше факта), факт комиссии — из ордера (order_state.rs).

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
                format!("Gate {symbol}: ставка комиссии ещё не получена — котировки нет"),
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
            for s in &symbols {
                match fetch(&ctx, s).await {
                    Ok(r) => {
                        self.cache
                            .write()
                            .unwrap_or_else(|p| p.into_inner())
                            .insert(s.clone(), r);
                    }
                    Err(e) => {
                        let m = format!("Ставка комиссии {s} не получена: {}", e.message);
                        let _ = ctx
                            .journal
                            .write(Level::Warn, "venue.fees", Some(super::ctx::ID), m, None)
                            .await;
                    }
                }
                tokio::time::sleep(gap).await;
            }
            tokio::time::sleep(Duration::from_secs(period)).await;
        }
    }
}

/// Запрос ставок одной пары (GET /wallet/fee?currency_pair=, см. шапку). Ошибки: ключи, связь, ответ Gate.
async fn fetch(ctx: &Ctx, symbol: &str) -> GatewayResult<Rates> {
    let p = vec![("currency_pair", symbol.to_string())];
    let data = rest::signed(ctx, Method::GET, "/wallet/fee", p, None, Op::Other).await?;
    Ok(Rates {
        taker: num::dec(&data, "taker_fee")?,
        maker: num::dec(&data, "maker_fee")?,
    })
}
