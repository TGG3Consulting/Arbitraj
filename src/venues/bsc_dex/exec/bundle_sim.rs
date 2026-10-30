//! Файл: venues/bsc_dex/exec/bundle_sim.rs
//! Что делает: чистая подготовка бандла свопов к отправке (решение 84) — без сети, тестируется на крафт-итогах:
//! (1) build_sim — разметка одной симуляции eth_simulateV1 всего упорядоченного бандла: перед каждым свопом
//! вставляется balanceOf выходного токена (его значение в симуляции = баланс кошелька ПОСЛЕ предыдущих групп →
//! КУМУЛЯТИВНАЯ база BALANCE_CHECK той группы, ревью #80 MEDIUM), сам своп — с базой 0; (2) collect — лимиты газа
//! (gasUsed × надбавка) по транзакциям и базы BALANCE_CHECK по свопам из итогов симуляции; (3) sign_txs — подпись
//! всех транзакций бандла по очереди ключом кошелька (nonce уже проставлены на 80.8a, база BALANCE_CHECK — на своп).
//! Сеть/очередь/отправка — exec/bundle_send.rs.
//! Кто вызывает: exec/bundle_send.rs; тесты.
//! От чего зависит: exec/bundle.rs (Bundle/BundleTx), exec/ur.rs, check/simulate.rs (Call/CallResult), node/abi.rs
//! (IERC20), send/tx.rs (legacy), wallet/signer.rs (sign_with/SignedTx).
//! Настройки: своих нет (надбавка газа send.gas_limit_margin_pct читается вызывающим).

use std::collections::HashMap;

use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::check::simulate::{Call, CallResult};
use super::super::ctx::Ctx;
use super::super::node::abi::IERC20;
use super::super::send::tx;
use super::super::wallet::signer::{self, SignedTx};
use super::bundle::{Bundle, BundleTx};
use super::fork_swap;
use super::ur;

/// Разметка симуляции бандла: вызовы, суммы BNB и где итог каждой транзакции и где balanceOf перед свопом.
pub struct SimPlan {
    pub calls: Vec<Call>,
    pub values: Vec<(usize, U256)>,
    /// Индекс основного вызова каждой транзакции бандла (по её порядку).
    pub main: Vec<usize>,
    /// (индекс транзакции-свопа в бандле, индекс её balanceOf в симуляции).
    pub bal: Vec<(usize, usize)>,
}

/// Строит симуляцию бандла: перед каждым свопом — balanceOf выходного токена (его значение = база BALANCE_CHECK той
/// группы по состоянию ПОСЛЕ предыдущих, ревью #80), сам своп — с базой 0 (порог проходит: кошелёк уже получил ≥ min);
/// разрешения — как есть. Чистая. Ошибок нет.
pub fn build_sim(bundle: &Bundle, wallet: Address) -> SimPlan {
    let bal_input = Bytes::from(IERC20::balanceOfCall { owner: wallet }.abi_encode());
    let mut calls = Vec::new();
    let mut values = Vec::new();
    let mut main = Vec::with_capacity(bundle.txs.len());
    let mut bal = Vec::new();
    for (i, t) in bundle.txs.iter().enumerate() {
        match t {
            BundleTx::Approve { to, data, .. } => {
                calls.push(Call {
                    from: wallet,
                    to: *to,
                    input: data.clone(),
                });
                main.push(calls.len() - 1);
            }
            BundleTx::Swap { plan, .. } => {
                calls.push(Call {
                    from: wallet,
                    to: plan.token_out,
                    input: bal_input.clone(),
                });
                bal.push((i, calls.len() - 1));
                let sidx = calls.len();
                calls.push(Call {
                    from: wallet,
                    to: plan.router,
                    input: plan.data(U256::ZERO),
                });
                if !plan.value().is_zero() {
                    values.push((sidx, plan.value()));
                }
                main.push(sidx);
            }
            // Своп форка: без balanceOf-базы — защита встроена в amountOutMin самого роутера (FeeOnTransfer меряет
            // дельту баланса получателя-кошелька ≥ min, решение 40, кумулятивно поверх прошлых групп). Вход — ERC-20
            // (value 0). Выход лёг на кошелёк до следующих групп, поэтому их база BALANCE_CHECK его учтёт.
            BundleTx::ForkSwap {
                router,
                path,
                amount_in,
                min_out,
                to,
                deadline,
                ..
            } => {
                let sidx = calls.len();
                calls.push(Call {
                    from: wallet,
                    to: *router,
                    input: fork_swap::swap_data(*amount_in, *min_out, path, *to, *deadline),
                });
                main.push(sidx);
            }
        }
    }
    SimPlan {
        calls,
        values,
        main,
        bal,
    }
}

