//! Файл: venues/bsc_dex/pools/mid_dex.rs
//! Что делает: решение 276 — подпись промежуточной монеты, адрес которой не знает ни одна биржа (при
//! dex.intermediate_dex_priced = да). Тикер промежуточной нужен только как подпись затрат и шагов (quote/steps.rs
//! Names::of): затраты шага в ней — «в цене» (in_price), ядро курс для них не требует (core/costs.rs); цена и глубина
//! — пулом «промежуточная — USDT» (quote/rate.rs). Подпись — symbol() токена с цепи. ОПАСНОСТЬ: ядро пересчитывает
//! затраты по строке тикера (core/convert.rs: to_usdt, in_coin; bnb_total — всё с тикером «BNB»), поэтому подпись НЕ
//! должна совпасть (без учёта регистра) ни с тикером монет подписки DEX (tokens.all()), ни с уже найденными
//! промежуточными, ни с «BNB» (газ), dex.native_ticker, venue.quote_asset, монетами venue.price_assets любой биржи
//! (решение 181: GT и др.). Подпись с цепи принимается (ревью 276), только если после снятия пробелов ASCII по краям
//! она не пуста, состоит из символов A–Z, a–z, 0–9, «.», «_», «-» и не длиннее dex.intermediate_label_max_len; иначе
//! (и при сбое чтения, не-ABI ответе, незаданной длине) подпись = адрес токена `0x…` (строчными). Причина — в журнал.
//! Кто вызывает: pools/mids.rs (read).
//! От чего зависит: ctx.rs, tokens.rs, node/rpc.rs, node/call.rs (decode), node/abi.rs (IERC20::symbol),
//! wallet/balances.rs (BNB).
//! Настройки: dex.intermediate_label_max_len, dex.native_ticker, venue.quote_asset [bsc_dex], venue.price_assets
//! [любая биржа] (все необязательные здесь: длина не задана — подпись всегда адрес; остальные — не в списке занятых).
//!
//! symbol() — EIP-20, раздел «Methods» (https://eips.ethereum.org/EIPS/eip-20): OPTIONAL, `returns (string)`.
//! eth_call — https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/execute.yaml

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};
use serde_json::json;

use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::node::abi::IERC20;
use super::super::node::{call, rpc};
use super::super::tokens::Tokens;
use super::super::wallet::balances::BNB;

/// Почему подпись — адрес, а не symbol() с цепи.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// Ответ symbol() не по ABI `string` (bytes32 у старых токенов и т. п.).
    NotAbi,
    /// symbol() пуст.
    Empty,
    /// Есть символы вне A–Z, a–z, 0–9, «.», «_», «-» (юникод-двойники, символы формата, пробелы внутри).
    BadChars,
    /// dex.intermediate_label_max_len не задана.
    NoMaxLen,
    /// Длиннее dex.intermediate_label_max_len (значение настройки).
    TooLong(u64),
    /// Совпадает с занятым тикером (какой именно).
    Taken(String),
}

impl Why {
    /// Текст причины для журнала. Ошибок нет.
    pub fn text(&self) -> String {
        match self {
            Why::NotAbi => "ответ symbol() не по ABI string".into(),
            Why::Empty => "symbol() пуст".into(),
            Why::BadChars => "в symbol() есть символы вне A–Z, a–z, 0–9, «.», «_», «-»".into(),
            Why::NoMaxLen => "dex.intermediate_label_max_len не задана".into(),
            Why::TooLong(n) => format!("symbol() длиннее dex.intermediate_label_max_len = {n}"),
            Why::Taken(t) => format!("symbol() совпадает с занятым тикером «{t}»"),
        }
    }
}

