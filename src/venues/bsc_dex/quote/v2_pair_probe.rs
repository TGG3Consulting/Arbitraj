//! Файл: venues/bsc_dex/quote/v2_pair_probe.rs
//! Что делает: решение 280 — V2-форк без роутера (v2_fork_executor) в симуляциях без отдельной сверки: проба налогов
//! (check/probe.rs) и хвост сухой сделки (exec/dry_surplus_tail.rs). «Ожидалось без налога» там бралось у роутера
//! (Router.getAmountsOut — математика резервов без переводов); роутера нет — тот же расчёт: getReserves пары в той же
//! симуляции + формула V2 (math/v2.rs) с комиссией шага исполнителя (dex.v2_fee_pct; quote/v2_pair_quote.rs fee).
//! Комиссию здесь проверяет сам обмен через исполнитель в той же симуляции: пара откатила покупку — налоги ПУЛА не
//! измерены (пул в маршрут не идёт, решение 146), монета не голосует «не прошла» (buy_unmeasured → SwapReverted,
//! check/taxes.rs); откат перевода монеты из пары («…: TRANSFER_FAILED») — механика монеты, голосует как у всех. Остальные виды — как раньше (quote/quoter_calls.rs, буква в букву).
//! Кто вызывает: quote/probe_quote.rs (request_any, decode_any — проба check/probe.rs, решение 388), check/probe.rs
//! (buy_unmeasured), exec/dry_surplus_tail.rs.
//! От чего зависит: quote/{quoter_calls, v2_pair_quote, v2_pair_sim}.rs, math/v2.rs, pools/abi_v2v3.rs, check/{simulate, probe_calls}.rs.
//! Настройки: через v2_pair_quote::fee — dex.v2_fee_pct [DEX].

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::gateway::errors::GatewayResult;

use super::super::check::probe_calls;
use super::super::check::simulate::CallResult;
use super::super::ctx::Ctx;
use super::super::math::v2;
use super::super::pools::abi_v2v3::IV2Pair;
use super::super::pools::registry::Entry;
use super::v2_pair_quote::{fee, on, pair};
use super::v2_pair_sim;

/// «Ожидалось без налога» у вида без роутера: вызов getReserves пары. Ошибки: NotConfigured (комиссия — проверяется
/// заранее, чтобы разбор не молчал), Internal (пул не контракт).
pub fn quote_request(ctx: &Ctx, e: &Entry, token_in: Address) -> GatewayResult<(Address, Bytes)> {
    fee(ctx, e, token_in)?;
    Ok((
        pair(ctx, e)?,
        Bytes::from(IV2Pair::getReservesCall {}.abi_encode()),
    ))
}

/// Разбор ответа quote_request: формула V2 с нашей комиссией. Принимает: ответ, токен входа, сумму. Возвращает: выход;
/// None — не по ABI, нулевые резервы или комиссия не задана. Ошибок нет.
pub fn quote_decode(
    ctx: &Ctx,
    e: &Entry,
    raw: &[u8],
    token_in: Address,
    amount: U256,
) -> Option<U256> {
    let x = IV2Pair::getReservesCall::abi_decode_returns(raw).ok()?;
    let (a, b) = (U256::from(x.reserve0), U256::from(x.reserve1));
    let (ri, ro) = if token_in == e.pool.token0 {
        (a, b)
    } else {
        (b, a)
    };
    v2::amount_out(amount, ri, ro, fee(ctx, e, token_in).ok()?.1)
}

/// Вызов «ожидалось без налога» для симуляции любого пула: у вида без роутера — quote_request, иначе квотер DEX
/// (quote/quoter_calls.rs). Ошибки: как у вызываемых.
pub fn request_any(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    if on(e) {
        quote_request(ctx, e, token_in)
    } else {
        super::quoter_calls::request(ctx, e, token_in, amount)
    }
}

/// Разбор ответа request_any. None — не по ABI (или комиссия не задана у вида без роутера). Ошибок нет.
pub fn decode_any(
    ctx: &Ctx,
    e: &Entry,
    raw: &[u8],
    token_in: Address,
    amount: U256,
) -> Option<U256> {
    if on(e) {
        quote_decode(ctx, e, raw, token_in, amount)
    } else {
        super::quoter_calls::decode(e, raw)
    }
}

/// Откат первой симуляции пробы у пула вида без роутера: «ожидалось» (getReserves) или покупка шагом V2 исполнителя
/// (номер шага ≥ шага «ожидалось») — свойство ПУЛА (пара отклонила обмен: K — комиссия пары выше dex.v2_fee_pct, или
/// обмен нестандартный), не монеты: налоги пула не измерены. Откат перевода монеты из пары (Error(string)
/// «…: TRANSFER_FAILED» — _safeTransfer пары V2: монета отказала в переводе — запрет торговли, антибот; ревью 280) —
/// механика МОНЕТЫ: голосует как у обычных пулов (None → check/probe_calls.rs buy_revert). Срок транзакции (deadline
/// исполнителя) и шаги до «ожидалось» (перевод USDT, разрешения) — как у всех. Принимает: пул, номер отката, номер
/// «ожидалось», итог. Возвращает: причину или None — решать как у всех. Ошибок нет.
pub fn buy_unmeasured(e: &Entry, i: usize, quote: usize, r: &CallResult) -> Option<String> {
    if !on(e) || i < quote || probe_calls::late(r) || v2_pair_sim::coin_transfer(r) {
        return None;
    }
    Some(format!(
        "{}: пробная покупка шагом V2 исполнителя откатилась (шаг {i}) — налоги пула не измерены, пул в маршрут не идёт \
         (пара отклонила обмен: комиссия пары выше dex.v2_fee_pct или обмен нестандартный, решение 280): {}",
        e.family.title,
        r.why()
    ))
}