/// Лимиты газа по транзакциям и базы BALANCE_CHECK по свопам из итогов симуляции. Чистая (тестируется на крафт-итогах).
/// Ошибки: NoRoute/StaleData (шаг откатился), BadResponse (база не читается / газ не помещается).
pub fn collect(
    ctx: &Ctx,
    res: &[CallResult],
    plan: &SimPlan,
    margin: Decimal,
) -> GatewayResult<(Vec<u64>, HashMap<usize, U256>)> {
    let mut limits = Vec::with_capacity(plan.main.len());
    for (i, &ci) in plan.main.iter().enumerate() {
        let r = &res[ci];
        if !r.ok() {
            let text = r.error.as_ref().map(|e| e.message.as_str()).unwrap_or("");
            let code = if ur::deadline_passed(&r.return_data, text) {
                ErrorCode::StaleData
            } else {
                ErrorCode::NoRoute
            };
            return Err(ctx.err(
                code,
                format!("симуляция бандла: транзакция {i} откатилась — {}", r.why()),
            ));
        }
        let g =
            Decimal::from(r.gas_used.to::<u64>()) * (Decimal::ONE + margin / Decimal::ONE_HUNDRED);
        limits.push(u64::try_from(g.ceil()).map_err(|_| {
            ctx.err(
                ErrorCode::BadResponse,
                format!("лимит газа {g} не помещается"),
            )
        })?);
    }
    let mut bases = HashMap::new();
    for &(tx_i, bi) in &plan.bal {
        let r = &res[bi];
        if !r.ok() {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                format!(
                    "симуляция бандла: balanceOf перед свопом {tx_i} откатился — {}",
                    r.why()
                ),
            ));
        }
        let base = U256::try_from_be_slice(&r.return_data).ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                "симуляция бандла: balanceOf вернул не 32 байта — база BALANCE_CHECK не прочитана",
            )
        })?;
        bases.insert(tx_i, base);
    }
    Ok((limits, bases))
}

/// Подписывает транзакции бандла по очереди (nonce из бандла, цена и лимит газа — на транзакцию, база BALANCE_CHECK —
/// на своп по индексу). Чистая: без узла, подписант передан. Ошибки: Internal (число лимитов ≠ транзакциям, нет базы
/// свопа, подпись), BadResponse (цена газа).
pub fn sign_txs(
    ctx: &Ctx,
    signer: &PrivateKeySigner,
    bundle: &Bundle,
    price: U256,
    limits: &[u64],
    bases: &HashMap<usize, U256>,
) -> GatewayResult<Vec<SignedTx>> {
    if limits.len() != bundle.txs.len() {
        return Err(ctx.err(
            ErrorCode::Internal,
            "число лимитов газа не равно числу транзакций бандла",
        ));
    }
    let mut out = Vec::with_capacity(bundle.txs.len());
    for (i, (t, &gas)) in bundle.txs.iter().zip(limits).enumerate() {
        let (to, input, value) = match t {
            BundleTx::Approve { to, data, .. } => (*to, data.clone(), U256::ZERO),
            BundleTx::Swap { plan, .. } => {
                let base = *bases.get(&i).ok_or_else(|| {
                    ctx.err(ErrorCode::Internal, "нет базы BALANCE_CHECK свопа бандла")
                })?;
                (plan.router, plan.data(base), plan.value())
            }
            // Форк: calldata фиксирована (amountOutMin встроен), базы BALANCE_CHECK нет; вход ERC-20 (value 0).
            BundleTx::ForkSwap {
                router,
                path,
                amount_in,
                min_out,
                to,
                deadline,
                ..
            } => (
                *router,
                fork_swap::swap_data(*amount_in, *min_out, path, *to, *deadline),
                U256::ZERO,
            ),
        };
        let legacy = tx::legacy(ctx, t.nonce(), price, gas, to, value, input)?;
        let signed = signer::sign_with(signer, legacy).map_err(|e| {
            ctx.err(
                ErrorCode::Internal,
                format!("подпись транзакции бандла: {e}"),
            )
        })?;
        out.push(signed);
    }
    Ok(out)
}
