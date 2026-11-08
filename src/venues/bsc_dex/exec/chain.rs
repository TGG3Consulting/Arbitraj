//! Файл: venues/bsc_dex/exec/chain.rs
//! Что делает: шаги исполнения пути (quote/path.rs) — по команде Universal Router на каждый пул, по правилам
//! официального SDK (research/flow/bsc_smart_router.md, раздел 3.1): первый шаг платит кошелёк (Permit2) своей суммой;
//! следующие — роутер всем своим балансом (CONTRACT_BALANCE), промежуточный выход — роутеру (ADDRESS_THIS), последний —
//! кошельку. Решение 122 R2: путь из любого числа пулов (1..dex.route_max_hops) одного Universal Router — цепочка
//! команд одного execute (map(ADDRESS_THIS) = address(this) — pancake-v4-universal-router 1b6d479e
//! src/base/Dispatcher.sol#L353-L361, universal-router edf19bfd contracts/base/Dispatcher.sol#L332-L340;
//! CONTRACT_BALANCE во входе V3 — V3SwapRouter.sol#L85 / #L115, V2 через Payments.pay — Payments.sol#L32 / #L30). Минимум выхода — у каждого шага: у промежуточного — ожидаемое после крючка × (1 − exec.dex_slippage_pct)
//! (SDK ставит 0, но обмен без минимума здесь не отправляется), у последнего — минимум сделки (exec/order.rs;
//! окончательный по всем долям — exec/leg_min.rs, решение 228).
//! Кто вызывает: exec/trade.rs, exec/trade_smart.rs, exec/plan.rs, exec/universal.rs.
//! От чего зависит: exec/order.rs, exec/swap_v2v3.rs, exec/swap_v4.rs, exec/ur.rs, quote/path.rs, num.rs.
//! Настройки: exec.dex_slippage_pct [bsc_dex].

use alloy_primitives::{Address, Bytes};
use rust_decimal::Decimal;

use crate::core::types::{PoolKind, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::pools::families::Proto;
use super::super::pools::registry::Entry;
use super::super::quote::path::Path;
use super::order::order;
use super::plan::Order;
use super::ur::{ADDRESS_THIS, CONTRACT_BALANCE, Format};
use super::{swap_v2v3, swap_v4};

/// Шаг исполнения: пул, обмен, кто платит и кто получает.
pub struct Step {
    pub entry: Entry,
    pub order: Order,
    /// true — платит кошелёк (Permit2) суммой order.amount_in; false — роутер всем балансом (CONTRACT_BALANCE).
    pub from_wallet: bool,
    /// true — выход на кошелёк; false — роутеру (следующему шагу).
    pub to_wallet: bool,
}

/// Шаги пути сделки. Принимает: запрос, живой путь (1..N пулов, решение 122 R2), количество монет доли,
/// (decimals монеты, USDT). Первый шаг платит кошелёк суммой сделки, следующие — роутер всем балансом
/// (CONTRACT_BALANCE); выход промежуточных шагов — роутеру (ADDRESS_THIS), последнего — кошельку с минимумом сделки.
/// Ошибки: как у exec/order.rs (худшая цена ≤ 0, минимум 0); NoRoute (путь без пулов); QuoteMismatch (минимум
/// промежуточного шага 0); BadResponse (числа); NotConfigured (допуск).
pub fn steps(
    ctx: &Ctx,
    req: &TradeRequest,
    p: &Path,
    qty: Decimal,
    decs: (u8, u8),
) -> GatewayResult<Vec<Step>> {
    let fin = |e: &Entry| order(ctx, req, e, qty, (p.coins, p.usdt), decs);
    let n = p.hops.len();
    let (Some(first_hop), Some(last_hop)) = (p.hops.first(), p.hops.last()) else {
        return Err(ctx.err(ErrorCode::NoRoute, "путь без пулов"));
    };
    if n == 1 {
        return Ok(vec![Step {
            entry: first_hop.entry.clone(),
            order: fin(&first_hop.entry)?,
            from_wallet: true,
            to_wallet: true,
        }]);
    }
    // Вход первого шага — как у сделки (USDT при покупке, монеты при продаже).
    let first_in = fin(&first_hop.entry)?.amount_in;
    let slip = Decimal::ONE - ctx.dec("exec.dex_slippage_pct")? / Decimal::ONE_HUNDRED;
    let mut out = Vec::with_capacity(n);
    for (i, h) in p.hops[..n - 1].iter().enumerate() {
        // Промежуточный шаг: минимум — ожидаемое после крючка × (1 − exec.dex_slippage_pct), выход роутеру.
        let keep = Decimal::ONE - h.usable.hook_pct / Decimal::ONE_HUNDRED;
        let min_mid = num::to_units(h.pool_out * keep * slip, h.dec_out).ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                "минимум промежуточного шага не считается",
            )
        })?;
        if min_mid.is_zero() {
            return Err(ctx.err(
                ErrorCode::QuoteMismatch,
                format!(
                    "{}: минимум промежуточного шага 0 — обмен без минимума не отправляется",
                    h.entry.family.title
                ),
            ));
        }
        out.push(Step {
            entry: h.entry.clone(),
            order: Order {
                token_in: h.token_in,
                token_out: h.token_out,
                amount_in: if i == 0 { first_in } else { CONTRACT_BALANCE },
                min_out: min_mid,
            },
            from_wallet: i == 0,
            to_wallet: false,
        });
    }
    // Последний шаг: токены и минимум сделки; вход — весь баланс роутера.
    let mut o = fin(&last_hop.entry)?;
    (o.token_in, o.token_out, o.amount_in) =
        (last_hop.token_in, last_hop.token_out, CONTRACT_BALANCE);
    out.push(Step {
        entry: last_hop.entry.clone(),
        order: o,
        from_wallet: false,
        to_wallet: true,
    });
    Ok(out)
}

