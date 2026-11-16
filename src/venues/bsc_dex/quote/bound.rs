//! Файл: venues/bsc_dex/quote/bound.rs
//! Что делает: решение 361 — строгий верх курса стороны DEX по монете без котировок (контракт rate_bound,
//! gateway/rate_bound.rs). Верх цикла DEX ↔ DEX (решение 362) — quote/bound_cycle.rs. Доказательство — заметка
//! docs/notes/t162-proven-sieve-2026-11-17.md, раздел 2 (лемма 3 проекта: потенциалы по слоям хопов).
//! Набор пулов — ВСЕ пулы реестра пар из {монета, USDT, активные промежуточные} (как signal_epoch.rs state — шире, чем
//! допуск котировки: недопущенный пул только ослабляет верх). Предел хопов H: классика (direct / smart) — 2 (пул монеты с
//! USDT или пара пулов через промежуточную, quote/via_pools.rs); dex.router_mode = graph — max(2, dex.route_max_hops).
//! Верх ρ — наибольшее произведение r_e (bound_rate.rs) по путям ≤ H хопов (динамика по слоям, повторный заход в
//! промежуточную разрешён — путей только больше). Строго, потому что монета и USDT в путях не внутренние (route_paths.rs:
//! без повторного захода, внутренние — промежуточные) ⇒ пулы монеты и пулы с USDT идут только в одну сторону, а обмены
//! одного направления в одном пуле только ухудшают его цену (каждый ≤ r_e·вход). Метод НЕ действует (NotApplicable с
//! классом причины): нет адреса монеты; налоги монеты не измерены или < 0 (налог < 0 даёт keep > 1); H ≥ 3 и есть пул
//! между двумя промежуточными (встречные доли — строгой границы в коде нет); любой пул набора без строгого r_e (вид без
//! записи-эталона, крючок); decimals не известны; голова узла не свежая или лента событий отстаёт; пути нет.
//! Пересчёт — только при изменении входов: ключ — поколение настроек, H, промежуточные, налоги монеты и по каждому пулу
//! набора (ссылка, поколение состояния, достоверность); тот же ключ — итог из памяти.
//! Кто вызывает: venue.rs (rate_bound), тесты (rho_raw).
//! От чего зависит: quote/bound_rate.rs, parts.rs (реестр, промежуточные, проверки, decimals, голова, лента), core/types,
//! gateway/rate_bound.rs.
//! Настройки: dex.router_mode, dex.route_max_hops [bsc_dex]; через bound_rate — dex.v2_fee_pct [вид пула V2].

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{Asset, CheckState, PoolRef, Side};
use crate::gateway::rate_bound::{DexBound, RateBound};

use super::super::ctx::Ctx;
use super::super::parts::Parts;
use super::super::pools::registry::Entry;
use super::bound_rate::{ceil_dec, mul_up, pow10, rate};

/// Итоги по (монета, сторона) с ключом входов.
type Memo = HashMap<(Address, bool), (String, RateBound)>;

/// Память итогов (одна на процесс: реестр пулов у адаптера один, вид на кошелёк делит его — venue_account.rs).
static MEMO: LazyLock<Mutex<Memo>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Что делает: верх курса стороны DEX (см. шапку). Принимает: части адаптера, монету, сторону. Возвращает: RateBound.
/// Ошибок нет — любая невозможность → NotApplicable с причиной.
pub fn rate_bound(p: &Parts, asset: &Asset, side: Side) -> RateBound {
    let ctx = &p.ctx;
    let Some(coin) = asset.bsc_contract else {
        return RateBound::na("no_data", "нет адреса контракта монеты");
    };
    if p.heads.fresh(ctx).is_err() || p.lag.check(ctx).is_err() {
        return RateBound::na("stale", "голова узла не свежая или лента событий отстаёт");
    }
    let Ok(usdt) = p.tokens.usdt(ctx) else {
        return RateBound::na("no_data", "адрес USDT не задан");
    };
    if let Err(why) = coin_taxes_ok(p, coin) {
        return RateBound::na("taxes", why);
    }
    let hops = match hop_limit(ctx) {
        Ok(h) => h,
        Err(why) => return RateBound::na("no_data", why),
    };
    let mids: Vec<Address> = p.mids.active(ctx).iter().map(|m| m.address).collect();
    let pools = pool_set(p, coin, usdt, &mids, hops);
    // Запись вне реестра (поколение 0, signal_epoch.rs) — входы не отследить: метод не действует.
    if pools.iter().any(|e| e.state_gen == 0) {
        return RateBound::na("no_data", "пул набора вне реестра (поколение состояния 0)");
    }
    let key = key_of(
        ctx,
        hops,
        &mids,
        &pools,
        &format!("{:?}", p.checks.coin(&coin).map(|c| c.taxes)),
    );
    let sell = side == Side::Sell;
    if let Some((k, r)) = memo().get(&(coin, sell))
        && *k == key
    {
        return r.clone();
    }
    let r = compute(p, coin, usdt, &mids, &pools, hops, sell);
    memo().insert((coin, sell), (key, r.clone()));
    r
}

/// Память итогов под замком (отравленный замок — данные как есть). Ошибок нет.
fn memo() -> std::sync::MutexGuard<'static, Memo> {
    MEMO.lock().unwrap_or_else(|e| e.into_inner())
}

