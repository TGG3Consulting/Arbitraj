//! Файл: tests/bsc_dex_v2_executor_revert.rs
//! Что делает: ревью решения 280 — на РЕАЛЬНЫХ симуляциях записи tests/data/v2_executor_balance_2026-11-15 (блок
//! 133628447), без сети: (1) прежняя сверка (перевод владельцу, approve и execute на запрошенную сумму) у продажи
//! dao_swap (монета с налогом на перевод из пары) откатывалась «token call» — transferFrom не хватило, а не «K»; у
//! orangedx (без налога) проходила; (2) классификация отката: «…: K» — пара (pair_k), «…: TRANSFER_FAILED» — перевод
//! монеты (coin_transfer); строки require — из кода пар записи (pair_code_strings).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{quote::v2_pair_sim, check::simulate}.

use alloy::sol_types::{Revert, SolError};
use alloy_primitives::U64;
use serde_json::Value;

use arb::venues::bsc_dex::check::simulate::CallResult;
use arb::venues::bsc_dex::quote::v2_pair_sim::{coin_transfer, pair_k};

const DATA: &str = "tests/data/v2_executor_balance_2026-11-15/sims.json";

fn rec() -> Value {
    serde_json::from_str(&std::fs::read_to_string(DATA).unwrap()).unwrap()
}

fn results(v: &Value) -> Vec<CallResult> {
    serde_json::from_value(v.clone()).unwrap()
}

#[test]
fn old_layout_failed_taxed_sell_and_classification() {
    let r = rec();
    let dao = results(&r["dex"]["dao_swap"]["sell_old_layout"]["results"]);
    assert!(!dao[5].ok() && !pair_k(&dao[5]) && dao[5].why().ends_with("token call"));
    let orange = results(&r["dex"]["orangedx"]["sell_old_layout"]["results"]);
    assert!(
        orange.iter().all(|c| c.ok()),
        "без налога прежняя сверка проходила"
    );
    for (id, d) in r["dex"].as_object().unwrap() {
        let k: Vec<&str> = d["pair_code_strings"]["k"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert!(!k.is_empty(), "{id}");
        for s in d["pair_code_strings"]["transfer_failed"]
            .as_array()
            .unwrap()
        {
            // Error(string) с текстом require пары из её кода — так пара откатывает несостоявшийся перевод монеты.
            let reason = s.as_str().unwrap().to_string();
            let c = CallResult {
                status: U64::ZERO,
                return_data: Revert { reason }.abi_encode().into(),
                gas_used: U64::ZERO,
                error: None,
                logs: Vec::new(),
            };
            assert!(coin_transfer(&c) && !pair_k(&c), "{id}");
        }
    }
}
