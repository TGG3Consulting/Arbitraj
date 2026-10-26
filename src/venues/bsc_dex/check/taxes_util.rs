//! Файл: venues/bsc_dex/check/taxes_util.rs
//! Что делает: части проверки налогов: пулы монеты для пробы (фильтр ликвидности, порядок V2 → V3 → V4), налоги в
//! процентах по замеру пробы и выбор источника встречного токена для пробы.
//! Кто вызывает: check/taxes.rs.
//! От чего зависит: check/probe.rs (Measured), pools/registry.rs, node/call.rs, num.rs.
//! Настройки: через quote/select.rs — dex.min_pool_liquidity_usd.
//!
//! Налог, % = (ожидалось − пришло) / ожидалось · 100 — research/flow/bsc_dex.md, раздел 6.
//! balanceOf — EIP-20: https://eips.ethereum.org/EIPS/eip-20#balanceof

use std::collections::HashMap;
use std::sync::Mutex;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{PoolKind, PoolRef, TokenTaxes};
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::abi::IERC20;
use super::super::node::call;
use super::super::num;
use super::super::pools::registry::{Entry, Registry};
use super::super::quote::select;
use super::super::tokens::Tokens;
use super::probe::Measured;

/// Налоги в процентах. None — делить не на что (ожидалось 0).
pub fn percent(m: &Measured, decimals: u8) -> Option<TokenTaxes> {
    let pct = |sent: U256, got: U256| pct(sent, got, decimals);
    Some(TokenTaxes {
        buy_pct: pct(m.buy_expected, m.buy_got)?,
        sell_pct: pct(m.sell_sent, m.sell_got)?,
        transfer_pct: pct(m.transfer_sent, m.transfer_got)?,
    })
}

/// Налог, %: (отправлено − дошло) / отправлено · 100 (дошло больше — 0). Принимает: суммы в минимальных единицах,
/// знаки токена. Возвращает: None — отправлено 0 или не переводится. Ошибок нет.
pub fn pct(sent: U256, got: U256, decimals: u8) -> Option<Decimal> {
    let s = num::to_decimal(sent, decimals)?;
    let g = num::to_decimal(got.min(sent), decimals)?;
    if s.is_zero() {
        None
    } else {
        Some((s - g) / s * Decimal::ONE_HUNDRED)
    }
}

/// Балансы встречного токена у пулов реестра — на один проход проверки налогов: пачка balanceOf по всем пулам
/// читается один раз на токен за проход, а не на каждую монету и пул (так проход стоит в разы меньше CU узла).
/// Баланс мог измениться за проход — тогда перевод из источника в пробе откатится, и это «не выполнена», не вердикт.
#[derive(Default)]
pub struct Sources {
    /// Токен → пулы по убыванию баланса.
    by_token: Mutex<HashMap<Address, Vec<(Address, U256)>>>,
}

/// Источник встречного токена (USDT или промежуточной монеты) для пробы: пул V2/V3 реестра (не проверяемый)
/// с наибольшим балансом этого токена ≥ суммы пробы. Кандидаты — только достоверные пулы с этим встречным токеном,
/// прошедшие фильтр ликвидности (`deep`): у мелких и чужих пулов баланс не нужен, пачка balanceOf в разы меньше.
/// Ошибки: узел. None — такого пула нет.
pub async fn quote_source(
    ctx: &Ctx,
    reg: &Registry,
    sources: &Sources,
    usdt: Address,
    exclude: Option<Address>,
    need: U256,
    deep: impl Fn(&Entry) -> bool,
) -> GatewayResult<Option<Address>> {
    let known = sources
        .by_token
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&usdt)
        .cloned();
    let list = match known {
        Some(l) => l,
        None => {
            let pools: Vec<Address> = reg
                .refs()
                .into_iter()
                .filter_map(|r| match (&r, reg.get(&r)) {
                    (PoolRef::Contract(a), Some(e)) if e.valid && e.usdt == usdt && deep(&e) => {
                        Some(*a)
                    }
                    _ => None,
                })
                .collect();
            let calls: Vec<_> = pools
                .iter()
                .map(|a| IERC20::balanceOfCall { owner: *a })
                .collect();
            let balances = call::view_many(ctx, usdt, &calls, "latest").await?;
            let mut l: Vec<(Address, U256)> = pools.into_iter().zip(balances).collect();
            l.sort_by_key(|x| std::cmp::Reverse(x.1));
            sources
                .by_token
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(usdt, l.clone());
            l
        }
    };
    Ok(list
        .into_iter()
        .find(|(a, b)| Some(*a) != exclude && *b >= need)
        .map(|(a, _)| a))
}

