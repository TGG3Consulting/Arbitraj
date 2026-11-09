//! Файл: venues/bsc_dex/exec/swap_send.rs
//! Что делает: общие шаги отправки свопа прямого и умного пути: разрешения по порядку (каждое ждёт блока), отказ при
//! новом разрешении после пересборки (решение 121 B1), уведомление dex.rebuilt_after_approve и сам обмен (база
//! проверки баланса — прямо перед подписью, чаевые — к обмену, резерв BNB у нативной продажи). Решение 236, дополнение
//! (техлид 09.11): газ разрешений — в итог обмена (with_approve_gas), ядро берёт его в затраты сделки.
//! Кто вызывает: exec/trade_direct.rs, exec/trade_smart.rs (через реэкспорт exec/trade.rs).
//! От чего зависит: exec/txs.rs, exec/swap_ctx.rs (отметка approve в «чем найти», решение 231), exec/plan.rs, exec/native.rs, wallet/approve.rs, wallet/balances.rs,
//! wallet/inflows.rs, wallet/nonce.rs.
//! Настройки: через вызываемые — send.*, channel.*, risk.min_bnb_balance.

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{ChainTx, TradeRequest, TradeResult, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::events::MarketEvent;

use super::super::ctx::Ctx;
use super::super::wallet::approve::Approval;
use super::super::wallet::balances;
use super::super::wallet::inflows::SwapGuard;
use super::super::wallet::nonce::Slot;
use super::native;
use super::plan::Plan;
use super::txs::{self, Env};

/// Разрешения по порядку (без чаевых), каждое ждёт своего блока. Принимает: окружение, место в очереди, разрешения
/// плана. Возвращает: Some(газ BNB всех разрешений по квитанциям) — хоть одно отправлено (прошли секунды блоков:
/// маршрут, минимумы и calldata обмена устарели и собираются заново — решение 121 B1, как решение 99 у KyberSwap);
/// None — разрешений не было. Решение 236, дополнение (техлид 09.11): газ разрешений — в затраты сделки (ядро,
/// core/deal_gas.rs): вызывающий кладёт его в итог обмена (with_approve_gas); у ошибки здесь — газ разрешений, уже
/// в блоке до неё (GatewayError::approve_gas_bnb; газ самого не прошедшего разрешения — в его факте tx).
/// Ошибки: каналы, OutcomeUnknown (nonce занят, квитанции нет), SwapReverted (разрешение не прошло — с фактом).
pub async fn send_approvals(
    env: &Env,
    slot: &Slot,
    approvals: &[Approval],
) -> GatewayResult<Option<Decimal>> {
    let mut gas: Option<Decimal> = None;
    for (to, data) in approvals {
        // Решение 231 (дополнение): исход разрешения неизвестен — отметка approve в «чем найти» (обмен не отправлялся).
        let d = txs::send(env, slot, *to, data.clone(), U256::ZERO, false, None)
            .await
            .map_err(super::swap_ctx::approve)
            .map_err(|e| e.with_approve_gas(gas))?;
        if d.chain.outcome == TxOutcome::Replaced {
            // «Чем найти» с хешами отмен — позднее выяснение (resolve_single.rs).
            let seed =
                super::swap_ctx::with_cancels(super::swap_ctx::seed(&d.chain, None), &d.cancels);
            return Err(super::swap_ctx::approve(
                env.ctx
                    .err(
                        ErrorCode::OutcomeUnknown,
                        "Исход разрешения (approve) неизвестен — nonce занят, квитанции нет",
                    )
                    .with_tx(d.chain)
                    .pending(seed),
            )
            .with_approve_gas(gas));
        }
        if d.chain.outcome != TxOutcome::Included {
            return Err(env
                .ctx
                .err(
                    ErrorCode::SwapReverted,
                    format!("Разрешение (approve) не прошло: {:?}", d.chain.outcome),
                )
                .with_tx(d.chain)
                .with_approve_gas(gas));
        }
        // Квитанция в блоке — газ по ней (gasUsed × effectiveGasPrice, exec/chain_tx.rs).
        gas = Some(gas.unwrap_or_default() + d.chain.gas_bnb.unwrap_or_default());
    }
    Ok(gas)
}

/// Решение 236, дополнение (техлид 09.11): газ разрешений, отправленных перед обменом, — в итог обмена: успех — в факт
/// транзакции (ChainTx::approve_gas_bnb), ошибка — в поле ошибки (GatewayError::approve_gas_bnb; факт tx не трогается).
/// Принимает: итог обмена, газ разрешений (None — не было). Возвращает: тот же итог. Ошибок своих нет.
pub fn with_approve_gas<T: HasChain>(
    r: GatewayResult<T>,
    gas: Option<Decimal>,
) -> GatewayResult<T> {
    if gas.is_none() {
        return r;
    }
    match r {
        Ok(mut t) => {
            if let Some(c) = t.chain_mut() {
                c.approve_gas_bnb = gas;
            }
            Ok(t)
        }
        Err(e) => Err(e.with_approve_gas(gas)),
    }
}

/// Итог обмена с фактом транзакции (TradeResult — свой; итог цикла — его trade). Ошибок нет.
pub trait HasChain {
    /// Факт транзакции для правки (None — факта нет).
    fn chain_mut(&mut self) -> Option<&mut ChainTx>;
}

impl HasChain for TradeResult {
    fn chain_mut(&mut self) -> Option<&mut ChainTx> {
        self.chain.as_mut()
    }
}

impl HasChain for crate::core::types::CycleTradeResult {
    fn chain_mut(&mut self) -> Option<&mut ChainTx> {
        self.trade.chain.as_mut()
    }
}

/// Свежая сборка после разрешений (решение 121 B1) не должна требовать новых: иначе вход вырос (покупка) или
/// разрешение не легло — второй approve не делаем (маршрут снова устареет), своп не отправляем. Ошибки: QuoteMismatch.
pub fn no_new_approvals(ctx: &Ctx, p: &Plan) -> GatewayResult<()> {
    if p.approvals.is_empty() {
        return Ok(());
    }
    Err(ctx.err(
        ErrorCode::QuoteMismatch,
        "после approve свежий маршрут требует нового разрешения (вход больше разрешённого) — своп не отправлен",
    ))
}

/// Уведомление «после approve маршрут собран заново» (решение 121 B1) — в журнале интерфейса и в Telegram, если
/// событие dex.rebuilt_after_approve включено. Принимает: окружение, запрос. Ошибок нет.
pub fn rebuilt_notice(env: &Env, req: &TradeRequest) {
    env.stats.emit(MarketEvent::Notice {
        venue: env.ctx.venue(),
        code: "dex.rebuilt_after_approve".into(),
        text: format!(
            "{:?} {} {}: разрешения (approve) отправлены — маршрут, минимумы и транзакция обмена собраны заново и \
             снова сверены с худшей ценой {}",
            req.side, req.qty, req.asset.ticker, req.worst_price
        ),
    });
}

/// Обмен после разрешений: база проверки баланса — баланс выходного токена кошелька, прочитанный прямо перед подписью
/// обмена; чаевые — к обмену. Принимает: окружение, место в очереди, план, отметку «идёт обмен» (держится до занятия
/// nonce обмена). Возвращает: итог транзакции обмена. Ошибки: каналы, узел (баланс), резерв BNB.
pub async fn send_swap(
    env: &Env,
    slot: &Slot,
    p: Plan,
    guard: SwapGuard,
) -> GatewayResult<txs::Done> {
    let base = balances::token_units(&env.ctx, p.token_out, p.wallet).await?;
    let data = p.data(base);
    // Продажа BNB (решение 62): сумма и газ — только сверх резерва на газ (exec/native.rs).
    if !p.value().is_zero() {
        native::reserve_ok(env, p.wallet, (p.router, &data), p.value()).await?;
    }
    txs::send(env, slot, p.router, data, p.value(), true, Some(guard)).await
}
