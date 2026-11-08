//! Файл: venues/bsc_dex/exec/balance_fact.rs
//! Что делает: решение 222 — факт свопа DEX по монете сделки по ИЗМЕНЕНИЮ БАЛАНСА кошелька, а не по событиям Transfer.
//! Живой случай 08.11 (сделка 20261108094918374PIT, mexc → DEX, кошелёк w2): у токена-рефлексии (тип SafeMoon) налог
//! списывается с баланса отправителя без отдельного события Transfer — по логам «продано» 291889628787.9168 PIT, а с
//! баланса ушло 304051696654.08 (= 291.89 / 0.96); в учёте остался «фантомный остаток» 12162067866.1632, повторы
//! продажи откатывались. Теперь: продажа — списано с кошелька = balanceOf(блок−1) − balanceOf(блок), покупка — зачислено
//! = balanceOf(блок) − balanceOf(блок−1); средняя цена = USDT по логам / это количество (так же, как средняя цена
//! котировки продажи: выход USDT / монет на входе, quote/costs.rs). USDT и промежуточные монеты — по логам, как раньше
//! (у них налога нет — расхождение было только по монете сделки).
//! Честность блока: изменение баланса — наше, только если в блоке нет ДРУГИХ переводов этой монеты с кошелька и на
//! него (другая наша транзакция, поступление с биржи). Проверка — события Transfer монеты с кошелька и на кошелёк во
//! всём блоке (eth_getLogs): их итог должен совпасть с итогом по квитанции сделки. Не совпал, узел не отдал состояние
//! блока−1, суммы не читаются — факт остаётся по логам, запись dex.balance_fact в журнал с суммами (сверка остатка перед
//! продажей остатка — core/remain_fact.rs — страхует такой случай). Логи Transfer остаются для разбора и сверки.
//! Нативная BNB (решение 62) — как раньше (газ платится тем же BNB, баланс до/после его включает).
//! Кто вызывает: venue.rs (контракт trade — все пути: свой сборщик, KyberSwap, «лучший из двух»).
//! От чего зависит: exec/balance_read.rs (пачка JSON-RPC: логи блока и balanceOf), tokens.rs, num.rs, wallet/signer.rs,
//! exec/native.rs, core/types.
//! Настройки: нет.
//!
//! balanceOf(owner) — https://eips.ethereum.org/EIPS/eip-20#balanceof ; Transfer — https://eips.ethereum.org/EIPS/eip-20#transfer-1 ;
//! eth_call(transaction, blockNumber) и eth_getLogs(filter {fromBlock, toBlock, address, topics}) —
//! https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth (execute.yaml, filter.yaml).
//! Состояние недавних блоков у полного узла geth (схема path): последние 128 блоков — в памяти (diff-слои), «the
//! persistent state is 128 blocks in the past» — https://geth.ethereum.org/docs/fundamentals/archive ; чтение идёт
//! сразу после квитанции (блок−1 — в пределах этих 128), иначе — откат на логи с записью в журнал.

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{Side, TradeResult, TradeStatus, TxOutcome};
use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::num;
use super::super::tokens::Tokens;
use super::super::wallet::signer;

/// Что видно по блоку сделки (минимальные единицы монеты): баланс кошелька до блока и в блоке, события Transfer
/// монеты во всём блоке — ушло с кошелька и пришло на него.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seen {
    pub before: U256,
    pub after: U256,
    pub block_out: U256,
    pub block_in: U256,
}

/// Количество монеты сделки по балансу. Принимает: сторону, итог по квитанции сделки (продажа — ушло с кошелька минус
/// вернулось, покупка — пришло минус ушло), то, что видно по блоку. Возвращает: продажа — списано с кошелька, покупка —
/// зачислено. Ошибки (текст — факт остаётся по логам): в блоке есть другие переводы монеты кошелька; баланс
/// изменился не в ту сторону.
pub fn judge(side: Side, ours: U256, s: &Seen) -> Result<U256, String> {
    let (net, delta) = match side {
        Side::Buy => (
            s.block_in.checked_sub(s.block_out),
            s.after.checked_sub(s.before),
        ),
        Side::Sell => (
            s.block_out.checked_sub(s.block_in),
            s.before.checked_sub(s.after),
        ),
    };
    if net != Some(ours) {
        return Err(format!(
            "в блоке есть другие переводы монеты кошелька: по событиям блока ушло {}, пришло {}, по квитанции сделки {ours}",
            s.block_out, s.block_in
        ));
    }
    delta.ok_or_else(|| {
        format!(
            "баланс кошелька изменился не в сторону сделки: до {}, после {}",
            s.before, s.after
        )
    })
}

