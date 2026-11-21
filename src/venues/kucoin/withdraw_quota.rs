//! Файл: venues/kucoin/withdraw_quota.rs
//! Что делает: решение 166 (Тигран, 05.11.2026) — сколько котируемой монеты KuCoin держит от вывода прямо сейчас, по
//! квоте вывода. Живой случай 05.11 (сделка 20261105020010981ION, kucoin → mexc): покупка 905432 ION на USDT, недавно
//! вернувшиеся на KuCoin с DEX; вывод — «KuCoin, код 115008: Insufficient withdrawable assets: Assets equivalent to 44.91
//! USDT (898299.261 ION) are currently locked from withdrawal…». Удержание живёт по стоимости в USDT и переходит на
//! купленную монету — поэтому до покупки спрашиваем квоту по котируемой монете (USDT) и вычитаем lockedAmount.
//! Кто вызывает: mod.rs (withdraw_hold из контракта).
//! От чего зависит: rest.rs (Pool::Management), num.rs, ctx.rs, errors.rs, core/types (WithdrawHold).
//! Настройки: venue.bsc_network [kucoin] — chain запроса (chainId сети BSC, как у вывода).
//!
//! Официальная документация: «Get Withdrawal Quotas» GET /api/v1/withdrawals/quotas
//! https://www.kucoin.com/docs-new/rest/account-info/withdrawals/get-withdrawal-quotas (спецификация SDK
//! kucoin-universal-sdk spec/rest/api/openapi-account-withdrawal.json, x-api-doc …/api-3470143): права General, пул
//! Management, вес 20; currency* , chain («The chainId of currency»). Поля data (все обязательные): availableAmount
//! «Current available withdrawal amount», remainAmount «Remaining amount available to withdraw the current day»,
//! lockedAmount «Total locked amount (including the amount locked into USDT for each currency)», reason «Reasons for
//! restriction. Usually empty.», isWithdrawEnabled, quotaCurrency и др.
//! Почему lockedAmount, а не availableAmount: вывод KuCoin идёт только с main (115007), а торгуем на trade — какой счёт
//! считает availableAmount, в документации не сказано; «можно вывести = свободно на trade − удержано» (правило решения
//! 150) не блокирует торговлю, когда USDT просто лежат на trade. СВЕРИТЬ на живом ответе: в случае 115008 lockedAmount
//! по USDT должен быть ≈ 44.91 (единица — USDT при currency = USDT по описанию поля).

use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{HoldItem, WithdrawHold};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::reply::Pool;
use super::{num, rest};

/// Удержание вывода котируемой монеты сейчас. Принимает: котируемую монету (USDT). Возвращает: удержание
/// (lockedAmount > 0) или пусто. Ошибки: настройка venue.bsc_network, ключ, связь, лимит, непонятный ответ.
pub async fn withdraw_hold(ctx: &Ctx, quote: &str) -> GatewayResult<WithdrawHold> {
    let chain = ctx.text("venue.bsc_network")?;
    let q = vec![("currency", quote.to_string()), ("chain", chain)];
    let data = rest::signed(
        ctx,
        Method::GET,
        "/api/v1/withdrawals/quotas",
        q,
        None,
        Op::Other,
        Pool::Management,
    )
    .await?;
    parse(&data)
}

/// Разбор data ответа quotas. Принимает: data. Возвращает: удержание lockedAmount в монете currency (пусто при 0).
/// Ошибки: BadResponse — нет currency или lockedAmount, не число.
pub fn parse(data: &Value) -> GatewayResult<WithdrawHold> {
    let asset = num::text(data, "currency")?;
    let locked = num::dec(data, "lockedAmount")?;
    if locked <= Decimal::ZERO {
        return Ok(WithdrawHold::of(vec![]));
    }
    let field = |f: &str| num::text_opt(data, f).unwrap_or_else(|| "не указано".into());
    let why = num::text_opt(data, "reason")
        .filter(|r| !r.is_empty())
        .map(|r| format!(", причина KuCoin: {r}"))
        .unwrap_or_default();
    let reason = format!(
        "KuCoin держит от вывода {locked} {asset} (квота вывода: lockedAmount; можно вывести сейчас {}, \
         остаток дневного предела {}{why})",
        field("availableAmount"),
        field("remainAmount")
    );
    Ok(WithdrawHold::of(vec![HoldItem {
        asset,
        amount: locked,
        reason,
    }]))
}
