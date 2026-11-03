//! Файл: venues/bsc_dex/pools/load_inf_stable.rs
//! Что делает: чтение состояния пула PancakeSwap Infinity Stable на блоке (решение «122 (Infinity Stable)») одним
//! try_many (Multicall3) у КРЮЧКА пула: N_COINS, coins(0..2), balances(0..2) (stored − admin), stored_rates() (курсы
//! монет — у монет с оракулом курса и ERC4626 крючок сам зовёт внешние контракты на этом блоке), A_precise() (A·100 с
//! рампой по времени блока), fee(), offpeg_fee_multiplier(), pool_manager(). Каждое чтение заново сверяет крючок:
//! pool_manager() = dex.pool_manager, N_COINS = 2, coins = валюты пары. Собирает PoolState::StableSwap с шаблоном NG.
//! Откат stored_rates() (оракул курса не отвечает) — обмен откатился бы тоже: пул «остановлен» (halted) до следующего
//! удачного чтения, балансы и A всё равно показываются.
//! Кто вызывает: pools/load.rs (ветка Proto::InfinityStable).
//! От чего зависит: ctx.rs, node/call.rs, abi_inf_stable.rs, math/stableswap_ng.rs (A_PRECISION, MAX_ROUNDS).
//! Настройки: dex.pool_manager [DEX]; dex.multicall3 [bsc_dex] (не задан — пачкой JSON-RPC, call::try_many).
//!
//! Источник — verified-код крючка (abi_inf_stable.rs): `_balances` L299-L320, `_stored_rates` L259-L296, `_A`
//! L922-L941, `A_precise` L1267-L1270, `balances` L1273-L1282, `stored_rates` L1291-L1294, конструктор L1489-L1581
//! (coins отсортированы L1530, pool_manager = factory.pool_manager() L1527).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolState, StableData, StableNg};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::stableswap_ng::{A_PRECISION, MAX_ROUNDS};
use super::super::node::call;
use super::abi_inf_stable::{HOOK_COINS, IInfStableHook as H};
use super::families::Family;

/// Состояние пула на блоке. Принимает: окружение, вид (id DEX — для dex.pool_manager), адрес крючка, пару (token0,
/// token1), блок-тег. Ошибки: NotConfigured (dex.pool_manager), узел, ответ не по ABI, крючок не того менеджера,
/// не две монеты или монеты не те, что у пары.
pub async fn load(
    ctx: &Ctx,
    f: &Family,
    hook: Address,
    pair: (Address, Address),
    tag: &str,
) -> GatewayResult<PoolState> {
    let bad = |what: String| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Infinity Stable {hook}: {what}"),
        )
    };
    let manager = ctx.dex_addr("dex.pool_manager", f.id)?;
    let mut calls: Vec<Bytes> = vec![
        H::N_COINSCall {}.abi_encode().into(),
        H::pool_managerCall {}.abi_encode().into(),
        H::A_preciseCall {}.abi_encode().into(),
        H::feeCall {}.abi_encode().into(),
        H::offpeg_fee_multiplierCall {}.abi_encode().into(),
        H::stored_ratesCall {}.abi_encode().into(),
    ];
    for k in 0..HOOK_COINS {
        let i = U256::from(k);
        calls.push(H::coinsCall { i }.abi_encode().into());
        calls.push(H::balancesCall { i }.abi_encode().into());
    }
    let calls: Vec<(Address, Bytes)> = calls.into_iter().map(|d| (hook, d)).collect();
    let got = call::try_many(ctx, &calls, tag).await?;
    if got.len() != calls.len() {
        return Err(bad(format!("ответов {} из {}", got.len(), calls.len())));
    }
    let ok = |i: usize| {
        got[i]
            .as_ref()
            .map_err(|r| bad(format!("вызов {i} откатился: {r}")))
    };
    let n = call::decode::<H::N_COINSCall>(ok(0)?)?;
    if n != U256::from(HOOK_COINS) {
        return Err(bad(format!("N_COINS = {n}, крючок пула — две монеты")));
    }
    let pm = call::decode::<H::pool_managerCall>(ok(1)?)?;
    if pm != manager {
        return Err(bad(format!(
            "pool_manager() = {pm}, у DEX {} — {manager} (dex.pool_manager)",
            f.id
        )));
    }
    let amp = call::decode::<H::A_preciseCall>(ok(2)?)?;
    let fee = call::decode::<H::feeCall>(ok(3)?)?;
    let offpeg = call::decode::<H::offpeg_fee_multiplierCall>(ok(4)?)?;
    // Откат stored_rates — оракул курса монеты не отвечает: обмен откатился бы (raw_call L275-L281) → пул стоит.
    let rates = match got[5].as_ref() {
        Ok(b) => Some(call::decode::<H::stored_ratesCall>(b)?),
        Err(_) => None,
    };
    let mut coins = Vec::with_capacity(HOOK_COINS);
    let mut balances = Vec::with_capacity(HOOK_COINS);
    for k in 0..HOOK_COINS {
        coins.push(call::decode::<H::coinsCall>(ok(6 + 2 * k)?)?);
        balances.push(call::decode::<H::balancesCall>(ok(7 + 2 * k)?)?);
    }
    if (coins[0], coins[1]) != pair {
        return Err(bad(format!(
            "монеты крючка {} / {} ≠ валюты ключа пула {} / {}",
            coins[0], coins[1], pair.0, pair.1
        )));
    }
    let halted = rates.as_ref().is_none_or(|r| r.len() != HOOK_COINS);
    let rates = rates
        .filter(|r| r.len() == HOOK_COINS)
        .unwrap_or_else(|| vec![U256::ZERO; HOOK_COINS]);
    Ok(PoolState::StableSwap(Box::new(StableData {
        balances,
        rates,
        amp,
        fee,
        amp_precision: U256::from(A_PRECISION),
        max_rounds: MAX_ROUNDS,
        // get_D / get_y крючка откатываются при несходимости (raise L811, L862).
        strict: true,
        guard: None,
        halted,
        // currency0 = coins[0], currency1 = coins[1] (beforeInitialize L1654).
        i0: 0,
        i1: 1,
        ng: Some(StableNg {
            offpeg_fee_multiplier: offpeg,
        }),
    })))
}
