//! Файл: venues/bsc_dex/exec/bundle_native.rs
//! Что делает: нативная монета BNB в мульти-роутерном бандле свопов (решение 122, часть R3; решение 62 — BNB в одной
//! транзакции UR). Обёртка/развёртка WBNB — отдельной командой в execute КАЖДОГО Universal Router бандла, как в прямом
//! и умном пути (exec/native.rs, exec/plan.rs):
//! - продажа: транзакция роутера несёт value = сумма входов его ног → WRAP_ETH(роутер, сумма) первой командой → первый
//!   шаг каждой ноги платит роутер своей точной суммой (без Permit2);
//! - покупка: последний шаг каждой ноги отдаёт WBNB роутеру → UNWRAP_WETH(кошелёк, сумма минимумов ног роутера)
//!   последней командой: весь WBNB роутера — нативным BNB на кошелёк, меньше минимума — откат (решение 40); бандл
//!   атомарен — откат одного свопа роняет весь бандл.
//!
//! Почему не отдельной транзакцией WBNB.deposit / withdraw: у покупки сумма развёртки на момент подписи неизвестна
//! (только минимум) — остаток WBNB остался бы на кошельке не нативным; WRAP_ETH / UNWRAP_WETH — официальные команды
//! обоих UR (exec/ur.rs) и уже проверены живой записью для V2/V3 (решение 62).
//! Нога форка (свой classic V2 Router) обёртку не делает — BNB через форк неисполним (quote/graph_exec.rs, здесь —
//! защитный отказ). Резерв на газ (risk.min_bnb_balance) сделкой не тратится: value всех свопов + газ всех транзакций
//! бандла (лимиты симуляции × цена) — только сверх резерва. Факт — по WBNB Deposit / Withdrawal всех роутеров бандла.
//! Кто вызывает: exec/bundle_trade.rs (adapt, routers), exec/bundle.rs (group_wrap), exec/bundle_send.rs
//! (reserve_ok), exec/result.rs (moved_all).
//! От чего зависит: exec/native.rs, exec/chain.rs (Step), exec/bundle.rs (Bundle), wallet/balances.rs, num.rs.
//! Настройки: risk.min_bnb_balance (система); через exec/native.rs — dex.native_ticker, dex.native_wrapped [bsc_dex].
//!
//! WRAP_ETH / UNWRAP_WETH: pancake-v4-universal-router 1b6d479e src/modules/Payments.sol#L76-L104, universal-router
//! edf19bfd contracts/modules/Payments.sol#L91-L119 (wrapETH — точная сумма ≤ баланса BNB роутера; unwrapWETH9 —
//! весь WBNB роутера, меньше amountMinimum — откат).

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::Side;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::settings::read::At;

use super::super::ctx::Ctx;
use super::super::num;
use super::super::send::track::Receipt;
use super::super::wallet::balances;
use super::bundle::{Bundle, BundleTx};
use super::chain::Step;
use super::native::{self, Wrap};

/// Переделывает ноги бандла под BNB (продажа — первый шаг платит роутер; покупка — последний отдаёт роутеру).
/// Принимает: сторону, ноги. Ошибки: NoRoute — нога форка (обёртки нет) или пул на стороне BNB не V2/V3.
pub fn adapt(ctx: &Ctx, side: Side, legs: &mut [Vec<Step>]) -> GatewayResult<()> {
    if legs.iter().flatten().any(|s| s.entry.family.fork) {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "нативная монета BNB через ногу форка: обёртка WBNB делается только командой Universal Router — не исполняется",
        ));
    }
    // Сумма обёртки здесь не нужна: её считает каждая роутер-группа (group_wrap).
    native::adapt(ctx, side, legs).map(|_| ())
}

/// Обёртка одной роутер-группы по флагам шагов (после adapt): продажа — WRAP_ETH на сумму входов ног группы;
/// покупка — UNWRAP_WETH; обычный токен — без обёртки. Ошибки: Internal — ноги группы обёрнуты по-разному.
pub fn group_wrap(ctx: &Ctx, legs: &[&[Step]]) -> GatewayResult<Wrap> {
    let wraps: Vec<&Step> = legs
        .iter()
        .filter_map(|l| l.first())
        .filter(|s| !s.from_wallet)
        .collect();
    let unwraps = legs
        .iter()
        .filter_map(|l| l.last())
        .filter(|s| !s.to_wallet)
        .count();
    match (wraps.len(), unwraps) {
        (0, 0) => Ok(Wrap::None),
        (w, 0) if w == legs.len() => {
            Ok(Wrap::Wrap(wraps.iter().fold(U256::ZERO, |a, s| {
                a.saturating_add(s.order.amount_in)
            })))
        }
        (0, u) if u == legs.len() => Ok(Wrap::Unwrap),
        _ => Err(ctx.err(
            ErrorCode::Internal,
            "ноги одной роутер-группы бандла обёрнуты по-разному (BNB) — план не собирается",
        )),
    }
}

/// Роутеры свопов бандла (у них WRAP_ETH / UNWRAP_WETH) — для факта по событиям WBNB. Ошибок нет.
pub fn routers(b: &Bundle) -> Vec<Address> {
    let mut out: Vec<Address> = Vec::new();
    for t in &b.txs {
        if let BundleTx::Swap { plan, .. } = t
            && !out.contains(&plan.router)
        {
            out.push(plan.router);
        }
    }
    out
}

/// Сколько WBNB обернули (продажа) или развернули (покупка) все роутеры бандла по квитанции. Ошибок нет.
pub fn moved_all(r: &Receipt, wbnb: Address, routers: &[Address], side: Side) -> U256 {
    routers.iter().fold(U256::ZERO, |a, &router| {
        a.saturating_add(native::moved(r, wbnb, router, side))
    })
}

/// Резерв на газ не тратится: BNB кошелька − value всех свопов − газ всех транзакций бандла (лимиты симуляции × цена)
/// ≥ risk.min_bnb_balance. Без value (не продажа BNB) — проверки нет. Принимает: окружение, кошелёк, бандл, лимиты
/// газа по транзакциям, цену газа (wei). Ошибки: InsufficientBalance, NotConfigured (резерв), BadResponse, узел.
pub async fn reserve_ok(
    ctx: &Ctx,
    wallet: Address,
    b: &Bundle,
    limits: &[u64],
    price: U256,
) -> GatewayResult<()> {
    let value = b.txs.iter().fold(U256::ZERO, |a, t| match t {
        BundleTx::Swap { plan, .. } => a.saturating_add(plan.value()),
        _ => a,
    });
    if value.is_zero() {
        return Ok(());
    }
    let reserve = ctx
        .settings
        .decimal("risk.min_bnb_balance", At::default())
        .map_err(|e| ctx.tag(e))?;
    let gas: u64 = limits.iter().sum();
    let wei = value.saturating_add(price.saturating_mul(U256::from(gas)));
    let spend = num::to_decimal(wei, num::BNB_DECIMALS).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("{wei} wei не переводится в BNB"),
        )
    })?;
    let have = balances::bnb(ctx, wallet).await?;
    if have - spend < reserve {
        return Err(ctx.err(
            ErrorCode::InsufficientBalance,
            format!(
                "BNB (бандл): нужно {spend} (сумма + газ), а сверх резерва на газ {reserve} (risk.min_bnb_balance) на кошельке только {}",
                (have - reserve).max(Decimal::ZERO)
            ),
        ));
    }
    Ok(())
}
