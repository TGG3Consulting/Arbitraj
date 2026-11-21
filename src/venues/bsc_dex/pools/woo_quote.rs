//! Файл: venues/bsc_dex/pools/woo_quote.rs
//! Что делает: решение 305 — котируемая монета общего контракта WooFi WooPPV2 (`quoteToken()` контракта dex.pmm_pool)
//! с цепи, по каждой подключённой DEX вида woofi_v2 с dex.multi_pair = да (multi_pair::woo_pairs). Пары WooFi —
//! (dex.pmm_pool, монета) со встречным токеном = quoteToken() КОНТРАКТА (решение 122 F, 3; проверка пары — verify_woo.rs),
//! поэтому поиск по оракулу (factory_calls.rs) спрашивает state(монета) только для этой пары, а не со всеми встречными
//! токенами подписки (те пары проверка всё равно отклоняет). Читается каждым проходом поиска по фабрикам
//! (factory_scan.rs) и сразу идёт в план; dex.multi_pair не задана / нет — не читается (поиск как до решения 305).
//! Кто вызывает: pools/factory_scan.rs (read), pools/factory_calls.rs (quote_of).
//! От чего зависит: ctx.rs, families.rs (all, classify, Proto), multi_pair.rs (woo_pairs), abi_woo.rs
//! (IWooPP::quoteToken), node/call.rs (try_chunks: Multicall3).
//! Настройки: dex.pmm_pool, dex.multi_pair [DEX]; через classify — dex.kind, dex.pool_enabled, dex.oracle,
//! dex.pool_implementation.
//!
//! Источник вызова: woonetwork/WooPoolV2 f73a0f64 contracts/interfaces/IWooPPV2.sol L67 — `function quoteToken()
//! external view returns (address)` (abi_woo.rs; тот же вызов уже делает проверка пары verify_woo.rs).

use std::collections::HashMap;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_woo::IWooPP;
use super::families::{self, Proto};
use super::multi_pair;

/// Котируемые монеты контрактов WooFi: id DEX → (контракт dex.pmm_pool, его quoteToken()).
pub type WooQuotes = HashMap<String, (Address, Address)>;

/// Что делает: читает quoteToken() контракта dex.pmm_pool каждой подключённой DEX вида woofi_v2 с dex.multi_pair = да
/// одной пачкой (IWooPPV2.sol L67). Принимает: окружение, размер части пачки (dex.load_batch). Возвращает:
/// (прочитанное, заметки по DEX, вызовов без ответа узла). DEX не подключена, режим не включён или dex.pmm_pool
/// не задан — не читается (заметку о не подключённой DEX даёт factory_calls.rs). Ошибок нет.
pub async fn read(ctx: &Ctx, size: usize) -> (WooQuotes, Vec<String>, usize) {
    let mut ids = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for f in families::all(ctx) {
        if f.proto != Proto::WooPP
            || !multi_pair::woo_pairs(ctx, f)
            || families::classify(ctx, f.id).is_err()
        {
            continue;
        }
        if let Ok(pool) = ctx.dex_addr("dex.pmm_pool", f.id) {
            ids.push((f.id.to_string(), pool));
            calls.push((pool, IWooPP::quoteTokenCall {}.abi_encode().into()));
        }
    }
    let mut out = WooQuotes::new();
    let (mut notes, mut no_answer) = (Vec::new(), 0);
    if calls.is_empty() {
        return (out, notes, no_answer);
    }
    let answers = call::try_chunks(ctx, &calls, "latest", size).await;
    for ((id, pool), a) in ids.into_iter().zip(answers) {
        match a {
            Ok(Ok(b)) => match IWooPP::quoteTokenCall::abi_decode_returns(&b) {
                Ok(q) if q != Address::ZERO => {
                    out.insert(id, (pool, q));
                }
                Ok(_) => notes.push(format!(
                    "{id}: quoteToken() контракта {pool:#x} — нулевой адрес"
                )),
                Err(e) => notes.push(format!("{id}: ответ quoteToken() не по ABI: {e}")),
            },
            Ok(Err(why)) => notes.push(format!(
                "{id}: quoteToken() контракта {pool:#x} откатился: {why}"
            )),
            Err(_) => no_answer += 1,
        }
    }
    (out, notes, no_answer)
}

/// Что делает: котируемая монета контракта WooFi этой DEX, если она прочитана для ТЕКУЩЕГО контракта. Принимает:
/// прочитанное (read), id DEX, текущий dex.pmm_pool (None — не задан / не читается). Возвращает: адрес или None
/// (не прочитана, другой контракт, не WooFi). Ошибок нет.
pub fn quote_of(q: &WooQuotes, id: &str, pool: Option<Address>) -> Option<Address> {
    match (q.get(id), pool) {
        (Some((c, quote)), Some(p)) if *c == p => Some(*quote),
        _ => None,
    }
}
