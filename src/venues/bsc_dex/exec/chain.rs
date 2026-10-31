//! Файл: venues/bsc_dex/exec/chain.rs
//! Что делает: шаги исполнения пути (quote/path.rs) — по команде Universal Router на каждый пул, по правилам
//! официального SDK (research/flow/bsc_smart_router.md, раздел 3.1): первый шаг платит кошелёк (Permit2) своей суммой;
//! следующий — роутер всем своим балансом (CONTRACT_BALANCE), промежуточный выход — роутеру (ADDRESS_THIS), последний —
//! кошельку. Минимум выхода — у каждого шага: у промежуточного — ожидаемое после крючка × (1 − exec.dex_slippage_pct)
//! (SDK ставит 0, но обмен без минимума здесь не отправляется), у последнего — минимум сделки (exec/order.rs).
//! Кто вызывает: exec/trade.rs, exec/trade_smart.rs, exec/plan.rs, exec/universal.rs.
//! От чего зависит: exec/order.rs, exec/swap_v2v3.rs, exec/swap_v4.rs, exec/ur.rs, quote/path.rs, num.rs.
//! Настройки: exec.dex_slippage_pct [bsc_dex].

use alloy_primitives::{Address, Bytes};
use rust_decimal::Decimal;

use crate::core::types::{PoolKind, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
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

/// Шаги пути сделки. Принимает: запрос, живой путь, количество монет доли, (decimals монеты, USDT).
/// Ошибки: как у exec/order.rs (худшая цена ≤ 0, минимум 0); BadResponse (числа); NotConfigured (допуск).
pub fn steps(
    ctx: &Ctx,
    req: &TradeRequest,
    p: &Path,
    qty: Decimal,
    decs: (u8, u8),
) -> GatewayResult<Vec<Step>> {
    let fin = |e: &Entry| order(ctx, req, e, qty, (p.coins, p.usdt), decs);
    match p.hops.as_slice() {
        [h] => Ok(vec![Step {
            entry: h.entry.clone(),
            order: fin(&h.entry)?,
            from_wallet: true,
            to_wallet: true,
        }]),
        [a, b] => {
            // Последний шаг: токены и минимум сделки; вход — весь баланс роутера.
            let mut o = fin(&b.entry)?;
            (o.token_in, o.token_out, o.amount_in) = (b.token_in, b.token_out, CONTRACT_BALANCE);
            // Первый шаг: вход — как у сделки (USDT при покупке, монеты при продаже), минимум — промежуточная.
            let first = fin(&a.entry)?;
            let slip = Decimal::ONE - ctx.dec("exec.dex_slippage_pct")? / Decimal::ONE_HUNDRED;
            let keep = Decimal::ONE - a.usable.hook_pct / Decimal::ONE_HUNDRED;
            let min_mid = num::to_units(a.pool_out * keep * slip, a.dec_out).ok_or_else(|| {
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
                        a.entry.family.title
                    ),
                ));
            }
            let mid = Order {
                token_in: a.token_in,
                token_out: a.token_out,
                amount_in: first.amount_in,
                min_out: min_mid,
            };
            Ok(vec![
                Step {
                    entry: a.entry.clone(),
                    order: mid,
                    from_wallet: true,
                    to_wallet: false,
                },
                Step {
                    entry: b.entry.clone(),
                    order: o,
                    from_wallet: false,
                    to_wallet: true,
                },
            ])
        }
        _ => Err(ctx.err(
            ErrorCode::NoRoute,
            "путь — один пул или два через промежуточную монету",
        )),
    }
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
    }
}
