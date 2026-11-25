from pathlib import Path
import shutil
root=Path("srcbot")
tools=Path("control/tools/wallet_presell_control")
raw_file=root/"src/venues/bsc_dex/exec/dry_fund.rs"
legacy=not (root/"src/venues/bsc_dex/exec/dry_fund_gather.rs").is_file()
if legacy:
    s=raw_file.read_text()
    line='let balances = call::view_many(&q.ctx, token, &calls, "latest").await?;'
    assert s.count(line)==1
    stub="""let captured: serde_json::Value = serde_json::from_str(include_str!("../../../../tests/wallet_audit_inputs.json")).unwrap();
    let balances: Vec<U256> = pools.iter().map(|pool|{
        let d=captured["donors"].as_array().unwrap().iter().find(|d| d["address"].as_str().unwrap().parse::<Address>().unwrap()==*pool).expect("unexpected balanceOf call");
        d["raw"].as_str().unwrap().parse::<U256>().unwrap()
    }).collect();"""
    raw_file.write_text(s.replace(line,stub))
b=(tools/"base.rs").read_text(encoding="utf-8-sig")
if "pub price_band:" in (root/"src/venues/mexc/symbols.rs").read_text(): b=b.replace("/*PRICE_BAND_FIELD*/","price_band:None,")
(root/"tests/wallet_audit.rs").write_text(b+"\n"+(tools/("legacy.rs" if legacy else "current.rs")).read_text(encoding="utf-8-sig"))
shutil.copyfile(tools/"cases.json",root/"tests/wallet_audit_inputs.json")
