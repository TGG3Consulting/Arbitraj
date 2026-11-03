//! Файл: venues/bsc_dex/exec/executor/deploy.rs
//! Что делает: развёртывание своего исполнителя ArbExecutor с кошелька бота (решение 122, часть X) — ТОЛЬКО по кнопке
//! человека с подтверждением (действие с деньгами): оценка (eth_estimateGas транзакции создания, цена газа как у сделки,
//! стоимость в BNB и в USDT по пулу WBNB — USDT реестра) и отправка транзакции создания через очередь кошелька
//! (решение 45) существующими каналами (exec/txs.rs). Байткод — только из репозитория (contracts/out/, хеш сверен,
//! code.rs); подтверждение несёт SHA-256, который видел человек, — не совпал — не отправляем. После блока: код по адресу
//! (eth_getCode) сверяется с ArbExecutor.bin-runtime. Адрес в exec.executor_address Тигран вписывает сам.
//! Кто вызывает: venue.rs (контракт dex_executor_quote / dex_executor_deploy), exec/executor/dry.rs (next_nonce).
//! От чего зависит: exec/executor/code.rs, exec/txs.rs, send/{mod, tx}.rs, quote/{gas, rate}.rs, wallet/{signer, nonce}.rs,
//! node/rpc.rs, num.rs.
//! Настройки: через вызываемые — send.*, channel.*, dex.native_wrapped (цена в USDT), venue.quote_asset.
//! Уведомление: executor.deployed (журнал и Telegram, если событие включено).
//!
//! Адрес контракта CREATE = keccak256(rlp([отправитель, nonce]))[12:] — Ethereum Yellow Paper, раздел 7 (alloy
//! Address::create); eth_getTransactionCount / eth_getCode — execution-apis 16caee27 src/eth/state.yaml.

use alloy_primitives::{Address, Bytes, TxKind, U64, U256};
use serde_json::json;

use crate::core::types::{ExecutorDeployQuote, ExecutorDeployed, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::super::super::ctx::Ctx;
use super::super::super::node::rpc;
use super::super::super::num;
use super::super::super::quote::{Quoting, rate};
use super::super::super::send::{self, tx};
use super::super::super::wallet::nonce::Nonces;
use super::super::super::wallet::signer;
use super::super::txs::{self, Env};
use super::code;

/// Nonce кошелька по последнему блоку. Ошибки: узел.
pub async fn next_nonce(ctx: &Ctx, wallet: Address) -> GatewayResult<u64> {
    let n: U64 = rpc::call(ctx, "eth_getTransactionCount", json!([wallet, "latest"])).await?;
    Ok(n.to::<u64>())
}

/// Код создания после сверки хеша. Ошибки: Internal (артефакт не сверен).
fn bytecode(ctx: &Ctx) -> GatewayResult<Bytes> {
    code::creation().map_err(|w| ctx.err(ErrorCode::Internal, w))
}

/// Оценка развёртывания — ничего не отправляет. Ошибки: ключ кошелька, каналы, узел (оценка газа откатилась — текст).
pub async fn quote(q: &Quoting, env: &Env) -> GatewayResult<ExecutorDeployQuote> {
    let ctx = &q.ctx;
    let wallet = signer::address(ctx)?;
    let bin = bytecode(ctx)?;
    let nonce = next_nonce(ctx, wallet).await?;
    let gas = tx::limit(ctx, wallet, TxKind::Create, &bin, U256::ZERO).await?;
    let ready = send::select(ctx, &env.stats)?;
    let price = env
        .gas
        .wei(ctx, &env.heads)
        .await?
        .max(send::gas_floor(&ready));
    let wei = price.saturating_mul(U256::from(gas));
    let cost_bnb = num::to_decimal(wei, num::BNB_DECIMALS).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("газ {wei} wei не переводится в BNB"),
        )
    })?;
    let (cost_usdt, note) = match usdt_of(q, wei).await {
        Ok(v) => (Some(v), None),
        Err(e) => (
            None,
            Some(format!("цена в USDT не посчитана: {}", e.message)),
        ),
    };
    Ok(ExecutorDeployQuote {
        venue: ctx.venue(),
        wallet,
        nonce,
        address: wallet.create(nonce),
        gas,
        gas_price_wei: price.to_string(),
        cost_bnb,
        cost_usdt,
        bytecode_sha256: code::creation_sha256(),
        solc: code::solc().to_string(),
        bytecode_len: bin.len(),
        note,
    })
}

