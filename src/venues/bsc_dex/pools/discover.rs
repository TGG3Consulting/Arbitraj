//! Файл: venues/bsc_dex/pools/discover.rs
//! Что делает: поиск пулов «монета — встречный токен» (USDT или промежуточная монета, решение 52) по всем включённым
//! видам пулов. V2 — getPair фабрики по каждой монете;
//! V3 и V4 — история событий создания (PoolCreated, Initialize) с USDT в индексированном поле. Прочитанная
//! история хранится в памяти: следующий поиск читает только новые блоки.
//! Кто вызывает: pools/market.rs.
//! От чего зависит: ctx.rs, pools/discover_v2.rs, discover_v3.rs, discover_v4.rs, families.rs, key.rs, hooks.rs.
//! Настройки: dex.enabled [DEX], dex.pool_enabled [вид пула], dex.scan_from_block [вид пула], адреса dex.* [вид пула].
//!
//! Поиск по событиям создания — research/flow/bsc_dex.md, раздел 2 и схема п. 2; research/flow/bsc_v4.md, раздел 3
//! («единственный источник списка пулов V4 — логи Initialize»).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use alloy_primitives::Address;

use crate::core::types::{HookInfo, PoolRef};
use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::families::{FAMILIES, Family, Proto};
use super::key::PoolKey;
use super::{discover_v2, discover_v3, discover_v4};

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

/// История вида пула: последний прочитанный блок и найденные пулы.
type History = (u64, Vec<Found>);

/// Прочитанная история событий по виду пула и встречному токену (USDT или промежуточная монета, решение 52):
/// до какого блока и что найдено (все пулы с этим токеном, любые монеты).
#[derive(Clone, Default)]
pub struct Scanned {
    inner: Arc<Mutex<HashMap<(&'static str, Address), History>>>,
}

impl Scanned {
    /// Найденное и последний прочитанный блок вида пула со встречным токеном. None — ещё не читали.
    fn get(&self, family: &'static str, quote: Address) -> Option<(u64, Vec<Found>)> {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&(family, quote))
            .cloned()
    }

    /// Дописывает новые пулы и сдвигает прочитанный блок.
    fn extend(&self, family: &'static str, quote: Address, to: u64, mut found: Vec<Found>) {
        let mut m = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let e = m.entry((family, quote)).or_default();
        e.0 = to;
        e.1.append(&mut found);
    }
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

/// Пулы V3/V4 из истории событий: дочитывает новые блоки до head.
/// Ошибки: NotConfigured (dex.scan_from_block, адрес фабрики или менеджера), узел.
async fn from_history(
    ctx: &Ctx,
    f: &'static Family,
    usdt: Address,
    head: u64,
    scanned: &Scanned,
) -> GatewayResult<Vec<Found>> {
    let from = match scanned.get(f.id, usdt) {
        Some((to, _)) => to + 1,
        None => ctx.dex_int("dex.scan_from_block", f.id)?,
    };
    if from <= head {
        let found = match f.proto {
            Proto::PancakeV3 | Proto::UniswapV3 => {
                discover_v3::scan(ctx, f, usdt, from, head).await?
            }
            _ => discover_v4::scan(ctx, f, usdt, from, head).await?,
        };
        scanned.extend(f.id, usdt, head, found);
    }
    Ok(scanned.get(f.id, usdt).map(|(_, v)| v).unwrap_or_default())
}

/// Пара токенов, отсортированная по адресу (currency0 < currency1 — так во всех видах пулов). Ошибок нет.
pub fn sorted(a: Address, b: Address) -> (Address, Address) {
    if a < b { (a, b) } else { (b, a) }
}
