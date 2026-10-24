//! Файл: venues/bsc_dex/exec/native.rs
//! Что делает: нативная монета BNB в сделке DEX (решение 62). Ядро торгует BNB как монетой с адресом обёртки WBNB
//! (dex.native_wrapped, core/native.rs), а в кошельке и на бирже это нативный BNB:
//! - продажа: BNB уходит с транзакцией (value) → WRAP_ETH(роутер) → обмен WBNB → USDT, первый шаг каждой доли платит
//!   роутер (без Permit2), проверка USDT — BALANCE_CHECK_ERC20 как обычно;
//! - покупка: обмен USDT → WBNB на роутер → UNWRAP_WETH(кошелёк, сумма минимумов) — на кошелёк приходит нативный BNB;
//! - резерв на газ (risk.min_bnb_balance) сделкой не тратится: продать или перевести можно только сверх него;
//! - факт по событиям WBNB Deposit / Withdrawal роутера (WETH9), а не Transfer на кошелёк.
//!
//! Почему внутри одной транзакции, а не отдельной транзакцией обёртки: WRAP_ETH / UNWRAP_WETH — официальные команды
//! обоих Universal Router (exec/ur.rs), одна транзакция — один газ и одна очередь nonce, нет второй транзакции, которая
//! может не попасть в блок между обёрткой и обменом, и минимум проверяется в той же транзакции (решение 40).
//!
//! Кто вызывает: exec/trade.rs, exec/trade_smart.rs, exec/result.rs, check/native_probe.rs, quote/*, wallet/*.
//! От чего зависит: exec/chain.rs (Step), ctx.rs, wallet/balances.rs, num.rs.
//! Настройки: dex.native_ticker, dex.native_wrapped [bsc_dex]; risk.min_bnb_balance (система).
//!
//! WETH9: deposit() → Deposit(dst, wad), withdraw(wad) → Withdrawal(src, wad) —
//! https://github.com/gnosis/canonical-weth/blob/0dd1ea3e295eef916d0c6223ec63141137d22d67/contracts/WETH9.sol#L25-L42 ;
//! WBNB в BSC — тот же контракт (weth9 Universal Router в BSC, exec/ur.rs).

use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{Asset, PoolKind, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::settings::read::At;

use super::super::ctx::Ctx;
use super::super::num;
use super::super::send::track::Receipt;
use super::super::wallet::balances;
use super::chain::Step;

alloy::sol! {
    interface IWETH9 {
        event Deposit(address indexed dst, uint256 wad);
        event Withdrawal(address indexed src, uint256 wad);
    }
}

/// Нативная монета в плане обмена.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wrap {
    /// Обычный токен.
    None,
    /// Продажа BNB: обернуть столько wei (value транзакции) первой командой.
    Wrap(U256),
    /// Покупка BNB: развернуть весь WBNB роутера на кошелёк последней командой.
    Unwrap,
}

/// Обёртка нативной монеты по настройкам. None — dex.native_ticker или dex.native_wrapped не заданы. Ошибок нет.
pub fn wrapped(ctx: &Ctx) -> Option<(String, Address)> {
    Some((
        ctx.text("dex.native_ticker").ok()?,
        ctx.addr("dex.native_wrapped").ok()?,
    ))
}

/// Нативная ли монета: адрес = dex.native_wrapped и тикер = dex.native_ticker (монета биржи без адреса, которой ядро
/// дало адрес обёртки). Токен WBNB, который биржа сама перечисляет с адресом под другим тикером, — обычный токен.
pub fn is_native(ctx: &Ctx, a: &Asset) -> bool {
    wrapped(ctx).is_some_and(|(t, w)| a.ticker == t && a.bsc_contract == Some(w))
}

/// Переделывает шаги долей под нативную монету. Продажа — первый шаг каждой доли платит роутер своей точной суммой
/// (после WRAP_ETH на сумму всех долей); покупка — последний шаг отдаёт роутеру (дальше UNWRAP_WETH).
/// Принимает: сторону, доли. Возвращает: режим плана. Ошибки: NoRoute — шаг V4 оказался бы и «из роутера», и «роутеру»
/// (путь через промежуточную с пулом V4 на стороне BNB: такого входа у команды нет, exec/swap_v4.rs).
pub fn adapt(ctx: &Ctx, side: Side, legs: &mut [Vec<Step>]) -> GatewayResult<Wrap> {
    let mut total = U256::ZERO;
    for leg in legs.iter_mut() {
        let n = leg.len();
        let s = match side {
            Side::Sell => leg.first_mut(),
            Side::Buy => leg.last_mut(),
        }
        .ok_or_else(|| ctx.err(ErrorCode::Internal, "доля без шагов"))?;
        let v4 = matches!(s.entry.pool.kind, PoolKind::Cl | PoolKind::Bin);
        if v4 && n > 1 {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                format!("{}: нативная монета через промежуточную в пуле V4 — нет такой команды Universal Router", s.entry.family.title),
            ));
        }
        match side {
            Side::Sell => {
                s.from_wallet = false;
                total = total.saturating_add(s.order.amount_in);
            }
            Side::Buy => s.to_wallet = false,
        }
    }
    Ok(match side {
        Side::Sell => Wrap::Wrap(total),
        Side::Buy => Wrap::Unwrap,
    })
}

/// Резерв на газ не тратится: BNB кошелька − value ≥ risk.min_bnb_balance. Принимает: кошелёк, сколько уходит (wei).
/// Ошибки: InsufficientBalance (сверх резерва не хватает), NotConfigured (резерв не задан), узел.
pub async fn reserve_ok(ctx: &Ctx, wallet: Address, value: U256) -> GatewayResult<()> {
    let reserve = ctx
        .settings
        .decimal("risk.min_bnb_balance", At::default())
        .map_err(|e| ctx.tag(e))?;
    let have = balances::bnb(ctx, wallet).await?;
    let spend = num::to_decimal(value, num::BNB_DECIMALS).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("{value} wei не переводится в BNB"),
        )
    })?;
    if have - spend < reserve {
        return Err(ctx.err(
            ErrorCode::InsufficientBalance,
            format!(
                "BNB: нужно {spend}, а сверх резерва на газ {reserve} (risk.min_bnb_balance) на кошельке только {}",
                (have - reserve).max(Decimal::ZERO)
            ),
        ));
    }
    Ok(())
}

/// Сколько WBNB роутер обернул (Deposit, продажа) или развернул (Withdrawal, покупка) по квитанции — столько монет BNB
/// реально ушло из сделки или пришло на кошелёк. Принимает: квитанцию, WBNB, роутер, сторону. Ошибок нет.
pub fn moved(r: &Receipt, wbnb: Address, router: Address, side: Side) -> U256 {
    r.logs
        .iter()
        .filter(|l| l.address() == wbnb)
        .filter_map(|l| match side {
            Side::Sell => IWETH9::Deposit::decode_log_data(&l.inner.data)
                .ok()
                .filter(|e| e.dst == router)
                .map(|e| e.wad),
            Side::Buy => IWETH9::Withdrawal::decode_log_data(&l.inner.data)
                .ok()
                .filter(|e| e.src == router)
                .map(|e| e.wad),
        })
        .fold(U256::ZERO, |a, b| a.saturating_add(b))
}
