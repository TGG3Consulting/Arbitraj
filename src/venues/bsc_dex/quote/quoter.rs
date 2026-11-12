//! Файл: venues/bsc_dex/quote/quoter.rs
//! Что делает: выход обмена по квотеру своей DEX через eth_call (точный вход) — для сверки локальной математики
//! и для измерения влияния крючка: V2 — Router.getAmountsOut; V3 — QuoterV2; Infinity CL/Bin — CLQuoter/BinQuoter;
//! Uniswap V4 — V4Quoter. Квотер исполняет настоящий обмен (с крючками) и откатывается — research/flow/bsc_v4.md, раздел 5.
//! Сверка: |локально − квотер| / квотер · 100 > dex.quoter_max_diff_pct. Периодическая сверка пулов (решение 261) —
//! по модулю на ОДНОМ блоке (out_at, check/quoter_compare.rs): расхождение → пауза стороны пула. Перед сделкой
//! (решение 261) — односторонне: выход сети (квотер) больше нашего — в нашу сторону, не стоп (Judged::favours_us).
//! Решение 177: judge отдаёт цифры расхождения (Judged::Diff) — продажа выхода продаёт по выходу квотера (exec/exit_quote.rs).
//! У пула с крючком B/C квотер исполняет крючок, а математика — нет: локальный выход берётся за вычетом измеренного
//! влияния крючка (check/hooks_probe.rs) — так сверяются и математика пула, и то, что крючок ведёт себя как при пробе.
//! Infinity Stable (решение «122 (Infinity Stable, ответы)»): основной квотер — CLQuoter, дополнительный — get_dy крючка
//! (quote/quoter_extra.rs), оба параллельно; расхождение или отказ любого — пул не сверен.
//! Кто вызывает: check/quoter_compare.rs (цикл dex.quoter_check_s, out_at), check/hooks_probe.rs, exec/precheck.rs
//! (перед сделкой), quote/mixed_quoter.rs (less_hook, diff_pct), quote/quoter_extra.rs (judge).
//! От чего зависит: node/rpc.rs, quote/quoter_calls.rs, quote/virtual_quoter.rs, pools/registry.rs.
//! Решение 186: dex.quoter_virtual [DEX] = да — квотер DEX не нужен, выход считает код самого пула (virtual_quoter.rs).
//! Настройки: dex.router (V2), dex.quoter (V3, V4) [вид пула], dex.quoter_virtual [DEX], dex.quoter_max_diff_pct [bsc_dex].
//!
//! Источники: шапка node/abi_dex.rs; «When to use quoter: сверка локального расчёта, финальная проверка» —
//! research/flow/bsc_dex.md, раздел 3, bsc_v4.md, раздел 5. eth_call(transaction, block): block — номер блока (hex) или
//! тег «latest» (BlockNumberOrTag) — execution-apis
//! https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth (eth_call).

use alloy_primitives::{Address, Bytes, U256};
use rust_decimal::Decimal;
use serde_json::json;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::rpc;
use super::super::pools::registry::Entry;
use super::quoter_calls;

/// Выход по квотеру на точный вход (eth_call на «latest»). Ошибки: NotConfigured (адрес), узел (откат квотера — его
/// текст), BadResponse (ответ не по ABI).
pub async fn out(ctx: &Ctx, e: &Entry, token_in: Address, amount: U256) -> GatewayResult<U256> {
    out_at(ctx, e, token_in, amount, "latest").await
}

/// Выход по квотеру на точный вход на заданном блоке (решение 261: периодическая сверка — на блоке состояния пула).
/// Принимает: запись, токен входа, сумму, блок (номер hex — node/call.rs block_tag — или «latest»). Ошибки: как у out;
/// StaleData — состояния блока у узла нет (node errors.rs from_rpc).
pub async fn out_at(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
    block: &str,
) -> GatewayResult<U256> {
    // Решение 186: DEX без своего квотера — выход по коду самого пула (quote/virtual_quoter.rs).
    if super::virtual_quoter::on(ctx, e.family.id, e.family.proto) {
        return super::virtual_quoter::out_at(ctx, e, token_in, amount, block).await;
    }
    let (to, data) = quoter_calls::request(ctx, e, token_in, amount)?;
    let raw: Bytes =
        rpc::call(ctx, "eth_call", json!([{ "to": to, "input": data }, block])).await?;
    quoter_calls::decode(e, &raw).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("{}: ответ квотера не по ABI", e.family.title),
        )
    })
}

