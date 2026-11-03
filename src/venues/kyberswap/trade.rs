//! Файл: venues/kyberswap/trade.rs
//! Что делает: своп через роутер агрегатора KyberSwap. Решение 123: две части — «получить маршрут» (trade_prep.rs:
//! свежий GET /routes на объём сделки, POST /route/build со slippageTolerance и deadline, проверка худшей цены) и
//! «исполнить» (execute здесь: при нехватке разрешения — ERC-20 approve роутеру из ответа и пересборка, отправка
//! свопа, ожидание квитанции, факт по логам Transfer — trade_fact.rs). Режим «лучший из двух» (bsc_dex/best_of/)
//! сравнивает подготовленный маршрут со своим и исполняет его этим же execute — без второго запроса к KyberSwap.
//! Маршрут берётся свежим на сделке (документация: маршрут живёт секунды) — req.route из котировки не используется.
//! Не исполнилось — ошибка с фактом транзакции (газ мог сгореть), повторов нет (стартовое правило Тиграна).
//! Кто вызывает: bsc_dex/venue.rs (trade), bsc_dex/best_of/trade.rs (execute).
//! От чего зависит: ctx.rs, state.rs, trade_prep.rs, trade_fact.rs, tx.rs, node.rs, abi.rs.
//! Настройки: через trade_prep.rs — kyber.slippage_bps, kyber.deadline_s; через tx.rs — kyber.gas_*, kyber.receipt_*.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{TradeRequest, TradeResult, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::abi::IERC20;
use super::ctx::Ctx;
use super::state::Parts;
use super::trade_prep::{self, Prepared};
use super::{node, trade_fact, tx};

pub use super::trade_fact::moved;

/// Своп на объём req.qty: строгая очередь кошелька, подготовка (trade_prep.rs) и исполнение. Ошибки: NoRoute,
/// QuoteMismatch (цена хуже допустимой), SwapReverted / TxNotIncluded / OutcomeUnknown (с фактом транзакции), узел, сеть.
pub async fn trade(p: &Parts, req: TradeRequest) -> GatewayResult<TradeResult> {
    // Строгая очередь кошелька: маршрут, approve и своп одной сделки идут без параллельных транзакций (решение 45).
    let _guard = p.wallet_lock.lock().await;
    let prep = trade_prep::prepare(p, &req).await?;
    execute(p, req, prep).await
}

/// Исполнение подготовленного свопа. Очередь кошелька (p.wallet_lock) держит ВЫЗЫВАЮЩИЙ (замок не повторный).
/// Принимает: состояние, запрос, подготовку (сборка уже прошла проверку худшей цены). Возвращает: факт свопа.
/// Ошибки: QuoteMismatch (после approve маршрут требует больше входа / цена хуже), SwapReverted / TxNotIncluded /
/// OutcomeUnknown (с фактом транзакции), узел, сеть.
pub async fn execute(p: &Parts, req: TradeRequest, prep: Prepared) -> GatewayResult<TradeResult> {
    let ctx = &p.ctx;
    let (wallet, token_in) = (prep.wallet, prep.token_in);
    let mut built = prep.built;
    // Разрешение на списание входного токена роутером (ERC-20 approve), если его не хватает; у свопа с нативной
    // монетой на входе (value > 0) approve не нужен.
    if built.value.is_zero()
        && ensure_allowance(
            ctx,
            &p.nonce_next,
            token_in,
            wallet,
            built.router,
            built.amount_in,
        )
        .await?
    {
        // Решение 99: approve — отдельная транзакция, ждём её блока (секунды). Маршрут и транзакция свопа, собранные
        // ДО него, к этому моменту устарели — симуляция свопа откатывалась «Return amount is not enough» (сделка
        // 20261101182135312CAKE). Собираем заново на свежем маршруте и снова сверяем худшую цену.
        let pair = (prep.coin, prep.usdt);
        built = trade_prep::build(p, &req, pair, prep.decs, wallet, prep.slippage)
            .await?
            .0;
        // Новая сборка могла потребовать больше входа (покупка) — второй approve не делаем (снова устареет маршрут).
        if allowance(ctx, token_in, wallet, built.router).await? < built.amount_in {
            return Err(ctx.err(
                ErrorCode::QuoteMismatch,
                "после approve свежий маршрут требует больше входа, чем разрешено — своп не отправлен",
            ));
        }
    }

    let d = tx::send(
        ctx,
        &p.nonce_next,
        wallet,
        built.router,
        built.data.clone(),
        built.value,
    )
    .await?;
    outcome_ok(ctx, &d)?;
    let r = d.receipt.clone().ok_or_else(|| {
        ctx.err(ErrorCode::OutcomeUnknown, "своп в блоке, но нет квитанции")
            .with_tx(d.chain.clone())
    })?;
    let tokens = (token_in, prep.token_out);
    trade_fact::result(ctx, req, d, &r, wallet, tokens, prep.decs)
}

/// Текущее разрешение роутеру на списание токена (ERC-20 allowance). Ошибки: узел.
async fn allowance(
    ctx: &Ctx,
    token: Address,
    wallet: Address,
    router: Address,
) -> GatewayResult<U256> {
    node::view(
        ctx,
        token,
        &IERC20::allowanceCall {
            owner: wallet,
            spender: router,
        },
    )
    .await
}

/// Разрешение на списание токена роутером: читает allowance, при нехватке — approve(router, amount) и ждёт блока.
/// Принимает: окружение, запомненный nonce (tx.rs), токен, кошелёк, роутер, сумму.
/// Возвращает: true — approve отправлялся (прошло время, маршрут свопа надо собрать заново).
/// Ошибки: узел, SwapReverted (approve не прошёл — с фактом транзакции), OutcomeUnknown.
async fn ensure_allowance(
    ctx: &Ctx,
    next: &std::sync::Mutex<Option<u64>>,
    token: Address,
    wallet: Address,
    router: Address,
    amount: U256,
) -> GatewayResult<bool> {
    if allowance(ctx, token, wallet, router).await? >= amount {
        return Ok(false);
    }
    let data = Bytes::from(
        IERC20::approveCall {
            spender: router,
            value: amount,
        }
        .abi_encode(),
    );
    let d = tx::send(ctx, next, wallet, token, data, U256::ZERO).await?;
    match d.chain.outcome {
        TxOutcome::Included => Ok(true),
        TxOutcome::NotIncluded => Err(ctx
            .err(ErrorCode::TxNotIncluded, "approve не попал в блок")
            .with_tx(d.chain)),
        _ => Err(ctx
            .err(ErrorCode::SwapReverted, "approve не прошёл")
            .with_tx(d.chain)),
    }
}

/// Проверяет исход транзакции свопа. Ошибки: SwapReverted / TxNotIncluded (с фактом транзакции).
fn outcome_ok(ctx: &Ctx, d: &tx::Done) -> GatewayResult<()> {
    match d.chain.outcome {
        TxOutcome::Included => Ok(()),
        TxOutcome::Reverted => Err(ctx
            .err(
                ErrorCode::SwapReverted,
                format!("Своп откатился в блоке (газ сгорел), tx {}", d.chain.hash),
            )
            .with_tx(d.chain.clone())),
        _ => Err(ctx
            .err(
                ErrorCode::TxNotIncluded,
                format!("Своп не попал в блок за срок, tx {}", d.chain.hash),
            )
            .with_tx(d.chain.clone())),
    }
}
