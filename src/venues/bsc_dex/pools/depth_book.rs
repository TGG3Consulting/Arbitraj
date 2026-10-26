//! Файл: venues/bsc_dex/pools/depth_book.rs
//! Что делает: глубина пулов по направлениям (решение 73) — в памяти, на текущем состоянии пула, с гистерезисом:
//! - глубина покупки и продажи в полосе dex.depth_band_pct (quote/depth_band.rs) пересчитывается, когда сменилось
//!   состояние пула (блок и logIndex последнего учтённого события) или настройки глубины; иначе — из памяти;
//! - направление «глубокое», если его глубина не ниже dex.min_pool_liquidity_usd; глубокое становится мелким, только
//!   когда глубина упала ниже порога × (1 − dex.depth_hysteresis_pct / 100) — пул не мигает на границе;
//! - смена «глубокий ↔ мелкий» по направлению — запись в журнал, по пулу не чаще dex.depth_log_min_s; смена внутри
//!   окна не теряется, а откладывается до его конца (pools/depth_log.rs); настройка необязательна — не задана: глубина
//!   считается, записей о смене нет;
//! - направление стало глубоким — сигнал «пул готов» подаётся здесь же (on_rise): кто бы первым ни посчитал глубину
//!   на новом состоянии (отбор пула, экран «Монеты», проверки), сигнал не теряется и подаётся один раз;
//! - курс промежуточной монеты (пул с BNB, решение 52) — часть ключа: сменился пул «промежуточная — USDT» —
//!   глубина пересчитывается.
//!
//! Кто вызывает: quote/select.rs (deep_side), pools/listen_log.rs (emit — пересчёт на каждой смене состояния),
//! pools/depth_facts.rs (экран «Монеты»).
//! От чего зависит: quote/depth_band.rs, depth_key.rs, depth_log.rs, registry.rs, tokens.rs, ctx.rs.
//! Настройки: dex.depth_band_pct, dex.depth_hysteresis_pct, dex.depth_log_min_s, dex.min_pool_liquidity_usd [bsc_dex].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use rust_decimal::Decimal;
use tokio::sync::Notify;

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::quote::depth_band::{self, Band};
use super::super::tokens::Tokens;
use super::depth_log::{self, Said};
use super::registry::{Entry, Registry};

/// Глубина пула по направлениям и вердикт.
#[derive(Debug, Clone, PartialEq)]
pub struct Depth {
    pub band: Band,
    /// Полоса, % — на момент расчёта.
    pub band_pct: Decimal,
    /// Порог dex.min_pool_liquidity_usd — на момент расчёта.
    pub min: Decimal,
    /// Гистерезис, % — на момент расчёта.
    pub hyst: Decimal,
    pub deep_buy: bool,
    pub deep_sell: bool,
}

pub(super) struct Slot {
    pub(super) depth: Depth,
    key: super::depth_key::Key,
    /// Журнал смен (depth_log.rs).
    pub(super) said: Said,
    /// Для текстов журнала: вид, тикер, адрес пула.
    pub(super) label: String,
}

/// Глубина всех пулов площадки. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Book {
    pub(super) m: Arc<Mutex<HashMap<PoolRef, Slot>>>,
    wake: Arc<Mutex<Vec<Weak<Notify>>>>,
}

/// Какая настройка глубины не задана (её имя из ошибки хранилища — GatewayError.setting, не текст): для перепроверки
/// ждущих итогов именно при её задании (settings_wait.rs). None — все заданы. Ошибок нет.
pub fn missing_setting(ctx: &Ctx) -> Option<String> {
    [
        "dex.min_pool_liquidity_usd",
        "dex.depth_band_pct",
        "dex.depth_hysteresis_pct",
    ]
    .into_iter()
    .find_map(|n| ctx.dec(n).err().and_then(|e| e.setting))
}

