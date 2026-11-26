//! Файл: venues/bsc_dex/pools/load_woo_ladder.rs
//! Что делает: «лесенка цен пула» WooFi (решение 122 F, 3; выбор Тиграна) на блоке — ответы квотера самого контракта
//! `query(from, to, amount)` (IWooPPV2.sol L91) на лесенке объёмов dex.pmm_ladder_usd в обе стороны: проход 1 —
//! котируемая (USDT) → базовая на суммах лесенки; проход 2 — базовая → котируемая на полученных в проходе 1 количествах
//! базовой (тот же объём в долларах). Оба прохода — Multicall3 на одном блоке. Откат квотера в точке (пределы пула) —
//! край лесенки: точек после него нет. Своих формул пула нет — только ответы квотера.
//! Кто вызывает: pools/load_woo.rs; usd и usdt_quote — ещё pools/ladder_counter.rs (решение 306).
//! От чего зависит: ctx.rs, node/call.rs, tokens.rs, abi_woo.rs, ladder_counter.rs, registry.rs, core/types (LadderPoint).
//! Настройки: dex.pmm_ladder_usd, dex.pmm_ladder_counter [DEX] (не задана — лесенки нет, пара остаётся сигналом).
//! Решение 297: лесенка — только у пары, где встречный токен = USDT площадки (tokens.rs usdt, venue.quote_asset) —
//! правило usdt_quote, общее для WooFi, Metric, iZiSwap / Biswap V3 (load_metric_ladder.rs). Решение 306: при
//! dex.pmm_ladder_counter = да — и пары не с USDT, суммы во встречном токене по курсу к USDT (ladder_counter.rs).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use rust_decimal::Decimal;

use crate::core::types::LadderPoint;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::super::tokens::Tokens;
use super::abi_woo::IWooPP;
use super::ladder_counter;
use super::registry::Registry;

/// Лесенка в долларах из dex.pmm_ladder_usd: суммы > 0 по возрастанию. Принимает: окружение, id DEX.
/// Возвращает: список; пусто — настройка не задана. Ошибки: NotConfigured — значение не число > 0 или не по возрастанию.
pub fn usd(ctx: &Ctx, id: &str) -> GatewayResult<Vec<Decimal>> {
    let list = match ctx.dex_list("dex.pmm_ladder_usd", id) {
        Ok(l) => l,
        Err(e) if e.code == ErrorCode::NotConfigured => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    parse_sizes(ctx, "dex.pmm_ladder_usd", &list)
}

pub(in crate::venues::bsc_dex) fn parse_sizes(
    ctx: &Ctx,
    setting: &str,
    list: &[String],
) -> GatewayResult<Vec<Decimal>> {
    let mut out: Vec<Decimal> = Vec::new();
    for s in list {
        let v: Decimal = s
            .trim()
            .parse()
            .map_err(|_| ctx.not_set(setting, format!("«{s}» — не число (сумма USDT)")))?;
        if v <= Decimal::ZERO || out.last().is_some_and(|l| *l >= v) {
            return Err(ctx.not_set(setting, "суммы лесенки — больше нуля и по возрастанию"));
        }
        out.push(v);
    }
    Ok(out)
}

/// Решение 297: строится ли лесенка у пары. Суммы dex.pmm_ladder_usd — в USDT, поэтому только встречный токен = сам
/// USDT площадки. Встречный токен записи реестра (Entry.usdt) — «USDT или промежуточная» (решение 52): у пары
/// монета / WBNB это WBNB, и суммы стали бы WBNB (живой бот 15.11.2026, пулы CYBER/WBNB и ETC/WBNB — ложная пауза
/// сверки с квотером). Принимает: USDT площадки (None — не задан / нет в списке монет), встречный токен пары.
/// Возвращает: true — лесенка строится. Ошибок нет.
pub fn usdt_quote(usdt: Option<Address>, quote: Address) -> bool {
    usdt == Some(quote)
}

/// Обе стороны лесенки на блоке. Принимает: окружение, токены, реестр (курс встречного к USDT, решение 306), id DEX,
/// контракт, базовую, котируемую (встречный токен пары), тег. Возвращает: (котируемая → базовая, базовая →
/// котируемая); пусто — лесенки нет (не задана; котируемая не USDT площадки и dex.pmm_ladder_counter не «да» или нет
/// курса — ladder_counter.rs amounts, решения 297 и 306). Ошибки: узел, ответ не по ABI, неверная dex.pmm_ladder_usd.
#[allow(clippy::too_many_arguments)]
pub async fn read(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    id: &str,
    pool: Address,
    coin: Address,
    quote: Address,
    tag: &str,
) -> GatewayResult<(Vec<LadderPoint>, Vec<LadderPoint>)> {
    let amounts = ladder_counter::amounts(ctx, tokens, reg, id, quote).await?;
    if amounts.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let quote_in = pass(ctx, pool, quote, coin, &amounts, tag).await?;
    let base_amounts: Vec<U256> = quote_in.iter().map(|p| p.amount_out).collect();
    let base_in = pass(ctx, pool, coin, quote, &base_amounts, tag).await?;
    Ok((quote_in, base_in))
}

/// Один проход: query(from, to, amount) по суммам; точки — до первого отката или нулевого выхода. Ошибки: узел.
async fn pass(
    ctx: &Ctx,
    pool: Address,
    from: Address,
    to: Address,
    amounts: &[U256],
    tag: &str,
) -> GatewayResult<Vec<LadderPoint>> {
    let calls: Vec<(Address, Bytes)> = amounts
        .iter()
        .map(|a| {
            let q = IWooPP::queryCall {
                fromToken: from,
                toToken: to,
                fromAmount: *a,
            };
            (pool, q.abi_encode().into())
        })
        .collect();
    let got = call::try_many(ctx, &calls, tag).await?;
    let mut out = Vec::new();
    for (a, r) in amounts.iter().zip(got) {
        let Ok(b) = r else { break };
        let o = call::decode::<IWooPP::queryCall>(&b)?;
        if o.is_zero() || a.is_zero() || out.last().is_some_and(|p: &LadderPoint| p.amount_in >= *a)
        {
            break;
        }
        out.push(LadderPoint {
            amount_in: *a,
            amount_out: o,
        });
    }
    Ok(out)
}
