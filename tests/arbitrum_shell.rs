//! Файл: tests/arbitrum_shell.rs
//! Что проверяет: решение 486 — Shell Protocol Proteus (вид shell_proteus) на Arbitrum One.
//! 1. По записи своего узла (tests/data/arbitrum_shell_<дата>/, tools/arbitrum_record/shell.py; #[ignore], пока записи
//!    нет): по каждому пулу записи состояние собирается (load_shell::state) из ответов на те же calldata, что читает
//!    бот (load_shell::calls1 / calls2); порт swapGivenInputAmount (math/proteus_swap.rs) даёт ТОТ ЖЕ выход, что
//!    реализация пула на балансах блока, а где реализация откатила — None.
//! 2. Без записи (факты кода): вид shell_proteus — в графе, без своего пути, шаг SHELL = 31 только с исполнителя v5,
//!    вид ядра Proteus, 6 topic0 событий пула; id нативной монеты учёта — keccak256(0x…4574686572, 0).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools::{load_shell, abi_shell, shell_sides, families, exec_kind,
//! exec_callback}, math::proteus_swap, exec::executor::code}.

use std::path::{Path, PathBuf};

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256, address};
use serde_json::Value;

use arb::core::types::{PoolKind, PoolState, ProteusUtility};
use arb::venues::bsc_dex::exec::executor::code::Version;
use arb::venues::bsc_dex::math::proteus_swap;
use arb::venues::bsc_dex::pools::abi_shell::{
    IProteus as R, IShellOcean as O, IShellPool as P, IShellToken as T,
};
use arb::venues::bsc_dex::pools::exec_kind::{self, ExecKind};
use arb::venues::bsc_dex::pools::load_shell::{self, Body, Head};
use arb::venues::bsc_dex::pools::{abi_shell, exec_callback, families};

/// Обёртка ETH Arbitrum One (WETH) — токен пары у нативной стороны учёта (в расчёт не входит).
const WETH: Address = address!("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1");

/// Реализации формулы V1 (docs/SETTINGS_CASES.md dex.proteus_utility_v1: Sourcify = Shell-Protocol bfbba01).
const V1: [Address; 3] = [
    address!("0x537e2d22cc140c15ec03e99be399223750de7737"),
    address!("0xd5a7edb913eae552e341b3ba0704cb2f6971bf7b"),
    address!("0xb827ce2b24518eaa13663aff541632c8cb3e8c71"),
];

/// Последняя по имени папка записи tests/data/arbitrum_shell_*. Паника — записи нет.
fn dir() -> PathBuf {
    let base = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data"));
    let mut found: Vec<PathBuf> = std::fs::read_dir(&base)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.starts_with("arbitrum_shell_")
        })
        .collect();
    found.sort();
    found
        .pop()
        .expect("нет записи: tools/arbitrum_record/shell.py")
}

/// JSON из файла записи.
fn json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Ответ записи на вызов (to, data): байты или None (откат / нет ответа).
fn answer(rec: &Value, to: Address, data: &Bytes) -> Option<Bytes> {
    let k = format!("{to:#x}|{data}").to_lowercase();
    rec["answers"]
        .get(&k)?
        .get("result")?
        .as_str()?
        .parse()
        .ok()
}

/// Разбор ответа функции. Паника — не по ABI.
fn dec<C: SolCall>(b: &[u8]) -> C::Return {
    C::abi_decode_returns(b).unwrap()
}

