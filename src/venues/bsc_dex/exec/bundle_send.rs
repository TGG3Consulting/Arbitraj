//! Файл: venues/bsc_dex/exec/bundle_send.rs
//! Что делает: отправка собранного бандла свопов (exec/bundle.rs, решение 84, фаза 80.8b) — самостоятельная функция;
//! врезку в боевой путь (exec/trade.rs) и флип executable делает 80.8c. Шаги: (1) симуляция всего упорядоченного
//! бандла eth_simulateV1 на своём узле (check/simulate.rs) — видит эффект ноги N на ногу N+1 (межтранзакционная
//! передача) и валидирует порядок; откат любого шага → не отправляем; попутно считает газ каждой транзакции и
//! КУМУЛЯТИВНУЮ базу BALANCE_CHECK каждого свопа (ревью #80 MEDIUM): перед свопом вставляется balanceOf выходного
//! токена, его значение в симуляции = баланс кошелька ПОСЛЕ предыдущих групп, поэтому порог «реально получено ≥ min»
//! у каждой группы отдельный (защита от fee-on-transfer не ослабляется выходом прошлых групп того же токена);
//! (2) цена газа — как у одиночной (eth_gasPrice с надбавкой, не ниже минимума каналов); (3) подпись по очереди
//! (nonce проставлены на 80.8a) ключом кошелька (wallet/signer); (4) отправка одним бандлом через бандл-каналы
//! (send/mod.rs dispatch_slice → eth_sendBundle, revertingTxHashes пуст = всё-или-ничего, maxBlockNumber = голова +
//! send.bundle_max_blocks); (5) ожидание блока своим узлом (решение 42) без отмен — приватный бандл истекает по
//! maxBlockNumber (газ не тратится, решение 40). Очередь кошелька (Hold/Slot) и settle — в боевой врезке 80.8c.
//! Классику/одиночную отправку не трогает.
//! Кто вызывает: exec/trade.rs (80.8c); тесты.
//! От чего зависит: exec/bundle.rs, exec/plan.rs, exec/txs.rs (Env), check/simulate.rs, node/abi.rs (IERC20),
//! send/{mod,tx,receipt,track_util}.rs, wallet/signer.rs, node/heads.rs, exec/ur.rs.
//! Настройки: send.gas_limit_margin_pct, send.bundle_max_blocks, send.max_wait_blocks, send.queue_max_wait_s,
//! feed.stale_after_ms [bsc_dex] (через SendCfg); цена газа — через quote/gas.rs; каналы — channel.* (bundle).

use std::collections::HashMap;
use std::time::Instant;

use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};
use rust_decimal::Decimal;

use crate::core::types::TxOutcome;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::check::simulate::{self, Call, CallResult};
use super::super::ctx::Ctx;
use super::super::node::abi::IERC20;
use super::super::node::heads::Heads;
use super::super::send::track_util::{self, Quiet, Tracked};
use super::super::send::tx::SendCfg;
use super::super::send::{self, receipt, tx};
use super::super::wallet::signer::{self, SignedTx};
use super::bundle::{Bundle, BundleTx};
use super::txs::Env;
use super::ur;

/// Итог отправки бандла.
pub struct Sent {
    /// Хеши транзакций бандла по порядку.
    pub hashes: Vec<B256>,
    /// Каналы, где бандл может быть (приняли или не ответили ясно).
    pub channels: Vec<String>,
    /// Попадание в блок по своему узлу.
    pub tracked: Tracked,
}

/// Разметка симуляции бандла: вызовы, суммы BNB и где итог каждой транзакции и где balanceOf перед свопом.
pub struct SimPlan {
    pub calls: Vec<Call>,
    values: Vec<(usize, U256)>,
    /// Индекс основного вызова каждой транзакции бандла (по её порядку).
    pub main: Vec<usize>,
    /// (индекс транзакции-свопа в бандле, индекс её balanceOf в симуляции).
    pub bal: Vec<(usize, usize)>,
}

