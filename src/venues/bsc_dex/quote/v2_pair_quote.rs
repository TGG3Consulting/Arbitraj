//! Файл: venues/bsc_dex/quote/v2_pair_quote.rs
//! Что делает: решение 280 — сверка пары V2-форка без известного роутера (dex.kind = v2_fork_executor: Proto::V2,
//! tradable = нет, исполнение — шаг V2 контракта-исполнителя). Роутера нет — getAmountsOut спросить не у кого, поэтому
//! расчёт сверяется НАСТОЯЩИМ обменом через код ArbExecutor одним шагом V2 с нашей комиссией в eth_simulateV1 на блоке
//! сверки (подмена кода и слота владельца — как сухая сделка «через исполнитель», exec/executor/dry.rs):
//!   0 getReserves пары (резервы блока) → 1 пара переводит вход сделки владельцу подмены (from = пара, без подписи,
//!   validation = false) → 2 sync() пары (резерв входа = баланс) → 3 getReserves (резервы, по которым считает
//!   исполнитель) → 4 approve исполнителю → 5 execute(вход, сумма, выход, 1, без срока, [шаг V2 пары, наша комиссия]).
//! Пара откатила шаг 5 («…: K») — реальная комиссия пары выше нашей или swap нестандартный: сверка не прошла, пул не
//! берётся (с причиной). Прошла — выход сети = формула V2 по резервам шага 0 с ДОКАЗАННОЙ симуляцией комиссией (пара её
//! приняла своим кодом). Простая «getReserves + наша формула» без обмена ошибку комиссии не ловит — здесь она только
//! вторая половина после принятия обмена парой. Источник входа — сама пара (другого держателя без реестра не найти);
//! после sync резерв входа меньше на сумму сверки — проверка K с комиссией от точки кривой не зависит.
//! Для пробы налогов и хвоста сухой сделки (без обмена) «ожидалось без налога» — getReserves + формула с той же
//! комиссией (как Router.getAmountsOut: математика резервов без переводов); комиссию там проверяет сам обмен через
//! исполнитель в той же симуляции (откат — пул «не измерен», check/probe.rs) — это quote/v2_pair_probe.rs.
//! Кто вызывает: quote/quoter.rs (out_at), quote/v2_pair_probe.rs (on, fee), тесты (judge, calls).
//! От чего зависит: check/simulate.rs, exec/executor/{abi, code, entry, kind}.rs, quote/v2_pair_sim.rs (разбор), pools/abi_v2v3.rs.
//! Настройки: через entry.rs — dex.v2_fee_pct [DEX] (процент × 10000 — целое).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U64, U256, keccak256};
use serde_json::json;

use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::check::simulate::{self, Call, Override};
use super::super::ctx::Ctx;
use super::super::exec::executor::abi::ArbExecutor;
use super::super::exec::executor::kind::{self, ALL_BPS, Params, StepSpec};
use super::super::exec::executor::{code, entry};
use super::super::node::rpc;
use super::super::pools::abi_v2v3::IV2Pair;
use super::super::pools::families::{Family, Proto};
use super::super::pools::registry::Entry;
use super::v2_pair_sim::{Sim, judge};

alloy::sol! {
    function sync() external;
    function transfer(address to, uint256 value) external returns (bool);
    function approve(address spender, uint256 value) external returns (bool);
}

/// Вид без своего роутера: протокол V2 и нет своего пути исполнения (v2_fork_executor). Ошибок нет.
pub fn kind_on(f: &Family) -> bool {
    f.proto == Proto::V2 && !f.tradable
}

/// Пул такого вида. Ошибок нет.
pub fn on(e: &Entry) -> bool {
    kind_on(e.family)
}

/// Адрес подменённого исполнителя: последние 20 байт keccak256("arb.v2_pair_quote") — кода по нему в сети нет, подмена
/// живёт один вызов eth_simulateV1. Ошибок нет.
pub fn executor() -> Address {
    Address::from_slice(&keccak256(b"arb.v2_pair_quote")[12..])
}

/// Владелец подменённого исполнителя (слот owner): keccak256("arb.v2_pair_quote.owner"). Ошибок нет.
pub fn owner() -> Address {
    Address::from_slice(&keccak256(b"arb.v2_pair_quote.owner")[12..])
}

