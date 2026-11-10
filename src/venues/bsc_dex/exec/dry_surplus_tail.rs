//! Файл: venues/bsc_dex/exec/dry_surplus_tail.rs
//! Что делает: хвост доли продажи для поправки излишка от пополнения (решение 252, вариант «б», exec/dry_surplus_cut.rs):
//! выход шагов 2…N доли на две суммы входа (выход первого шага в симуляции и без прироста излишка) — квотер каждого
//! шага (quote/quoter_calls.rs; DEX с dex.quoter_virtual — виртуальный квотер подменой кода, решение 186) одной
//! eth_simulateV1 на базовом блоке той же симуляции. Пулы хвоста пополняемого токена не держат — пополнение их
//! состояние не меняет, поэтому базовый блок = их состояние в момент обмена симуляции. Квотер исполняет настоящий код
//! пула и откатывается (или view) — состояние не меняется, оба вызова в одной симуляции независимы.
//! Кто вызывает: exec/dry_surplus_cut.rs.
//! От чего зависит: check/simulate.rs (run_with, Override), quote/{quoter_calls, virtual_quoter}.rs, exec/chain.rs (Step).
//! Настройки: через вызываемые — dex.router / dex.quoter [DEX], dex.quoter_virtual [DEX].

use alloy_primitives::{Address, U256};

use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::check::simulate::{self, Base, Call, Override};
use super::super::ctx::Ctx;
use super::super::quote::{quoter_calls, virtual_quoter};
use super::chain::Step;

/// Выход хвоста доли (шаги по порядку) на две суммы входа — квотер каждого шага в eth_simulateV1 на базовом блоке
/// симуляции (оба вызова — в одной симуляции, квотеры состояние не меняют). Ошибки: NotConfigured (адрес квотера),
/// узел, BadResponse (ответ не по ABI), SwapReverted (виртуальный квотер: ликвидности не хватает).
pub async fn tail(
    ctx: &Ctx,
    steps: &[Step],
    at: Base,
    mut a: [U256; 2],
) -> GatewayResult<[U256; 2]> {
    for s in steps {
        let e = &s.entry;
        let virt = virtual_quoter::on(ctx, e.family.id, e.family.proto);
        let mut calls = Vec::with_capacity(2);
        let mut over = Vec::new();
        for v in a {
            let (to, input) = if virt {
                let PoolRef::Contract(pool) = e.pool.reference else {
                    return Err(ctx.err(ErrorCode::Internal, "виртуальный квотер: пул не контракт"));
                };
                let zero = s.order.token_in == e.pool.token0;
                (
                    virtual_quoter::address(),
                    virtual_quoter::data(pool, zero, v),
                )
            } else {
                quoter_calls::request(ctx, e, s.order.token_in, v)?
            };
            calls.push(Call {
                from: Address::ZERO,
                to,
                input,
            });
        }
        if virt {
            let code = virtual_quoter::runtime().map_err(|m| ctx.err(ErrorCode::Internal, m))?;
            over.push(Override {
                address: virtual_quoter::address(),
                code,
                state: Vec::new(),
            });
        }
        let out = simulate::run_with(ctx, &calls, &[], &over, at).await?;
        for (k, c) in out.iter().enumerate() {
            if !c.ok() {
                return Err(ctx.err(
                    ErrorCode::NoRoute,
                    format!("{}: квотер откатился — {}", e.family.title, c.why()),
                ));
            }
            let got = if virt {
                match virtual_quoter::decode(&c.return_data, a[k]) {
                    Some(Ok(v)) => Some(v),
                    Some(Err(why)) => return Err(ctx.err(ErrorCode::SwapReverted, why)),
                    None => None,
                }
            } else {
                quoter_calls::decode(e, &c.return_data)
            };
            a[k] = got.ok_or_else(|| {
                ctx.err(
                    ErrorCode::BadResponse,
                    format!("{}: ответ квотера не по ABI", e.family.title),
                )
            })?;
        }
    }
    Ok(a)
}
