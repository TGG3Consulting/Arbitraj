//! Файл: venues/bsc_dex/pools/depth_book.rs
//! Что делает: глубина пулов по направлениям (решение 73) — в памяти, на текущем состоянии пула, с гистерезисом:
//! - глубина покупки и продажи в полосе dex.depth_band_pct (quote/depth_band.rs) пересчитывается, когда сменилось
//!   состояние пула (блок и logIndex последнего учтённого события) или настройки глубины; иначе — из памяти;
//! - направление «глубокое», если его глубина не ниже dex.min_pool_liquidity_usd; глубокое становится мелким, только
//!   когда глубина упала ниже порога × (1 − dex.depth_hysteresis_pct / 100) — пул не мигает на границе;
//! - смена «глубокий ↔ мелкий» по направлению — запись в журнал, по пулу не чаще dex.depth_log_min_s (необязательна:
//!   не задана — глубина считается, записей о смене нет).
//!
//! Кто вызывает: quote/select.rs (deep_side), pools/listen_log.rs (emit — пересчёт на каждой смене состояния),
//! pools/depth_facts.rs (экран «Монеты»).
//! От чего зависит: quote/depth_band.rs, registry.rs, tokens.rs, ctx.rs.
//! Настройки: dex.depth_band_pct, dex.depth_hysteresis_pct, dex.depth_log_min_s, dex.min_pool_liquidity_usd [bsc_dex].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rust_decimal::Decimal;

use crate::core::types::PoolRef;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::quote::depth_band::{self, Band};
use super::super::tokens::Tokens;
use super::registry::{Entry, Registry};

/// Глубина пула по направлениям и вердикт.
#[derive(Debug, Clone, PartialEq)]
pub struct Depth {
    pub band: Band,
    /// Полоса, % — на момент расчёта.
    pub band_pct: Decimal,
    /// Порог dex.min_pool_liquidity_usd — на момент расчёта.
    pub min: Decimal,
    pub deep_buy: bool,
    pub deep_sell: bool,
}

/// Что посчитано и на чём: состояние пула (блок, logIndex) и настройки (полоса, порог, гистерезис).
type Key = (u64, Option<u64>, Decimal, Decimal, Decimal);

struct Slot {
    depth: Depth,
    key: Key,
    /// Когда по пулу последний раз писали о смене.
    logged: Option<Instant>,
}

/// Глубина всех пулов площадки. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Book {
    m: Arc<Mutex<HashMap<PoolRef, Slot>>>,
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
    /// Последняя посчитанная глубина пула (без пересчёта). Ошибок нет.
    pub fn last(&self, r: &PoolRef) -> Option<Depth> {
        self.lock().get(r).map(|s| s.depth.clone())
    }

    /// Глубина пула на его текущем состоянии (см. шапку). Принимает: окружение, реестр, токены, запись, знаки USDT.
    /// Возвращает: (глубина, стало ли глубоким хоть одно направление — повод разбудить проверки). Ошибки (текст): не
    /// задана настройка глубины, гистерезис вне 0–100%, глубина не считается (quote/depth_band.rs).
    pub fn depth(
        &self,
        ctx: &Ctx,
        reg: &Registry,
        tokens: &Tokens,
        e: &Entry,
        usdt_dec: u8,
    ) -> Result<(Depth, bool), String> {
        let read = |n: &str| ctx.dec(n).map_err(|e| e.message);
        let (band_pct, min) = (
            read("dex.depth_band_pct")?,
            read("dex.min_pool_liquidity_usd")?,
        );
        let hyst = read("dex.depth_hysteresis_pct")?;
        if hyst.is_sign_negative() || hyst >= Decimal::ONE_HUNDRED {
            return Err(format!(
                "dex.depth_hysteresis_pct = {hyst}: нужно от 0 до 100"
            ));
        }
        // Необязательна (решение 73): не задана — записи о смене выключены.
        let log_min = ctx.int("dex.depth_log_min_s").ok();
        let key = (e.synced_block, e.synced_log, band_pct, min, hyst);
        let r = &e.pool.reference;
        let prev = match self.lock().get(r) {
            Some(s) if s.key == key => return Ok((s.depth.clone(), false)),
            Some(s) => Some((s.depth.deep_buy, s.depth.deep_sell, s.logged)),
            None => None,
        };
        let band = depth_band::band_depth(ctx, reg, tokens, e, usdt_dec, band_pct)?;
        let deep_buy = next_deep(prev.map(|p| p.0), band.buy, min, hyst);
        let deep_sell = next_deep(prev.map(|p| p.1), band.sell, min, hyst);
        let depth = Depth {
            band,
            band_pct,
            min,
            deep_buy,
            deep_sell,
        };
        let changed = prev.is_some_and(|p| (p.0, p.1) != (deep_buy, deep_sell));
        let rose = prev.is_some_and(|p| (!p.0 && deep_buy) || (!p.1 && deep_sell));
        let mut logged = prev.and_then(|p| p.2);
        let due = |min: u64| logged.is_none_or(|t| t.elapsed().as_secs() >= min);
        if changed && log_min.is_some_and(due) {
            logged = Some(Instant::now());
            journal(ctx, e, &depth, hyst);
        }
        let slot = Slot {
            depth: depth.clone(),
            key,
            logged,
        };
        self.lock().insert(r.clone(), slot);
        Ok((depth, rose))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PoolRef, Slot>> {
        self.m.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Запись в журнал о смене «глубокий ↔ мелкий» (отдельной задачей: вызывающий синхронный). Вне среды tokio (тесты
/// без неё) — без записи. Ошибок нет.
fn journal(ctx: &Ctx, e: &Entry, d: &Depth, hyst: Decimal) {
    let word = |deep: bool| {
        if deep {
            "глубокий"
        } else {
            "мелкий"
        }
    };
    let msg = format!(
        "{} {} ({}): покупка — {} ({} USDT), продажа — {} ({} USDT) в полосе ±{}% (порог dex.min_pool_liquidity_usd {}, \
         вниз — ниже порога на dex.depth_hysteresis_pct {}%){}",
        e.family.title,
        e.ticker,
        label(&e.pool.reference),
        word(d.deep_buy),
        d.band.buy.round_dp(2),
        word(d.deep_sell),
        d.band.sell.round_dp(2),
        d.band_pct,
        d.min,
        hyst,
        if d.band.full {
            ""
        } else {
            "; полоса прочитана не целиком — глубина до края прочитанной карты"
        }
    );
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        let ctx = ctx.clone();
        rt.spawn(async move { ctx.log(Level::Info, "dex.pool", msg, None).await });
    }
}

/// Адрес пула или PoolId (V4) — для текстов. Ошибок нет.
pub fn label(r: &PoolRef) -> String {
    match r {
        PoolRef::Contract(a) => a.to_string(),
        PoolRef::Singleton { id, .. } => id.to_string(),
    }
}
