//! Файл: tests/bsc_dex_native.rs
//! Что делает: нативная монета BNB (решение 62) без сети:
//! 1) план сделки: продажа BNB — команды Universal Router [WRAP_ETH, V2_SWAP_EXACT_IN (платит роутер), BALANCE_CHECK_ERC20],
//!    value транзакции = сумма; покупка — [V2_SWAP_EXACT_IN (выход роутеру), UNWRAP_WETH(кошелёк, минимум)], без
//!    BALANCE_CHECK и без value; входы WRAP/UNWRAP — abi.encode(recipient, amount) (Dispatcher.sol обоих UR, exec/ur.rs);
//! 2) путь через промежуточную с пулом V4 на стороне BNB — честный отказ NoRoute;
//! 3) список ТОЧНЫХ вызовов узла для записи живых ответов (tests/data/native_calls_2026-10-24.json) собран этим же кодом
//!    (план сделки) — тест сверяет файл с кодом; запись ответов — Тигран (eth_simulateV1 с WRAP_ETH/UNWRAP_WETH,
//!    газ и события Deposit/Withdrawal). Перезаписать файл: cargo test --test bsc_dex_native -- --ignored
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::{native, plan, chain, ur}, arb::settings (настоящая SQLite во временной
//! папке). Адреса BSC: WBNB — weth9 в параметрах развёртывания Universal Router (PancakeSwap DeployBsc.s.sol#L24,
//! Uniswap DeployBSC.s.sol#L11); USDT — как у BingX в сети BSC (записи tests/data); Universal Router PancakeSwap —
//! tx_to настоящих обменов в tests/data/indexers_2026-10-24/arb_node_responses_2_2026-10-24.json.

use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, Bytes, U256, address};
use chrono::Utc;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::types::{Pool, PoolRef, PoolState, Side, VenueId};
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::exec::chain::Step;
use arb::venues::bsc_dex::exec::native;
use arb::venues::bsc_dex::exec::plan::{self, Order, Plan};
use arb::venues::bsc_dex::exec::ur::{self, ADDRESS_THIS, IUniversalRouter};
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::registry::Entry;

const WBNB: Address = address!("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c");
const USDT: Address = address!("0x55d398326f99059fF775485246999027B3197955");
const PCS_UR: Address = address!("0xd9C500DfF816a1Da21A48A732d3498Bf09dc9AEB");
const PCS_V2_FACTORY: Address = address!("0xcA143Ce32Fe78f1f7019d7d551a6402fC5350c73");
const PCS_V2_ROUTER: Address = address!("0x10ED43C718714eb63d5aA57B78B54704E256024E");
/// Кошелёк для записи симуляции: горячий кошелёк Binance в BSC (есть BNB и USDT). Нет — любой адрес с ними.
const HOLDER: Address = address!("0x8894E0a0c962CB723c1976a4421c95949bE2D4E3");
/// Срок execute для записи: блок времени + dex.deadline_s (60) = 2 000 000 000 (2033 г.).
const NOW: u64 = 1_999_999_940;
const CALLS: &str = "tests/data/native_calls_2026-10-24.json";

/// Окружение на настоящей SQLite: настройки DEX строки pancakeswap_v2 и нативной монеты.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-native-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    let c = Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    };
    let v = Scope::Venue("bsc_dex".into());
    let s = Scope::Dex("pancakeswap_v2".into());
    for (name, scope, val) in [
        ("dex.native_ticker", v.clone(), json!("BNB")),
        ("dex.native_wrapped", v.clone(), json!(WBNB.to_string())),
        ("dex.deadline_s", v, json!(60)),
        ("dex.kind", s.clone(), json!("pancakeswap_v2")),
        ("dex.universal_router", s.clone(), json!(PCS_UR.to_string())),
        (
            "dex.universal_router_version",
            s.clone(),
            json!("pancakeswap_infinity"),
        ),
        // Permit2 в плане нативной монеты не нужен (платит роутер), но настройка строки обязательна.
        (
            "dex.permit2",
            s,
            json!("0x31c2F6fcFf4F8759b3Bd5Bf0e1084A055615c768"),
        ),
    ] {
        c.settings
            .set(name, scope, Some(val), "тест")
            .await
            .unwrap();
    }
    c
}

