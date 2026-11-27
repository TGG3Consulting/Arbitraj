//! Сверяет адреса, зашитые в выбранном модуле, прямо в уже прочитанном runtime до котировки.
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::ctx::Ctx;
use alloy_primitives::{Address, B256};

/// PC указывает на начало инструкции, а не на совпавшие байты внутри другой константы.
fn pushed(code: &[u8], at: usize) -> Option<Address> {
    let mut pc = 0;
    while pc <= at {
        let op = *code.get(pc)?;
        let len = if (0x60..=0x7f).contains(&op) {
            usize::from(op - 0x5f)
        } else {
            0
        };
        if pc == at {
            if len == 0 {
                return None;
            }
            let bytes = code.get(pc + 1..pc + 1 + len)?;
            let mut word = [0u8; 32];
            word[32 - len..].copy_from_slice(bytes);
            if word[..12] != [0u8; 12] {
                return None;
            }
            return Some(Address::from_slice(&word[12..]));
        }
        pc += 1 + len;
    }
    None
}

fn bound(code: &[u8], hash: B256, rows: &[String], targets: &[Address]) -> bool {
    let mut matched = false;
    for row in rows {
        let mut fields = row.split_whitespace();
        let Some(h) = fields.next().and_then(|x| x.parse::<B256>().ok()) else {
            return false;
        };
        let Some(offsets) = fields
            .map(|x| x.parse::<usize>().ok())
            .collect::<Option<Vec<_>>>()
        else {
            return false;
        };
        if offsets.len() != targets.len() {
            return false;
        }
        if h == hash {
            if matched
                || !offsets
                    .into_iter()
                    .zip(targets)
                    .all(|(n, a)| !a.is_zero() && pushed(code, n) == Some(*a))
            {
                return false;
            }
            matched = true;
        }
    }
    matched
}

pub(super) fn verify(ctx: &Ctx, id: &str, code: &[u8], hash: B256) -> GatewayResult<()> {
    let kind = ctx.dex_text("dex.kind", id)?;
    let wrong = || {
        ctx.err(
            ErrorCode::NotApplicable,
            format!("{id}: модуль не связан с рабочим рынком и хранилищем"),
        )
    };
    if kind=="kyber_elastic_module" {
        let roles=[ctx.dex_addr("dex.factory",id)?,ctx.dex_addr("dex.elastic_native_router",id)?];
        return bound(code,hash,&ctx.dex_list("dex.pmm_module_bindings",id)?,&roles).then_some(()).ok_or_else(wrong);
    }
    if matches!(kind.as_str(),"virtuals_bonding_module" | "virtuals_unicorn_module") {
        let front=ctx.dex_list("dex.bonding_roles",id)?.get(1).and_then(|v|v.split_whitespace().next()).and_then(|v|v.parse().ok()).ok_or_else(wrong)?;
        let roles=[ctx.dex_addr("dex.factory",id)?,ctx.dex_addr("dex.quoter",id)?,front];
        return bound(code,hash,&ctx.dex_list("dex.pmm_module_bindings",id)?,&roles).then_some(()).ok_or_else(wrong);
    }
    let with_vault = match kind.as_str() {
        "baibai_module" | "elfomofi_module" => true,
        "lunarbase_module" => false,
        "aqua_swapvm" | "lambda_propamm" | "integral_size_v5" | "eulerswap_v1_module" | "eulerswap_v2_module" | "stabull_module" | "maverick_v1_module" | "degenswap_module" | "safeswap_trade_module" | "bundles_swap_v5" | "omni_flux_native_cl" | "omni_flux_native_bin" | "launch_o1_wrapped" | "umbrae_dlmm_module" | "umbrae_damm_module" | "bunnyswap_module" | "memebox_v5" | "registered_v3_module" | "shibaswap_v2_module" | "vvs_v3_module" | "base_native_v3_module" | "solidly_v3_module" | "fraxswap_twamm_v5" | "oasisswap_v2_v5" | "ethervista_v5" => return Ok(()),
        _ => return Err(wrong()),
    };
    let mut roles = vec![ctx.dex_addr("dex.pmm_pool", id)?];
    if with_vault {
        roles.push(ctx.dex_addr("dex.vault", id)?);
    }
    let rows = ctx.dex_list("dex.pmm_module_bindings", id)?;
    if !bound(code, hash, &rows, &roles) {
        return Err(wrong());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::keccak256;
    use serde_json::Value;
    #[test]
    fn virtuals_and_elastic_roles_are_bound_to_each_accepted_module() {
        let archives:Value=serde_json::from_str(include_str!("../../../../tests/data/first_run_dex_2026-11-26/pmm_module_bindings.json")).unwrap();
        let rows:Vec<Value>=serde_json::from_str(include_str!("../../../../config/first_run_dex.json")).unwrap();
        for id in ["virtuals-base","virtuals-unicorn-base","horizondex-base"] {
            let row=&rows.iter().find(|v|v["id"]==id && v["chain_id"]==8453).unwrap()["settings"];
            let mut roles=vec![row["dex.factory"].as_str().unwrap().parse::<Address>().unwrap()];
            if id=="horizondex-base" {
                roles.push(row["dex.elastic_native_router"].as_str().unwrap().parse().unwrap());
            }else{
                roles.push(row["dex.quoter"].as_str().unwrap().parse().unwrap());
                roles.push(row["dex.bonding_roles"][1].as_str().unwrap().split_whitespace().next().unwrap().parse().unwrap());
            }
            let bindings:Vec<String>=serde_json::from_value(row["dex.pmm_module_bindings"].clone()).unwrap();
            for hash in row["dex.module_runtime_hashes"].as_array().unwrap() {
                let h=hash.as_str().unwrap();
                let code=hex::decode(archives["versions"][h]["runtime"].as_str().unwrap()).unwrap();
                let hash=h.parse().unwrap();assert_eq!(hash,keccak256(&code));
                assert!(bound(&code,hash,&bindings,&roles));
                for n in 0..roles.len(){let mut changed=roles.clone();changed[n]=Address::with_last_byte(17);assert!(!bound(&code,hash,&bindings,&changed));}
            }
        }
    }
    #[test]
    fn archived_modules_reject_a_new_market_or_wrong_vault() {
        let archives: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/pmm_module_bindings.json"
        ))
        .unwrap();
        let rows: Vec<Value> =
            serde_json::from_str(include_str!("../../../../config/first_run_dex.json")).unwrap();
        for (id, spec) in archives["bindings"].as_object().unwrap() {
            let s = &rows
                .iter()
                .find(|r| r["id"].as_str() == Some(id.as_str()))
                .unwrap()["settings"];
            assert_eq!(spec, &s["dex.pmm_module_bindings"]);
            let bindings: Vec<String> = serde_json::from_value(spec.clone()).unwrap();
            let mut roles = vec![s["dex.pmm_pool"].as_str().unwrap().parse().unwrap()];
            if let Some(vault) = s["dex.vault"].as_str() {
                roles.push(vault.parse().unwrap());
            }
            for h in s["dex.module_runtime_hashes"].as_array().unwrap() {
                let h = h.as_str().unwrap();
                let code =
                    hex::decode(archives["versions"][h]["runtime"].as_str().unwrap()).unwrap();
                let hash = h.parse::<B256>().unwrap();
                assert_eq!(keccak256(&code), hash);
                assert!(bound(&code, hash, &bindings, &roles));
                for n in 0..roles.len() {
                    let mut changed = roles.clone();
                    changed[n] = Address::with_last_byte(17);
                    assert!(!bound(&code, hash, &bindings, &changed));
                }
            }
        }
    }
}
