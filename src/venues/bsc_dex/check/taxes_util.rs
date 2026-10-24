//! Файл: venues/bsc_dex/check/taxes_util.rs
//! Что делает: части проверки налогов: налоги в процентах по замеру пробы и выбор источника встречного токена для пробы.
//! Кто вызывает: check/taxes.rs.
//! От чего зависит: check/probe.rs (Measured), pools/registry.rs, node/call.rs, num.rs.
//! Настроек нет.
//!
//! Налог, % = (ожидалось − пришло) / ожидалось · 100 — research/flow/bsc_dex.md, раздел 6.
//! balanceOf — EIP-20: https://eips.ethereum.org/EIPS/eip-20#balanceof

use std::collections::HashMap;
use std::sync::Mutex;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{PoolRef, TokenTaxes};
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::abi::IERC20;
use super::super::node::call;
use super::super::num;
use super::super::pools::registry::{Entry, Registry};
use super::probe::Measured;

/// Налоги в процентах. None — делить не на что (ожидалось 0).
pub fn percent(m: &Measured, decimals: u8) -> Option<TokenTaxes> {
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
