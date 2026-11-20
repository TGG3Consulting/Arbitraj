//! Файл: venues/bsc_dex/pools/balancer_v3_factories.rs
//! Что делает: решение 441 (T433-F2) — фабрики пулов Balancer V3 этой DEX из настроек и правило пула по фабрике:
//! у разных выпусков фабрик onSwap разный (math/balancer_v3.rs), поэтому каждый выпуск — свой список адресов:
//! dex.bv3_weighted_factories (весовой 20241205-v3-weighted-pool), dex.bv3_weighted_min_factories (весовой
//! 20260115-v3-weighted-pool-v2 — нижние балансы), dex.bv3_stable_factories (стейблсвап 20241205-v3-stable-pool и
//! 20250324-v3-stable-pool-v2), dex.bv3_stable_guard_factories (стейблсвап 20260116-v3-stable-pool-v3 — проверка
//! дисбаланса). Пул подтверждает isPoolFromFactory фабрики из списка (BasePoolFactory.sol L69-L72: только пулы, созданные
//! этой фабрикой).
//! Кто вызывает: pools/verify_balancer_v3.rs, pools/load_balancer_v3_fixed.rs, pools/families.rs (required).
//! От чего зависит: ctx.rs (dex_list).
//! Настройки: dex.bv3_weighted_factories, dex.bv3_weighted_min_factories, dex.bv3_stable_factories,
//! dex.bv3_stable_guard_factories [DEX вида balancer_v3].

use std::str::FromStr;

use alloy_primitives::Address;

use super::super::ctx::Ctx;

/// Выпуск фабрики → правило onSwap пула.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// Весовой 20241205-v3-weighted-pool.
    Weighted,
    /// Весовой 20260115-v3-weighted-pool-v2 (нижние балансы, баланс входа + 1).
    WeightedMin,
    /// Стейблсвап 20241205-v3-stable-pool / 20250324-v3-stable-pool-v2.
    Stable,
    /// Стейблсвап 20260116-v3-stable-pool-v3 (проверка дисбаланса).
    StableGuard,
}

impl Release {
    /// Весовой ли пул (иначе стейблсвап). Ошибок нет.
    pub fn weighted(self) -> bool {
        matches!(self, Release::Weighted | Release::WeightedMin)
    }
}

/// Настройки списков фабрик по выпускам (порядок — порядок проверки). Ошибок нет.
pub const LISTS: [(&str, Release); 4] = [
    ("dex.bv3_weighted_factories", Release::Weighted),
    ("dex.bv3_weighted_min_factories", Release::WeightedMin),
    ("dex.bv3_stable_factories", Release::Stable),
    ("dex.bv3_stable_guard_factories", Release::StableGuard),
];

/// Все фабрики DEX с выпуском. Принимает: окружение, id DEX. Возвращает: (фабрика, выпуск). Ошибки (текст): список не
/// задан (пустой список — допустимо: выпуска нет у DEX), не адрес; все списки пусты — «фабрик нет».
pub fn all(ctx: &Ctx, id: &str) -> Result<Vec<(Address, Release)>, String> {
    let mut out = Vec::new();
    for (name, r) in LISTS {
        let list = ctx.dex_list(name, id).map_err(|e| e.message)?;
        for s in list {
            let a = Address::from_str(s.trim())
                .map_err(|_| format!("{name} [dex:{id}]: «{s}» — не адрес"))?;
            out.push((a, r));
        }
    }
    if out.is_empty() {
        return Err(format!(
            "у DEX {id} все списки фабрик Balancer V3 (dex.bv3_*_factories) пусты"
        ));
    }
    Ok(out)
}

/// Выпуск пула по ответам isPoolFromFactory (по порядку `all`). Принимает: фабрики, ответы (true — пул этой фабрики).
/// Возвращает: выпуск; ни одна не подтвердила или подтвердили фабрики разных выпусков — причина (текст).
pub fn release(list: &[(Address, Release)], yes: &[bool]) -> Result<Release, String> {
    let mut found: Option<Release> = None;
    for ((f, r), ok) in list.iter().zip(yes) {
        if !*ok {
            continue;
        }
        match found {
            Some(x) if x != *r => {
                return Err(format!(
                    "пул подтвердили фабрики разных выпусков ({x:?} и {r:?}, {f:#x})"
                ));
            }
            _ => found = Some(*r),
        }
    }
    let why = "пул не создан фабриками DEX (dex.bv3_*_factories): код не сверен с исходником";
    found.ok_or_else(|| why.to_string())
}
