//! Файл: venues/bsc_dex/exec/bundle_send.rs
//! Что делает: отправка собранного бандла свопов (exec/bundle.rs, решение 84, фаза 80.8b) — самостоятельная функция,
//! в боевой путь (exec/trade.rs) и флип executable не врезана (это 80.8c). Шаги: (1) газ каждой транзакции — симуляцией
//! всего упорядоченного бандла eth_simulateV1 на своём узле (check/simulate.rs): симуляция видит эффект ноги N на
//! ногу N+1 (межтранзакционная передача) и валидирует порядок; откат любого шага — бандл не отправляем; (2) цена газа —
//! как у одиночной отправки (eth_gasPrice с надбавкой, не ниже минимума каналов); (3) подпись каждой транзакции строго
//! по очереди (nonce проставлен на 80.8a) ключом кошелька (wallet/signer); (4) отправка всех транзакций одним бандлом
//! через существующие бандл-каналы (send/mod.rs dispatch_slice → eth_sendBundle, revertingTxHashes пуст = всё-или-ничего,
//! maxBlockNumber = голова + send.bundle_max_blocks); (5) ожидание попадания в блок своим узлом (решение 42) без отмен —
//! приватный бандл с maxBlockNumber сам истекает (газ не тратится, решение 40). Очередь кошелька (Hold/Slot) и settle —
//! в боевой врезке 80.8c. Классику/одиночную отправку не трогает.
//! Кто вызывает: exec/trade.rs (80.8c); тесты.
//! От чего зависит: exec/bundle.rs, exec/plan.rs, exec/txs.rs (Env), check/simulate.rs, send/{mod,tx,receipt,track_util}.rs,
//! wallet/{signer,balances}.rs, node/heads.rs, exec/ur.rs.
//! Настройки: send.gas_limit_margin_pct, send.bundle_max_blocks, send.max_wait_blocks, send.queue_max_wait_s,
//! feed.stale_after_ms [bsc_dex] (через SendCfg); цена газа — через quote/gas.rs; каналы — channel.* (bundle).

use std::collections::HashMap;
use std::time::Instant;

use alloy::signers::local::PrivateKeySigner;
use alloy_primitives::{Address, B256, Bytes, U256};
use rust_decimal::Decimal;

use crate::core::types::TxOutcome;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::check::simulate::{self, Call, CallResult};
use super::super::ctx::Ctx;
use super::super::node::heads::Heads;
use super::super::send::track_util::{self, Quiet, Tracked};
use super::super::send::tx::SendCfg;
use super::super::send::{self, receipt, tx};
use super::super::wallet::balances;
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

/// Отправляет собранный бандл. Принимает: окружение отправки, бандл (nonce уже проставлены на 80.8a). Возвращает: итог.
/// Ошибки: NotConfigured (настройки, каналы, нет бандл-канала), NoRoute/StaleData (симуляция откатилась), ChannelRejected
/// (ни один канал не принял), узел, Internal (пустой бандл, подпись).
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
    let bases = bases(ctx, bundle).await?;
    let (at, _now) = env.heads.at(ctx).await?;
    let margin = ctx.dec("send.gas_limit_margin_pct")?;
    let limits = simulate_gas(ctx, bundle, wallet, &bases, at, margin).await?;
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

/// Баланс кошелька по каждому выходному токену свопов (для BALANCE_CHECK, читается до бандла). Ошибки: узел.
async fn bases(ctx: &Ctx, bundle: &Bundle) -> GatewayResult<HashMap<Address, U256>> {
    let mut m = HashMap::new();
    for t in &bundle.txs {
        if let BundleTx::Swap { plan, .. } = t
            && !m.contains_key(&plan.token_out)
        {
            let b = balances::token_units(ctx, plan.token_out, plan.wallet).await?;
            m.insert(plan.token_out, b);
        }
    }
    Ok(m)
}

