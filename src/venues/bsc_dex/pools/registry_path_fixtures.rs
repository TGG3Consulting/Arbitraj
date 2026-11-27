//! Записи реестра из снимков штатных пар BSC и Ethereum.
use super::*;
use crate::core::types::{Pool, VenueId};
use crate::venues::bsc_dex::pools::abi_v2v3::IV2Pair;
use alloy::sol_types::SolCall;
use alloy_primitives::{Bytes, address};

fn contract_pool(
    family: &'static families::Family,
    venue: &str,
    pair: Address,
    tokens: [Address; 2],
    state: PoolState,
    block: u64,
) -> Entry {
    Entry {
        family,
        pool: Pool {
            venue: VenueId(venue.into()),
            family: family.id.into(),
            kind: family.kind,
            reference: PoolRef::Contract(pair),
            token0: tokens[0],
            token1: tokens[1],
            fee_pips: None,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state,
            block,
            received_at: chrono::Utc::now(),
        },
        ticker: String::new(),
        coin: tokens[0],
        usdt: tokens[1],
        key: None,
        ticks: None,
        bins: None,
        synced_block: block,
        synced_log: None,
        valid: true,
        asleep: false,
        state_gen: 0,
    }
}

pub(super) fn pancake() -> Entry {
    let archive: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/routes_2026-10-30/responses.json"
    )).unwrap();
    let row = archive["results"].as_array().unwrap().iter()
        .find(|row| row["name"] == "WBNB-USDT v2 getReserves").unwrap();
    let params = &row["request"]["params"];
    let block = u64::from_str_radix(params[1].as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
    let result: Bytes = row["response"]["result"].as_str().unwrap().parse().unwrap();
    let raw = IV2Pair::getReservesCall::abi_decode_returns(&result).unwrap();
    let usdt = address!("55d398326f99059ff775485246999027b3197955");
    let wbnb = address!("bb4cdb9cbd36b01bd1cbaebf2de08d9173bc095c");
    let mut e = contract_pool(
        families::make("pancakeswap", "pancakeswap_v2").unwrap(),
        "bsc_dex", addr(&params[0]["to"]), [usdt, wbnb],
        PoolState::V2 { reserve0: U256::from(raw.reserve0), reserve1: U256::from(raw.reserve1) },
        block,
    );
    e.ticker = "WBNB".into();
    e.coin = wbnb;
    e.usdt = usdt;
    e.pool.fee_pips = Some(2500);
    e
}

pub(super) fn moto() -> Entry {
    let a: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/motoswap_ethereum.json.gz")[..],
    )).unwrap();
    let paths: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/motoswap_ethereum_paths.json.gz")[..],
    )).unwrap();
    let book = &a["directory"];
    let r = &book["pairs"][0];
    assert!(paths["payments"][0]["markets"].as_array().unwrap().contains(&r["pool"]));
    let tokens = [&r["tokens"][0], &r["tokens"][1]];
    let amounts = |key: &str| [0, 1].map(|n| r[key][n].as_str().unwrap().parse::<U256>().unwrap());
    let bps = |key: &str| u16::try_from(book[key].as_u64().unwrap()).unwrap();
    let state = MotoswapState {
        sides: None,
        path_module: Some(addr(&paths["module"])),
        fee_router_frozen: book["fee_router_frozen"].as_bool().unwrap(),
        reserves: amounts("reserves"),
        balances: amounts("balances"),
        lp_bps: bps("lp_bps"),
        protocol_bps: bps("protocol_bps"),
        creator_bps: bps("creator_bps"),
        creators: tokens.iter().filter(|t| !addr(&book["tokens"][t.as_str().unwrap()]["creator"]).is_zero()).count() as u8,
        fee_on_output: [0, 1].map(|n| r["fee_on_output"][n].as_bool().unwrap()),
    };
    contract_pool(
        families::make("motoswap", "motoswap_v2").unwrap(), "eth_dex", addr(&r["pool"]),
        tokens.map(addr), PoolState::Motoswap(Box::new(state)), a["block"].as_u64().unwrap(),
    )
}
