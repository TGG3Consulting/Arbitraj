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
//! Скидка GT (решение 135, Тигран 04.11): вариант списания — настройка venue.fee_deduction [gate] (coin — комиссия в
//! монете сделки по taker_fee; gt — в GT по gt_taker_fee, «Taker fee rate if using GT deduction. It will be 0 if GT
//! deduction is disabled»; gt_discount — «Whether GT deduction discount is enabled»). Вариант сверяется со счётом:
//! coin при включённой скидке GT или gt при выключенной — котировки нет (Gate списал бы не так, как посчитано). Что
//! Gate делает при нехватке GT, в документации API нет (вопрос Тиграну, docs/TASKS.md); факт комиссии — из ордера
//! (order_state.rs: fee / fee_currency или gt_fee).

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

/// Ставки комиссии пары (доли, например 0.001 = 0,1%) и скидка GT аккаунта.
#[derive(Debug, Clone, Copy)]
pub struct Rates {
    pub taker: Decimal,
    pub maker: Decimal,
    /// Скидка GT включена на счёте (gt_discount).
    pub gt_discount: bool,
    /// Ставка taker при списании в GT (gt_taker_fee; 0 — скидка выключена).
    pub gt_taker: Decimal,
}

impl Rates {
    /// Что делает: ставки из ответа GET /wallet/fee (поля — шапка). Принимает: data ответа. Возвращает: Rates.
    /// Ошибки: BadResponse — нет taker_fee / maker_fee или не число; gt_* отсутствуют — скидки нет (0, false).
    pub fn parse(data: &serde_json::Value) -> GatewayResult<Self> {
        Ok(Self {
            taker: num::dec(data, "taker_fee")?,
            maker: num::dec(data, "maker_fee")?,
            gt_discount: data.get("gt_discount").and_then(|v| v.as_bool()) == Some(true),
            gt_taker: num::dec_opt(data, "gt_taker_fee")?.unwrap_or_default(),
        })
    }

    /// Что делает: ставка taker для расчёта по варианту списания venue.fee_deduction (coin / gt) и проверка, что
    /// вариант совпадает со счётом. Принимает: вариант. Возвращает: (ставка, комиссия в GT ли).
    /// Ошибки: текст причины (котировки нет): вариант не совпадает со скидкой GT счёта или неизвестен.
    pub fn plan(&self, mode: &str) -> Result<(Decimal, bool), String> {
        match (mode, self.gt_discount) {
            (FEE_COIN, false) => Ok((self.taker, false)),
            (FEE_GT, true) if self.gt_taker > Decimal::ZERO => Ok((self.gt_taker, true)),
            (FEE_COIN, true) => Err(
                "venue.fee_deduction = coin, а на счёте Gate включена скидка GT (gt_discount) — комиссия ушла бы в GT; \
                 выключите скидку в кабинете или поставьте gt"
                    .into(),
            ),
            (FEE_GT, _) => Err(format!(
                "venue.fee_deduction = gt, а скидка GT на счёте Gate выключена (gt_discount = {}, gt_taker_fee = {}) — \
                 включите её в кабинете или поставьте coin",
                self.gt_discount, self.gt_taker
            )),
            (other, _) => Err(format!(
                "venue.fee_deduction = {other}: у Gate варианты {FEE_COIN} (в монете сделки) или {FEE_GT} (в GT)"
            )),
        }
    }
}

/// Варианты venue.fee_deduction [gate]: комиссия в монете сделки или в GT (решение 135, Тигран 04.11).
pub const FEE_COIN: &str = "coin";
pub const FEE_GT: &str = "gt";

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
    Rates::parse(&data)
}