/// Отправляет собранный бандл. Принимает: окружение отправки, бандл (nonce уже проставлены на 80.8a). Возвращает: итог.
/// Ошибки: NotConfigured (настройки, каналы, нет бандл-канала), NoRoute/StaleData (симуляция откатилась), ChannelRejected,
/// узел, Internal (пустой бандл, подпись).
pub async fn send(env: &Env, bundle: &Bundle) -> GatewayResult<Sent> {
    let ctx = &env.ctx;
    let cfg = SendCfg::read(ctx)?;
    let ready = send::select(ctx, &env.stats)?;
    let wallet = wallet_of(ctx, bundle)?;
    let price = env
        .gas
        .wei(ctx, &env.heads)
        .await?
        .max(send::gas_floor(&ready));
    let (at, _now) = env.heads.at(ctx).await?;
    let margin = ctx.dec("send.gas_limit_margin_pct")?;
    let (limits, bases) = prepare(ctx, bundle, wallet, at, margin).await?;
    let s = signer::signer(ctx)?;
    let signed = sign_txs(ctx, &s, bundle, price, &limits, &bases)?;
    let head = env.heads.fresh(ctx)?;
    let max_block = head.number + cfg.bundle_max;
    let raw: Vec<Bytes> = signed.iter().map(|t| t.raw.clone()).collect();
    let dispatched = send::dispatch_slice(ctx, &env.stats, &ready, &raw, max_block).await?;
    let hashes: Vec<B256> = signed.iter().map(|t| t.hash).collect();
    let tracked = confirm(ctx, &env.heads, &cfg, &hashes, max_block).await;
    Ok(Sent {
        hashes,
        channels: dispatched.all(),
        tracked,
    })
}

/// Кошелёк бандла (отправитель всех транзакций) — из первого свопа. Ошибки: Internal — в бандле нет свопа.
fn wallet_of(ctx: &Ctx, bundle: &Bundle) -> GatewayResult<Address> {
    bundle
        .txs
        .iter()
        .find_map(|t| match t {
            BundleTx::Swap { plan, .. } => Some(plan.wallet),
            _ => None,
        })
        .ok_or_else(|| ctx.err(ErrorCode::Internal, "бандл без свопа — нечего отправлять"))
}

/// Газ каждой транзакции и база BALANCE_CHECK каждого свопа — одной симуляцией всего бандла. Принимает: бандл,
/// кошелёк, базовый блок, надбавку газа (%). Возвращает: лимиты газа по транзакциям и базы по индексам транзакций-свопов.
/// Ошибки: NoRoute/StaleData (шаг откатился), BadResponse (база/число), узел.
async fn prepare(
    ctx: &Ctx,
    bundle: &Bundle,
    wallet: Address,
    at: u64,
    margin: Decimal,
) -> GatewayResult<(Vec<u64>, HashMap<usize, U256>)> {
    let plan = build_sim(bundle, wallet);
    let res = simulate::run_valued(ctx, &plan.calls, &plan.values, at).await?;
    collect(ctx, &res, &plan, margin)
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

/// Ждёт попадания бандла в блок своим узлом (решение 42) без отмен: приватный бандл с maxBlockNumber истекает сам —
/// прошёл предел блока, квитанции нет → NotIncluded (газ не потрачен, решение 40). Принимает: хеши бандла, предел блока.
/// Возвращает: итог. Ошибок нет — сбои узла повторяются; итог не установлен за send.queue_max_wait_s → Pending.
async fn confirm(
    ctx: &Ctx,
    heads: &Heads,
    cfg: &SendCfg,
    hashes: &[B256],
    max_block: u64,
) -> Tracked {
    let started = Instant::now();
    let mut rx = track_util::heads_rx(heads);
    let repr = hashes.first().copied().unwrap_or_default();
    let mut quiet = Quiet::new(ctx, repr);
    loop {
        if started.elapsed() > cfg.queue_max_wait {
            let why = format!(
                "итог бандла не установлен за send.queue_max_wait_s: {}",
                quiet.reason()
            );
            return Tracked::bare(TxOutcome::Pending, Some(why));
        }
        let Some(head) = track_util::next_head(&mut rx, cfg, &mut quiet).await else {
            continue;
        };
        match receipt::find(ctx, hashes).await {
            Ok(Some((_, r))) => {
                let ok = r.status == alloy_primitives::U64::from(1u8);
                return Tracked {
                    outcome: if ok {
                        TxOutcome::Included
                    } else {
                        TxOutcome::Reverted
                    },
                    receipt: Some(r),
                    cancel: None,
                    unresolved: None,
                };
            }
            Ok(None) => quiet.ok("receipt").await,
            Err(e) => {
                quiet
                    .warn(
                        "receipt",
                        format!("квитанция бандла не прочитана: {}", e.message),
                    )
                    .await;
                continue;
            }
        }
        if head > max_block {
            return Tracked::bare(TxOutcome::NotIncluded, None);
        }
    }
}
