//! Проверяет фактические привязки proxy и private immutable по закреплённому EIP-1898 blockHash.
use super::{abi_woo, pmm_profile::Profile};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{ctx::Ctx, node::rpc};
use alloy_primitives::{Address, B256, Bytes, keccak256};
use futures_util::future::try_join_all;
use serde_json::{Value, json};

fn code_matches(code: &[u8], expected: B256, immutables: &[(usize, Address)]) -> bool {
    !code.is_empty()
        && keccak256(code) == expected
        && immutables.iter().all(|&(n, role)| {
            n.checked_sub(1).and_then(|i| code.get(i)) == Some(&0x7f)
                && n.checked_add(32).and_then(|end| code.get(n..end))
                    == Some(role.into_word().as_slice())
        })
}

pub(super) async fn verify(p: &Profile, ctx: &Ctx, tag: &Value) -> GatewayResult<()> {
    let wrong = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Код, immutable или слот реализации PMM расходится с настройками на каноническом блоке",
        )
    };
    let codes = p.code.iter().copied().map(|(addr, expected)| async move {
        let code: Bytes = rpc::call(ctx, "eth_getCode", json!([addr, tag])).await?;
        let immutables = if addr == p.pool {
            p.immutables.as_slice()
        } else {
            &[]
        };
        if !code_matches(&code, expected, immutables) {
            return Err(wrong());
        }
        Ok(())
    });
    let slots = p
        .proxies
        .iter()
        .copied()
        .map(|(proxy, implementation)| async move {
            let word: B256 = rpc::call(
                ctx,
                "eth_getStorageAt",
                json!([proxy, abi_woo::implementation_slot(), tag]),
            )
            .await?;
            if word != implementation.into_word() {
                return Err(wrong());
            }
            Ok(())
        });
    tokio::try_join!(try_join_all(codes), try_join_all(slots))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redeployed_router_and_changed_hash_or_swapped_roles() {
        for addresses in [[3, 4], [7, 9]] {
            let [oracle, vault] = addresses.map(Address::with_last_byte);
            let mut code = vec![0x7f];
            code.extend_from_slice(oracle.into_word().as_slice());
            code.push(0x7f);
            code.extend_from_slice(vault.into_word().as_slice());
            let hash = keccak256(&code);
            assert!(code_matches(&code, hash, &[(1, oracle), (34, vault)]));
            assert!(!code_matches(
                &code,
                B256::ZERO,
                &[(1, oracle), (34, vault)]
            ));
            assert!(!code_matches(&code, hash, &[(1, vault), (34, oracle)]));
            assert!(!code_matches(&code, hash, &[(usize::MAX, oracle)]));
            assert!(!code_matches(&[], keccak256([]), &[]));
        }
    }
}
