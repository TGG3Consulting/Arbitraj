//! Файл: venues/kucoin/fees.rs
//! Что делает: ставки торговой комиссии нашего аккаунта по паре (taker/maker) с коэффициентом пары. Обновляются фоновой
//! задачей пачками по 10 пар (не в момент котировки); котировка берёт ставку только из памяти (урок 15).
//! Кто вызывает: subs.rs (запускает фоновое обновление для подписанных пар), book.rs (читает ставку).
//! От чего зависит: rest.rs, num.rs, symbols.rs (коэффициенты), ctx.rs, settings/wait.rs.
//! Настройки: venue.fee_cache_s (период круга), venue.fee_requests_per_sec (частота запросов) [kucoin].
//!
//! Официально — Get Actual Fee - Spot/Margin GET /api/v1/trade-fees?symbols= (пул Spot, вес 3)
//! https://www.kucoin.com/docs-new/api-3470150: «You can inquire about fee rates of 10 trading pairs each time at most»;
//! ответ — массив {symbol, takerFeeRate, makerFeeRate}. Коэффициент пары — GET /api/v2/symbols: «The actual fee needs
//! to be multiplied by this coefficient to get the final fee … If set to 0, it means no fee» — ставка = ставка
//! trade-fees × коэффициент (сверить с feeRate живых сделок /api/v1/hf/fills — docs/TASKS.md).

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
use super::reply::Pool;
use super::symbols::Symbols;
use super::{num, rest};

/// Сколько пар в одном запросе — официально «10 trading pairs each time at most».
const PAIRS_PER_REQUEST: usize = 10;

/// Ставки комиссии пары (доли, например 0.001 = 0,1%) — уже с коэффициентом пары.
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
                format!("KuCoin {symbol}: ставка комиссии ещё не получена — котировки нет"),
            ))
        })
    }

    /// Фоновое обновление ставок по списку символов: пачками по 10, не чаще venue.fee_requests_per_sec, круг — раз в
    /// venue.fee_cache_s. Работает, пока задачу не остановят. Ошибки — в журнал; незаданные настройки — одна запись и
    /// ожидание их задания (settings/wait.rs).
    pub async fn refresh_loop(self, ctx: Ctx, symbols: Symbols, list: Vec<String>) {
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
            let Some((period, rate)) = until_ok(
                &ctx.settings,
                note,
                "Обновление ставок комиссии KuCoin",
                read,
            )
            .await
            else {
                return;
            };
            let gap = Duration::from_secs_f64(1.0 / rate.max(1) as f64);
            for chunk in list.chunks(PAIRS_PER_REQUEST) {
                match fetch(&ctx, chunk).await {
                    Ok(got) => {
                        let rules = symbols.all();
                        let mut c = self.cache.write().unwrap_or_else(|p| p.into_inner());
                        for (sym, r) in got {
                            // Коэффициент пары по символу; пары нет в справочнике — без коэффициента (1).
                            let (tc, mc) = rules
                                .values()
                                .find(|x| x.symbol == sym)
                                .map_or((Decimal::ONE, Decimal::ONE), |x| {
                                    (x.taker_coef, x.maker_coef)
                                });
                            c.insert(
                                sym,
                                Rates {
                                    taker: r.taker * tc,
                                    maker: r.maker * mc,
                                },
                            );
                        }
                    }
                    Err(e) => {
                        let m = format!(
                            "Ставки комиссии {} не получены: {}",
                            chunk.join(","),
                            e.message
                        );
                        ctx.log(Level::Warn, "venue.fees", m).await;
                    }
                }
                tokio::time::sleep(gap).await;
            }
            tokio::time::sleep(Duration::from_secs(period)).await;
        }
    }
}

/// Запрос ставок пачки пар. Возвращает: (символ, ставки без коэффициента). Ошибки: ключи, связь, ответ KuCoin.
async fn fetch(ctx: &Ctx, chunk: &[String]) -> GatewayResult<Vec<(String, Rates)>> {
    let p = vec![("symbols", chunk.join(","))];
    let data = rest::signed(
        ctx,
        Method::GET,
        "/api/v1/trade-fees",
        p,
        None,
        Op::Other,
        Pool::Spot,
    )
    .await?;
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("trade-fees: data не массив", &data.to_string()))?;
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
