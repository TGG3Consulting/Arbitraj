//! Связь VVSV3LmPool с этим пулом и проверенной реализацией внешнего MasterChef на каноническом блоке.
use super::{nami_batch as batch, solidly_classic_factory};
use crate::{gateway::errors::{ErrorCode, GatewayResult}, venues::bsc_dex::ctx::Ctx};
use alloy::sol;
use alloy_primitives::{Address,B256};
use serde_json::Value;

sol!{
    interface IRewards {
        function pool() external view returns(address);
        function masterChef() external view returns(address);
    }
}

struct Roles {
    hook: Address,
    code: B256,
    chef: Address,
    chef_code: B256,
    implementation: Address,
    implementation_code: B256,
}

impl Roles {
    fn parse(row: &str) -> Option<Self> {
        let mut fields=row.split_whitespace();
        let r=Self {
            hook: fields.next()?.parse().ok()?, code: fields.next()?.parse().ok()?,
            chef: fields.next()?.parse().ok()?, chef_code: fields.next()?.parse().ok()?,
            implementation: fields.next()?.parse().ok()?, implementation_code: fields.next()?.parse().ok()?,
        };
        (fields.next().is_none() && ![r.hook,r.chef,r.implementation].contains(&Address::ZERO)
            && ![r.code,r.chef_code,r.implementation_code].contains(&B256::ZERO)).then_some(r)
    }
}

pub(super) async fn check(ctx:&Ctx,id:&str,pool:Address,hook:Address,at:&Value)->GatewayResult<()> {
    let bad=||ctx.err(ErrorCode::NotApplicable,"VVS: неподтверждённая связь рынка, модуля наград и MasterChef");
    let rows=ctx.dex_list("dex.v3_reward_modules",id)?;
    let mut matched=None;
    for row in &rows {
        let role=Roles::parse(row).ok_or_else(bad)?;
        if role.hook==hook && matched.replace(role).is_some() {return Err(bad());}
    }
    let r=matched.ok_or_else(bad)?;
    let p=IRewards::poolCall{};let c=IRewards::masterChefCall{};
    let ((),(),[pool_bytes,chef_bytes])=tokio::try_join!(
        solidly_classic_factory::plain(ctx,hook,r.code,at),
        solidly_classic_factory::bind(ctx,r.chef,r.chef_code,Some((r.implementation,r.implementation_code)),at),
        batch::read(ctx,[batch::view(at,hook,p.clone()),batch::view(at,hook,c.clone())])
    )?;
    if batch::decode(&pool_bytes,p)?!=pool || batch::decode(&chef_bytes,c)?!=r.chef {return Err(bad());}
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use super::super::{families,exec_kind::ExecKind};
    #[test]
    fn all_paid_reward_markets_bind_original_pool_and_implementation() {
        let read=|bytes:&[u8]|serde_json::from_reader::<_,Value>(GzDecoder::new(bytes)).unwrap();
        let directory=read(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/vvs_ethereum_directory.json.gz"));
        let wallet=read(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/vvs_ethereum_wallets.json.gz"));
        assert_eq!(directory["selector"],wallet["selector"]);
        let config:Value=serde_json::from_str(include_str!("../../../../config/first_run_dex.json")).unwrap();
        let s=&config.as_array().unwrap().iter().find(|p|p["id"]=="vvs-v3-ethereum").unwrap()["settings"];
        assert_eq!(s["dex.module_runtime_hashes"][0],wallet["module_hash"]);
        assert_eq!(s["dex.v3_native_router"],directory["roles"]["router"]["address"]);
        let rows=s["dex.v3_reward_modules"].as_array().unwrap();
        assert_eq!(rows.len(),2);
        for r in directory["rewards"].as_array().unwrap(){
            let row=["hook","runtime","chef","chef_runtime","implementation","implementation_runtime"].map(|k|r[k].as_str().unwrap()).join(" ");
            let entry=Roles::parse(&row).unwrap();
            assert!(rows.iter().any(|v|v.as_str()==Some(row.as_str())));
            assert_ne!(entry.chef,entry.implementation);
            assert!(Roles::parse(&row.split_whitespace().take(4).collect::<Vec<_>>().join(" ")).is_none());
            let p=directory["markets"].as_array().unwrap().iter().find(|m|m["pool"]==r["pool"]).unwrap();
            assert_eq!(p["liquidity"],"0");
            assert_eq!(p["lm_pool"],r["hook"]);
            let pay=wallet["cases"].as_array().unwrap();
            for wire in ["router","module","v5"]{
                let legs:Vec<_>=pay.iter().filter(|case|case["pool"]==r["pool"] && case["wire"]==wire).collect();
                assert_eq!(legs.len(),4);
                for leg in legs {
                    assert!(leg["minimum_plus_one"].is_object() && leg["expired"].is_object());
                    assert!(leg["gas"].as_u64().unwrap()>0);
                }
            }
        }
        let native=families::make("verified-vvs-native","vvs_v3").unwrap();
        let module=families::make("verified-vvs-module","vvs_v3_module").unwrap();
        assert!(native.variant.exact_router() && !native.direct_module() && native.executor_kind.is_none());
        assert!(module.direct_module() && module.variant.exact_router());
        assert_eq!(module.executor_kind,Some(ExecKind::Balancer));
    }
}
