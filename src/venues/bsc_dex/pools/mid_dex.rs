//! Файл: venues/bsc_dex/pools/mid_dex.rs
//! Что делает: решение 276 — подпись промежуточной монеты, адрес которой не знает ни одна биржа (при
//! dex.intermediate_dex_priced = да). Тикер промежуточной нужен только как подпись затрат и шагов (quote/steps.rs
//! Names::of): затраты шага в ней — «в цене» (in_price), ядро курс для них не требует (core/costs.rs); цена и глубина
//! — пулом «промежуточная — USDT» (quote/rate.rs). Подпись — symbol() токена с цепи. ОПАСНОСТЬ: ядро пересчитывает
//! затраты по строке тикера (core/convert.rs: to_usdt, in_coin; bnb_total — всё с тикером «BNB»), поэтому подпись НЕ
//! должна совпасть ни с тикером монет подписки (tokens.all()), ни с уже найденными промежуточными, ни с «BNB»
//! (газ), dex.native_ticker, venue.quote_asset — при совпадении (без учёта регистра), пустом или не-ABI ответе
//! symbol() (bytes32 у старых токенов, revert) подпись = адрес токена `0x…` (строчными).
//! Кто вызывает: pools/mids.rs (read).
//! От чего зависит: ctx.rs, tokens.rs, node/call.rs, node/abi.rs (IERC20::symbol), wallet/balances.rs (BNB).
//! Настройки: dex.native_ticker, venue.quote_asset [bsc_dex] (необязательные здесь: не заданы — не в списке занятых).
//!
//! symbol() — EIP-20, раздел «Methods» (https://eips.ethereum.org/EIPS/eip-20): OPTIONAL, `returns (string)`.

use alloy_primitives::Address;

use super::super::ctx::Ctx;
use super::super::node::abi::IERC20;
use super::super::node::call;
use super::super::tokens::Tokens;
use super::super::wallet::balances::BNB;

/// Подпись промежуточной монеты, которую не знает биржа. Принимает: окружение, токены подписки, адрес, подписи уже
/// найденных промежуточных. Возвращает: symbol() с цепи или адрес (см. шапку). Ошибок нет: сбой узла или не-ABI
/// ответ — подпись адресом.
pub async fn label(ctx: &Ctx, tokens: &Tokens, address: Address, taken: &[String]) -> String {
    let mut busy: Vec<String> = tokens.all().into_iter().map(|t| t.ticker).collect();
    busy.extend(taken.iter().cloned());
    busy.push(BNB.to_string());
    busy.extend(ctx.text("dex.native_ticker").ok());
    busy.extend(ctx.text("venue.quote_asset").ok());
    let symbol = call::view(ctx, address, &IERC20::symbolCall {}, "latest")
        .await
        .ok();
    pick(symbol, address, &busy)
}

/// Выбор подписи. Принимает: ответ symbol() (None — не прочитан или не-ABI), адрес, занятые тикеры.
/// Возвращает: symbol() без пробелов по краям, если он не пуст, без управляющих символов и не совпадает с занятым
/// (без учёта регистра); иначе адрес `{address:#x}`. Чистая функция, ошибок нет.
pub fn pick(symbol: Option<String>, address: Address, taken: &[String]) -> String {
    let fallback = || format!("{address:#x}");
    let Some(s) = symbol.map(|s| s.trim().to_string()) else {
        return fallback();
    };
    let bad = s.is_empty()
        || s.chars().any(char::is_control)
        || taken.iter().any(|t| t.trim().eq_ignore_ascii_case(&s));
    if bad { fallback() } else { s }
}