/// Сторона записи (токен, нативная) — нативная сторона в паре бота — обёртка.
fn side(v: &Value) -> (Address, bool) {
    let native = v["native"].as_bool().unwrap();
    let t: Address = v["token"].as_str().unwrap().parse().unwrap();
    (if native { WETH } else { t }, native)
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/shell.py"]
fn recorded_quotes_match_implementation() {
    let recs = json(&dir().join("pools.json"));
    let (mut checked, mut same, mut pools) = (0, 0, 0);
    for rec in recs.as_array().unwrap() {
        if let Some(why) = rec.get("skip") {
            eprintln!("{}: пропущен в записи — {why}", rec["pool"]);
            continue;
        }
        let pool: Address = rec["pool"].as_str().unwrap().parse().unwrap();
        let c1 = load_shell::calls1(pool);
        let a = |i: usize| answer(rec, c1[i].0, &c1[i].1).expect("ответ calls1");
        let head = Head {
            ocean: dec::<P::oceanCall>(&a(0)),
            x_id: dec::<P::xTokenCall>(&a(1)),
            y_id: dec::<P::yTokenCall>(&a(2)),
            implementation: dec::<P::implementationCall>(&a(3)),
            frozen: dec::<P::poolFrozenCall>(&a(4)),
        };
        let (x, y) = (side(&rec["sides"][0]), side(&rec["sides"][1]));
        let c2 = load_shell::calls2(pool, &head, x, y);
        let b = |i: usize| answer(rec, c2[i].0, &c2[i].1).expect("ответ calls2");
        let mut n = 7;
        let mut decs = [None, None];
        for (k, s) in [x, y].iter().enumerate() {
            if !s.1 {
                decs[k] = Some(dec::<T::decimalsCall>(&b(n)));
                n += 1;
            }
        }
        let body = Body {
            balances: dec::<O::balanceOfBatchCall>(&b(0)),
            divisor: dec::<O::unwrapFeeDivisorCall>(&b(1)),
            slopes: dec::<R::getSlopesCall>(&b(2)),
            a: dec::<R::getAsCall>(&b(3)),
            b: dec::<R::getBsCall>(&b(4)),
            k: dec::<R::getKsCall>(&b(5)),
            base_fee: dec::<R::BASE_FEECall>(&b(6)),
            dec_x: decs[0],
            dec_y: decs[1],
        };
        let utility = if V1.contains(&head.implementation) {
            ProteusUtility::V1
        } else {
            ProteusUtility::V2
        };
        let PoolState::Proteus(d) = load_shell::state(&head, x, y, utility, &body).unwrap() else {
            panic!("не Proteus");
        };
        pools += 1;
        for q in rec["quotes"].as_array().unwrap() {
            let amount: U256 = q["amount"].as_str().unwrap().parse().unwrap();
            let in_x = q["side"].as_u64().unwrap() == 0;
            let theirs: Option<U256> = q
                .get("result")
                .and_then(|r| r.as_str())
                .and_then(|r| r.parse::<Bytes>().ok())
                .map(|r| dec::<R::swapGivenInputAmountCall>(&r));
            let ours = proteus_swap::swap_given_input(&d, d.x_bal, d.y_bal, amount, in_x);
            checked += 1;
            if ours == theirs {
                same += 1;
            } else {
                eprintln!("{pool:#x} x={in_x} {amount}: реализация {theirs:?}, порт {ours:?}");
            }
        }
    }
    eprintln!("пулов {pools}, квот {same}/{checked}");
    assert!(checked > 0, "в записи нет квот");
    assert_eq!(same, checked, "порт swapGivenInputAmount ≠ реализации");
}

#[test]
fn family_kind_and_step() {
    let (pk, proto, graphable, tradable, fork) = families::spec("shell_proteus").unwrap();
    assert_eq!(pk, PoolKind::Proteus);
    assert_eq!(proto, families::Proto::ShellProteus);
    assert!(graphable && !tradable && !fork);
    assert_eq!(exec_kind::of("shell_proteus"), Some(ExecKind::Shell));
    assert_eq!(ExecKind::Shell.name(), "SHELL");
    assert_eq!(exec_callback::since(ExecKind::Shell), Version::V5);
    assert!(exec_callback::step_absent(ExecKind::Shell, Version::V4).is_some());
    assert_eq!(abi_shell::topics().len(), 6);
    assert_eq!(abi_shell::liquidity_topics().len(), 3);
    // id «Ether» учёта — keccak256(0x…4574686572, 0); то же значение xToken / yToken пулов ETH (чтение 20.11.2026).
    let ether: U256 = "0x97a93559a75ccc959fc63520f07017eed6d512f74e4214a7680ec0eefb5db5b4"
        .parse()
        .unwrap();
    assert_eq!(abi_shell::ether_id(), ether);
}
