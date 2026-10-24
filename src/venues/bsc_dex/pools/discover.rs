//! Файл: venues/bsc_dex/pools/discover.rs
//! Что делает: поиск пулов «монета — встречный токен» (USDT или промежуточная монета, решение 52) по всем включённым
//! видам пулов. V2 — getPair фабрики, V3 — getPool фабрики по уровням комиссии (без истории событий);
//! V4 — из истории событий Initialize: её загружает в фоне history_v4.rs (SQLite, затем от сохранённого блока или от
//! блока развёртывания менеджера), а здесь, когда история догнала сеть, дочитываются только новые блоки.
//! Кто вызывает: pools/market.rs, market_mids.rs, created.rs.
//! От чего зависит: ctx.rs, pools/discover_v2.rs, discover_v3.rs, discover_v4.rs, history_store.rs, families.rs, key.rs, hooks.rs.
//! Настройки: dex.enabled [DEX], dex.pool_enabled [вид пула], dex.pool_manager [вид пула V4], адреса dex.* [вид пула].
//!
//! V2/V3 — research/flow/bsc_dex.md, раздел 2 и схема п. 2 (getPair, getPool); V4 — research/flow/bsc_v4.md,
//! раздел 3 («единственный источник списка пулов V4 — логи Initialize»).

use std::collections::HashMap;
use std::sync::Arc;

use alloy_primitives::Address;

use crate::core::types::{HookInfo, PoolRef};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::families::{FAMILIES, Family, Proto};
use super::key::PoolKey;
use super::{discover_v2, discover_v3, discover_v4, history_store};

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

/// Включён ли вид пула: dex.enabled [DEX] и dex.pool_enabled [вид]. Не задано — выключен (с записью в журнал).
pub async fn enabled(ctx: &Ctx, f: &Family) -> bool {
    match (
        ctx.dex_flag("dex.enabled", f.dex),
        ctx.dex_flag("dex.pool_enabled", f.id),
    ) {
        (Ok(a), Ok(b)) => a && b,
        (Err(e), _) | (_, Err(e)) => {
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

/// Все пулы монет со встречным токеном по включённым видам.
/// Принимает: монеты (адрес → тикер), встречный токен (USDT или промежуточная монета), текущий блок, память истории,
/// v4 — брать ли пулы V4 из истории (нет — при подписке на рынок: V4 догружается в фоне, history_v4.rs).
/// Возвращает: найденные пулы. Ошибка вида пула (не задан адрес, узел) пишется в журнал, вид пропускается;
/// остальные виды ищутся дальше — так видно, что именно не настроено.
pub async fn find_all(
    ctx: &Ctx,
    coins: &HashMap<Address, String>,
    usdt: Address,
    head: u64,
    scanned: &Scanned,
    v4: bool,
) -> Vec<Found> {
    let mut out = Vec::new();
    for f in FAMILIES {
        if !v4 && is_v4(f) {
            continue;
        }
        if !enabled(ctx, f).await {
            continue;
        }
        let res = match f.proto {
            Proto::V2 => discover_v2::find(ctx, f, coins, usdt, head).await,
            Proto::PancakeV3 | Proto::UniswapV3 => {
                discover_v3::find(ctx, f, coins, usdt, head).await
            }
            _ => from_history(ctx, f, usdt, head, scanned).await.map(|all| {
                all.into_iter()
                    .filter(|p| coins.contains_key(&p.coin))
                    .collect()
            }),
        };
        match res {
            Ok(mut found) => out.append(&mut found),
            Err(e) => {
                ctx.log(
                    Level::Error,
                    "dex.scan",
                    format!("{}: поиск пулов: {}", f.title, e.message),
                    None,
                )
                .await;
            }
        }
    }
    out
}

/// Вид пула V4 (пулы — только из истории Initialize). Ошибок нет.
pub fn is_v4(f: &Family) -> bool {
    matches!(
        f.proto,
        Proto::InfinityCl | Proto::InfinityBin | Proto::UniswapV4
    )
}

/// Пулы V4 со встречным токеном из памяти истории. Если история уже догнала сеть — сначала дочитывает новые блоки
/// до head (и сохраняет их в SQLite); пока не догнала — только найденное фоновой загрузкой (она и читает дальше).
/// Ошибки: NotConfigured (dex.pool_manager; адрес сменился — история читается заново с новой подписки на рынок),
/// узел (лимиты eth_getLogs).
async fn from_history(
    ctx: &Ctx,
    f: &'static Family,
    quote: Address,
    head: u64,
    scanned: &Scanned,
) -> GatewayResult<Vec<Found>> {
    let manager = ctx.dex_addr("dex.pool_manager", f.id)?;
    let mut m = scanned.inner.lock().await;
    let Some(h) = m.get_mut(f.id) else {
        return Ok(Vec::new());
    };
    if h.manager != manager {
        let msg = format!(
            "адрес dex.pool_manager сменился ({} → {manager}) — история пулов читается заново с новой подписки на рынок",
            h.manager
        );
        return Err(ctx.err(ErrorCode::NotConfigured, msg));
    }
    if h.caught_up && h.last < head {
        let logs = discover_v4::logs(ctx, f, manager, h.last + 1, head).await?;
        let mut found = discover_v4::parse_all(ctx, f, &logs)?;
        if let Err(e) = history_store::save(ctx, f, manager, &logs, head).await {
            ctx.log(Level::Error, "dex.scan", e.message, None).await;
        }
        h.pools.append(&mut found);
        h.last = head;
    }
    Ok(h.pools.iter().filter_map(|p| against(p, quote)).collect())
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
