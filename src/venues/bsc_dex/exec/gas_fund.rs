//! Файл: venues/bsc_dex/exec/gas_fund.rs
//! Что делает: решение 247, п. З (Тигран 10.11: согласовано; независимое ревью: продажа ТОКЕНА без BNB на газ —
//! проверка резерва была только у нативной BNB, exec/native.rs reserve_ok) — перед отправкой любой транзакции кошелька
//! (обмен, разрешение, перевод) BNB кошелька сверяется с её полной стоимостью: value + лимит газа × цена газа (те же,
//! что уйдут в транзакцию: exec/txs.rs) + чаевые (сумма + газ их перевода). Не хватает — отказ до отправки с понятной
//! причиной (InsufficientBalance: «на газ нужно N BNB, на кошельке M — пополните BNB»), транзакция не уходит.
//! Резерв risk.min_bnb_balance здесь не проверяется — он про продажу самой BNB (native.rs).
//! Решение 435 (433-д): цена газа здесь — потолок цены транзакции (send/tx_price.rs cap): legacy — gasPrice (как было),
//! тип 2 — maxFeePerGas (EIP-1559: баланс отправителя обязан покрывать gas_limit × max_fee_per_gas + value).
//! Решение 437 (2): у eip1559_l1 предпроверка остаётся по maxFeePerGas × весь лимит газа (не по разбиению L2/L1):
//! это предел списания при проверке узлом — nitro-geth core/state_transition.go:474-480 (buyGas: balanceCheck =
//! GasLimit × GasFeeCap + Value), а не фактическая плата.
//! Кто вызывает: exec/txs.rs (send).
//! От чего зависит: wallet/balances.rs (eth_getBalance), num.rs.
//! Настройки: нет.

use alloy_primitives::{Address, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::wallet::balances;

/// Полная стоимость транзакции, wei: value + цена газа × лимит + чаевые (сумма + цена газа × лимит их перевода).
/// Принимает: value, цену газа, лимит газа, чаевые (сумма wei, лимит газа) или None. Ошибок нет (насыщение).
pub fn need_wei(value: U256, price: U256, gas: u64, tip: Option<(U256, u64)>) -> U256 {
    let base = value.saturating_add(price.saturating_mul(U256::from(gas)));
    tip.map_or(base, |(amount, g)| {
        base.saturating_add(amount)
            .saturating_add(price.saturating_mul(U256::from(g)))
    })
}

/// BNB кошелька хватает на транзакцию. Принимает: окружение, кошелёк, нужно wei (need_wei). Ошибки:
/// InsufficientBalance (не хватает), BadResponse (число), узел.
pub async fn ensure(ctx: &Ctx, wallet: Address, need: U256) -> GatewayResult<()> {
    let ticker = match ctx.place.id {
        "base_dex" | "eth_dex" => "ETH",
        _ => "BNB",
    };
    let need_bnb = num::to_decimal(need, num::BNB_DECIMALS).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("{need} wei не переводится в {ticker}"),
        )
    })?;
    let have = balances::bnb(ctx, wallet).await?;
    if have < need_bnb {
        let why = format!(
            "на газ транзакции нужно {} {ticker} (с суммой и чаевыми), а на кошельке {wallet:#x} только {} {ticker} — \
             пополните {ticker} кошелька, транзакция не отправлена",
            need_bnb.normalize(),
            have.normalize()
        );
        return Err(ctx.err(ErrorCode::InsufficientBalance, why));
    }
    Ok(())
}
