//! Файл: venues/bsc_dex/check/taxes.rs
//! Что делает: проверка налогов одной монеты (решение 30): пробный обмен в каждом её пуле (налоги — по первому
//! удачному; V2 впереди: только они поддерживают монеты с налогом), газ обмена и разрешений — по каждому пулу.
//! Источник USDT для пробы — другой пул V2/V3 реестра с наибольшим балансом USDT (проверяемый пул не трогается).
//! Итог — CoinCheck: налоги известны, или «проверка не прошла» с причиной (чёрный список у ядра).
//! Кто вызывает: check/cycle.rs.
//! От чего зависит: check/probe.rs, check/state.rs, pools/registry.rs, tokens.rs, num.rs, wallet/signer.rs, node/call.rs.
//! Настройки: check.probe_usdt [bsc_dex] (сумма пробы); через probe.rs — адреса Universal Router, Permit2, квотера.
//!
//! Налог, % = (ожидалось − пришло) / ожидалось · 100 — research/flow/bsc_dex.md, раздел 6 (модель налога у
//! Uniswap SDK: buyFeeBps/sellFeeBps — доля, удерживаемая при переводе).

use alloy_primitives::{Address, U256};
use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Asset, CheckState, CoinCheck, PoolKind, PoolRef, TokenTaxes};
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::abi::IERC20;
use super::super::node::call;
use super::super::num;
use super::super::pools::registry::{Entry, Registry};
use super::super::tokens::Tokens;
use super::super::wallet::signer;
use super::probe::{self, Measured};
use super::state::Checks;

/// Имя источника проверки в CoinCheck.source (решение 30: источники подключаемые).
pub const SOURCE: &str = "eth_simulateV1";

/// Проверяет монету и пишет газ пулов в checks. Принимает: тикер, адрес монеты, USDT, время блока (секунды).
/// Возвращает: CoinCheck (ошибки проверки — внутри как Failed с причиной).
#[allow(clippy::too_many_arguments)]
pub async fn check_coin(
    ctx: &Ctx,
    reg: &Registry,
    checks: &Checks,
    tokens: &Tokens,
    ticker: &str,
    coin: Address,
    usdt: Address,
    block_time: u64,
) -> CoinCheck {
    let taxes = match run(ctx, reg, checks, tokens, ticker, usdt, block_time).await {
        Ok(Ok(t)) => CheckState::Known(t),
        Ok(Err(reason)) => CheckState::Failed { reason },
        Err(e) => CheckState::Failed { reason: e.message },
    };
    CoinCheck {
        venue: ctx.venue(),
        asset: Asset {
            ticker: ticker.to_string(),
            bsc_contract: Some(coin),
        },
        taxes,
        source: SOURCE.to_string(),
        checked_at: Some(Utc::now()),
    }
}

/// Пробы по пулам монеты. Ok(Err) — ни одна проба не прошла (причины); Err — настройки, ключ, узел.
async fn run(
    ctx: &Ctx,
    reg: &Registry,
    checks: &Checks,
    tokens: &Tokens,
    ticker: &str,
    usdt: Address,
    block_time: u64,
) -> GatewayResult<Result<TokenTaxes, String>> {
    let wallet = signer::address(ctx)?;
    let usdt_dec = tokens.decimals(ctx, usdt).await?;
    let probe = num::to_units(ctx.dec("check.probe_usdt")?, usdt_dec).unwrap_or(U256::ZERO);
    let mut pools: Vec<Entry> = reg
        .by_ticker(ticker)
        .into_iter()
        .filter(|e| e.valid)
        .collect();
    // V2 впереди: только они поддерживают монеты с налогом (решение 39), затем V3, затем V4.
    pools.sort_by_key(|e| match e.pool.kind {
        PoolKind::V2 => 0,
        PoolKind::V3 => 1,
        _ => 2,
    });
    if pools.is_empty() {
        return Ok(Err(
            "нет достоверного пула с USDT для пробного обмена".into()
        ));
    }
    let mut taxes = None;
    let mut reasons = Vec::new();
    for e in &pools {
        let exclude = match e.pool.reference {
            PoolRef::Contract(a) => Some(a),
            PoolRef::Singleton { .. } => None,
        };
        let Some(source) = usdt_source(ctx, reg, usdt, exclude, probe).await? else {
            return Ok(Err(format!(
                "нет другого пула с {probe} единиц USDT для пробы"
            )));
        };
        match probe::run(ctx, e, source, wallet, probe, block_time).await? {
            Ok(m) => {
                checks.update_pool(&e.pool.reference, |f| {
                    f.gas_buy = Some(m.gas_buy);
                    f.gas_approve_buy = Some(m.gas_approve_buy);
                    f.gas_sell = m.gas_sell.map(|g| g.0);
                    f.gas_approve_sell = m.gas_sell.map(|g| g.1);
                });
                checks.set_transfer_gas(ticker, m.gas_transfer);
                if taxes.is_none() {
                    taxes = Some(percent(&m, tokens.decimals(ctx, e.coin).await?));
                }
            }
            Err(why) => reasons.push(format!("{}: {why}", e.family.title)),
        }
    }
    Ok(match taxes {
        Some(Some(t)) => Ok(t),
        Some(None) => Err("пробный обмен: расчётный выход 0 — налог не определить".into()),
        None => Err(reasons.join("; ")),
    })
}

/// Налоги в процентах. None — делить не на что (ожидалось 0).
fn percent(m: &Measured, decimals: u8) -> Option<TokenTaxes> {
    let pct = |sent: U256, got: U256| -> Option<Decimal> {
        let s = num::to_decimal(sent, decimals)?;
        let g = num::to_decimal(got.min(sent), decimals)?;
        if s.is_zero() {
            None
        } else {
            Some((s - g) / s * Decimal::ONE_HUNDRED)
        }
    };
    Some(TokenTaxes {
        buy_pct: pct(m.buy_expected, m.buy_got)?,
        sell_pct: pct(m.sell_sent, m.sell_got)?,
        transfer_pct: pct(m.transfer_sent, m.transfer_got)?,
    })
}

/// Источник USDT для пробы: пул V2/V3 реестра (не проверяемый) с наибольшим балансом USDT ≥ суммы пробы.
/// Ошибки: узел. None — такого пула нет.
async fn usdt_source(
    ctx: &Ctx,
    reg: &Registry,
    usdt: Address,
    exclude: Option<Address>,
    need: U256,
) -> GatewayResult<Option<Address>> {
    let list: Vec<Address> = reg
        .refs()
        .into_iter()
        .filter_map(|r| match r {
            PoolRef::Contract(a) if Some(a) != exclude => Some(a),
            _ => None,
        })
        .collect();
    let calls: Vec<_> = list
        .iter()
        .map(|a| IERC20::balanceOfCall { owner: *a })
        .collect();
    let balances = call::view_many(ctx, usdt, &calls, "latest").await?;
    Ok(list
        .into_iter()
        .zip(balances)
        .filter(|(_, b)| *b >= need)
        .max_by_key(|(_, b)| *b)
        .map(|(a, _)| a))
}
