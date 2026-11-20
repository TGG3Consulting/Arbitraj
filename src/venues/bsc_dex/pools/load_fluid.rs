//! Файл: venues/bsc_dex/pools/load_fluid.rs
//! Что делает: решение 440 — чтение состояния пула Fluid DEX T1 на блоке одним try_many (Multicall3):
//! getPoolReservesAdjusted(pool) официального резолвера (центральная цена, резервы смарт-залога и смарт-долга в
//! приведённых единицах — те, что пул считает сам на этом блоке: резолвер зовёт getPricesAndExchangePrices и
//! getCollateralReserves / getDebtReserves пула; пределы снятия/займа), constantsView (токены, слой ликвидности,
//! фабрика), constantsView2 (множители точности), readFromStorage(0) и (1) (слова цен / настроек пула), VERSION()
//! (есть — пул 1.1.0+, откат — старый пул) и время блока (Multicall3.getCurrentBlockTimestamp; без dex.multicall3 —
//! load_smardex::block_secs). Собирает PoolState::SmartColDebt. Выход обмена зависит от времени блока (сдвиг
//! центральной цены и диапазона, начисление процентов слоя ликвидности) и от операций других пользователей слоя
//! ликвидности (событий пула нет) — вид перечитывается каждым блоком (per_block.rs).
//! Кто вызывает: pools/load.rs (ветка Proto::FluidT1).
//! От чего зависит: ctx.rs, node/call.rs, node/multicall.rs, abi_fluid.rs, abi_smardex.rs (IMulticall3Time),
//! load_smardex.rs (block_secs), stable_native.rs (wrapped).
//! Настройки: dex.quoter [DEX] (адрес резолвера FluidDexReservesResolver), dex.native_wrapped, dex.multicall3 [bsc_dex].
//!
//! Источник: Instadapp/fluid-contracts-public@404d73f — periphery/resolvers/dexReserves/main.sol L1176-L1192
//! (getPoolReservesAdjusted: центральная цена 0 и нулевые резервы, если пул откатил getPricesAndExchangePrices),
//! L1117-L1120 (fee = биты 2..18 dexVariables2); poolT1/common/variables.sol и SPEC.md §6 (dexVariables2: бит 0 —
//! смарт-залог, бит 1 — смарт-долг, биты 2–18 — комиссия, 19–25 — доля протокола, бит 255 — пауза);
//! coreModule/core/main.sol L95-L105 (комиссия и доля протокола из dexVariables2).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};

use crate::core::types::{ColReserves, DebtReserves, PoolState, SmartColDebtData};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::{call, multicall};
use super::abi_fluid::{
    IFluidDexReservesResolver as R, IFluidDexT1 as P, NATIVE, SLOT_VARS, SLOT_VARS2,
};
use super::abi_smardex::IMulticall3Time;
use super::{load_smardex, stable_native};

/// Данные вызова. Ошибок нет.
fn enc<C: SolCall>(c: C) -> Bytes {
    Bytes::from(c.abi_encode())
}

/// Вызовы состояния. Принимает: резолвер, пул. Ошибок нет.
pub fn calls(resolver: Address, pool: Address) -> Vec<(Address, Bytes)> {
    let v0 = B256::from(U256::from(SLOT_VARS).to_be_bytes::<32>());
    let v2 = B256::from(U256::from(SLOT_VARS2).to_be_bytes::<32>());
    vec![
        (resolver, enc(R::getPoolReservesAdjustedCall { pool_: pool })),
        (pool, enc(P::constantsViewCall {})),
        (pool, enc(P::constantsView2Call {})),
        (pool, enc(P::readFromStorageCall { slot_: v0 })),
        (pool, enc(P::readFromStorageCall { slot_: v2 })),
        (pool, enc(P::VERSIONCall {})),
    ]
}

/// Состояние пула на блоке. Принимает: окружение, резолвер (dex.quoter), пул, блок. Ошибки: узел; BadResponse — вызов
/// откатился, ответ не по ABI, пул не читается (центральная цена 0), нативная монета без dex.native_wrapped.
pub async fn load(
    ctx: &Ctx,
    resolver: Address,
    pool: Address,
    block: u64,
) -> GatewayResult<PoolState> {
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("Fluid {pool}: {what}"));
    let mut list = calls(resolver, pool);
    let mc = multicall::address(ctx);
    if let Some(m) = mc {
        list.push((m, enc(IMulticall3Time::getCurrentBlockTimestampCall {})));
    }
    let got = call::try_many(ctx, &list, &call::block_tag(block)).await?;
    if got.len() != list.len() {
        return Err(bad(format!("ответов {} из {}", got.len(), list.len())));
    }
    let ok = |i: usize| {
        got[i]
            .as_ref()
            .map_err(|r| bad(format!("вызов {i} откатился: {r}")))
    };
    let block_ts = match mc {
        Some(_) => call::decode::<IMulticall3Time::getCurrentBlockTimestampCall>(ok(6)?)?,
        None => U256::from(load_smardex::block_secs(ctx, block).await?),
    };
    let raw = Raw {
        reserves: call::decode::<R::getPoolReservesAdjustedCall>(ok(0)?)?,
        constants: call::decode::<P::constantsViewCall>(ok(1)?)?,
        scales: call::decode::<P::constantsView2Call>(ok(2)?)?,
        vars: call::decode::<P::readFromStorageCall>(ok(3)?)?,
        vars2: call::decode::<P::readFromStorageCall>(ok(4)?)?,
        versioned: got[5].is_ok(),
        block_ts,
    };
    state(&raw, stable_native::wrapped(ctx)).map_err(bad)
}

