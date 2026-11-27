//! Допуск прямого Router по проверенным реализациям ERC20 в настройках выбранной DEX.
use super::solidly_classic_factory::plain;
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::{Address, B256, keccak256};
use serde_json::Value;
use std::collections::HashMap;

const KEY: &str = "dex.router_exact_tokens";

fn bindings(rows: &[String]) -> Option<HashMap<Address, B256>> {
    let mut tokens = HashMap::with_capacity(rows.len());
    for row in rows {
        let mut fields = row.split_whitespace();
        let token: Address = fields.next()?.parse().ok()?;
        let code: B256 = fields.next()?.parse().ok()?;
        if fields.next().is_some()
            || token.is_zero()
            || code.is_zero()
            || code == keccak256([])
            || tokens.insert(token, code).is_some()
        {
            return None;
        }
    }
    Some(tokens)
}

struct Profiles {
    store: crate::settings::store::Store,
    generation: u64,
    tokens: HashMap<&'static str, Option<HashMap<Address, B256>>>,
}
thread_local! {
    static PROFILES:std::cell::RefCell<Option<Profiles>>=const {std::cell::RefCell::new(None)};
}
fn pair(ctx: &Ctx, id: &'static str, input: Address, output: Address) -> Option<(B256, B256)> {
    if input == output || ctx.chain_id().is_none() {
        return None;
    }
    PROFILES.with_borrow_mut(|slot| {
        let generation = ctx.settings.generation();
        if slot
            .as_ref()
            .is_some_and(|p| p.generation != generation || !p.store.same(&ctx.settings))
        {
            *slot = None;
        }
        let p = slot.get_or_insert_with(|| Profiles {
            store: ctx.settings.clone(),
            generation,
            tokens: HashMap::new(),
        });
        let tokens = p
            .tokens
            .entry(id)
            .or_insert_with(|| bindings(&ctx.dex_list(KEY, id).ok()?))
            .as_ref()?;
        Some((*tokens.get(&input)?, *tokens.get(&output)?))
    })
}
pub(crate) fn eligible(ctx: &Ctx, id: &'static str, input: Address, output: Address) -> bool {
    pair(ctx, id, input, output).is_some()
}
pub(crate) async fn check(
    ctx: &Ctx,
    id: &'static str,
    input: Address,
    output: Address,
    at: &Value,
) -> GatewayResult<()> {
    let generation = ctx.settings.generation();
    let (source,target)=pair(ctx,id,input,output).ok_or_else(||ctx.not_set(KEY,format!("{KEY} [dex:{id}]: нужны однозначные адреса и проверенные хеши обеих точных реализаций ERC20")))?;
    tokio::try_join!(
        plain(ctx, input, source, at),
        plain(ctx, output, target, at)
    )?;
    if ctx.settings.generation() != generation {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Во время проверки переводов ERC20 изменились настройки",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_run_profiles_use_verified_exact_token_code_and_both_sides() {
        let evidence: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/ethereum_exact_transfers.json"
        ))
        .unwrap();
        let config: Value =
            serde_json::from_str(include_str!("../../../../config/first_run_dex.json")).unwrap();
        for id in ["wagmi-ethereum", "lif3-ethereum", "solidlydex"] {
            let p = config
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["id"] == id && p["network"] == "ethereum")
                .unwrap();
            let rows: Vec<String> = serde_json::from_value(p["settings"][KEY].clone()).unwrap();
            let configured = bindings(&rows).unwrap();
            for row in evidence["tokens"].as_object().unwrap().values() {
                let a: Address = row["address"].as_str().unwrap().parse().unwrap();
                let h: B256 = row["keccak256"].as_str().unwrap().parse().unwrap();
                assert_eq!(configured.get(&a), Some(&h));
                assert!(
                    row["sources"]
                        .as_object()
                        .unwrap()
                        .values()
                        .all(|v| v.as_str().is_some_and(|s| s.len() == 64))
                );
            }
            assert_eq!(
                configured.len(),
                evidence["tokens"].as_object().unwrap().len()
            );
            let mut invalid = rows.clone();
            invalid.push(rows[0].clone());
            assert!(bindings(&invalid).is_none());
            assert!(bindings(&[format!("{} {}", Address::ZERO, B256::ZERO)]).is_none());
            assert!(bindings(&[format!("{} extra", rows[0])]).is_none());
            assert!(bindings(&[]).unwrap().is_empty());
        }
    }
}