/// Газ каждой транзакции бандла симуляцией всего упорядоченного бандла eth_simulateV1 (видит межтранзакционную
/// передачу). Принимает: бандл, кошелёк, базы балансов, базовый блок, надбавку газа (%). Возвращает: лимит газа на
/// транзакцию в порядке бандла. Ошибки: NoRoute/StaleData (шаг откатился), BadResponse, узел.
async fn simulate_gas(
    ctx: &Ctx,
    bundle: &Bundle,
    wallet: Address,
    bases: &HashMap<Address, U256>,
    at: u64,
    margin: Decimal,
) -> GatewayResult<Vec<u64>> {
    let mut calls = Vec::with_capacity(bundle.txs.len());
    let mut values = Vec::new();
    for (i, t) in bundle.txs.iter().enumerate() {
        let (to, input, value) = tx_parts(ctx, t, bases)?;
        if !value.is_zero() {
            values.push((i, value));
        }
        calls.push(Call {
            from: wallet,
            to,
            input,
        });
    }
    let res = simulate::run_valued(ctx, &calls, &values, at).await?;
    gas_from(ctx, &res, margin)
}

/// Лимиты газа из итогов симуляции: любой откат — ошибка (срок вышел → StaleData, иначе NoRoute), иначе gasUsed с
/// надбавкой. Принимает: итоги по транзакциям, надбавку (%). Ошибки: NoRoute/StaleData (откат), BadResponse (число).
pub fn gas_from(ctx: &Ctx, res: &[CallResult], margin: Decimal) -> GatewayResult<Vec<u64>> {
    let mut out = Vec::with_capacity(res.len());
    for (i, r) in res.iter().enumerate() {
        if !r.ok() {
            let text = r.error.as_ref().map(|e| e.message.as_str()).unwrap_or("");
            if ur::deadline_passed(&r.return_data, text) {
                return Err(ctx.err(
                    ErrorCode::StaleData,
                    format!("симуляция бандла: шаг {i} — срок транзакции вышел (увеличьте dex.deadline_s): {}", r.why()),
                ));
            }
            return Err(ctx.err(
                ErrorCode::NoRoute,
                format!("симуляция бандла: шаг {i} откатился — {}", r.why()),
            ));
        }
        let g =
            Decimal::from(r.gas_used.to::<u64>()) * (Decimal::ONE + margin / Decimal::ONE_HUNDRED);
        out.push(u64::try_from(g.ceil()).map_err(|_| {
            ctx.err(
                ErrorCode::BadResponse,
                format!("лимит газа {g} не помещается"),
            )
        })?);
    }
    Ok(out)
}

/// (адрес, данные, сумма BNB) транзакции бандла. Для свопа данные достраиваются базой баланса (BALANCE_CHECK).
/// Ошибки: Internal — нет базы выходного токена.
fn tx_parts(
    ctx: &Ctx,
    t: &BundleTx,
    bases: &HashMap<Address, U256>,
) -> GatewayResult<(Address, Bytes, U256)> {
    match t {
        BundleTx::Approve { to, data, .. } => Ok((*to, data.clone(), U256::ZERO)),
        BundleTx::Swap { plan, .. } => {
            let base = *bases.get(&plan.token_out).ok_or_else(|| {
                ctx.err(
                    ErrorCode::Internal,
                    "нет базы баланса выходного токена для BALANCE_CHECK",
                )
            })?;
            Ok((plan.router, plan.data(base), plan.value()))
        }
    }
}

/// Подписывает транзакции бандла по порядку (nonce из бандла, цена и лимит газа — на транзакцию). Чистая: без узла,
/// подписант передан. Принимает: подписанта, бандл, цену газа, лимиты газа (по транзакции), базы балансов.
/// Ошибки: Internal (число лимитов ≠ числу транзакций, нет базы, подпись), BadResponse (цена газа).
pub fn sign_txs(
    ctx: &Ctx,
    signer: &PrivateKeySigner,
    bundle: &Bundle,
    price: U256,
    limits: &[u64],
    bases: &HashMap<Address, U256>,
) -> GatewayResult<Vec<SignedTx>> {
    if limits.len() != bundle.txs.len() {
        return Err(ctx.err(
            ErrorCode::Internal,
            "число лимитов газа не равно числу транзакций бандла",
        ));
    }
    let mut out = Vec::with_capacity(bundle.txs.len());
    for (t, &gas) in bundle.txs.iter().zip(limits) {
        let (to, input, value) = tx_parts(ctx, t, bases)?;
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