/// Расчёт верха по набору пулов: ρ в минимальных единицах → в целых монетах (decimals). Ошибок нет.
fn compute(
    p: &Parts,
    coin: Address,
    usdt: Address,
    mids: &[Address],
    pools: &[Entry],
    hops: usize,
    sell: bool,
) -> RateBound {
    let (Some(dc), Some(du)) = (p.tokens.decimals_known(coin), p.tokens.decimals_known(usdt))
    else {
        return RateBound::na("no_data", "decimals монеты или USDT ещё не известны");
    };
    let (from, to) = if sell { (coin, usdt) } else { (usdt, coin) };
    let raw = match rho_raw(&p.ctx, pools, from, to, mids, hops) {
        Ok(r) => r,
        Err((class, why)) => return RateBound::na(class, why),
    };
    // Продажа: USDT-ед. за ед. монеты × 10^(dc − du) = USDT за монету; покупка — наоборот.
    let k = if sell {
        i32::from(dc) - i32::from(du)
    } else {
        i32::from(du) - i32::from(dc)
    };
    match pow10(k).and_then(|m| mul_up(raw, m)) {
        Some(rho) => RateBound::Bound(DexBound {
            rho,
            pools: pools.len(),
            hops,
        }),
        None => RateBound::na("overflow", "верх курса не помещается в число"),
    }
}

/// Что делает: верх отдачи `to` за единицу `from` (минимальные единицы) по путям ≤ hops хопов через пулы `pools`
/// (динамика по слоям, Якоби). Внутренние узлы — промежуточные; рёбра в `from` и из `to` не берутся.
/// Принимает: окружение, пулы, откуда, куда, промежуточные, предел хопов. Возвращает: верх.
/// Ошибки: (класс, причина) — пул без строгого r_e (вид, крючок), пул между промежуточными при hops ≥ 3, пути нет.
pub fn rho_raw(
    ctx: &Ctx,
    pools: &[Entry],
    from: Address,
    to: Address,
    mids: &[Address],
    hops: usize,
) -> Result<Decimal, (&'static str, String)> {
    let mid: HashSet<Address> = mids.iter().copied().collect();
    let mut edges: Vec<(Address, Address, Decimal)> = Vec::new();
    for e in pools {
        let (a, b) = (e.pool.token0, e.pool.token1);
        if hops >= 3 && mid.contains(&a) && mid.contains(&b) {
            return Err((
                "mid_pool",
                format!(
                    "{}: пул между промежуточными при маршруте ≥ 3 хопов",
                    e.family.title
                ),
            ));
        }
        for (i, o) in [(a, b), (b, a)] {
            if i == to || o == from {
                continue;
            }
            let r = rate(ctx, e, i).map_err(|w| ("pool_kind", w))?;
            let r = ceil_dec(r)
                .ok_or_else(|| ("overflow", "верх пула не помещается в число".to_string()))?;
            edges.push((i, o, r));
        }
    }
    let mut v: HashMap<Address, Decimal> = HashMap::from([(to, Decimal::ONE)]);
    for _ in 0..hops {
        let mut next = v.clone();
        for (i, o, r) in &edges {
            let Some(w) = v.get(o) else { continue };
            let Some(c) = mul_up(*r, *w) else {
                return Err(("overflow", "верх пути не помещается в число".into()));
            };
            let cur = next.entry(*i).or_insert(c);
            if c > *cur {
                *cur = c;
            }
        }
        v = next;
    }
    v.get(&from)
        .copied()
        .ok_or_else(|| ("no_route", "пути по пулам набора нет".into()))
}

/// Налоги монеты из проверки известны и не отрицательны (налог < 0 — keep > 1, верх не строгий). Err — причина.
fn coin_taxes_ok(p: &Parts, coin: Address) -> Result<(), String> {
    match p.checks.coin(&coin).map(|c| c.taxes) {
        Some(CheckState::Known(t)) => {
            if t.buy_pct.is_sign_negative()
                || t.sell_pct.is_sign_negative()
                || t.transfer_pct.is_sign_negative()
            {
                return Err("налог монеты отрицательный".into());
            }
            Ok(())
        }
        _ => Err("налоги монеты не измерены".into()),
    }
}

/// Предел хопов маршрута: классика — 2; режим graph — max(2, dex.route_max_hops). Err — режим graph без предела.
fn hop_limit(ctx: &Ctx) -> Result<usize, String> {
    if ctx.text("dex.router_mode").ok().as_deref() != Some("graph") {
        return Ok(2);
    }
    let h = ctx.int("dex.route_max_hops").map_err(|e| e.message)?;
    Ok(usize::try_from(h).unwrap_or(usize::MAX).max(2))
}

/// Пулы набора: все пары из {монета, USDT, промежуточные} (обе ориентации реестра), без повторов; при H ≤ 2 пулы между
/// двумя промежуточными не берутся (маршрут их не проходит). Ошибок нет.
fn pool_set(p: &Parts, coin: Address, usdt: Address, mids: &[Address], hops: usize) -> Vec<Entry> {
    let reg = &p.registry;
    let mut tokens = vec![coin, usdt];
    tokens.extend(mids.iter().copied());
    let mut seen: HashSet<PoolRef> = HashSet::new();
    let mut out = Vec::new();
    for (i, a) in tokens.iter().enumerate() {
        for b in &tokens[i + 1..] {
            if hops < 3 && i >= 2 {
                continue;
            }
            for e in reg.by_pair(*a, *b).into_iter().chain(reg.by_pair(*b, *a)) {
                if seen.insert(e.pool.reference.clone()) {
                    out.push(e);
                }
            }
        }
    }
    out
}

/// Ключ входов итога (см. шапку). Ошибок нет.
fn key_of(ctx: &Ctx, hops: usize, mids: &[Address], pools: &[Entry], taxes: &str) -> String {
    let mut parts: Vec<String> = pools
        .iter()
        .map(|e| format!("{:?}:{}:{}", e.pool.reference, e.state_gen, e.valid))
        .collect();
    parts.sort();
    format!(
        "{}|{hops}|{mids:?}|{taxes}|{}",
        ctx.settings.generation(),
        parts.join(",")
    )
}
