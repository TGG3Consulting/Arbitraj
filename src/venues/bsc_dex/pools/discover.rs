//! Файл: venues/bsc_dex/pools/discover.rs
//! Что делает: пулы «монета — встречный токен» (USDT или промежуточная монета, решение 52) — из справочника
//! индексатора, проверенные на своём узле (решение 58, pools/index.rs; перебор фабрик по уровням комиссии убран).
//! Найденный пул (Found), выбор встречного токена (against), включена ли DEX, память истории V4 (необязательной:
//! history_v4.rs, dex.history_scan — для пулов Uniswap V4 без позиций через PositionManager).
//! Кто вызывает: pools/market.rs, market_mids.rs, index.rs, verify*.rs, created.rs, history_v4.rs.
//! От чего зависит: ctx.rs, pools/index.rs, families.rs, key.rs.
//! Настройки: dex.pool_enabled [DEX].

use std::collections::HashMap;
use std::sync::Arc;

use alloy_primitives::Address;

use crate::core::types::{HookInfo, PoolRef};
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::families::{Family, Proto};
use super::key::PoolKey;

/// Найденный пул: всё, что известно до чтения состояния.
#[derive(Debug, Clone)]
pub struct Found {
    pub family: &'static Family,
    pub reference: PoolRef,
    pub token0: Address,
    pub token1: Address,
    /// Монета пула (не встречный токен: не USDT и не промежуточная, с которой искали).
    pub coin: Address,
    /// V3 — fee из события; V4 — PoolKey.fee (0x800000 — динамическая); V2 — None.
    pub fee_pips: Option<u32>,
    pub tick_spacing: Option<i32>,
    pub bin_step: Option<u32>,
    pub key: Option<PoolKey>,
    pub hooks: Option<HookInfo>,
}

/// История вида пула V4 в памяти: менеджер, последний прочитанный блок, все найденные пулы (любые валюты),
/// догнала ли история сеть (до этого её ведёт только фоновая загрузка history_v4.rs).
pub struct History {
    pub manager: Address,
    pub last: u64,
    pub pools: Vec<Found>,
    pub caught_up: bool,
}

/// Истории видов пулов V4. Замок держится на время дочитывания куска: одновременные поиски (USDT, промежуточные
/// монеты, новые пулы, фоновая загрузка) не читают одни и те же блоки дважды.
#[derive(Clone, Default)]
pub struct Scanned {
    pub inner: Arc<tokio::sync::Mutex<HashMap<&'static str, History>>>,
}

/// Включена ли DEX: dex.pool_enabled [DEX]. Не задано — выключена (с записью в журнал).
pub async fn enabled(ctx: &Ctx, f: &Family) -> bool {
    match ctx.dex_flag("dex.pool_enabled", f.id) {
        Ok(v) => v,
        Err(e) => {
            ctx.log(
                Level::Warn,
                "dex.config",
                format!("{}: не слушается — {}", f.title, e.message),
                None,
            )
            .await;
            false
        }
    }
}

/// Пулы монет со встречным токеном (USDT или промежуточная монета) — из справочника индексатора, уже проверенные на
/// своём узле (pools/index.rs, решение 58). Принимает: монеты (адрес → тикер), встречный токен.
/// Возвращает: найденные пулы. Ошибок нет: справочника нет или он не задан — пусто (причина — в журнале и на экране).
pub fn find_all(ctx: &Ctx, coins: &HashMap<Address, String>, quote: Address) -> Vec<Found> {
    super::index::taken_against(ctx, coins, quote)
}

/// Вид пула V4 (менеджер-синглтон, ключ PoolKey). Ошибок нет.
pub fn is_v4(f: &Family) -> bool {
    matches!(
        f.proto,
        Proto::InfinityCl | Proto::InfinityBin | Proto::UniswapV4
    )
}

/// Пул со встречным токеном: coin — другая валюта пула. None — токена в пуле нет. Ошибок нет.
pub fn against(p: &Found, quote: Address) -> Option<Found> {
    let coin = if p.token0 == quote {
        p.token1
    } else if p.token1 == quote {
        p.token0
    } else {
        return None;
    };
    Some(Found { coin, ..p.clone() })
}

/// Пара токенов, отсортированная по адресу (currency0 < currency1 — так во всех видах пулов). Ошибок нет.
pub fn sorted(a: Address, b: Address) -> (Address, Address) {
    if a < b { (a, b) } else { (b, a) }
}
