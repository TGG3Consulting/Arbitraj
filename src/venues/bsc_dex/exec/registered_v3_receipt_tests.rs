//! Квитанции Router с двух фабрик и границы исходного сохранённого поручения.
use super::*;
use crate::venues::bsc_dex::check::simulate::SimLog;
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn wallet_limits_and_identity_survive_serialization() {
    let proof: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/ethereum_exact_transfers.json"
    ))
    .unwrap();
    let exact: Vec<Address> = proof["tokens"]
        .as_object()
        .unwrap()
        .values()
        .map(|r| r["address"].as_str().unwrap().parse().unwrap())
        .collect();
    for raw in [
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/wagmi_ethereum_wallets.json.gz"
        )
        .as_slice(),
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/lif3_ethereum_wallets.json.gz"
        )
        .as_slice(),
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/blueprint_ethereum_wallets.json.gz").as_slice(),
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/vvs_ethereum_wallets.json.gz").as_slice(),
    ] {
        let archive: Value = serde_json::from_reader(GzDecoder::new(raw)).unwrap();
        let parse = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
        let extended=archive["module_hash"].is_string();
        let mut allowed=exact.clone();
        if extended {
            let directory:Value=serde_json::from_reader(GzDecoder::new(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/vvs_ethereum_directory.json.gz").as_slice())).unwrap();
            allowed.push(parse(&directory["exact_token"]["address"]));
        }
        let mut count = 0;
        for row in archive["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["wire"] == "router")
        {
            let input = parse(&row["source"]);
            let output = parse(&row["target"]);
            if !allowed.contains(&input) || !allowed.contains(&output) {
                continue;
            }
            let pool = parse(&row["pool"]);
            let target = parse(&archive["roles"]["router"]["address"]);
            let payer: Address = "0x730a17276a9720871c98622ffce5db23f38acf99"
                .parse()
                .unwrap();
            let spent: U256 = row["spent"].as_str().unwrap().parse().unwrap();
            let received: U256 = row["received"].as_str().unwrap().parse().unwrap();
            let node = archive["markets"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["pool"] == row["pool"])
                .unwrap();
            let market = Market {
                pool,
                algebra_deployer:None,fee_event:extended,wallet_module:false,
                fee: U24::from(node["fee"].as_u64().unwrap() as u32),
            };
            let bound = Proof {
                market,
                target,
                payer,
                input,
                output,
                budget: spent,
                minimum: received,
                deadline: U256::from(1000),
            };
            assert_eq!(serde_json::to_value(market).unwrap().get("fee_event").is_some(),extended);
            let mut proof: Proof =
                serde_json::from_value(serde_json::to_value(&bound).unwrap()).unwrap();
            assert_eq!(bound.data(), proof.data());
            let logs: Vec<SimLog> = serde_json::from_value(row["logs"].clone()).unwrap();
            assert_eq!(proof.measured(&logs), Some((spent, received)));
            // Если на границе диапазона исполнилась часть поручения, расход остаётся фактическим.
            proof.budget += U256::ONE;
            assert_eq!(proof.measured(&logs), Some((spent, received)));
            proof.minimum += U256::ONE;
            assert!(proof.measured(&logs).is_none());
            proof.minimum = received;
            proof.budget = spent - U256::ONE;
            assert!(proof.measured(&logs).is_none());
            proof.budget = spent;
            proof.target = payer;
            assert!(proof.measured(&logs).is_none());
            proof.target = target;
            proof.payer = target;
            assert!(proof.measured(&logs).is_none());
            proof.payer = payer;
            let mut duplicated = logs.clone();
            duplicated.push(
                logs.iter()
                    .find(|l| {
                        l.address == pool
                            && l.topics.first()
                                == Some(&keccak256(market.event().0))
                    })
                    .unwrap()
                    .clone(),
            );
            assert!(proof.measured(&duplicated).is_none());
            count += 1;
        }
        assert!(count > 0);
    }
}

#[test]
fn community_wallet_recovers_the_signed_pool_exact_debit_and_terminal_minimum() {
    let archive: Value = serde_json::from_reader(GzDecoder::new(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/community_v3_ethereum_wallets.json.gz"
    ).as_slice())).unwrap();
    let parse = |v: &Value| serde_json::from_value::<Address>(v.clone()).unwrap();
    let amount = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let cases = archive["cases"].as_array().unwrap();
    let mut count = 0;
    for row in cases.iter().filter(|r| r["wire"] == "module") {
        let tx = &row["signed"];
        let data: Bytes = serde_json::from_value(tx["data"].clone()).unwrap();
        let market = archive["markets"].as_array().unwrap().iter().find(|e| e["pool"] == row["pool"]).unwrap();
        let proof = Proof {
            market: Market { pool:parse(&row["pool"]), fee:U24::from(market["fee"].as_u64().unwrap() as u32), algebra_deployer:None,fee_event:false,wallet_module:true },
            target:parse(&tx["to"]), payer:parse(&tx["from"]),input:parse(&row["source"]),output:parse(&row["target"]),
            budget:amount(&row["spent"]),minimum:amount(&row["received"]),deadline:U256::from_be_slice(&data[data.len()-32..]),
        };
        assert_eq!(proof.data(), data);
        let logs: Vec<SimLog> = serde_json::from_value(row["logs"].clone()).unwrap();
        assert_eq!(proof.measured(&logs),Some((proof.budget,proof.minimum)));
        let mut restored: Proof = serde_json::from_value(serde_json::to_value(&proof).unwrap()).unwrap();
        assert_eq!(restored.data(), data);
        restored.market.wallet_module = false;
        assert_ne!(restored.data(), data);
        restored.market.wallet_module = true;
        restored.budget += U256::ONE;
        assert!(restored.measured(&logs).is_none());
        restored.budget = proof.budget;
        restored.minimum += U256::ONE;
        assert!(restored.measured(&logs).is_none());
        for wire in ["router", "v5"] {
            let alt = cases.iter().find(|r|r["wire"]==wire && r["pool"]==row["pool"] && r["source"]==row["source"] && r["spent"]==row["spent"]).unwrap();
            assert_eq!(row["received"],alt["received"]);
        }
        count+=1;
    }
    assert_eq!(count,16);
}

#[test]
fn cypher_router_recovers_the_selected_custom_deployer_and_actual_wallet(){
    let token_source:Value=serde_json::from_str(include_str!("../../../../tests/data/first_run_dex_2026-11-26/ethereum_exact_transfers.json")).unwrap();
    let mut exact_tokens:Vec<Address>=token_source["tokens"].as_object().unwrap().values().map(|r|r["address"].as_str().unwrap().parse().unwrap()).collect();
    let core:Value=serde_json::from_reader(GzDecoder::new(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/cypher_v4_ethereum_directory.json.gz").as_slice())).unwrap();
    exact_tokens.extend(core["exact_assets"].as_object().unwrap().values().map(|r|r["address"].as_str().unwrap().parse::<Address>().unwrap()));
    let mut admitted=0;let mut fills=0;
    for raw in [include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/cypher_v4_ethereum_wallets_0.json.gz").as_slice(),
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/cypher_v4_ethereum_wallets_1.json.gz").as_slice(),
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/cypher_v4_ethereum_wallets_2.json.gz").as_slice(),
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/cypher_v4_ethereum_wallets_3.json.gz").as_slice()]{
        let d:Value=serde_json::from_reader(GzDecoder::new(raw)).unwrap();
        let parse=|v:&Value|v.as_str().unwrap().parse::<Address>().unwrap();
        for m in d["markets"].as_array().unwrap(){
            for c in m["cases"].as_array().unwrap().iter().filter(|r|r["mode"]=="router" && r["buy"].is_object()){
                for side in ["buy","sell"]{
                    let leg=&c[side];let x=&leg["exact"];if !x.is_object(){continue}
                    let tx=x["calldata"].as_str().unwrap().parse::<Bytes>().unwrap();
                    let call=ICypherRouter::exactInputSingleCall::abi_decode(&tx).unwrap().args;
                    assert_eq!(call.deployer,parse(&m["deployer"]));
                    let p=Proof{market:Market{pool:parse(&m["pool"]),fee:U24::ZERO,fee_event:false,wallet_module:false,algebra_deployer:Some(call.deployer)},
                        target:parse(&d["router"]),payer:parse(&x["payer"]),input:call.tokenIn,output:call.tokenOut,budget:call.amountIn,minimum:call.minimum,deadline:call.deadline};
                    assert_eq!(p.data(),tx);assert_eq!(p.payer,call.recipient);assert_eq!(x["spent"],p.budget.to_string().as_str());
                    let q=leg["official_quote"]["response"].as_str().unwrap().parse::<Bytes>().unwrap();
                    assert_eq!(super::super::super::pools::nami::decoded(&q),Some(p.minimum));
                    let logs:Vec<SimLog>=serde_json::from_value(x["logs"].clone()).unwrap();
                    let pair_topic=keccak256(p.market.event().0);
                    assert!(logs.iter().any(|l|l.address==p.market.pool && l.topics.first()==Some(&pair_topic)));
                    // Прямая выплата admitted только у ETH-активов с проверенной чистой передачей.
                    // У иных ERC20 архивное сравнение использует физические балансы, в боте работает V5.
                    if exact_tokens.contains(&p.input) && exact_tokens.contains(&p.output){
                        assert_eq!(p.measured(&logs),Some((p.budget,p.minimum)));admitted+=1;
                    }
                    fills+=1;
                }
            }
        }
    }
    assert_eq!(fills,292);assert!(admitted>0);
}
