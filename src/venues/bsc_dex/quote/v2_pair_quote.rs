//! Файл: venues/bsc_dex/quote/v2_pair_quote.rs
//! Что делает: решение 280 — сверка пары V2-форка без известного роутера (dex.kind = v2_fork_executor: Proto::V2,
//! tradable = нет, исполнение — шаг V2 контракта-исполнителя). Роутера нет — getAmountsOut спросить не у кого, поэтому
//! расчёт сверяется НАСТОЯЩИМ обменом через код ArbExecutor одним шагом V2 с нашей комиссией в eth_simulateV1 на блоке
//! сверки (подмена кода и слота владельца — как сухая сделка «через исполнитель», exec/executor/dry.rs):
//!   вызовы — quote/v2_pair_calls.rs: пара переводит вход ИСПОЛНИТЕЛЮ, sync, balanceOf исполнителя (фактически
//!   пришло — у монеты с налогом меньше суммы), execute с amountIn = 0 (шаг берёт весь баланс исполнителя),
//!   getReserves после обмена (доработка по ревью: продажа монеты с налогом на перевод из пары не откатывается).
//! Пара откатила обмен проверкой K — реальная комиссия пары выше нашей или обмен нестандартный: сверка не прошла, пул
//! не берётся (SwapReverted с этой причиной); иной откат обмена — SwapReverted «не проверкой K»; сверка не собрана —
//! NoRoute; пара отдала меньше формулы V2 больше dex.quoter_max_diff_pct — QuoteMismatch (разбор — v2_pair_sim.rs).
//! Прошла — выход сети = формула V2 по резервам шага 0 на запрошенную сумму с ДОКАЗАННОЙ симуляцией комиссией (пара её
//! приняла своим кодом). Простая «getReserves + наша формула» без обмена ошибку комиссии не ловит — здесь она только
//! вторая половина после принятия обмена парой. Источник входа — сама пара (другого держателя без реестра не найти);
//! после sync резерв входа меньше на сумму сверки — проверка K с комиссией от точки кривой не зависит.
//! Для пробы налогов и хвоста сухой сделки (без обмена) «ожидалось без налога» — getReserves + формула с той же
//! комиссией (как Router.getAmountsOut: математика резервов без переводов); комиссию там проверяет сам обмен через
//! исполнитель в той же симуляции (откат — пул «не измерен», check/probe.rs) — это quote/v2_pair_probe.rs.
//! Кто вызывает: quote/quoter.rs (out_at), quote/v2_pair_probe.rs (on, fee), тесты (judge, calls).
//! От чего зависит: check/simulate.rs, exec/executor/{code, entry, kind}.rs, quote/{v2_pair_calls, v2_pair_sim}.rs.
//! Настройки: через entry.rs — dex.v2_fee_pct [DEX] (процент × 10000 — целое); dex.quoter_max_diff_pct [bsc_dex].

use alloy_primitives::{Address, B256, Bytes, U64, U256, keccak256};
use serde_json::json;

use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::check::simulate::{self, Call, Override};
use super::super::ctx::Ctx;
use super::super::exec::executor::kind::Params;
use super::super::exec::executor::{code, entry};
use super::super::node::rpc;
use super::super::pools::families::{Family, Proto};
use super::super::pools::registry::Entry;
use super::v2_pair_calls;
use super::v2_pair_sim::{Sim, judge};

/// Вид без своего роутера: протокол V2 и нет своего пути исполнения (v2_fork_executor). Ошибок нет.
pub fn kind_on(f: &Family) -> bool {
    !f.variant.umbrae() && f.proto == Proto::V2 && (!f.tradable || f.variant.dual_v2_execution() || f.variant == super::super::pools::family_variant::Variant::OmniV2) && !f.variant.frax_twamm()
        && !matches!(f.variant,super::super::pools::family_variant::Variant::Antfarm | super::super::pools::family_variant::Variant::Surge | super::super::pools::family_variant::Variant::Dagrow | super::super::pools::family_variant::Variant::Temple | super::super::pools::family_variant::Variant::Carbon | super::super::pools::family_variant::Variant::MintClub | super::super::pools::family_variant::Variant::Virtuals | super::super::pools::family_variant::Variant::BunnySwap | super::super::pools::family_variant::Variant::Memebox | super::super::pools::family_variant::Variant::Oasis | super::super::pools::family_variant::Variant::EtherVista)
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

/// Семь вызовов сверки (quote/v2_pair_calls.rs) и подмена кода. Принимает: пул, токен входа, сумму. Возвращает:
/// вызовы, подмену, дробь комиссии. Ошибки: NotConfigured (комиссия), Internal (артефакт исполнителя не сверен по
/// хешу), NoRoute.
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
    let (x, o) = (executor(), owner());
    let list = v2_pair_calls::build(p, (token_in, token_out), pips, amount, (x, o))
        .map_err(|w| ctx.err(ErrorCode::NoRoute, w))?;
    let internal = |w: String| ctx.err(ErrorCode::Internal, w);
    let over = Override {
        address: x,
        code: code::runtime().map_err(internal)?,
        state: vec![(
            code::owner_slot().map_err(internal)?,
            B256::left_padding_from(o.as_slice()),
        )],
        diff: false,
    };
    Ok((list, over, ratio))
}

