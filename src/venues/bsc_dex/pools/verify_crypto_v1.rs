//! Файл: venues/bsc_dex/pools/verify_crypto_v1.rs
//! Что делает: проверка пула фабрики crypto v1 Curve (решение 453, вид curve_crypto_v1) на своём узле. Кто
//! подтверждает пул (решение 59) — реестр фабрики dex.factory: `get_coins(pool)` отдаёт монеты только пулу этой фабрики
//! (pool_data чужого адреса пуст — нулевые адреса). Формула пула — внутри его кода: пул фабрики — прокси EIP-1167 на
//! шаблон (Factory.vy deploy_pool L206: create_forwarder_to(pool_implementation)), а шаблон фабрика может сменить
//! (set_pool_implementation L269) — поэтому код пула (eth_getCode) должен быть прокси EIP-1167 на адрес из
//! dex.curve_crypto_v1_templates (шаблоны, с исходником которых сверена math/crypto_v1_2.rs). Пул берётся, если монеты
//! не нулевые, монета и встречный токен среди них и шаблон из списка; иначе — отказ с причиной. get_coins — частями
//! по dex.verify_batch, код — по одному eth_getCode на кандидата, прошедшего реестр.
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), node/rpc.rs (eth_getCode), abi_crypto_v1.rs, discover.rs,
//! verify.rs.
//! Настройки: dex.factory, dex.curve_crypto_v1_templates [DEX].
//!
//! Источники: curvefi/curve-factory-crypto 7ed1011, contracts/Factory.vy get_coins L353-L360, deploy_pool L151-L230;
//! EIP-1167 (https://eips.ethereum.org/EIPS/eip-1167): код прокси 363d3d373d3d3d363d73 <адрес> 5af43d82803e903d91602b57fd5bf3;
//! eth_getCode — ethereum/execution-apis src/eth/state.yaml (см. deploy_block.rs).

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};
use serde_json::json;

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::{call, rpc};
use super::abi_crypto_v1::ICryptoFactoryV1 as F;
use super::discover::{Found, sorted};
use super::verify::{Candidate, Verdict};

/// Настройка адресов шаблонов пула фабрики crypto v1.
pub const TEMPLATES: &str = "dex.curve_crypto_v1_templates";

/// Начало и конец кода прокси EIP-1167 (между ними — 20 байт адреса шаблона).
const PRE: [u8; 10] = [0x36, 0x3d, 0x3d, 0x37, 0x3d, 0x3d, 0x3d, 0x36, 0x3d, 0x73];
const POST: [u8; 15] = [
    0x5a, 0xf4, 0x3d, 0x82, 0x80, 0x3e, 0x90, 0x3d, 0x91, 0x60, 0x2b, 0x57, 0xfd, 0x5b, 0xf3,
];

/// Пул, фабрика и шаблоны кандидата.
struct Plan {
    pool: Address,
    factory: Address,
    templates: Vec<Address>,
}

/// Проверка пачки кандидатов. Возвращает: итог по каждому. Ошибок нет (нет настройки или пул не из реестра — отказ с
/// причиной; сбой узла — Failed).
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let plans: Vec<Result<Plan, String>> = cands.iter().map(|c| plan(ctx, c)).collect();
    let calls: Vec<(Address, Bytes)> = plans
        .iter()
        .flatten()
        .map(|p| {
            let data = F::get_coinsCall { pool: p.pool }.abi_encode();
            (p.factory, data.into())
        })
        .collect();
    let mut got = call::try_chunks(ctx, &calls, "latest", size)
        .await
        .into_iter();
    let mut out = Vec::new();
    for (c, plan) in cands.iter().zip(plans) {
        let p = match plan {
            Ok(p) => p,
            Err(w) => {
                out.push(Verdict::Rejected(w));
                continue;
            }
        };
        let coins = match got.next() {
            Some(Ok(Ok(b))) => b,
            Some(Ok(Err(r))) => {
                let why = format!("реестр фабрики crypto v1 откатил get_coins: {r}");
                out.push(Verdict::Rejected(why));
                continue;
            }
            Some(Err(node)) => {
                out.push(Verdict::Failed(node));
                continue;
            }
            None => {
                out.push(Verdict::Failed(
                    "реестр фабрики crypto v1: нет ответа".into(),
                ));
                continue;
            }
        };
        let code: Result<Bytes, _> = rpc::call(ctx, "eth_getCode", json!([p.pool, "latest"])).await;
        out.push(match code {
            Err(e) => Verdict::Failed(e.message),
            Ok(code) => match decide(c, &p, &coins, &code) {
                Ok(f) => Verdict::Taken(Box::new(f)),
                Err(w) => Verdict::Rejected(w),
            },
        });
    }
    out
}