/// Итог сделки с количеством по балансу: продажа — не больше запрошенного (больше — монеты не этой сделки), средняя
/// цена = USDT / количество, статус заново; пометка в executor (хронология сделки). Принимает: итог по логам, новое
/// количество. Возвращает: итог. Ошибок нет.
pub fn rewrite(mut t: TradeResult, qty: Decimal) -> TradeResult {
    let qty = match t.request.side {
        Side::Sell => qty.min(t.request.qty),
        Side::Buy => qty,
    };
    if qty == t.filled_qty || qty.is_zero() {
        return t;
    }
    let usdt = t.avg_price * t.filled_qty;
    let what = if t.request.side == Side::Sell {
        "списано с кошелька"
    } else {
        "зачислено на кошелёк"
    };
    let note = format!(
        "{what} {qty} (по событиям Transfer {}) — по балансу, решение 222",
        t.filled_qty
    );
    t.executor = Some(match t.executor.take() {
        Some(e) => format!("{e}; {note}"),
        None => note,
    });
    t.filled_qty = qty;
    t.avg_price = usdt / qty;
    t.status = if qty < t.request.qty {
        TradeStatus::PartiallyFilled
    } else {
        TradeStatus::Filled
    };
    t
}

/// Поправка итога сделки по балансу (решение 222). Принимает: окружение, токены, итог сделки. Возвращает: тот же итог
/// (ошибка, транзакция не в блоке, нативная BNB, нет монет по логам) или с количеством по балансу. Ошибок своих нет:
/// не вышло — итог по логам и запись dex.balance_fact в журнал.
pub async fn apply(
    ctx: &Ctx,
    tokens: &Tokens,
    r: GatewayResult<TradeResult>,
) -> GatewayResult<TradeResult> {
    let t = r?;
    let (Some(chain), Some(token)) = (t.chain.as_ref(), t.request.asset.bsc_contract) else {
        return Ok(t);
    };
    let Some(block) = chain.block.filter(|_| chain.outcome == TxOutcome::Included) else {
        return Ok(t);
    };
    if t.filled_qty.is_zero() || super::native::is_native(ctx, &t.request.asset) {
        return Ok(t);
    }
    let hash = chain.hash;
    match by_balance(ctx, tokens, &t, token, block).await {
        Ok(q) if q == t.filled_qty => Ok(t),
        Ok(q) => {
            let msg = format!(
                "{} {:?}, tx {hash}: по событиям Transfer {}, по балансу кошелька {q} — учёт по балансу",
                t.request.asset.ticker, t.request.side, t.filled_qty
            );
            ctx.log(Level::Warn, "dex.balance_fact", msg, None).await;
            Ok(rewrite(t, q))
        }
        Err(why) => {
            let msg = format!(
                "{} {:?}, tx {hash}: факт по событиям Transfer ({}) — по балансу не проверен: {why}",
                t.request.asset.ticker, t.request.side, t.filled_qty
            );
            ctx.log(Level::Warn, "dex.balance_fact", msg, None).await;
            Ok(t)
        }
    }
}

/// Количество по балансу кошелька на блоке сделки. Ошибки — текст (узел, состояние блока−1, разбор, judge).
async fn by_balance(
    ctx: &Ctx,
    tokens: &Tokens,
    t: &TradeResult,
    token: Address,
    block: u64,
) -> Result<Decimal, String> {
    let wallet = signer::address(ctx).map_err(|e| e.message)?;
    let dec = tokens.decimals(ctx, token).await.map_err(|e| e.message)?;
    let ours = num::to_units(t.filled_qty, dec).ok_or("количество по логам не в единицах")?;
    let s = super::balance_read::read(ctx, token, wallet, block).await?;
    let q = judge(t.request.side, ours, &s)?;
    num::to_decimal(q, dec).ok_or_else(|| format!("количество {q} (decimals {dec}) не в числе"))
}
