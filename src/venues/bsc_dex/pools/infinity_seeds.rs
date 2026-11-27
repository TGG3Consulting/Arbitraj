//! Начальные рынки Infinity при включённом поиске фабрик. Ключ даёт только кандидата.
//! verify_v4 независимо получает его с менеджера и проверяет валюты и полный идентификатор.
use super::{
    abi_check::IInfinityKeys,
    families::{self, Proto},
    stable_list,
};
use crate::venues::bsc_dex::ctx::Ctx;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, keccak256};
use std::collections::HashMap;

fn candidate(raw: &str, manager: Address) -> Option<(B256, [Address; 2])> {
    let bytes: Bytes = raw.parse().ok()?;
    if bytes.len() != 192 {
        return None;
    }
    let k = IInfinityKeys::poolIdToPoolKeyCall::abi_decode_returns_validate(&bytes).ok()?;
    (k.poolManager == manager && !manager.is_zero() && k.currency0 < k.currency1)
        .then(|| (keccak256(&bytes), [k.currency0, k.currency1]))
}

pub(super) fn extend(
    ctx: &Ctx,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    out: &mut stable_list::Extra,
) {
    for f in families::all(ctx)
        .into_iter()
        .filter(|f| matches!(f.proto, Proto::InfinityCl | Proto::InfinityBin))
    {
        let Ok(keys) = ctx.dex_list("dex.infinity_pool_keys", f.id) else {
            continue;
        };
        if keys.is_empty() {
            continue;
        }
        if let Err(why) = families::classify(ctx, f.id) {
            out.notes.push(why);
            continue;
        }
        let Ok(manager) = ctx.dex_addr("dex.pool_manager", f.id) else {
            continue;
        };
        for raw in keys {
            let Some((pool, mut tokens)) = candidate(&raw, manager) else {
                out.notes.push(format!(
                    "{}: ключ Infinity повреждён или указывает на другого менеджера",
                    f.id
                ));
                continue;
            };
            if f.variant.flux_native() {
                let Ok(wrapped) = ctx.addr("dex.native_wrapped") else {
                    continue;
                };
                if !tokens[0].is_zero() || wrapped.is_zero() || tokens[1] == wrapped {
                    continue;
                }
                tokens[0] = wrapped;
            }
            for row in stable_list::candidates(f.id, pool, &tokens, coins, quotes) {
                out.found.entry(row.base).or_default().push(row);
            }
        }
    }
}
