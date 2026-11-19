//! Файл: venues/bsc_dex/quote/bound.rs
//! Что делает: решение 372 — строгий верх курса стороны DEX по монете без котировок (контракт rate_bound,
//! gateway/rate_bound.rs). Верх цикла DEX ↔ DEX (решение 373) — quote/bound_cycle.rs. Доказательство — заметка
//! docs/notes/t162-proven-sieve-2026-11-17.md, раздел 2 (лемма 3 проекта: потенциалы по слоям хопов).
//! Набор пулов и предел хопов — quote/bound_set.rs (все пулы пар {монета, USDT, активные промежуточные}; классика —
//! via.rs CLASSIC_MAX_HOPS, граф — max(классика, dex.route_max_hops)); верх по путям — quote/bound_graph.rs rho_raw.
//! Метод НЕ действует (NotApplicable с классом причины): нет адреса монеты; голова узла не свежая или лента событий
//! отстаёт; налоги монеты не измерены или < 0 (налог < 0 даёт keep > 1); decimals монеты или USDT ещё не известны (класс
//! «decimals» — ревью t162, Н6: НЕ запоминается, метод включается, как только decimals появятся; пока выключен — счётчик
//! na_decimals в сводке dex.quote_count, с её частотой log.quote_count_s); пул набора вне реестра (поколение 0); H ≥ 3 и
//! есть пул между двумя промежуточными; любой пул набора без строгого r_e (вид без записи-эталона, крючок); пути нет.
//! Пересчёт — только при изменении входов: ключ — поколение настроек, H, промежуточные, налоги монеты и поколения
//! состояния пулов набора (registry pair_marks — без копий записей, ревью t162, Н7: память проверяется ДО сборки набора
//! с копиями); тот же ключ — итог из памяти. Поколение пула растёт при смене состояния, тиков, бинов, достоверности и
//! комиссии; новый или ушедший пул меняет список поколений.
//! Кто вызывает: venue.rs (rate_bound), тесты (bound_now).
//! От чего зависит: quote/bound_set.rs, quote/bound_graph.rs, quote/bound_rate.rs, parts.rs (реестр, промежуточные,
//! проверки, decimals, голова, лента), core/types, gateway/rate_bound.rs.
//! Настройки: dex.router_mode, dex.route_max_hops [bsc_dex]; через bound_rate — dex.v2_fee_pct [вид пула V2].

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use alloy_primitives::Address;

use crate::core::types::{Asset, Side};
use crate::gateway::rate_bound::{DexBound, RateBound};

use super::super::parts::Parts;
pub use super::bound_graph::rho_raw;
use super::bound_rate::{mul_up, pow10};
use super::bound_set::{coin_taxes_ok, gens, hop_limit, pool_set};

/// Ключ входов итога (см. шапку).
#[derive(Debug, Clone, PartialEq)]
struct Key {
    settings: u64,
    hops: usize,
    mids: Vec<Address>,
    taxes: String,
    gens: Vec<u64>,
}

/// Итоги по (монета, сторона) с ключом входов.
type Memo = HashMap<(Address, bool), (Key, RateBound)>;

/// Память итогов (одна на процесс: реестр пулов у адаптера один, вид на кошелёк делит его — venue_account.rs).
static MEMO: LazyLock<Mutex<Memo>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Что делает: верх курса стороны DEX (см. шапку). Принимает: части адаптера, монету, сторону. Возвращает: RateBound.
/// Ошибок нет — любая невозможность → NotApplicable с причиной.
pub fn rate_bound(p: &Parts, asset: &Asset, side: Side) -> RateBound {
    let ctx = &p.ctx;
    if p.heads.fresh(ctx).is_err() || p.lag.check(ctx).is_err() {
        return RateBound::na("stale", "голова узла не свежая или лента событий отстаёт");
    }
    bound_now(p, asset, side)
}

/// Что делает: то же без проверки свежести головы и ленты (её делает rate_bound; тесты на записях — напрямую).
/// Принимает: части адаптера, монету, сторону. Возвращает: RateBound. Ошибок нет.
pub fn bound_now(p: &Parts, asset: &Asset, side: Side) -> RateBound {
    let ctx = &p.ctx;
    let Some(coin) = asset.contract else {
        return RateBound::na("no_data", "нет адреса контракта монеты");
    };
    let Ok(usdt) = p.tokens.usdt(ctx) else {
        return RateBound::na("no_data", "адрес USDT не задан");
    };
    if let Err(why) = coin_taxes_ok(p, coin) {
        return RateBound::na("taxes", why);
    }
    // Ревью t162, Н6: decimals — до памяти и без записи в неё: появятся — метод включится на следующем запросе.
    let (Some(dc), Some(du)) = (p.tokens.decimals_known(coin), p.tokens.decimals_known(usdt))
    else {
        return RateBound::na("decimals", "decimals монеты или USDT ещё не известны");
    };
    let hops = match hop_limit(ctx) {
        Ok(h) => h,
        Err(why) => return RateBound::na("no_data", why),
    };
    let mids: Vec<Address> = p.mids.active(ctx).iter().map(|m| m.address).collect();
    let g = gens(p, coin, usdt, &mids, hops);
    // Запись вне реестра (поколение 0, signal_epoch.rs) — входы не отследить: метод не действует.
    if g.contains(&0) {
        return RateBound::na("no_data", "пул набора вне реестра (поколение состояния 0)");
    }
    let key = Key {
        settings: ctx.settings.generation(),
        hops,
        taxes: format!("{:?}", p.checks.coin(&coin).map(|c| c.taxes)),
        mids,
        gens: g,
    };
    let sell = side == Side::Sell;
    if let Some((k, r)) = memo().get(&(coin, sell))
        && *k == key
    {
        return r.clone();
    }
    let pools = pool_set(p, coin, usdt, &key.mids, hops);
    let (from, to) = if sell { (coin, usdt) } else { (usdt, coin) };
    let r = match rho_raw(ctx, &pools, from, to, &key.mids, hops) {
        // Продажа: USDT-ед. за ед. монеты × 10^(dc − du) = USDT за монету; покупка — наоборот.
        Ok(raw) => {
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
        Err((class, why)) => RateBound::na(class, why),
    };
    memo().insert((coin, sell), (key, r.clone()));
    r
}

/// Память итогов под замком (отравленный замок — данные как есть). Ошибок нет.
fn memo() -> std::sync::MutexGuard<'static, Memo> {
    MEMO.lock().unwrap_or_else(|e| e.into_inner())
}
