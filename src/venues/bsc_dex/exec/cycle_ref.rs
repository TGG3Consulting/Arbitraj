//! Файл: venues/bsc_dex/exec/cycle_ref.rs
//! Что делает: решение 233 (дополнение, п. 4; Тигран 08.11: «ок», «делать сейчас») — «чем найти» сделки цикла DEX ↔ DEX
//! с неизвестным исходом и итог цикла при позднем выяснении. Цикл — своп «базовая → базовая» (exec/cycle_steps.rs
//! trade_request): контекст разбора тот же, что у свопа (exec/swap_ctx.rs: кошелёк, токены (базовая, базовая), decimals,
//! факт транзакции; у бандла — BundleRef: хеши всех транзакций, первый nonce, maxBlockNumber; подпись исполнителя цикла —
//! who), плюс курсы на момент сделки (CycleRef: USDT за базовую, USDT за BNB, BNB в базовой) — из них обычный путь
//! делает итог цикла (cycle_steps::finish), а заново их считать нельзя: пулы к моменту выяснения другие.
//! Позднее выяснение (resolve_cycle.rs): итог свопа — те же функции, что обычный путь цикла (одиночная через
//! исполнитель — swap_rebuild → result::result; бандл — bundle_rebuild → bundle_trade::bundle_result; «не исполнено»
//! бандла — bundle_rebuild::not_executed), затем здесь — cycle_steps::finish с курсами сделки. Решение 222 (баланс
//! до/после) обычный путь цикла не применяет (venue.rs dex_cycle_trade) — и здесь нет.
//! Кто вызывает: exec/cycle_exec.rs, exec/cycle_bundle_trade.rs (ctx), resolve_cycle.rs (finish), exec/swap_ctx.rs (поле).
//! От чего зависит: exec/{swap_ctx, cycle_steps}.rs, quote/cycle_cost.rs (Rates), core/types.
//! Настройки: нет.

use alloy_primitives::Address;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::{CycleTradeResult, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::quote::cycle_cost::Rates;
use super::cycle_steps;
use super::swap_ctx::SwapCtx;

/// Курсы цикла на момент сделки (quote/cycle_cost.rs Rates) — для итога цикла при позднем выяснении.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleRef {
    /// USDT за 1 базовой монеты.
    pub base_usdt: Decimal,
    /// USDT за 1 BNB.
    pub bnb_usdt: Decimal,
    /// BNB в базовой монете.
    pub bnb_in_base: Decimal,
}

impl CycleRef {
    /// Что делает: курсы сделки для «чем найти». Принимает: курсы. Ошибок нет.
    pub fn of(r: &Rates) -> Self {
        Self {
            base_usdt: r.base_usdt,
            bnb_usdt: r.bnb_usdt,
            bnb_in_base: r.bnb_in_base,
        }
    }

    /// Что делает: курсы обратно в вид пути исполнения. Ошибок нет.
    pub fn rates(&self) -> Rates {
        Rates {
            base_usdt: self.base_usdt,
            bnb_usdt: self.bnb_usdt,
            bnb_in_base: self.bnb_in_base,
        }
    }
}

/// Что делает: контекст разбора цикла (своп «базовая → базовая» кошелька) с курсами сделки. Бандл дополняет поле
/// bundle (exec/cycle_bundle_trade.rs). Принимает: кошелёк, базовую, её decimals, курсы. Ошибок нет.
pub fn ctx(wallet: Address, base: Address, dec: u8, r: &Rates) -> SwapCtx {
    let mut c = SwapCtx::swap(wallet, (base, base), (dec, dec), &[], None, None);
    c.cycle = Some(CycleRef::of(r));
    c
}

/// Что делает: итог цикла из найденного итога свопа — как обычный путь (cycle_steps::finish с курсами сделки).
/// Принимает: окружение (текст ошибки), итог свопа, контекст из «чем найти». Возвращает: итог цикла; итог «не исполнено»
/// (NotFilled, транзакции не в блоке) — тот же вид с нулями, ядро различает его по статусу и исходу транзакции.
/// Ошибки: NotApplicable — в контексте нет курсов цикла (выясняет человек).
pub fn finish(ctx: &Ctx, t: TradeResult, c: Option<&SwapCtx>) -> GatewayResult<CycleTradeResult> {
    let Some(r) = c.and_then(|c| c.cycle.as_ref()) else {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            format!(
                "итог свопа найден ({:?}, tx {}), но курсов цикла в «чем найти» нет — итог цикла проверяет человек",
                t.status, t.venue_ref
            ),
        ));
    };
    Ok(cycle_steps::finish(t, &r.rates()))
}