/// Пулы монеты для пробы: достоверные и глубокие в ОБА направления (проба покупает и продаёт тем же пулом; фильтр —
/// первым, quote/select.rs deep_side), V2 впереди (решение 39), затем V3, затем V4. Возвращает: (пулы, причины
/// ожидания по пулам, где глубина не считается, причины по мелким и спящим пулам — с цифрами глубины: «V4 ZKP 0x…:
/// глубина покупки 812 USDT …»; они — в текст ожидания, только когда пробовать нечем).
/// Ошибки: NotConfigured (не задана настройка глубины).
pub fn probe_pools(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    ticker: &str,
    usdt_dec: u8,
) -> GatewayResult<(Vec<Entry>, Vec<String>, Vec<String>)> {
    let (mut pools, mut env, mut shallow) = (Vec::new(), Vec::new(), Vec::new());
    for e in reg.by_ticker(ticker).into_iter().filter(|e| e.valid) {
        let label = super::super::pools::depth_book::label(&e.pool.reference);
        match select::deep_side(ctx, reg, tokens, &e, usdt_dec, select::Need::Both) {
            Ok(None) => pools.push(e),
            Ok(Some(why)) => {
                shallow.push(format!("{} {} {label}: {why}", e.family.title, e.ticker))
            }
            // Не задана настройка глубины — ошибка с её именем (по нему ждущие перепроверяются при её задании).
            Err(why) => match super::super::pools::depth_book::missing_setting(ctx) {
                Some(name) => return Err(ctx.not_set(&name, why)),
                None => env.push(format!("{} {} {label}: {why}", e.family.title, e.ticker)),
            },
        }
    }
    // V2 впереди: только они поддерживают монеты с налогом (решение 39), затем V3, затем V4.
    pools.sort_by_key(|e| match e.pool.kind {
        PoolKind::V2 => 0,
        PoolKind::V3 => 1,
        _ => 2,
    });
    Ok((pools, env, shallow))
}

/// «Не проверено: пулы монеты ещё загружаются» для экранов. Принимает: окружение, тикер, адрес, причину. Ошибок нет.
pub fn loading_check(
    p: &super::super::pools::listen::Pools,
    ticker: &str,
    addr: Address,
    why: String,
) -> crate::core::types::CoinCheck {
    crate::core::types::CoinCheck {
        venue: p.ctx.venue(),
        asset: crate::core::types::Asset {
            ticker: ticker.to_string(),
            bsc_contract: Some(addr),
        },
        taxes: crate::core::types::CheckState::NotChecked,
        source: super::taxes::SOURCE.into(),
        checked_at: None,
        wait: Some(why),
    }
}

/// Причина ожидания — ядру, если у монеты ещё нет итога (прошла / не прошла) и причина сменилась. Принимает: окружение,
/// прошлое состояние, проверку с NotChecked и причиной. Ошибок нет.
pub async fn note_wait(
    p: &super::super::pools::listen::Pools,
    prev: &Option<crate::core::types::CheckState<crate::core::types::TokenTaxes>>,
    c: crate::core::types::CoinCheck,
) {
    use crate::core::types::CheckState;
    if matches!(prev, Some(CheckState::Known(_) | CheckState::Failed { .. })) {
        return;
    }
    let old = p.checks.coin(&c.asset.ticker).and_then(|x| x.wait);
    if old == c.wait {
        return;
    }
    p.checks.set_coin(c.clone());
    let _ = p
        .out
        .send(crate::gateway::events::MarketEvent::CoinCheck(c))
        .await;
}
