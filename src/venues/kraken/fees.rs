//! Файл: venues/kraken/fees.rs
//! Что делает: ставки торговой комиссии нашего аккаунта по паре (taker/maker). Обновляются фоновой задачей пачками пар
//! (один TradeVolume на пачку — счётчик REST ключа очень мал, урок 15), не в момент котировки; котировка берёт ставку
//! только из памяти.
//! Кто вызывает: subs.rs (запускает фоновое обновление для подписанных пар), book.rs (читает ставку).
//! От чего зависит: rest.rs, num.rs, ctx.rs, settings/wait.rs.
//! Настройки: venue.fee_cache_s (период круга), venue.fee_requests_per_sec (пачек в секунду), venue.fee_pairs_per_request
//! (пар в пачке), venue.fee_retry_s (повтор круга со сбоями, решение 149) [kraken].
//!
//! Официально — POST /0/private/TradeVolume (OpenAPI https://docs.kraken.com/openapi/spot-rest.yaml): pair — список пар
//! через запятую, «Fees will not be included if pair is not specified»; result.fees (taker) и result.fees_maker — объекты
//! «пара → {fee, minfee, maxfee, nextfee, …}», fee — в процентах (0.2600 = 0,26%). С 08.09.2026 AssetPairs.fees пусты
//! (журнал изменений). Каждый вызов — +1 к счётчику REST ключа (Starter 15, убывание 0,33 в с) [RL-REST]. Скидки своей
//! монетой у Kraken нет — урок 23 (решение 158) неприменим; монета комиссии — котируемая (oflags fciq, order_rules.rs).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::settings::wait::{Note, until_ok};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Ставки комиссии пары (доли, например 0.0026 = 0,26%).
#[derive(Debug, Clone, Copy)]
pub struct Rates {
    pub taker: Decimal,
    pub maker: Decimal,
}

/// Ставки по имени пары Kraken. Дешёво клонируется.
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
                format!("Kraken {symbol}: ставка комиссии ещё не получена — котировки нет"),
            ))
        })
    }

    /// Фоновое обновление ставок по списку пар: пачками venue.fee_pairs_per_request, не чаще venue.fee_requests_per_sec
    /// пачек в секунду, круг — раз в venue.fee_cache_s; круг со сбоями — повтор через venue.fee_retry_s (решение 149).
    /// Работает, пока задачу не остановят. Ошибки — в журнал; незаданные настройки — ожидание их задания.
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
                    ctx.int("venue.fee_pairs_per_request")?,
                ))
            };
            let note = Note {
                journal: &ctx.journal,
                kind: "venue.fees",
                venue: Some(super::ctx::ID),
            };
            let Some((period, rate, per_req)) = until_ok(
                &ctx.settings,
                note,
                "Обновление ставок комиссии Kraken",
                read,
            )
            .await
            else {
                return;
            };
            let gap = Duration::from_secs_f64(1.0 / rate.max(1) as f64);
            let mut failed = false;
            for chunk in list.chunks(per_req.max(1) as usize) {
                match fetch(&ctx, chunk).await {
                    Ok((rates, missing)) => {
                        self.cache
                            .write()
                            .unwrap_or_else(|p| p.into_inner())
                            .extend(rates);
                        if !missing.is_empty() {
                            failed = true;
                            let m =
                                format!("TradeVolume: нет ставки для пар {}", missing.join(", "));
                            ctx.log(Level::Warn, "venue.fees", m).await;
                        }
                    }
                    Err(e) => {
                        failed = true;
                        let m = format!("Ставки комиссии Kraken не получены: {}", e.message);
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

/// Запрос ставок пачки пар. Возвращает: (ставки по паре, пары без ставки в ответе). Ошибки: ключи, связь, Kraken.
pub async fn fetch(
    ctx: &Ctx,
    pairs: &[String],
) -> GatewayResult<(HashMap<String, Rates>, Vec<String>)> {
    let form = vec![rest::p("pair", pairs.join(","))];
    let res = rest::private(ctx, "TradeVolume", form, Op::Other).await?;
    let rates = parse(&res)?;
    let missing = pairs
        .iter()
        .filter(|p| !rates.contains_key(*p))
        .cloned()
        .collect();
    Ok((rates, missing))
}

/// Разбор result TradeVolume: по каждой паре из fees — taker = fee / 100, maker — из fees_maker (нет — как taker:
/// бот ставит только taker-ордера IOC/FOK). Ошибка BadResponse — нет fees или fee не число.
pub fn parse(res: &Value) -> GatewayResult<HashMap<String, Rates>> {
    let taker = res
        .get("fees")
        .and_then(Value::as_object)
        .ok_or_else(|| super::errors::bad("TradeVolume: нет объекта fees", &res.to_string()))?;
    let maker = res.get("fees_maker").and_then(Value::as_object);
    let hundred = Decimal::ONE_HUNDRED;
    let mut out = HashMap::new();
    for (pair, f) in taker {
        let t = num::dec(f, "fee")? / hundred;
        let m = match maker.and_then(|m| m.get(pair)) {
            Some(mf) => num::dec(mf, "fee")? / hundred,
            None => t,
        };
        out.insert(pair.clone(), Rates { taker: t, maker: m });
    }
    Ok(out)
}