/// Подпись промежуточной монеты, которую не знает биржа, и запись в журнал: Info — подпись с цепи или адресом (с
/// причиной), Warn — symbol() не прочитан (текст ошибки узла), подпись адресом. Принимает: окружение, токены
/// подписки, адрес, подписи уже найденных промежуточных. Возвращает: подпись. Ошибок нет.
pub async fn label(ctx: &Ctx, tokens: &Tokens, address: Address, taken: &[String]) -> String {
    let busy = busy(ctx, tokens, taken);
    let max_len = ctx.int("dex.intermediate_label_max_len").ok();
    let head = format!("Промежуточная {address}: биржа не знает адрес");
    let tail = "цена — пулом с USDT (dex.intermediate_dex_priced)";
    let input = Bytes::from(IERC20::symbolCall {}.abi_encode());
    let raw: GatewayResult<Bytes> = rpc::call(
        ctx,
        "eth_call",
        json!([{ "to": address, "input": input }, "latest"]),
    )
    .await;
    let (label, level, msg) = match raw {
        Err(e) => {
            let l = fallback(address);
            let m = format!(
                "{head}; symbol() не прочитан: {} — подпись адресом {l}, {tail}",
                e.message
            );
            (l, Level::Warn, m)
        }
        Ok(out) => {
            let symbol = call::decode::<IERC20::symbolCall>(&out).ok();
            match pick(symbol, address, &busy, max_len) {
                (l, None) => {
                    let m = format!("{head} — подпись {l} с цепи (symbol()), {tail}");
                    (l, Level::Info, m)
                }
                (l, Some(why)) => {
                    let m = format!("{head} — подпись адресом {l}: {}, {tail}", why.text());
                    (l, Level::Info, m)
                }
            }
        }
    };
    ctx.log(level, "dex.intermediate", msg, None).await;
    label
}

/// Занятые тикеры (см. шапку). Принимает: окружение, токены подписки, подписи уже найденных промежуточных.
/// Возвращает: список (незаданные настройки в нём не участвуют). Ошибок нет.
pub fn busy(ctx: &Ctx, tokens: &Tokens, taken: &[String]) -> Vec<String> {
    let mut busy: Vec<String> = tokens.all().into_iter().map(|t| t.ticker).collect();
    busy.extend(taken.iter().cloned());
    busy.push(BNB.to_string());
    busy.extend(ctx.text("dex.native_ticker").ok());
    busy.extend(ctx.text("venue.quote_asset").ok());
    // Решение 181: venue.price_assets задаётся у каждой биржи своей областью — берутся значения всех областей.
    for (name, _, v) in ctx.settings.all() {
        if name == "venue.price_assets"
            && let Some(list) = v.as_array()
        {
            busy.extend(list.iter().filter_map(|x| x.as_str()).map(str::to_string));
        }
    }
    busy
}

/// Выбор подписи. Принимает: ответ symbol() (None — не по ABI), адрес, занятые тикеры, dex.intermediate_label_max_len
/// (None — не задана). Возвращает: (подпись, None) — symbol() без пробелов ASCII по краям прошёл все проверки (см.
/// шапку); (адрес `{address:#x}`, Some(причина)) — иначе. Чистая функция, ошибок нет.
pub fn pick(
    symbol: Option<String>,
    address: Address,
    taken: &[String],
    max_len: Option<u64>,
) -> (String, Option<Why>) {
    let Some(s) = symbol.map(|s| s.trim_ascii().to_string()) else {
        return (fallback(address), Some(Why::NotAbi));
    };
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    let why = if s.is_empty() {
        Some(Why::Empty)
    } else if !s.chars().all(allowed) {
        Some(Why::BadChars)
    } else {
        match max_len {
            None => Some(Why::NoMaxLen),
            Some(n) if s.len() as u64 > n => Some(Why::TooLong(n)),
            Some(_) => taken
                .iter()
                .map(|t| t.trim())
                .find(|t| t.eq_ignore_ascii_case(&s))
                .map(|t| Why::Taken(t.to_string())),
        }
    };
    match why {
        None => (s, None),
        Some(w) => (fallback(address), Some(w)),
    }
}

/// Подпись адресом: `0x…` строчными. Ошибок нет.
fn fallback(address: Address) -> String {
    format!("{address:#x}")
}