/// Пул кандидата и настройки DEX. Возвращает: Plan или причину отказа. Ошибок нет.
fn plan(ctx: &Ctx, c: &Candidate) -> Result<Plan, String> {
    let id = c.family.id;
    let pool = Address::from_str(&c.pool)
        .map_err(|_| format!("«{}» — не адрес контракта пула crypto v1", c.pool))?;
    let factory = ctx.dex_addr("dex.factory", id).map_err(|e| e.message)?;
    let list = ctx.dex_list(TEMPLATES, id).map_err(|e| e.message)?;
    let mut templates = Vec::with_capacity(list.len());
    for t in &list {
        let a = Address::from_str(t.trim())
            .map_err(|_| format!("{TEMPLATES} [dex:{id}]: «{t}» — не адрес"))?;
        templates.push(a);
    }
    if templates.is_empty() {
        return Err(format!(
            "{TEMPLATES} [dex:{id}] пуст — пулы crypto v1 не берутся"
        ));
    }
    Ok(Plan {
        pool,
        factory,
        templates,
    })
}

/// Шаблон прокси EIP-1167 из кода пула. Принимает: код. Возвращает: адрес шаблона или None (код не прокси). Ошибок нет.
pub fn proxy_target(code: &[u8]) -> Option<Address> {
    let body = code.strip_prefix(&PRE[..])?.strip_suffix(&POST[..])?;
    (body.len() == 20).then(|| Address::from_slice(body))
}

/// Решение по ответам (без сети): монеты реестра, пара среди них, код пула — прокси на шаблон из списка. Принимает:
/// кандидата, план, ответ get_coins, код пула. Возвращает: найденный пул или причину отказа.
fn decide(c: &Candidate, p: &Plan, coins: &[u8], code: &[u8]) -> Result<Found, String> {
    let pool = p.pool;
    let coins = F::get_coinsCall::abi_decode_returns(coins)
        .map_err(|e| format!("get_coins не по ABI: {e}"))?;
    if coins.iter().any(|a| a.is_zero()) {
        return Err(format!(
            "пул {pool:#x} не из реестра фабрики crypto v1 (get_coins пуст) — dex.factory DEX {}",
            c.family.id
        ));
    }
    if !coins.contains(&c.coin) || !coins.contains(&c.quote) {
        return Err(format!(
            "среди монет пула crypto v1 {pool:#x} нет пары {} / {}",
            c.coin, c.quote
        ));
    }
    match proxy_target(code) {
        Some(t) if p.templates.contains(&t) => {}
        Some(t) => {
            return Err(format!(
                "пул {pool:#x} — прокси на шаблон {t:#x}, его нет в {TEMPLATES}: формула не сверена"
            ));
        }
        None => {
            return Err(format!(
                "код пула {pool:#x} — не прокси EIP-1167: шаблон не установить"
            ));
        }
    }
    let (token0, token1) = sorted(c.coin, c.quote);
    Ok(Found {
        family: c.family,
        reference: PoolRef::Contract(pool),
        token0,
        token1,
        coin: c.coin,
        fee_pips: None,
        tick_spacing: None,
        bin_step: None,
        key: None,
        hooks: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Код прокси EIP-1167 — адрес шаблона между началом и концом; другой код — None.
    #[test]
    fn proxy_target_parses_eip1167() {
        let t = Address::repeat_byte(0xab);
        let mut code = PRE.to_vec();
        code.extend_from_slice(t.as_slice());
        code.extend_from_slice(&POST);
        assert_eq!(proxy_target(&code), Some(t));
        assert_eq!(proxy_target(&code[1..]), None);
    }
}
