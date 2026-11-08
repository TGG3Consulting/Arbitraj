//! Файл: venues/bsc_dex/exec/cycle_trade.rs
//! Что делает: сделка цикла DEX ↔ DEX (решение 124) — вход контракта Venue::dex_cycle_trade. Вход и граница minOut уже
//! посчитаны ядром (core/cycle_plan.rs: бюджет, размер, порог прибыли). Здесь: гейт dex_dex.execute (решение 86) →
//! пулы цикла перечитываются на голове одним Multicall3 (quote/cycle_legs.rs fresh, решение 83) → пересчёт на входе:
//! ожидаемый выход не ниже границы → курсы (USDT за базовую и за BNB) → форма (exec/cycle_gate.rs): исполнитель
//! ArbExecutor (exec/cycle_exec.rs) или бандл свопов (exec/cycle_bundle_trade.rs) → итог подписывается исполнителем
//! (решение 121 B7: «свой сборщик: цикл …»). Отправка — ОДИН раз, без повторов (стартовое правило Тиграна).
//! Кто вызывает: venue.rs (dex_cycle_trade).
//! От чего зависит: exec/{cycle_gate, cycle_exec, cycle_bundle_trade, cycle_steps, result}.rs, quote/{cycle_legs,
//! cycle_cost, cycle}.rs, wallet/{nonce, balances, signer}.rs.
//! Настройки: dex_dex.execute; через вызываемые — verify.*, exec.executor_address, dex_dex.min_profit_usd, send.*.

use crate::core::types::{CycleTradeRequest, CycleTradeResult};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::quote::{Quoting, cycle, cycle_cost, cycle_legs};
use super::super::wallet::nonce::Nonces;
use super::super::wallet::{balances, signer};
use super::cycle_gate::{self, Form};
use super::txs::Env;
use super::{cycle_bundle_trade, cycle_exec, cycle_steps};

/// Подпись исполнителя цикла через ArbExecutor (решение 121 B7).
pub const BY_CYCLE_EXECUTOR: &str = "свой сборщик: цикл через исполнитель";
/// Подпись исполнителя цикла бандлом (решение 84).
pub const BY_CYCLE_BUNDLE: &str = "свой сборщик: цикл бандлом";

/// Сделка цикла. Принимает: котировку, окружение отправки, очередь кошелька, запрос ядра. Возвращает: итог с фактом.
/// Ошибки: NoRoute (гейт закрыт, цикл не исполним, проба откатилась), QuoteMismatch (ниже границы, газ съел прибыль),
/// StaleData (пулы не перечитаны), InsufficientBalance, NotConfigured, узел, каналы, SwapReverted / TxNotIncluded /
/// OutcomeUnknown (с фактом транзакции) — всё с подписью «[свой сборщик: цикл …]».
pub async fn trade(
    q: &Quoting,
    env: &Env,
    nonces: &Nonces,
    req: CycleTradeRequest,
) -> GatewayResult<CycleTradeResult> {
    let ctx = &q.ctx;
    cycle_gate::require_execute(ctx)?;
    if req.input <= rust_decimal::Decimal::ZERO || req.min_out <= req.input {
        let why = format!(
            "граница цикла некорректна: вход {}, minOut {} (minOut должен быть больше входа)",
            req.input, req.min_out
        );
        return Err(ctx.err(ErrorCode::Internal, why));
    }
    let (legs, _block) = cycle_legs::fresh(q, &req.cycle).await?;
    let expected = cycle::eval(ctx, &legs, req.input)?.usdt;
    cycle_exec::below(ctx, expected, req.min_out)?;
    let rates = cycle_cost::rates(q, req.cycle.base).await?;
    let wallet = signer::address(ctx)?;
    let have = balances::token_units(ctx, req.cycle.base, wallet).await?;
    let need = cycle_steps::units(ctx, req.input, legs[0].dec_in)?;
    if have < need {
        let why = format!(
            "у кошелька {have} единиц {} — меньше входа цикла {need}",
            req.cycle.base_ticker
        );
        return Err(ctx.err(ErrorCode::InsufficientBalance, why));
    }
    let (res, by) = match cycle_gate::form(ctx, &legs)? {
        Form::Executor(x) => (
            cycle_exec::trade(q, env, nonces, &req, legs, (x, rates)).await,
            BY_CYCLE_EXECUTOR,
        ),
        Form::Bundle => (
            cycle_bundle_trade::trade(q, env, nonces, &req, legs, rates).await,
            BY_CYCLE_BUNDLE,
        ),
    };
    match res {
        Ok(mut r) => {
            r.trade.executor = Some(by.to_string());
            Ok(r)
        }
        Err(e) => {
            // Решение 233 (дополнение, п. 4): подпись — и в «чем найти» (у исхода неизвестен с фактом транзакции), чтобы
            // итог позднего выяснения подписался так же (exec/swap_rebuild.rs, exec/bundle_rebuild.rs — result::by).
            let mut e = super::swap_ctx::who(e, by);
            e.message = format!("[{by}] {}", e.message);
            Err(e)
        }
    }
}
