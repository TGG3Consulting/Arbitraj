//! Файл: venues/bsc_dex/pools/discover.rs
//! Что делает: поиск пулов «монета — встречный токен» (USDT или промежуточная монета, решение 52) по всем включённым
//! видам пулов. V2 — getPair фабрики, V3 — getPool фабрики по уровням комиссии (без истории событий);
//! V4 — история событий Initialize менеджера от блока его развёртывания (deploy_block.rs). Прочитанная история V4
//! хранится в памяти: следующий поиск читает только новые блоки.
//! Кто вызывает: pools/market.rs, market_mids.rs, created.rs.
//! От чего зависит: ctx.rs, pools/discover_v2.rs, discover_v3.rs, discover_v4.rs, deploy_block.rs, families.rs, key.rs, hooks.rs.
//! Настройки: dex.enabled [DEX], dex.pool_enabled [вид пула], dex.pool_manager [вид пула V4], адреса dex.* [вид пула].
//!
//! V2/V3 — research/flow/bsc_dex.md, раздел 2 и схема п. 2 (getPair, getPool); V4 — research/flow/bsc_v4.md,
//! раздел 3 («единственный источник списка пулов V4 — логи Initialize»).

use std::collections::HashMap;
use std::sync::Arc;

use alloy_primitives::Address;

use crate::core::types::{HookInfo, PoolRef};
use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::families::{FAMILIES, Family, Proto};
use super::key::PoolKey;
use super::{deploy_block, discover_v2, discover_v3, discover_v4};

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

/// История вида пула V4: адрес менеджера, последний прочитанный блок, все найденные пулы (любые валюты).
type History = (Address, u64, Vec<Found>);

/// Прочитанная история событий Initialize по видам пулов V4. Замок держится на время дочитывания:
/// одновременные поиски (USDT, промежуточные монеты, новые пулы) не читают одни и те же блоки дважды.
#[derive(Clone, Default)]
pub struct Scanned {
    inner: Arc<tokio::sync::Mutex<HashMap<&'static str, History>>>,
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
/// Принимает: монеты (адрес → тикер), встречный токен (USDT или промежуточная монета), текущий блок, память истории.
/// Возвращает: найденные пулы. Ошибка вида пула (не задан адрес, узел) пишется в журнал, вид пропускается;
/// остальные виды ищутся дальше — так видно, что именно не настроено.
pub async fn find_all(
    ctx: &Ctx,
    coins: &HashMap<Address, String>,
    usdt: Address,
    head: u64,
    scanned: &Scanned,
) -> Vec<Found> {
    let mut out = Vec::new();
    for f in FAMILIES {
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

/// Пулы V4 со встречным токеном из истории событий: дочитывает новые блоки до head. Первый раз (или новый адрес
/// менеджера) — от блока развёртывания менеджера (deploy_block.rs).
/// Ошибки: NotConfigured (dex.pool_manager, нет контракта), узел (нет истории состояния, лимиты eth_getLogs).
async fn from_history(
    ctx: &Ctx,
    f: &'static Family,
    quote: Address,
    head: u64,
    scanned: &Scanned,
) -> GatewayResult<Vec<Found>> {
    let manager = ctx.dex_addr("dex.pool_manager", f.id)?;
    let mut m = scanned.inner.lock().await;
    let from = match m.get(f.id) {
        Some((addr, to, _)) if *addr == manager => to + 1,
        _ => {
            m.remove(f.id);
            deploy_block::get(ctx, f, manager).await?
        }
    };
    if from <= head {
        let mut found = discover_v4::scan(ctx, f, manager, from, head).await?;
        let e = m.entry(f.id).or_insert((manager, 0, Vec::new()));
        e.1 = head;
        e.2.append(&mut found);
    }
    let all = m
        .get(f.id)
        .map(|(_, _, v)| v.as_slice())
        .unwrap_or_default();
    Ok(all.iter().filter_map(|p| against(p, quote)).collect())
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