/// Комиссия шага (как у сделки: entry.rs) → дробь «после комиссии» (1e6 − пипсы, 1e6) — ровно формула _v2Out контракта.
/// Ошибки: NotConfigured (dex.v2_fee_pct не задана или не целое число пипсов), NoRoute (пул не контракт).
pub fn fee(ctx: &Ctx, e: &Entry, token_in: Address) -> GatewayResult<(u32, (U256, U256))> {
    match entry::params(ctx, e, token_in)?.0 {
        Params::V2 { fee_pips } => Ok((
            fee_pips,
            (
                U256::from(1_000_000u32 - fee_pips.min(1_000_000)),
                U256::from(1_000_000u32),
            ),
        )),
        _ => Err(ctx.err(ErrorCode::Internal, "шаг пары V2 не вида V2")),
    }
}

/// Адрес пары. Ошибки: Internal — ссылка не контракт.
pub fn pair(ctx: &Ctx, e: &Entry) -> GatewayResult<Address> {
    match e.pool.reference {
        PoolRef::Contract(a) => Ok(a),
        _ => Err(ctx.err(ErrorCode::Internal, "пара V2 не контракт")),
    }
}

/// Шесть вызовов сверки и подмена кода (см. шапку). Принимает: пул, токен входа, сумму. Возвращает: вызовы, подмену,
/// дробь комиссии. Ошибки: NotConfigured (комиссия), Internal (артефакт исполнителя не сверен по хешу), NoRoute.
pub fn calls(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<(Vec<Call>, Override, (U256, U256))> {
    let p = pair(ctx, e)?;
    let token_out = if token_in == e.pool.token0 {
        e.pool.token1
    } else {
        e.pool.token0
    };
    let (pips, ratio) = fee(ctx, e, token_in)?;
    let step = kind::encode(&StepSpec {
        params: Params::V2 { fee_pips: pips },
        pool: p,
        token_in,
        token_out,
        share_bps: ALL_BPS,
    })
    .map_err(|w| ctx.err(ErrorCode::NoRoute, w))?;
    let (x, o) = (executor(), owner());
    let exec = ArbExecutor::executeCall {
        tokenIn: token_in,
        amountIn: amount,
        tokenOut: token_out,
        minOut: U256::from(1u8),
        // Срок — не настройка: симуляция одна, в сеть ничего не уходит; «без срока» = наибольшее uint256.
        deadline: U256::MAX,
        steps: vec![step],
    };
    let c = |from: Address, to: Address, data: Vec<u8>| Call {
        from,
        to,
        input: Bytes::from(data),
    };
    let reserves = IV2Pair::getReservesCall {}.abi_encode();
    let list = vec![
        c(o, p, reserves.clone()),
        c(
            p,
            token_in,
            transferCall {
                to: o,
                value: amount,
            }
            .abi_encode(),
        ),
        c(o, p, syncCall {}.abi_encode()),
        c(o, p, reserves),
        c(
            o,
            token_in,
            approveCall {
                spender: x,
                value: amount,
            }
            .abi_encode(),
        ),
        c(o, x, exec.abi_encode()),
    ];
    let internal = |w: String| ctx.err(ErrorCode::Internal, w);
    let over = Override {
        address: x,
        code: code::runtime().map_err(internal)?,
        state: vec![(
            code::owner_slot().map_err(internal)?,
            B256::left_padding_from(o.as_slice()),
        )],
    };
    Ok((list, over, ratio))
}

/// Выход сети на точный вход на блоке (см. шапку). Принимает: пул, токен входа, сумму, блок (hex-номер или «latest»).
/// Ошибки: SwapReverted — пара отвергла обмен с нашей комиссией; NoRoute — подготовка не прошла; NotConfigured, узел.
pub async fn out_at(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
    block: &str,
) -> GatewayResult<U256> {
    let (list, over, ratio) = calls(ctx, e, token_in, amount)?;
    let number = match block.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16)
            .map_err(|_| ctx.err(ErrorCode::Internal, "номер блока не hex"))?,
        None => rpc::call::<U64>(ctx, "eth_blockNumber", json!([]))
            .await?
            .to::<u64>(),
    };
    let r = simulate::run_with(ctx, &list, &[], &[over], number).await?;
    let t = e.family.title;
    match judge(&r, token_in == e.pool.token0, amount, ratio) {
        Sim::Ok { remote, .. } => Ok(remote),
        Sim::Prep(why) => Err(ctx.err(ErrorCode::NoRoute, format!("{t}: сверка через исполнитель не собрана — {why}"))),
        Sim::Rejected(why) => Err(ctx.err(
            ErrorCode::SwapReverted,
            format!(
                "{t}: пара отвергла обмен шагом V2 исполнителя с комиссией dex.v2_fee_pct — комиссия пары выше \
                 нашей или swap нестандартный (решение 280): {why}"
            ),
        )),
    }
}
