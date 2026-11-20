//! Файл: venues/bsc_dex/exec/executor/deploy.rs
//! Что делает: развёртывание своего исполнителя ArbExecutor с кошелька бота (решение 122, часть X) — ТОЛЬКО по кнопке
//! человека с подтверждением (действие с деньгами): оценка (eth_estimateGas транзакции создания, цена газа как у сделки,
//! стоимость в BNB и в USDT по пулу WBNB — USDT реестра) и отправка транзакции создания через очередь кошелька
//! (решение 45) существующими каналами (exec/txs.rs). Байткод — только из репозитория (contracts/out/, хеш сверен,
//! code.rs); подтверждение несёт SHA-256, который видел человек, — не совпал — не отправляем. Задача t114 (3в):
//! подтверждение несёт и адрес обёртки (аргумент конструктора), который человек видел в окне: хеш считается по коду
//! создания БЕЗ аргумента, поэтому без этой сверки смена dex.native_wrapped между оценкой и подтверждением ушла бы в
//! конструктор молча (навсегда — сеттера нет). Не совпал с текущей dex.native_wrapped — не отправляем. После блока: код по адресу
//! (eth_getCode) сверяется с ArbExecutor.bin-runtime. Адрес в exec.executor_address Тигран вписывает сам.
//! Решение 399 / 400: пока в записи симуляции на живых пулах у текущей сборки (v3; решение 441 — v4 после пересборки
//! contracts/build.sh, запись сверяется с её кодом — live_proof::verdicts_current; решение 461 — запись сети площадки
//! кошелька, dex.evm_chain_id) не принят ни один класс шага (deploy_gate.rs, live_proof.rs), развёртывание запрещено: deploy отказывает (NotApplicable, 400) до ключа, nonce и
//! сети; оценка отдаёт причину в поле warning — интерфейс показывает её и не даёт отправить; поле classes — принятые и
//! непринятые классы. Обходной настройки нет.
//! Кто вызывает: venue.rs (контракт dex_executor_quote / dex_executor_deploy), exec/executor/dry.rs (next_nonce).
//! От чего зависит: exec/executor/{code, deploy_gate, live_proof}.rs, exec/txs.rs, send/{mod, tx}.rs, quote/{gas, rate}.rs, wallet/{signer, nonce}.rs,
//! node/rpc.rs, num.rs.
//! Настройки: dex.native_wrapped — адрес обёртки, аргумент конструктора v2 (ревью 272, А1: пишется в исполнитель один раз,
//! сеттера нет; не задана — развёртывания нет, NotConfigured), он же — цена в USDT; через вызываемые — send.*, channel.*,
//! venue.quote_asset.
//! Уведомление: executor.deployed (журнал и Telegram, если событие включено).
//!
//! Адрес контракта CREATE = keccak256(rlp([отправитель, nonce]))[12:] — Ethereum Yellow Paper, раздел 7 (alloy
//! Address::create); eth_getTransactionCount / eth_getCode — execution-apis 16caee27 src/eth/state.yaml.

use alloy_primitives::{Address, Bytes, TxKind, U64, U256};
use serde_json::json;

use crate::core::types::{ExecutorClassProof, ExecutorDeployQuote, ExecutorDeployed, TxOutcome};
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
use super::{code, deploy_gate, live_proof};

/// Nonce кошелька по последнему блоку. Ошибки: узел.
pub async fn next_nonce(ctx: &Ctx, wallet: Address) -> GatewayResult<u64> {
    let n: U64 = rpc::call(ctx, "eth_getTransactionCount", json!([wallet, "latest"])).await?;
    Ok(n.to::<u64>())
}

/// Код транзакции создания после сверки хеша: код создания + адрес обёртки (конструктор v2, ревью 272 А1).
/// Возвращает: (код, адрес обёртки). Ошибки: NotConfigured (dex.native_wrapped не задана), Internal (артефакт не сверен).
fn bytecode(ctx: &Ctx) -> GatewayResult<(Bytes, Address)> {
    let w = ctx.addr("dex.native_wrapped").map_err(|_| {
        ctx.not_set(
            "dex.native_wrapped",
            "развёртывание исполнителя: не задана dex.native_wrapped — адрес обёртки WBNB навсегда пишется в исполнитель \
             при развёртывании (сеттера нет); задайте её («Настройки» → DEX) и запросите оценку заново",
        )
    })?;
    let bin = code::creation_with(w).map_err(|m| ctx.err(ErrorCode::Internal, m))?;
    Ok((bin, w))
}