/// Прочитанное с пула и резолвера (ответы как есть).
pub struct Raw {
    pub reserves: R::PoolWithReserves,
    pub constants: P::ConstantViews,
    pub scales: P::ConstantViews2,
    pub vars: U256,
    pub vars2: U256,
    pub versioned: bool,
    pub block_ts: U256,
}

/// Биты слова: (w >> shift) & (2^bits − 1), как u32. Ошибок нет (поля ≤ 17 бит).
fn bits(w: U256, shift: usize, n: usize) -> u32 {
    let one = U256::from(1u8);
    ((w >> shift) & ((one << n) - one)).to::<u32>()
}

/// Причина: у пула нативная монета, а обёртка не задана.
const NO_WRAP: &str = "нативная монета пула 0xEeee…, а dex.native_wrapped не задана";

/// Токен пула → токен пары (метка нативной монеты → обёртка). Ошибка (текст) — метка есть, обёртки нет.
fn coin(t: Address, wrapped: Option<Address>) -> Result<Address, String> {
    if t != NATIVE {
        return Ok(t);
    }
    wrapped.ok_or_else(|| NO_WRAP.to_string())
}

/// Сборка состояния из прочитанного (без сети; её же зовут тесты на записанных ответах). Принимает: прочитанное,
/// обёртку нативной монеты. Возвращает: SmartColDebt. Ошибка (текст) — пул не читается (центральная цена 0 —
/// резолвер получил откат getPricesAndExchangePrices), токены резолвера ≠ constantsView, время не в u64, нативная
/// монета без обёртки, обе монеты — нативная и её обёртка.
pub fn state(r: &Raw, wrapped: Option<Address>) -> Result<PoolState, String> {
    let p = &r.reserves;
    let c = &r.constants;
    if p.centerPrice.is_zero() {
        return Err("центральная цена 0 — откат getPricesAndExchangePrices пула".into());
    }
    if (p.token0, p.token1) != (c.token0, c.token1) {
        return Err("токены резолвера ≠ constantsView пула".into());
    }
    let Ok(block_ts) = u64::try_from(r.block_ts) else {
        return Err("время блока не в u64".into());
    };
    let token0 = coin(c.token0, wrapped)?;
    if token0 == coin(c.token1, wrapped)? {
        return Err("у пула и нативная монета, и её обёртка — пара неоднозначна".into());
    }
    let cr = &p.collateralReserves;
    let dr = &p.debtReserves;
    let l = &p.limits;
    let s = &r.scales;
    let w2 = r.vars2;
    Ok(PoolState::SmartColDebt(Box::new(SmartColDebtData {
        token0,
        native0: c.token0 == NATIVE,
        native1: c.token1 == NATIVE,
        col_on: bits(w2, 0, 1) == 1,
        debt_on: bits(w2, 1, 1) == 1,
        paused: bits(w2, 255, 1) == 1,
        fee: bits(w2, 2, 17),
        revenue_cut: bits(w2, 19, 7),
        center_price: p.centerPrice,
        col: ColReserves {
            real0: cr.token0RealReserves,
            real1: cr.token1RealReserves,
            imag0: cr.token0ImaginaryReserves,
            imag1: cr.token1ImaginaryReserves,
        },
        debt: DebtReserves {
            debt0: dr.token0Debt,
            debt1: dr.token1Debt,
            real0: dr.token0RealReserves,
            real1: dr.token1RealReserves,
            imag0: dr.token0ImaginaryReserves,
            imag1: dr.token1ImaginaryReserves,
        },
        withdrawable0: l.withdrawableToken0.available,
        withdrawable1: l.withdrawableToken1.available,
        borrowable0: l.borrowableToken0.available,
        borrowable1: l.borrowableToken1.available,
        num0: s.token0NumeratorPrecision,
        den0: s.token0DenominatorPrecision,
        num1: s.token1NumeratorPrecision,
        den1: s.token1DenominatorPrecision,
        dex_variables: r.vars,
        versioned: r.versioned,
        liquidity: c.liquidity,
        block_ts,
    })))
}
