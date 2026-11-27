//! Universal Router формата Omni V2. Перед разрешением проверяет код, неизменяемые
//! параметры конструктора, выбранный Permit2, фабрику и CREATE2 физической пары.
use super::{abi_v2v3::IV2Factory, nami, nami_batch, registry::Entry, verify};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::ctx::Ctx;
use alloy_primitives::{Address, B256, keccak256};
use serde_json::json;

fn embedded(code: &[u8], offsets: &[usize], expected: B256) -> bool {
    offsets
        .iter()
        .all(|i| code.get(*i..i + 32) == Some(expected.as_slice()))
}

fn word(address: Address) -> B256 {
    B256::left_padding_from(address.as_slice())
}

fn matches(code: &[u8], expected: B256) -> bool {
    !code.is_empty() && keccak256(code) == expected
}

pub(in crate::venues::bsc_dex) async fn admitted(
    ctx: &Ctx,
    e: &Entry,
    target: (Address, Address),
) -> GatewayResult<()> {
    let generation = ctx.settings.generation();
    let id = e.family.id;
    let (router, permit) = target;
    let wrong = || {
        ctx.err(
            ErrorCode::QuoteMismatch,
            "Omni V2: изменились код, Permit2, фабрика или привязка к паре",
        )
    };
    let factory = ctx.dex_addr("dex.factory", id)?;
    let hashes = ctx.dex_list("dex.pool_code_hashes", id)?;
    let [init] = hashes.as_slice() else {
        return Err(wrong());
    };
    let init: B256 = init.parse().map_err(|_| wrong())?;
    let hash = |key| ctx.dex_text(key, id)?.parse::<B256>().map_err(|_| wrong());
    let router_hash = hash("dex.router_runtime_hash")?;
    let permit_hash = hash("dex.permit2_runtime_hash")?;
    let (a, b) = (e.pool.token0, e.pool.token1);
    let pair = e.pool.reference.address();
    if (router, permit)
        != (
            ctx.dex_addr("dex.universal_router", id)?,
            ctx.dex_addr("dex.permit2", id)?,
        )
        || a >= b
        || init.is_zero()
        || verify::v2_create2(factory, a, b, init) != pair
    {
        return Err(wrong());
    }
    let at = nami::block(ctx, "latest").await?;
    let lookup = IV2Factory::getPairCall {
        tokenA: a,
        tokenB: b,
    };
    let [r, p, registered] = nami_batch::read(
        ctx,
        [
            ("eth_getCode", json!([router, &at])),
            ("eth_getCode", json!([permit, &at])),
            nami_batch::view(&at, factory, lookup.clone()),
        ],
    )
    .await?;
    // Смещения трёх групп immutableReferences из размещённого Dispatcher и Permit2Payments.
    if generation != ctx.settings.generation()
        || !matches(&r, router_hash)
        || !matches(&p, permit_hash)
        || nami_batch::decode(&registered, lookup)? != pair
        || !embedded(&r, &[6349, 9762, 14078], word(permit))
        || !embedded(&r, &[7226, 7798, 15181], word(factory))
        || !embedded(&r, &[7193, 7832, 15148], init)
    {
        return Err(wrong());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_rebound_or_truncated_immutable_and_other_code() {
        let permit = Address::repeat_byte(4);
        let mut bytes = vec![0; 99];
        for i in [1, 65] {
            bytes[i..i + 32].copy_from_slice(word(permit).as_slice());
        }
        assert!(embedded(&bytes, &[1, 65], word(permit)));
        assert!(!embedded(&bytes, &[1, 65], word(Address::repeat_byte(5))));
        assert!(!embedded(&bytes[..86], &[1, 65], word(permit)));
        let hash = keccak256(&bytes);
        bytes[2] = 1;
        assert!(!matches(&bytes, hash));
    }
}