/// Гистерезис (см. шапку): `prev` — прошлый вердикт (None — первого не было). Возвращает: глубокое ли направление.
/// Ошибок нет.
pub fn next_deep(prev: Option<bool>, depth: Decimal, min: Decimal, hyst_pct: Decimal) -> bool {
    match prev {
        Some(true) => depth >= min * (Decimal::ONE - hyst_pct / Decimal::ONE_HUNDRED),
        _ => depth >= min,
    }
}

impl Book {
    /// Кого будить, когда направление пула стало глубоким (сигнал «пул готов» подписки на рынок). Ошибок нет.
    pub fn on_rise(&self, n: Arc<Notify>) {
        let mut w = self.wake.lock().unwrap_or_else(|p| p.into_inner());
        w.retain(|x| x.strong_count() > 0);
        w.push(Arc::downgrade(&n));
    }

    /// Последняя посчитанная глубина пула (без пересчёта). Ошибок нет.
    pub fn last(&self, r: &PoolRef) -> Option<Depth> {
        self.lock().get(r).map(|s| s.depth.clone())
    }

    /// Глубина пула на его текущем состоянии (см. шапку). Принимает: окружение, реестр, токены, запись, знаки USDT.
    /// Возвращает: глубину. Ошибки (текст): не задана настройка глубины, значение вне допустимого, глубина не
    /// считается (quote/depth_band.rs).
    pub fn depth(
        &self,
        ctx: &Ctx,
        reg: &Registry,
        tokens: &Tokens,
        e: &Entry,
        usdt_dec: u8,
    ) -> Result<Depth, String> {
        let read = |n: &str| ctx.dec(n).map_err(|e| e.message);
        let (band_pct, min) = (
            read("dex.depth_band_pct")?,
            read("dex.min_pool_liquidity_usd")?,
        );
        let hyst = read("dex.depth_hysteresis_pct")?;
        if hyst.is_sign_negative() || hyst >= Decimal::ONE_HUNDRED {
            return Err(format!(
                "dex.depth_hysteresis_pct = {hyst}: нужно от 0 и меньше 100"
            ));
        }
        let key = super::depth_key::Key::new(ctx, reg, tokens, e, band_pct, min, hyst);
        let r = &e.pool.reference;
        let prev = match self.lock().get(r) {
            Some(s) if s.key == key => return Ok(s.depth.clone()),
            Some(s) => Some((s.depth.deep_buy, s.depth.deep_sell)),
            None => None,
        };
        let band = depth_band::band_depth(ctx, reg, tokens, e, usdt_dec, band_pct)?;
        let deep_buy = next_deep(prev.map(|p| p.0), band.buy, min, hyst);
        let deep_sell = next_deep(prev.map(|p| p.1), band.sell, min, hyst);
        let depth = Depth {
            band,
            band_pct,
            min,
            hyst,
            deep_buy,
            deep_sell,
        };
        let rose = prev.is_some_and(|p| (!p.0 && deep_buy) || (!p.1 && deep_sell));
        {
            let mut m = self.lock();
            let said = m.remove(r).map(|s| s.said).unwrap_or_default();
            let label = format!(
                "{} {} ({})",
                e.family.title,
                e.ticker,
                label(&e.pool.reference)
            );
            m.insert(
                r.clone(),
                Slot {
                    depth: depth.clone(),
                    key,
                    said,
                    label,
                },
            );
        }
        depth_log::after(self, ctx, r);
        if rose {
            self.rise();
        }
        Ok(depth)
    }

    /// Направление стало глубоким — «пул готов» всем, кто ждёт. Ошибок нет.
    fn rise(&self) {
        let w = self.wake.lock().unwrap_or_else(|p| p.into_inner());
        for n in w.iter().filter_map(Weak::upgrade) {
            n.notify_waiters();
        }
    }

    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PoolRef, Slot>> {
        self.m.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Адрес пула или PoolId (V4) — для текстов. Ошибок нет.
pub fn label(r: &PoolRef) -> String {
    match r {
        PoolRef::Contract(a) => a.to_string(),
        PoolRef::Singleton { id, .. } => id.to_string(),
    }
}