/// Итог сверки на одной сумме (решение 177 — чтобы продажа выхода знала цифры расхождения).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Judged {
    /// В пределах dex.quoter_max_diff_pct.
    Within,
    /// Расхождение больше допуска: локальный выход за вычетом крючка, выход квотера, расхождение %, допуск %.
    Diff {
        local: U256,
        remote: U256,
        pct: Decimal,
        limit: Decimal,
    },
    /// Сравнить нельзя (влияние крючка не считается, квотер 0, числа не помещаются) — причина.
    Bad(String),
}

impl Judged {
    /// Причина «не в допуске» — тот же текст, что был у within до решения 177. None — в допуске. Ошибок нет.
    pub fn reason(&self) -> Option<String> {
        match self {
            Judged::Within => None,
            Judged::Diff {
                local,
                remote,
                pct,
                limit,
            } => Some(format!(
                "расчёт {local} против квотера {remote}: расхождение {pct}% > {limit}% (dex.quoter_max_diff_pct)"
            )),
            Judged::Bad(why) => Some(why.clone()),
        }
    }

    /// Решение 261 (Тигран 12.11: «если в нашу сторону то ок если наоборот то не делаем»): расхождение больше допуска,
    /// но выход сети (квотер) БОЛЬШЕ нашего расчёта. Все квотеры — на точный вход: выход — то, что мы получаем (покупка —
    /// монеты, продажа — USDT, промежуточный шаг — промежуточная монета), больше — лучше для нас. Ошибок нет.
    pub fn favours_us(&self) -> bool {
        matches!(self, Judged::Diff { local, remote, .. } if remote > local)
    }

    /// Строка «в нашу сторону» для хронологии сделки: наш расчёт, квотер, %. None — не такое расхождение. Ошибок нет.
    pub fn favour_text(&self) -> Option<String> {
        match self {
            Judged::Diff {
                local, remote, pct, ..
            } if remote > local => Some(format!(
                "расчёт {local}, квотер {remote} (+{}%) — в нашу сторону, сделка идёт (решение 261)",
                pct.round_dp(4)
            )),
            _ => None,
        }
    }
}

/// Локальный выход (за вычетом крючка) против выхода квотера: в пределах dex.quoter_max_diff_pct?
/// Возвращает: Ok(()) или Err(причина с цифрами). Ошибки: NotConfigured (допуск).
pub fn within(
    ctx: &Ctx,
    local: U256,
    remote: U256,
    hook_pct: Decimal,
) -> GatewayResult<Result<(), String>> {
    Ok(match judge(ctx, local, remote, hook_pct)?.reason() {
        None => Ok(()),
        Some(why) => Err(why),
    })
}

/// Сверка с цифрами (within — её краткая форма). Принимает: локальный выход (до вычета крючка), выход квотера,
/// влияние крючка %. Возвращает: Judged. Ошибки: NotConfigured (допуск).
pub fn judge(ctx: &Ctx, local: U256, remote: U256, hook_pct: Decimal) -> GatewayResult<Judged> {
    let limit = ctx.dec("dex.quoter_max_diff_pct")?;
    let Some(local) = less_hook(local, hook_pct) else {
        return Ok(Judged::Bad(format!(
            "влияние крючка {hook_pct}% не считается"
        )));
    };
    Ok(match diff_pct(local, remote) {
        Some(d) if d <= limit => Judged::Within,
        Some(pct) => Judged::Diff {
            local,
            remote,
            pct,
            limit,
        },
        None => Judged::Bad(format!(
            "расчёт {local} против квотера {remote}: сравнить нельзя"
        )),
    })
}

/// Выход за вычетом влияния крючка: out · (1e6 − pct·1e4) / 1e6 (миллионные доли, как пипсы комиссий).
/// Отрицательное влияние (крючок даёт больше) не прибавляется. None — pct больше 100% или не число.
pub fn less_hook(out: U256, pct: Decimal) -> Option<U256> {
    let million = U256::from(1_000_000u32);
    let ppm = (pct.max(Decimal::ZERO) * Decimal::from(10_000u32)).round();
    let ppm = U256::from(u64::try_from(ppm).ok()?);
    if ppm > million {
        return None;
    }
    Some(out.saturating_mul(million - ppm) / million)
}

/// |a − b| / b · 100. None — b = 0 или числа не помещаются в Decimal.
pub fn diff_pct(a: U256, b: U256) -> Option<Decimal> {
    let (a, b) = (to_dec(a)?, to_dec(b)?);
    if b.is_zero() {
        return None;
    }
    Some((a - b).abs() / b * Decimal::ONE_HUNDRED)
}

/// U256 → Decimal без масштаба. None — больше 28 знаков.
fn to_dec(v: U256) -> Option<Decimal> {
    super::super::num::to_decimal(v, 0)
}