/// Команда шага. Принимает: шаг, формат Universal Router, кошелёк. Ошибки: как у swap_v2v3 / swap_v4.
pub fn command(ctx: &Ctx, s: &Step, f: Format, wallet: Address) -> GatewayResult<(u8, Bytes)> {
    let to = if s.to_wallet { wallet } else { ADDRESS_THIS };
    match s.entry.pool.kind {
        PoolKind::V2 | PoolKind::V3 => {
            swap_v2v3::command(ctx, &s.entry, f, &s.order, to, s.from_wallet)
        }
        PoolKind::Cl | PoolKind::Bin => {
            swap_v4::command(ctx, &s.entry, f, &s.order, (s.from_wallet, s.to_wallet))
        }
        // Infinity Stable (решение «122 (Infinity Stable)»): пул в крючке пула CL — обмен CL_SWAP_EXACT_IN_SINGLE с его
        // PoolKey (hooks = крючок): beforeSwap забирает весь вход и отдаёт выход своей кривой (verified-код L1677-L1713),
        // CL-пул меняет 0 (CLPool.sol#L251-L252); так же кодирует путь официальный SDK (@pancakeswap/smart-router 7.8.0
        // encodeInfinityRouteToPath, evm.mjs L629-L668: InfinityStable как CL, hookData «0x»).
        PoolKind::StableSwap if s.entry.family.proto == Proto::InfinityStable => {
            swap_v4::command(ctx, &s.entry, f, &s.order, (s.from_wallet, s.to_wallet))
        }
        // DODO PMM (решение 86b): не исполняется Universal Router — своп DODO будет в фазе 86c.
        PoolKind::Pmm => Err(ctx.err(
            ErrorCode::Internal,
            "DODO PMM не исполняется Universal Router (фаза 86c)",
        )),
        // Solidly (Topaz v2, THENA V1; решения 80, 85): tradable=false — своп Solidly будет в следующей фазе (свой
        // роутер форка). В граф/сигналы берётся, в авто-сделку — нет (guard executable здесь не пускает).
        PoolKind::Solidly => Err(ctx.err(
            ErrorCode::Internal,
            "Solidly не исполняется (нет пути исполнения — tradable=false, следующая фаза)",
        )),
        // StableSwap (решение 122 F, часть F): Universal Router его не исполняет — только шаг CURVE исполнителя.
        PoolKind::StableSwap => Err(ctx.err(
            ErrorCode::Internal,
            "StableSwap не исполняется Universal Router (шаг CURVE контракта-исполнителя)",
        )),
        // PMM от оракула (WooFi, решение 122 F, 3): не исполняется Universal Router.
        PoolKind::OraclePmm => Err(ctx.err(
            ErrorCode::Internal,
            "WooFi не исполняется Universal Router (общий контракт со своим swap)",
        )),
        // TickBin (Maverick V2, решение 122 F): Universal Router его не исполняет — только шаг MAVERICK исполнителя.
        PoolKind::TickBin => Err(ctx.err(
            ErrorCode::Internal,
            "Maverick V2 не исполняется Universal Router (шаг MAVERICK контракта-исполнителя)",
        )),
    }
}