/// Пул вида `kind` (адрес пары V2 в данные вызова не входит — путь задаётся токенами).
fn entry(kind: &str) -> Entry {
    let f = families::make(kind, kind).unwrap();
    Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Contract(Address::ZERO),
            token0: USDT,
            token1: WBNB,
            fee_pips: None,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::V2 {
                reserve0: U256::ZERO,
                reserve1: U256::ZERO,
            },
            block: 1,
            received_at: Utc::now(),
        },
        ticker: "BNB".into(),
        coin: WBNB,
        usdt: USDT,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
        asleep: false,
    }
}

/// План продажи (WRAP_ETH) или покупки (UNWRAP_WETH) BNB в одном пуле V2 — тем же кодом, что сделка.
async fn native_plan(c: &Ctx, side: Side, amount: U256, wallet: Address) -> Plan {
    let (tin, tout) = if side == Side::Sell {
        (WBNB, USDT)
    } else {
        (USDT, WBNB)
    };
    let o = Order {
        token_in: tin,
        token_out: tout,
        amount_in: amount,
        min_out: U256::from(1u8),
    };
    let mut legs = vec![plan::single(&entry("pancakeswap_v2"), o)];
    let w = native::adapt(c, side, &mut legs).unwrap();
    if side == Side::Buy {
        // Для записи без Permit2: USDT заранее на роутере (вызов transfer до execute) — платит роутер.
        legs[0][0].from_wallet = false;
    }
    plan::build(c, &legs[0], wallet, NOW, true)
        .await
        .unwrap()
        .with_native(w)
}

/// Команды и входы execute. Ошибок нет.
fn decode(data: &Bytes) -> (Vec<u8>, Vec<Bytes>) {
    let call = IUniversalRouter::executeCall::abi_decode(data).unwrap();
    (call.commands.to_vec(), call.inputs)
}

#[tokio::test]
async fn sell_wraps_and_buy_unwraps() {
    let c = ctx("plan").await;
    let wallet = HOLDER;
    let amount = U256::from(10u64).pow(U256::from(16u8));
    let sell = native_plan(&c, Side::Sell, amount, wallet).await;
    assert!(
        sell.approvals.is_empty(),
        "продажа BNB: платит роутер, разрешений нет"
    );
    assert_eq!(sell.value(), amount);
    let (cmds, inputs) = decode(&sell.data(U256::from(5u8)));
    assert_eq!(
        cmds,
        vec![ur::WRAP_ETH, ur::V2_SWAP_EXACT_IN, ur::BALANCE_CHECK_ERC20]
    );
    assert_eq!(
        inputs[0],
        Bytes::from((ADDRESS_THIS, amount).abi_encode_params())
    );
    // V2: (recipient, amountIn, amountOutMin, path, payerIsUser = false).
    let (to, a_in, _, path, payer): (Address, U256, U256, Vec<Address>, bool) =
        SolValue::abi_decode_params(&inputs[1]).unwrap();
    assert_eq!(
        (to, a_in, path, payer),
        (wallet, amount, vec![WBNB, USDT], false)
    );
    assert_eq!(inputs[2], ur::balance_check(wallet, USDT, U256::from(6u8)));

    let usdt = U256::from(5u64) * U256::from(10u64).pow(U256::from(18u8));
    let buy = native_plan(&c, Side::Buy, usdt, wallet).await;
    assert_eq!(buy.value(), U256::ZERO);
    let (cmds, inputs) = decode(&buy.data(U256::ZERO));
    assert_eq!(cmds, vec![ur::V2_SWAP_EXACT_IN, ur::UNWRAP_WETH]);
    let (to, _, _, path, _): (Address, U256, U256, Vec<Address>, bool) =
        SolValue::abi_decode_params(&inputs[0]).unwrap();
    assert_eq!((to, path), (ADDRESS_THIS, vec![USDT, WBNB]));
    assert_eq!(
        inputs[1],
        Bytes::from((wallet, U256::from(1u8)).abi_encode_params())
    );
}