/// Выход сети на точный вход на блоке (см. шапку). Принимает: пул, токен входа, сумму, блок (hex-номер или «latest»).
/// Ошибки: SwapReverted — пара отклонила обмен (K) или обмен откатился иначе; QuoteMismatch — пара отдала меньше
/// формулы больше dex.quoter_max_diff_pct; NoRoute — сверка не собрана; NotConfigured, узел.
pub async fn out_at(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
    block: &str,
) -> GatewayResult<U256> {
    if let crate::core::types::PoolState::CandySwap(state)=&e.pool.state
        && state.reserves!=state.balances {
        return Box::pin(super::candy_wallet::quote(ctx,e,state,token_in,amount,block)).await;
    }
    let limit = ctx.dec("dex.quoter_max_diff_pct")?;
    let number = match block.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16)
            .map_err(|_| ctx.err(ErrorCode::Internal, "номер блока не hex"))?,
        None => rpc::call::<U64>(ctx, "eth_blockNumber", json!([]))
            .await?
            .to::<u64>(),
    };
    let (r, ratio) =
        if e.family.variant.live_v2_fee() {
            current_rate_simulation(ctx, e, token_in, amount, number).await?
        } else {
            let (list, over, ratio) = calls(ctx, e, token_in, amount)?;
            (
                simulate::run_with(ctx, &list, &[], &[over], number).await?,
                ratio,
            )
        };
    let t = e.family.title;
    let (code, why) = match judge(&r, token_in == e.pool.token0, amount, ratio, limit) {
        Sim::Ok { remote, .. } => {
            if let crate::core::types::PoolState::CandySwap(state)=&e.pool.state {
                return super::super::math::candy::net(remote,state.payout_bps)
                    .filter(|v|!v.is_zero()).ok_or_else(||ctx.err(ErrorCode::NoRoute,"CandySwap: вся выплата удержана"));
            }
            return Ok(remote);
        }
        Sim::Prep(w) => (
            ErrorCode::NoRoute,
            format!(
                "сверка через исполнитель не собрана (перевод из пары не прошёл или обложен, или ответ пары не по ABI) — {w}"
            ),
        ),
        Sim::Rejected(w) => (
            ErrorCode::SwapReverted,
            format!(
                "пара отклонила обмен (K) шагом V2 исполнителя с комиссией dex.v2_fee_pct — комиссия пары выше нашей или обмен нестандартный: {w}"
            ),
        ),
        Sim::Failed(w) => (
            ErrorCode::SwapReverted,
            format!(
                "обмен шагом V2 исполнителя откатился не проверкой K пары (перевод монеты, требование исполнителя): {w}"
            ),
        ),
        Sim::Underpaid(w) => (
            ErrorCode::QuoteMismatch,
            format!("пара отдала меньше формулы V2 (dex.quoter_max_diff_pct): {w}"),
        ),
    };
    Err(ctx.err(code, format!("{t}: {why} (решение 280)")))
}

/// Fee и getReserves фиксируются внутри одного запуска EVM. При изменении ставки
/// повторяем только этот пул, сохраняя канонический хеш и штатную проверку исполнения.
async fn current_rate_simulation(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
    number: u64,
) -> GatewayResult<(Vec<simulate::CallResult>, (U256, U256))> {
    use super::super::pools::v2_live_fee::{Market, Rate};
    #[derive(serde::Deserialize)]
    struct Parent {
        hash: B256,
        number: U64,
    }
    let parent: Parent = rpc::call(
        ctx,
        "eth_getBlockByNumber",
        json!([U64::from(number), false]),
    )
    .await?;
    if parent.number != U64::from(number) {
        return Err(ctx.err(ErrorCode::BadResponse, "Неверная высота блока V2"));
    }
    if e.family.variant==super::super::pools::family_variant::Variant::DiamondV2 {
        let at=json!({"blockHash":parent.hash,"requireCanonical":true});
        super::super::pools::diamond::check(ctx,e.family.id,false,&at).await?;
    }
    let rate = Rate::configured(
        ctx,
        e.family,
        Market {
            pool: pair(ctx, e)?,
            tokens: (e.pool.token0, e.pool.token1),
        },
    )?;
    let (to, input) = rate.call();
    let mut fresh = e.clone();
    for _ in 0..2 {
        let (mut list, over, ratio) = calls(ctx, &fresh, token_in, amount)?;
        list.insert(
            0,
            Call {
                from: owner(),
                to,
                input: input.clone(),
            },
        );
        let candy = if let crate::core::types::PoolState::CandySwap(state) = &e.pool.state {
            list.insert(0,Call {from:owner(),to:pair(ctx,e)?,input:Bytes::from_static(&super::super::math::candy::PAYOUT_RATE_SELECTOR)});
            Some(state.payout_bps)
        } else {None};
        let mut results =
            simulate::run_anchored(ctx, &list, &[], &[over], number.into(), Some(parent.hash))
                .await?;
        if let Some(expected)=candy {
            let payout=results.remove(0);
            if !payout.ok() || payout.return_data.len()!=32 || U256::from_be_slice(&payout.return_data)!=U256::from(expected) {
                return Err(ctx.err(ErrorCode::StaleData,"CandySwap: ставка выплаты изменилась к блоку сверки"));
            }
        }
        let observed = results.remove(0);
        if observed.status != U64::from(1) {
            return Err(ctx.err(ErrorCode::BadResponse, "Вызов ставки V2 отклонён"));
        }
        let pips = rate.decode(&observed.return_data)?;
        if fresh.pool.fee_pips == Some(pips) {
            return Ok((results, ratio));
        }
        fresh.pool.fee_pips = Some(pips);
    }
    Err(ctx.err(
        ErrorCode::BadResponse,
        "Ставка V2 не воспроизвелась на одном каноническом блоке",
    ))
}
