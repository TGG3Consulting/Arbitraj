//! Файл: venues/bsc_dex/pools/balancer_v3_factories.rs
//! Что делает: решение 441 (T433-F2) — фабрики пулов Balancer V3 этой DEX из настроек и правило пула по фабрике:
//! у разных выпусков фабрик onSwap разный (math/balancer_v3.rs), поэтому каждый выпуск — свой список адресов:
//! dex.bv3_weighted_factories (весовой 20241205-v3-weighted-pool), dex.bv3_weighted_min_factories (весовой
//! 20260115-v3-weighted-pool-v2 — нижние балансы), dex.bv3_stable_factories (стейблсвап 20241205-v3-stable-pool и
//! 20250324-v3-stable-pool-v2), dex.bv3_stable_guard_factories (стейблсвап 20260116-v3-stable-pool-v3 — проверка
//! дисбаланса). Пул подтверждает isPoolFromFactory фабрики из списка (BasePoolFactory.sol L69-L72: только пулы, созданные
//! этой фабрикой).
//! Решение 452: + выпуски StableOpen (стейблсвап 20250324 и фабрики StableSurge 20250121 / 20250404 — onSwap без
//! onlyVault), Gyro 2-CLP, Gyro E-CLP, ReClamm (два правила чтения), LBP v1–v4, LBP с фиксированной ценой, QuantAMM; у
//! стейблсвапа 20250324 теперь свой список dex.bv3_stable_open_factories (в dex.bv3_stable_factories — только 20241205).
//! Кто вызывает: pools/verify_balancer_v3.rs, pools/load_balancer_v3_fixed.rs, pools/families.rs (required).
//! От чего зависит: ctx.rs (dex_list).
//! Настройки: dex.bv3_weighted_factories, dex.bv3_weighted_min_factories, dex.bv3_stable_factories,
//! dex.bv3_stable_open_factories, dex.bv3_stable_guard_factories, dex.bv3_gyro_2clp_factories,
//! dex.bv3_gyro_eclp_factories, dex.bv3_reclamm_factories, dex.bv3_reclamm_v3_factories, dex.bv3_lbp_v1_factories …
//! dex.bv3_lbp_v4_factories, dex.bv3_fixed_lbp_factories, dex.bv3_quantamm_factories [DEX вида balancer_v3].

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
    /// Стейблсвап 20241205-v3-stable-pool (onSwap с onlyVault).
    Stable,
    /// Решение 452: стейблсвап без проверки дисбаланса и с onSwap без onlyVault — 20250324-v3-stable-pool-v2 и фабрики
    /// StableSurge 20250121-v3-stable-surge, 20250404-v3-stable-surge-pool-factory-v2.
    StableOpen,
    /// Стейблсвап 20260116-v3-stable-pool-v3 и фабрика StableSurge 20260117-v3-stable-surge-pool-factory-v3 (проверка
    /// дисбаланса).
    StableGuard,
    /// Решение 452: Gyro 2-CLP (20250120, 20260123-v2).
    Gyro2Clp,
    /// Решение 452: Gyro E-CLP (20250124, 20260126-v2).
    GyroEclp,
    /// Решение 452: ReClamm 20250409 и 20250702-v2 (computeCurrentVirtualBalances — по lastBalancesLiveScaled18).
    ReClamm,
    /// Решение 452: ReClamm 20260428-v3 (computeCurrentVirtualBalances — по getCurrentLiveBalances).
    ReClammLive,
    /// Решение 452: LBP 20250307 (неизменяемое — структура выпуска 1).
    LbpV1,
    /// Решение 452: LBP 20250701-v2 (структура выпуска 2).
    LbpV2,
    /// Решение 452: LBP 20251219-v3 (виртуальный резерв).
    LbpV3,
    /// Решение 452: LBP 20260501-v4 (виртуальный резерв, WeightedPool 20260115).
    LbpV4,
    /// Решение 452: LBP с фиксированной ценой 20251205.
    FixedLbp,
    /// Решение 452: QuantAMM (QuantAMMProtocol/deployment 20250429-v3-arbitrum-quantamm).
    QuantAmm,
}

impl Release {
    /// Весовой пул с неизменяемыми весами (getNormalizedWeights читается один раз). Ошибок нет.
    pub fn weighted(self) -> bool {
        matches!(self, Release::Weighted | Release::WeightedMin)
    }

    /// Стейблсвап (A читается каждым блоком). Ошибок нет.
    pub fn stable(self) -> bool {
        matches!(
            self,
            Release::Stable | Release::StableOpen | Release::StableGuard
        )
    }

    /// Веса меняются со временем (LBP, QuantAMM) — getNormalizedWeights каждым блоком. Ошибок нет.
    pub fn moving_weights(self) -> bool {
        matches!(
            self,
            Release::LbpV1 | Release::LbpV2 | Release::LbpV3 | Release::LbpV4 | Release::QuantAmm
        )
    }

    /// onSwap пула открыт не только хранилищу (нет onlyVault) — его может звать крючок StableSurge (сверено по
    /// build-info каждого выпуска, DECISIONS 452). Ошибок нет.
    pub fn open(self) -> bool {
        matches!(
            self,
            Release::WeightedMin
                | Release::StableOpen
                | Release::StableGuard
                | Release::LbpV3
                | Release::LbpV4
                | Release::FixedLbp
        )
    }
}

/// Настройки списков фабрик по выпускам (порядок — порядок проверки). Ошибок нет.
pub const LISTS: [(&str, Release); 15] = [
    ("dex.bv3_weighted_factories", Release::Weighted),
    ("dex.bv3_weighted_min_factories", Release::WeightedMin),
    ("dex.bv3_stable_factories", Release::Stable),
    ("dex.bv3_stable_open_factories", Release::StableOpen),
    ("dex.bv3_stable_guard_factories", Release::StableGuard),
    ("dex.bv3_gyro_2clp_factories", Release::Gyro2Clp),
    ("dex.bv3_gyro_eclp_factories", Release::GyroEclp),
    ("dex.bv3_reclamm_factories", Release::ReClamm),
    ("dex.bv3_reclamm_v3_factories", Release::ReClammLive),
    ("dex.bv3_lbp_v1_factories", Release::LbpV1),
    ("dex.bv3_lbp_v2_factories", Release::LbpV2),
    ("dex.bv3_lbp_v3_factories", Release::LbpV3),
    ("dex.bv3_lbp_v4_factories", Release::LbpV4),
    ("dex.bv3_fixed_lbp_factories", Release::FixedLbp),
    ("dex.bv3_quantamm_factories", Release::QuantAmm),
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
