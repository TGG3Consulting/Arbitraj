//! Восстановление выплат Motoswap по записанным вызовам и квитанциям одного и нескольких пулов.
//! Тесты восстановления декодируют исходную calldata и сверяют журналы с архивной выплатой кошельку.
use super::*;
use crate::venues::bsc_dex::check::simulate::SimLog;
use serde_json::Value;
use std::io::Read;
fn archive(bytes: &[u8]) -> Value {
    let mut text = String::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_string(&mut text)
        .unwrap();
    serde_json::from_str(&text).unwrap()
}
#[test]
fn restores_the_original_and_guarded_payments_from_signed_calldata() {
    let original = archive(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/motoswap_ethereum.json.gz"
    ));
    let guarded = archive(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/motoswap_ethereum_wallets.json.gz"
    ));
    let bk = &original["directory"];
    let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let hash = |v: &Value| v.as_str().unwrap().parse::<B256>().unwrap();
    let num = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let fee = addr(&bk["roles"]["fee_router"]);
    let core = addr(&bk["roles"]["core"]);
    let code = Code {
        shell: hash(&bk["code"]["fee_router"]),
        implementation: addr(&bk["roles"]["fee_impl"]),
        runtime: hash(&bk["code"]["fee_impl"]),
    };
    let payer = addr(&original["wallet"]);
    let native = original["trials"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (&r["paid"], addr(&r["pool"]), false));
    let wallet = guarded["payments"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["mode"] == "wallet")
        .map(|r| (r, addr(&r["pool"]), true));
    let mut count = 0;
    for (r, pool, protected) in native.chain(wallet) {
        let raw: Bytes = r["calldata"].as_str().unwrap().parse().unwrap();
        let (input, output, maximum, minimum, deadline) = if protected {
            let c = abi::IModule::executeCall::abi_decode_validate(&raw).unwrap();
            (c.input, c.output, c.amount, c.minimum, c.deadline)
        } else {
            let c=abi::IFee::swapExactTokensForTokensSupportingFeeOnTransferTokensCall::abi_decode_validate(&raw).unwrap();
            (c.path[0], c.path[1], c.amountIn, c.amountOutMin, c.deadline)
        };
        let terms = Terms {
            pool,
            fee,
            core,
            fee_code: code,
            target: if protected {
                addr(&guarded["module"])
            } else {
                fee
            },
            module_hash: protected.then(|| hash(&guarded["manifest"]["runtime"])),
        };
        let proof = Proof {
            route: None,
            terms,
            payer,
            input,
            output,
            maximum,
            minimum,
            deadline,
        };
        let saved = serde_json::to_vec(&proof).unwrap();
        let mut restored: Proof = serde_json::from_slice(&saved).unwrap();
        let logs: Vec<SimLog> = serde_json::from_value(r["logs"].clone()).unwrap();
        assert_eq!(restored.data(), raw);
        assert_eq!(
            restored.measured(&logs),
            Some((num(&r["spent"]), num(&r["received"])))
        );
        restored.minimum += U256::ONE;
        assert!(restored.measured(&logs).is_none());
        count += 1;
    }
    assert_eq!(count, 32);
}
#[test]
fn recovers_both_physical_pools_and_the_signed_terminal_minimum() {
    let book = archive(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/motoswap_ethereum.json.gz"
    ));
    let data = archive(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/motoswap_ethereum_paths.json.gz"
    ));
    let dir = &book["directory"];
    let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let num = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let hash = |v: &Value| v.as_str().unwrap().parse::<B256>().unwrap();
    let fee = addr(&dir["roles"]["fee_router"]);
    let payer = addr(&data["owner"]);
    let mut count = 0;
    for r in data["payments"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["mode"] != "v5_steps")
    {
        let guarded = r["mode"] == "guarded";
        let raw: Bytes = r["calldata"].as_str().unwrap().parse().unwrap();
        let (path, amount, minimum, deadline) = if guarded {
            let c = abi::IModule::executePathCall::abi_decode_validate(&raw).unwrap();
            (c.path, c.amount, c.minimum, c.deadline)
        } else {
            let c=abi::IFee::swapExactTokensForTokensSupportingFeeOnTransferTokensCall::abi_decode_validate(&raw).unwrap();
            (c.path, c.amountIn, c.amountOutMin, c.deadline)
        };
        let pools: Vec<_> = r["markets"].as_array().unwrap().iter().map(addr).collect();
        let terms = Terms {
            pool: pools[0],
            target: if guarded { addr(&data["module"]) } else { fee },
            fee,
            core: addr(&dir["roles"]["core"]),
            fee_code: Code {
                shell: hash(&dir["code"]["fee_router"]),
                implementation: addr(&dir["roles"]["fee_impl"]),
                runtime: hash(&dir["code"]["fee_impl"]),
            },
            module_hash: guarded.then(|| hash(&data["runtime"])),
        };
        let p = Proof {
            terms,
            payer,
            input: path[0],
            output: *path.last().unwrap(),
            maximum: amount,
            minimum,
            deadline,
            route: Some(Route { pools, path }),
        };
        let mut saved: Proof =
            serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap();
        let logs: Vec<SimLog> = serde_json::from_value(r["logs"].clone()).unwrap();
        assert_eq!(saved.data(), raw);
        assert_eq!(
            saved.measured(&logs),
            Some((num(&r["spent"]), num(&r["received"])))
        );
        saved.minimum += U256::ONE;
        assert_eq!(saved.measured(&logs), None);
        saved.minimum -= U256::ONE;
        saved.route.as_mut().unwrap().pools.swap(0, 1);
        assert_eq!(saved.measured(&logs), None);
        count += 1;
    }
    assert_eq!(count, 12);
}
