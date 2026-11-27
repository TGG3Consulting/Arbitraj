//! Сверка расчёта Motoswap с архивными резервами, ставками и поступлениями.
//! Тесты расчёта читают сохранённые параметры и результаты обменов Ethereum.
use super::*;
use std::io::Read;
#[test]
fn both_directions_use_actual_post_purchase_reserves_and_creator_floors() {
    let archive = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/motoswap_ethereum.json.gz"
    );
    let mut body = String::new();
    flate2::read::GzDecoder::new(&archive[..])
        .read_to_string(&mut body)
        .unwrap();
    let data: serde_json::Value = serde_json::from_str(&body).unwrap();
    let book = &data["directory"];
    let mut tested = 0;
    for legs in data["trials"].as_array().unwrap().chunks_exact(2) {
        let pool = book["pairs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["pool"] == legs[0]["pool"])
            .unwrap();
        let a = pool["tokens"][0].as_str().unwrap();
        let b = pool["tokens"][1].as_str().unwrap();
        let num = |n: &serde_json::Value| n.as_str().unwrap().parse::<U256>().unwrap();
        let reserves = [num(&pool["reserves"][0]), num(&pool["reserves"][1])];
        let mut state = MotoswapState {
            sides: None,
            path_module:None,
            fee_router_frozen: false,
            reserves,
            balances: reserves,
            lp_bps: 30,
            protocol_bps: 70,
            creator_bps: 30,
            creators: [a, b]
                .iter()
                .filter(|t| {
                    book["tokens"][**t]["creator"].as_str().unwrap()
                        != "0x0000000000000000000000000000000000000000"
                })
                .count() as u8,
            fee_on_output: [
                pool["fee_on_output"][0].as_bool().unwrap(),
                pool["fee_on_output"][1].as_bool().unwrap(),
            ],
        };
        for leg in legs {
            let pay = &leg["paid"];
            let zero = pay["input"].as_str().unwrap() == a;
            let qty = num(&pay["spent"]);
            let net = num(&pay["received"]);
            assert_eq!(state.swap(zero, true, qty).unwrap().amount_out, net);
            let least = state.swap(zero, false, net).unwrap();
            assert!(least.amount_in <= qty && least.amount_out >= net);
            assert!(
                state
                    .swap(zero, true, least.amount_in - U256::ONE)
                    .is_none_or(|f| f.amount_out < net)
            );
            let i = usize::from(!zero);
            state.reserves[i] += num(&pay["pool_delta"][0]);
            state.reserves[1 - i] -= pay["pool_delta"][1]
                .as_str()
                .unwrap()
                .strip_prefix('-')
                .unwrap()
                .parse::<U256>()
                .unwrap();
            state.balances = state.reserves;
            tested += 1;
        }
    }
    assert_eq!(tested, 16);
}