#[tokio::test]
async fn v4_two_hop_on_native_side_is_refused() {
    let c = ctx("v4").await;
    let o = |a, b| Order {
        token_in: a,
        token_out: b,
        amount_in: U256::from(1u8),
        min_out: U256::from(1u8),
    };
    let step = |e: Entry, ord, fw, tw| Step {
        entry: e,
        order: ord,
        from_wallet: fw,
        to_wallet: tw,
    };
    let mut legs = vec![vec![
        step(entry("pancakeswap_infinity_cl"), o(WBNB, USDT), true, false),
        step(entry("pancakeswap_v2"), o(USDT, WBNB), false, true),
    ]];
    let err = native::adapt(&c, Side::Sell, &mut legs).unwrap_err();
    assert_eq!(err.code, ErrorCode::NoRoute);
}

/// Точные вызовы для записи ответов узла (план — тем же кодом, что сделка).
async fn calls() -> Value {
    let c = ctx("calls").await;
    let e16 = U256::from(10u64).pow(U256::from(16u8));
    let usdt = U256::from(5u64) * U256::from(10u64).pow(U256::from(18u8));
    let sell = native_plan(&c, Side::Sell, e16, HOLDER).await;
    let buy = native_plan(&c, Side::Buy, usdt, HOLDER).await;
    let erc20 = |data: Vec<u8>| Bytes::from(data);
    let transfer = arb::venues::bsc_dex::node::abi::IERC20::transferCall {
        to: PCS_UR,
        value: usdt,
    };
    let sym = arb::venues::bsc_dex::node::abi_dex::IV2Router::WETHCall {};
    let pair = arb::venues::bsc_dex::pools::abi_v2v3::IV2Factory::getPairCall {
        tokenA: WBNB,
        tokenB: USDT,
    };
    let empty = address!("0x000000000000000000000000000000000000dEaD");
    let sim = json!({
        "blockStateCalls": [{ "calls": [
            { "from": HOLDER, "to": PCS_UR, "input": sell.data(U256::ZERO), "value": sell.value() },
            { "from": HOLDER, "to": USDT, "input": erc20(transfer.abi_encode()) },
            { "from": HOLDER, "to": PCS_UR, "input": buy.data(U256::ZERO) },
            { "from": HOLDER, "to": empty, "input": "0x", "value": U256::from(10u64).pow(U256::from(15u8)) }
        ]}],
        "validation": false
    });
    json!({
        "about": "Точные вызовы узла BSC для решения 62 (BNB ↔ WBNB, tests/bsc_dex_native.rs). Сначала eth_blockNumber, затем остальные на том же блоке (вместо \"latest\" — номер из первого ответа). Ответы — как вернул узел: {name, request, response}. Симуляция: продажа 0,01 BNB с WRAP_ETH → USDT; перевод 5 USDT на Universal Router; покупка WBNB за эти USDT с UNWRAP_WETH на кошелёк; нативный перевод 0,001 BNB на пустой адрес. Нужны status, gasUsed, logs (с data) каждого вызова. Если у HOLDER нет BNB или 5 USDT — заменить HOLDER на любой адрес с ними во всех вызовах (например кошелёк бота) и сообщить.",
        "calls": [
            { "name": "block", "method": "eth_blockNumber", "params": [] },
            { "name": "WBNB code", "method": "eth_getCode", "params": [WBNB, "latest"] },
            { "name": "PancakeSwap V2 Router WETH()", "method": "eth_call", "params": [{ "to": PCS_V2_ROUTER, "input": erc20(sym.abi_encode()) }, "latest"] },
            { "name": "PancakeSwap V2 getPair(WBNB, USDT)", "method": "eth_call", "params": [{ "to": PCS_V2_FACTORY, "input": erc20(pair.abi_encode()) }, "latest"] },
            { "name": "native wrap/unwrap simulate", "method": "eth_simulateV1", "params": [sim, "latest"] }
        ]
    })
}

#[tokio::test]
async fn native_calls_file_matches_code() {
    let want = calls().await;
    let have: Value = serde_json::from_str(&std::fs::read_to_string(CALLS).unwrap()).unwrap();
    assert_eq!(
        have, want,
        "список вызовов устарел: cargo test --test bsc_dex_native -- --ignored"
    );
}

#[tokio::test]
#[ignore = "вручную: пишет список вызовов для записи ответов узла"]
async fn write_native_calls() {
    let v = calls().await;
    std::fs::write(CALLS, serde_json::to_string_pretty(&v).unwrap() + "\n").unwrap();
}
