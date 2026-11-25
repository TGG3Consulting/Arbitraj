mod route_common;
use arb::core::types::{PoolKind,PoolRef};
use arb::venues::bsc_dex::exec::dry_fund;

#[tokio::test]
async fn historical_single_donor_selection() {
    // Archived balanceOf calls are the only mocked production dependency in source().
    let p=route_common::parts().await;
    let token=addr("0x7ac2b2378608b090f9d54742b9b2391876fc01f4");
    let route=addr("0x51dc7dc02f6dadfc2068f89d8badc0a39326bfed");
    let mut tmpl=p.registry.all().into_iter().next().unwrap();
    tmpl.pool.kind=PoolKind::V2;tmpl.pool.reference=PoolRef::Contract(route);tmpl.pool.token1=token;tmpl.valid=true;p.registry.put(tmpl.clone());
    let q=p.quoting();let mut output=Vec::new();
    for r in inputs()["mboom"].as_array().unwrap() {
        for requested in [amount(&r["wanted"]),amount(&r["small"])]{
            assert_eq!(dry_fund::source(&q,token,&[PoolRef::Contract(route)],requested).await.unwrap(),None);
            output.push(json!({"id":r["id"],"sim_input":requested.to_string(),"eligible":false}));
        }
    }
    let asset=addr("0xdb0170e2d0c1cc1b2e7a90313d9b9afa4f250289");
    for d in inputs()["donors"].as_array().unwrap(){
        tmpl.pool.reference=PoolRef::Contract(addr(d["address"].as_str().unwrap()));tmpl.pool.token1=asset;p.registry.put(tmpl.clone());
    }
    for requested in ["107985845390000000000000","7147974183670266786116","4473575045234388233394"] {
        let pick=dry_fund::source(&q,asset,&[],number(requested)).await.unwrap();
        assert_eq!(pick.is_some(),requested.starts_with("4473"));output.push(json!({"id":221,"sim_input":requested,"eligible":pick.is_some()}));
    }
    fs::write("case-presell.json",serde_json::to_vec_pretty(&output).unwrap()).unwrap();
}