/// Оценка развёртывания — ничего не отправляет. Ошибки: ключ кошелька, каналы, узел (оценка газа откатилась — текст).
pub async fn quote(q: &Quoting, env: &Env) -> GatewayResult<ExecutorDeployQuote> {
    let ctx = &q.ctx;
    let wallet = signer::address(ctx)?;
    let (bin, wnative) = bytecode(ctx)?;
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
        wnative,
        note,
        // Решение 399: причина запрета развёртывания (None — разрешено); интерфейс не даёт отправить при Some.
        warning: deploy_gate::refusal_at(live_proof::chain_of(ctx)),
        // Решение 400: принятые и непринятые классы шага текущей сборки в сети кошелька (решение 461) — окно
        // развёртывания показывает их таблицей.
        classes: current_proofs(ctx),
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

/// Задача t114 (3в): совпадает ли то, что подтвердил человек, с тем, что уйдёт в транзакцию создания. Принимает:
/// подтверждённые (SHA-256 кода создания, адрес обёртки), текущий адрес обёртки (dex.native_wrapped). Возвращает: Ok
/// или причину (хеш не тот / обёртка не та). Ошибок иных нет.
pub fn confirm(confirmed: (&str, Address), wnative: Address) -> Result<(), String> {
    if !confirmed.0.eq_ignore_ascii_case(&code::creation_sha256()) {
        return Err("байткод исполнителя не тот, что подтверждён (SHA-256 не совпал) — запросите оценку заново".into());
    }
    if confirmed.1 != wnative {
        return Err(format!(
            "обёртка BNB для конструктора сейчас {wnative:#x} (dex.native_wrapped), а подтверждена {:#x} — настройку \
             сменили после оценки; запросите оценку заново",
            confirmed.1
        ));
    }
    Ok(())
}

/// Развёртывание. Принимает: котировку, окружение отправки, очередь кошелька, подтверждённые человеком SHA-256 байткода
/// и адрес обёртки (задача t114, 3в). Возвращает: итог (адрес, транзакция, совпал ли код). Ошибки: NotApplicable
/// (решение 399: текущая сборка (v3, после пересборки — v4) не принята на живых пулах — отказ первым, до любой работы с сетью), QuoteMismatch
/// (байткод или обёртка не те, что подтверждены), Internal (артефакт), ключ, каналы, узел, OutcomeUnknown.
pub async fn deploy(
    q: &Quoting,
    env: &Env,
    nonces: &Nonces,
    confirmed: (&str, Address),
) -> GatewayResult<ExecutorDeployed> {
    let ctx = &q.ctx;
    let refused = deploy_gate::refusal_at(live_proof::chain_of(ctx));
    gate(refused).map_err(|why| ctx.err(ErrorCode::NotApplicable, why))?;
    let (bin, wnative) = bytecode(ctx)?;
    confirm(confirmed, wnative).map_err(|why| ctx.err(ErrorCode::QuoteMismatch, why))?;
    // Решение 398: эталон кода в сети (ArbExecutor.bin-runtime, сверка по хешу) — до отправки: после неё сверка не
    // должна отказать на чтении артефакта, когда контракт уже развёрнут.
    let want = code::runtime().map_err(|w| ctx.err(ErrorCode::Internal, w))?;
    let slot = nonces.acquire(ctx).await?;
    let nonce = slot.nonce(ctx).await?;
    let address = slot.from.create(nonce);
    let d = txs::send(env, &slot, TxKind::Create, bin, U256::ZERO, false, None).await?;
    drop(slot);
    let code_ok =
        d.chain.outcome == TxOutcome::Included && on_chain_ok(ctx, address, &want).await?;
    let text = match (d.chain.outcome, code_ok) {
        (TxOutcome::Included, true) => format!(
            "Исполнитель развёрнут: {address:#x} (tx {}). Код в сети совпал с репозиторием, обёртка в конструкторе \
             {wnative:#x}. Адрес записан у кошелька (экран «Маппинг кошельков», решение 132) — маршруты через разные DEX \
             идут одной транзакцией",
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

/// Решение 461: классы шага текущей сборки по записи приёмки сети кошелька окружения (сборка не сверена — пусто).
/// Ошибок нет.
fn current_proofs(ctx: &Ctx) -> Vec<ExecutorClassProof> {
    let chain = live_proof::chain_of(ctx);
    match code::current_version() {
        Ok(v) => live_proof::proofs(v, chain),
        Err(_) => Vec::new(),
    }
}

/// Решение 399: пропуск развёртывания. Принимает: причину запрета (deploy_gate::refusal). Возвращает: Ok — запрета нет;
/// Err(причина) — отказ. Ошибок иных нет.
pub fn gate(refusal: Option<String>) -> Result<(), String> {
    match refusal {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

/// Код по адресу совпал с ArbExecutor.bin-runtime. Принимает: окружение, адрес, эталон (прочитан до отправки,
/// решение 398). Ошибки: узел.
async fn on_chain_ok(ctx: &Ctx, address: Address, want: &Bytes) -> GatewayResult<bool> {
    let have: Bytes = rpc::call(ctx, "eth_getCode", json!([address, "latest"])).await?;
    Ok(have == *want)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Задача t114 (3в): подтверждение, выданное для одной обёртки, при другой dex.native_wrapped не проходит; хеш —
    /// настоящий SHA-256 кода создания из contracts/out/ArbExecutor.bin.
    #[test]
    fn confirmation_bound_to_wrapper() {
        let sha = code::creation_sha256();
        let (a, b) = (Address::repeat_byte(0x01), Address::repeat_byte(0x02));
        assert!(confirm((&sha, a), a).is_ok());
        assert!(confirm((&sha.to_uppercase(), a), a).is_ok());
        assert!(confirm((&sha, a), b).is_err_and(|w| w.contains("обёртка")));
        assert!(confirm(("00", a), a).is_err_and(|w| w.contains("SHA-256")));
    }

    /// Решения 399 / 400 / 461: развёртывание запрещено, пока в записи пары (версия, сеть) не принят ни один класс шага;
    /// принят хотя бы один — пропуск. Вердикты — правило live_rule.rs на данных (без записи) и на вшитых записях.
    #[test]
    fn deploy_refused_until_a_class_is_accepted() {
        use super::super::code::Version;
        use super::super::live_rule::{CLASSES, Verdict};
        let v = |accepted: bool| Verdict {
            class: CLASSES[0],
            pools: 3,
            proven: 3,
            accepted,
            why: "тест".into(),
        };
        let (v4, bsc) = (Version::V4, Some(56));
        let list = Ok(vec![v(false)]);
        let none = deploy_gate::refusal_for(v4, bsc, &list).expect("запрет");
        assert!(none.contains(&deploy_gate::not_accepted(v4, bsc)));
        assert!(none.contains("не приняты"));
        let bad = deploy_gate::refusal_for(v4, bsc, &Err("нет записи".into()));
        assert!(bad.is_some_and(|w| w.contains("нет записи")));
        let two = Ok(vec![v(false), v(true)]);
        assert!(deploy_gate::refusal_for(v4, bsc, &two).is_none());
        // Решение 461: текущая сборка в каждой сети — запрет ровно тогда, когда запись пары (версия, сеть) не приняла
        // ни одного класса (записи нет — «не проверен», запрет; без обходной настройки).
        for chain in [Some(42161), Some(56), None] {
            let accepted = match live_proof::verdicts_current(chain) {
                Ok((_, Ok(l))) => l.iter().any(|x| x.accepted),
                _ => false,
            };
            assert_eq!(deploy_gate::refusal_at(chain).is_none(), accepted);
        }
        let closed = deploy_gate::closed_everywhere();
        assert_eq!(deploy_gate::need_v3(), deploy_gate::need_v3_for(closed));
        assert!(!deploy_gate::need_v3_for(true).contains("переразверн"));
    }
}