/// Стоимость в USDT: wei BNB → WBNB по лучшему пулу WBNB — USDT реестра (quote/rate.rs). Ошибки: NotConfigured
/// (dex.native_wrapped), NoRoute (пула нет), BadResponse.
async fn usdt_of(q: &Quoting, wei: U256) -> GatewayResult<rust_decimal::Decimal> {
    let ctx = &q.ctx;
    let usdt = q.tokens.usdt(ctx)?;
    let units = rate::to_usdt(ctx, &q.reg, usdt, ctx.addr("dex.native_wrapped")?, wei)?;
    let dec = q.tokens.decimals(ctx, usdt).await?;
    num::to_decimal(units, dec)
        .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "сумма USDT не переводится в число"))
}

/// Развёртывание. Принимает: котировку, окружение отправки, очередь кошелька, SHA-256 байткода, который подтвердил
/// человек. Возвращает: итог (адрес, транзакция, совпал ли код). Ошибки: QuoteMismatch (байткод не тот, что
/// подтверждён), Internal (артефакт), ключ, каналы, узел, OutcomeUnknown (исход транзакции неизвестен).
pub async fn deploy(
    q: &Quoting,
    env: &Env,
    nonces: &Nonces,
    confirmed_sha256: &str,
) -> GatewayResult<ExecutorDeployed> {
    let ctx = &q.ctx;
    let bin = bytecode(ctx)?;
    if !confirmed_sha256.eq_ignore_ascii_case(&code::creation_sha256()) {
        let why = "байткод исполнителя не тот, что подтверждён (SHA-256 не совпал) — запросите оценку заново";
        return Err(ctx.err(ErrorCode::QuoteMismatch, why));
    }
    let slot = nonces.acquire(ctx).await?;
    let nonce = slot.nonce(ctx).await?;
    let address = slot.from.create(nonce);
    let d = txs::send(env, &slot, TxKind::Create, bin, U256::ZERO, false, None).await?;
    drop(slot);
    let code_ok = d.chain.outcome == TxOutcome::Included && on_chain_ok(ctx, address).await?;
    let text = match (d.chain.outcome, code_ok) {
        (TxOutcome::Included, true) => format!(
            "Исполнитель развёрнут: {address:#x} (tx {}). Код в сети совпал с репозиторием. Адрес записан у кошелька \
             (экран «Маппинг кошельков», решение 132) — маршруты через разные DEX идут одной транзакцией",
            d.chain.hash
        ),
        (TxOutcome::Included, false) => format!(
            "Транзакция {} в блоке, но код по адресу {address:#x} не совпал с contracts/out/ArbExecutor.bin-runtime — \
             адрес не использовать, нужна ручная проверка",
            d.chain.hash
        ),
        (o, _) => format!(
            "Развёртывание не состоялось: {o:?}, tx {} (контракт по адресу {address:#x} не создан)",
            d.chain.hash
        ),
    };
    let level = if code_ok { Level::Info } else { Level::Error };
    ctx.log(level, "dex.executor", text.clone(), None).await;
    env.stats.emit(MarketEvent::Notice {
        venue: ctx.venue(),
        code: "executor.deployed".into(),
        text: text.clone(),
    });
    Ok(ExecutorDeployed {
        venue: ctx.venue(),
        address,
        chain: d.chain,
        code_ok,
        text,
    })
}

/// Код по адресу совпал с ArbExecutor.bin-runtime. Ошибки: Internal (артефакт), узел.
async fn on_chain_ok(ctx: &Ctx, address: Address) -> GatewayResult<bool> {
    let want = code::runtime().map_err(|w| ctx.err(ErrorCode::Internal, w))?;
    let have: Bytes = rpc::call(ctx, "eth_getCode", json!([address, "latest"])).await?;
    Ok(have == want)
}
