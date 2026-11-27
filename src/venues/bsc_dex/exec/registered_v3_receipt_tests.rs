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
                maverick_chain:None,maverick:false,algebra_wallet:None,algebra_short_event:false,algebra_deployer:None,fee_event:extended,router02:None,slipstream:None,wallet_module:false,
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
            market: Market { pool:parse(&row["pool"]), fee:U24::from(market["fee"].as_u64().unwrap() as u32), maverick_chain:None,maverick:false,algebra_wallet:None,algebra_short_event:false,algebra_deployer:None,fee_event:false,router02:None,slipstream:None,wallet_module:true },
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
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/cypher_v4_ethereum_wallets_3.json.gz").as_slice(),
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/cypher_v4_ethereum_extra_0.json.gz").as_slice(),
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/cypher_v4_ethereum_extra_1.json.gz").as_slice(),
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/cypher_v4_ethereum_extra_2.json.gz").as_slice(),
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/cypher_v4_ethereum_extra_3.json.gz").as_slice()]{
        let d:Value=serde_json::from_reader(GzDecoder::new(raw)).unwrap();
        let parse=|v:&Value|v.as_str().unwrap().parse::<Address>().unwrap();
        for m in d["markets"].as_array().unwrap(){
            for c in m["cases"].as_array().unwrap().iter().filter(|r|r["mode"]=="router" && r["buy"].is_object()){
                for side in ["buy","sell"]{
                    let leg=&c[side];let x=&leg["exact"];if !x.is_object(){continue}
                    let tx=x["calldata"].as_str().unwrap().parse::<Bytes>().unwrap();
                    let call=ICypherRouter::exactInputSingleCall::abi_decode(&tx).unwrap().args;
                    assert_eq!(call.deployer,parse(&m["deployer"]));
                    let p=Proof{market:Market{pool:parse(&m["pool"]),fee:U24::ZERO,maverick_chain:None,maverick:false,fee_event:false,router02:None,slipstream:None,wallet_module:false,algebra_wallet:None,algebra_short_event:false,algebra_deployer:Some(call.deployer)},
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
    assert_eq!(fills,336);assert!(admitted>0);
}

/// Каноническая операция раннего Router остаётся однозначной, даже если пул принял только часть бюджета.
#[test]
fn early_slipstream_recovers_actual_payment_and_spacing_from_saved_order() {
    let raw:Value=serde_json::from_reader(GzDecoder::new(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/early_slipstream_wallets.json.gz"
    ).as_slice())).unwrap();
    let meta:Value=serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/early_slipstream_base.json")).unwrap();
    let address=|v:&Value|v.as_str().unwrap().parse::<Address>().unwrap();
    let units=|v:&Value|v.as_str().unwrap().parse::<U256>().unwrap();
    let mut full=0;
    let mut partial=0;
    for row in raw["cases"].as_array().unwrap().iter().filter(|r|r["mode"]=="UniversalRouter"){
        let m=&meta["markets"][row["pool"].as_str().unwrap()];
        let input=address(&row["asset"]);
        let output=m["tokens"].as_array().unwrap().iter().map(address).find(|t|*t!=input).unwrap();
        let tx=&row["exact_transaction"];
        let calldata:Bytes=tx["data"].as_str().unwrap().parse().unwrap();
        let decoded=super::super::ur::IUniversalRouter::executeCall::abi_decode(&calldata).unwrap();
        let bound=Proof{market:Market{pool:address(&row["pool"]),fee:U24::ZERO,wallet_module:false,
            maverick_chain:None,maverick:false,algebra_wallet:None,algebra_short_event:false,algebra_deployer:None,fee_event:false,router02:None,slipstream:Some((m["tick_spacing"].as_i64().unwrap() as i32,
                   address(&meta["permit2"]["address"])))},payer:address(&raw["owner"]),
            target:address(&tx["to"]),input,output,budget:units(&row["amount"]),minimum:units(&row["received"]),deadline:decoded.deadline};
        let mut proof:Proof=serde_json::from_value(serde_json::to_value(bound).unwrap()).unwrap();
        assert_eq!(proof.data(),calldata);
        let logs:Vec<SimLog>=serde_json::from_value(row["trade"]["logs"].clone()).unwrap();
        let spent=units(&row["spent"]);
        assert_eq!(proof.measured(&logs),Some((spent,proof.minimum)));
        assert_eq!(row["minimum_plus_one"]["status"],"0x0");
        if spent<proof.budget {partial+=1;} else{full+=1;}
        proof.minimum+=U256::ONE;
        assert!(proof.measured(&logs).is_none());
    }
    assert_eq!((full,partial),(10,2));
}

#[test]
fn both_router02_keys_reproduce_paid_calldata_and_receipts_after_restart() {
    use crate::venues::bsc_dex::exec::v3_router02::Key;
    use alloy_primitives::aliases::I24;
    let fixture = include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/wagmi_thick_base.json.gz");
    let books: Value = serde_json::from_reader(GzDecoder::new(fixture.as_slice())).unwrap();
    let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let units = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let virtus = include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/virtus_cl_base.json.gz");
    let mut books=books;
    books["virtus"] = serde_json::from_reader(GzDecoder::new(virtus.as_slice())).unwrap();
    let guarded = include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/wagmi_thick_modules.json.gz");
    let modules: Value = serde_json::from_reader(GzDecoder::new(guarded.as_slice())).unwrap();
    for (name, wire, count) in [("wagmi","router02",16), ("thick","router02",16), ("2thick","router02",12), ("virtus","slipstream",36),
                                ("wagmi","module",16), ("thick","module",16)] {
        let book = &books[name];
        let mut verified = 0;
        let module = wire == "module";
        let original = if module { &modules[name] } else { book };
        assert_eq!(original["selector"],book["selector"]);
        for row in original["cases"].as_array().unwrap().iter().filter(|r| r["wire"] == wire) {
            let market = book["markets"].as_array().unwrap().iter().find(|m| m["pool"] == row["pool"]).unwrap();
            let key = if name=="virtus" {Key::InlineSpacing(I24::try_from(market["spacing"].as_i64().unwrap()).unwrap())} else if name == "2thick" { Key::Spacing(I24::try_from(market["spacing"].as_i64().unwrap()).unwrap()) } else { Key::Fee };
            let tx = &row["tx"];
            let signed: Bytes = tx["data"].as_str().unwrap().parse().unwrap();
            let proof = Proof {
                market: Market {
                    pool: addr(&row["pool"]), fee: U24::from(market["current_fee"].as_u64().unwrap() as u32),
                    algebra_wallet:None,algebra_short_event:false,algebra_deployer: None, fee_event:false, wallet_module:module,
                    maverick:false, maverick_chain:None, slipstream:None, router02:Some(key),
                },
                target:addr(&tx["to"]), payer:addr(&tx["from"]), input:addr(&row["input"]),
                output:addr(&row["output"]), budget:units(&row["spent"]), minimum:units(&row["received"]),
                deadline:U256::from_be_slice(if module { &signed[signed.len()-32..] } else if name=="virtus" { &signed[4+4*32..4+5*32] } else { &signed[4..36] }),
            };
            let restored: Proof = serde_json::from_value(serde_json::to_value(proof).unwrap()).unwrap();
            let logs: Vec<Log> = serde_json::from_value(row["logs"].clone()).unwrap();
            assert_eq!(restored.data(), signed);
            assert_eq!(restored.amounts(&logs,restored.payer,restored.input,restored.output),
                       Some((restored.budget,restored.minimum)));
            let mut other = restored.clone();
            other.market.router02 = None;
            other.market.wallet_module = false;
            assert_ne!(other.data(), signed);
            other.minimum += U256::from(1);
            assert!(other.amounts(&logs,other.payer,other.input,other.output).is_none());
            verified += 1;
        }
        assert_eq!(verified, count);
    }
}

#[test]
fn shibaswap_original_router_reconciles_all_recorded_wallet_payments() {
    const ARCHIVE: &[u8] = include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/shibaswap_v2_ethereum_payments.json.gz");
    let archive: Value = serde_json::from_reader(GzDecoder::new(ARCHIVE)).unwrap();
    let address = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let amount = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let mut matched = 0;
    for row in archive["cases"].as_array().unwrap().iter().filter(|r| r["wire"] == "router") {
        let signed = &row["signed"];
        let calldata: Bytes = signed["data"].as_str().unwrap().parse().unwrap();
        let order = IV3Router::exactInputSingleCall::abi_decode(&calldata).unwrap().params;
        let market = archive["markets"].as_array().unwrap().iter().find(|m| m["pool"] == row["pool"]).unwrap();
        let proof = Proof {
            market: Market {
                pool:address(&row["pool"]),fee:U24::from(market["fee"].as_u64().unwrap() as u32),
                algebra_wallet:None,algebra_short_event:false,algebra_deployer:None,fee_event:false,wallet_module:false,maverick:false,maverick_chain:None,
                slipstream:None,router02:None,
            },
            target:address(&signed["to"]),payer:address(&signed["from"]),input:address(&row["source"]),
            output:address(&row["target"]),budget:amount(&row["spent"]),minimum:amount(&row["received"]),
            deadline:order.deadline,
        };
        assert_eq!(proof.data(),calldata);
        assert_eq!(proof.target,address(&archive["roles"]["router"]["address"]));
        assert_eq!(amount(&row["quote"][0]),proof.minimum);
        let logs:Vec<Log>=serde_json::from_value(row["logs"].clone()).unwrap();
        assert_eq!(proof.amounts(&logs,proof.payer,proof.input,proof.output),Some((proof.budget,proof.minimum)));
        assert!(row["minimum_plus_one"].is_object() && row["expired"].is_object());
        matched+=1;
    }
    assert_eq!(matched,24);
}

#[test]
fn theorem_short_swap_event_recovers_the_signed_wallet_order() {
    let archive: Value = serde_json::from_reader(GzDecoder::new(
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/theorem_ethereum.json.gz").as_slice()
    )).unwrap();
    let address = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let units = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let mut matched = 0;
    let mut violated = 0;
    for row in archive["wallets"]["cases"].as_array().unwrap().iter().filter(|r| r["wire"] == "router") {
        for side in ["buy", "sell"] {
            let leg = &row[side];
            if let Some(controls) = leg["controls"].as_array() {
                assert_eq!(controls[0]["status"], "0x1");
                assert_eq!(units(&leg["quote"]["received"]), units(&controls[0]["received"]) + U256::ONE);
                violated += 1;
            }
            let Some(tx) = leg.get("transaction") else {continue};
            let accepted = [address(&leg["input"]), address(&leg["output"])].contains(
                &alloy_primitives::address!("f442ff10b8def89514560a66c0ad28777094636a"));
            if !accepted {continue}
            let data: Bytes = tx["data"].as_str().unwrap().parse().unwrap();
            let args = ICypherRouter::exactInputSingleCall::abi_decode(&data).unwrap().args;
            let proof = Proof {
                market:Market{pool:address(&row["pool"]),fee:U24::ZERO,algebra_deployer:Some(args.deployer),
                    algebra_wallet:None,algebra_short_event:true,fee_event:false,wallet_module:false,maverick:false,maverick_chain:None,
                    slipstream:None,router02:None},
                target:address(&tx["to"]),payer:address(&tx["from"]),input:args.tokenIn,output:args.tokenOut,
                budget:args.amountIn,minimum:args.minimum,deadline:args.deadline,
            };
            let mut saved: Proof = serde_json::from_value(serde_json::to_value(proof).unwrap()).unwrap();
            let logs: Vec<SimLog> = serde_json::from_value(leg["logs"].clone()).unwrap();
            assert_eq!(saved.data(), data);
            assert_eq!(saved.measured(&logs), Some((units(&leg["spent"]), units(&leg["received"]))));
            saved.market.algebra_short_event = false;
            assert!(saved.measured(&logs).is_none());
            matched += 1;
        }
    }
    assert_eq!((matched, violated), (4, 2));
}
