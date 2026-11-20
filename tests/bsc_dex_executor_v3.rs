//! Файл: tests/bsc_dex_executor_v3.rs
//! Что делает: решение 395 (t181) — исполнитель ArbExecutor v3: шаги UNI_V4 (19), INF_CL (20), INF_BIN (21) пулов внутри
//! менеджеров Uniswap V4 и PancakeSwap Infinity. Без сети:
//! 1. Запись EVM-теста (tests/data/executor_v3_evm_2026-11-17/singleton_steps.json — contracts/evmtest/singleton.mjs:
//!    байткод v3 из contracts/out/ и ОФИЦИАЛЬНЫЙ код PoolManager / Vault / CLPoolManager / BinPoolManager): данные шага,
//!    которые контракт принял и исполнил, побайтно равны кодированию бота (kind.rs Params::data из ключа), номер вида —
//!    тот же; владелец получил ровно выход события Swap менеджера, менеджер взял ровно вход.
//! 2. Ключи 37 настоящих пулов PancakeSwap Infinity CL с крючками (tests/data/hook_pools_2026-11-16, ответы узла
//!    poolIdToPoolKey): PoolId ключа = id пула записи, data шага INF_CL = слова ключа из ответа менеджера; пул с нативной
//!    валютой (address 0) шагом не кодируется (контракт: «native»).
//! 3. Версии: код v4 / v3 / v2 / v1 различается, вид шага 19–21 — с v3 (since / step_absent), прежние виды v2 — с v2.
//!    Решение 457: v3 — замороженная копия contracts/out/v3/, текущая сборка contracts/out/ — v4 (via-IR).
//! 4. ABI и код: unlockCallback(bytes) / lockAcquired(bytes) — в v3 и v4, нет в v2; в коде v3 и v4 — селекторы
//!    официальных подписей менеджеров (те же строки — в vendor-копиях v4-core / infinity-core); в коде v4 часть из них
//!    — в форме via-IR (exec_callback::has_selector).
//! 5. Шаг из записи реестра (entry.rs): ключ сверяется со ссылкой (PoolId, менеджер), пул шага — PoolManager / Vault.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{exec::executor::{code, kind, kind_key, entry}, pools::{key, exec_kind,
//! exec_callback, families}}, tests/hook_pools_common, tests/route_common.

mod hook_pools_common;
mod route_common;

use alloy::json_abi::JsonAbi;
use alloy_primitives::aliases::I24;
use alloy_primitives::{Address, B256, U256, keccak256};
use serde_json::{Value, json};

use arb::core::types::PoolRef;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::exec::executor::code::{self, Version};
use arb::venues::bsc_dex::exec::executor::entry;
use arb::venues::bsc_dex::exec::executor::kind::{self, ALL_BPS, InfKey, Params, StepSpec, V4Key};
use arb::venues::bsc_dex::pools::exec_callback;
use arb::venues::bsc_dex::pools::exec_kind::{self, ExecKind};
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::key::{InfinityPoolKeyData, PoolKey, UniswapPoolKeyData};

const DIR: &str = "tests/data/executor_v3_evm_2026-11-17";

fn addr(v: &Value) -> Address {
    v.as_str().expect("адрес").parse().expect("адрес")
}

fn num(v: &Value) -> U256 {
    U256::from_str_radix(v.as_str().expect("число"), 10).expect("число")
}

/// Параметры шага по ключу записи EVM-теста. Ошибок нет (паника — запись не по формату).
fn params_of(case: &Value) -> Params {
    let k = case["key"].as_array().expect("ключ");
    match case["kind"].as_u64().expect("вид") {
        19 => Params::UniV4 {
            key: V4Key {
                currency0: addr(&k[0]),
                currency1: addr(&k[1]),
                fee: k[2].as_str().unwrap().parse().unwrap(),
                tick_spacing: I24::try_from(k[3].as_str().unwrap().parse::<i32>().unwrap())
                    .unwrap(),
                hooks: addr(&k[4]),
            },
        },
        n => {
            let key = InfKey {
                currency0: addr(&k[0]),
                currency1: addr(&k[1]),
                hooks: addr(&k[2]),
                manager: addr(&k[3]),
                fee: k[4].as_str().unwrap().parse().unwrap(),
                parameters: k[5].as_str().unwrap().parse().unwrap(),
            };
            if n == 20 {
                Params::InfCl { key }
            } else {
                Params::InfBin { key }
            }
        }
    }
}

#[test]
fn evm_steps_are_bot_encoding() {
    let text = std::fs::read_to_string(format!("{DIR}/singleton_steps.json")).expect("запись");
    let rec: Value = serde_json::from_str(&text).unwrap();
    let cases = rec["cases"].as_array().expect("случаи");
    assert_eq!(cases.len(), 6, "UNI_V4, INF_CL, INF_BIN — по две стороны");
    for c in cases {
        let p = params_of(c);
        assert_eq!(u64::from(p.kind()), c["kind"].as_u64().unwrap());
        let s = kind::encode(&StepSpec {
            params: p,
            pool: addr(&c["pool"]),
            token_in: addr(&c["token_in"]),
            token_out: addr(&c["token_out"]),
            share_bps: ALL_BPS,
        })
        .expect("шаг кодируется");
        assert_eq!(
            format!("{}", s.data),
            c["data"].as_str().unwrap(),
            "{}: data шага бота = байты, принятые контрактом",
            c["name"]
        );
        assert_eq!(
            num(&c["got"]),
            num(&c["event_out"]),
            "{}: получено = выход менеджера",
            c["name"]
        );
        assert_eq!(
            num(&c["amount_in"]),
            num(&c["event_in"]),
            "{}: менеджер взял ровно вход",
            c["name"]
        );
    }
}

#[test]
fn recorded_infinity_keys_encode_as_manager_answers() {
    let rec = hook_pools_common::load();
    let mut seen = 0;
    for (id, p) in &rec.pools {
        if p.key_words.is_empty() {
            continue;
        }
        let key = InfKey {
            currency0: p.currency0,
            currency1: p.currency1,
            hooks: p.hooks,
            manager: p.manager,
            fee: p.fee,
            parameters: p.parameters,
        };
        assert_eq!(
            &key.pool_key().id().to_string(),
            id,
            "PoolId ключа = id пула записи"
        );
        let data = Params::InfCl { key }.data();
        assert_eq!(
            hex::encode(&data),
            p.key_words,
            "data шага INF_CL = слова ключа из poolIdToPoolKey"
        );
        let spec = |tin, tout| StepSpec {
            params: Params::InfCl { key },
            pool: p.manager,
            token_in: tin,
            token_out: tout,
            share_bps: ALL_BPS,
        };
        // В записи у всех 37 пулов обе валюты — ERC-20 (проверено по ответам poolIdToPoolKey).
        kind::encode(&spec(p.currency0, p.currency1)).expect("шаг 0 → 1");
        kind::encode(&spec(p.currency1, p.currency0)).expect("шаг 1 → 0");
        let other = Address::repeat_byte(0x77);
        let why = kind::encode(&spec(p.currency0, other)).expect_err("чужой токен");
        assert!(why.contains("не валюты ключа"), "{why}");
        // Тот же ключ с нативной валютой (address 0, как пул BNB/CAKE из документации PancakeSwap) — не шаг исполнителя.
        let native = InfKey {
            currency0: Address::ZERO,
            ..key
        };
        let why = kind::encode(&StepSpec {
            params: Params::InfCl { key: native },
            ..spec(Address::ZERO, p.currency1)
        })
        .expect_err("нативная валюта");
        assert!(why.contains("нативная валюта"), "{why}");
        seen += 1;
    }
    assert_eq!(seen, 37, "все пулы записи");
}

#[test]
fn versions_and_steps() {
    let (v4, v3, v2, v1) = (
        code::runtime().unwrap(),
        code::runtime_v3().unwrap(),
        code::runtime_v2().unwrap(),
        code::runtime_v1().unwrap(),
    );
    // Решение 457: текущая сборка contracts/out/ — v4.
    assert_eq!(code::version_of(&v4).unwrap(), Some(Version::V4));
    assert_eq!(code::version_of(&v3).unwrap(), Some(Version::V3));
    assert_eq!(code::version_of(&v2).unwrap(), Some(Version::V2));
    assert_eq!(code::version_of(&v1).unwrap(), Some(Version::V1));
    assert_eq!(Version::parse("v3"), Some(Version::V3));
    assert!(Version::V3.native() && Version::V2.native() && !Version::V1.native());
    for k in [ExecKind::UniV4, ExecKind::InfCl, ExecKind::InfBin] {
        assert_eq!(exec_callback::since(k), Version::V3);
        assert!(!exec_callback::v2_only(k));
        assert!(k.singleton());
        assert!(exec_callback::step_absent(k, Version::V3).is_none());
        assert!(exec_callback::step_absent(k, Version::V4).is_none());
        let why = exec_callback::step_absent(k, Version::V2).expect("v2 шага не знает");
        assert!(
            why.contains("v2") && why.contains("с v3") && why.contains(k.name()),
            "{why}"
        );
    }
    assert!(exec_callback::step_absent(ExecKind::Lb, Version::V2).is_none());
    assert!(exec_callback::step_absent(ExecKind::Lb, Version::V1).is_some());
    assert!(exec_callback::step_absent(ExecKind::V3, Version::V1).is_none());
    assert!(!ExecKind::V3.singleton());
    assert_eq!(exec_kind::of("uniswap_v4"), Some(ExecKind::UniV4));
    assert_eq!(
        exec_kind::of("pancakeswap_infinity_cl"),
        Some(ExecKind::InfCl)
    );
    assert_eq!(
        exec_kind::of("pancake_infinity_stable"),
        Some(ExecKind::InfCl)
    );
    assert_eq!(
        exec_kind::of("pancakeswap_infinity_bin"),
        Some(ExecKind::InfBin)
    );
    // Свой Universal Router у видов остаётся (tradable, не форк) — шаг исполнителя добавлен, а не заменил его.
    for k in [
        "uniswap_v4",
        "pancakeswap_infinity_cl",
        "pancakeswap_infinity_bin",
        "pancake_infinity_stable",
    ] {
        let f = families::make("x-395", k).unwrap();
        assert!(f.tradable && !f.fork && f.executor_kind.is_some(), "{k}");
    }
}

/// Решение 441 (ревью, п. 4): замороженный код v3 (contracts/out/v3/) — всегда v3, и до, и после пересборки v4.
#[test]
fn frozen_v3_is_v3() {
    let v3 = code::runtime_v3().expect("код v3 сверен по out/v3/ArbExecutor.sha256");
    assert_eq!(code::version_of(&v3).unwrap(), Some(Version::V3));
    // Запись решения 400 сверяется с SHA-256 кода v3 (out/v3/), а не текущей сборки.
    assert!(code::runtime_sha256_of(Version::V3).is_some());
    assert_eq!(code::runtime_sha256_of(Version::V2), None);
}
fn abi(path: &str) -> JsonAbi {
    let text = std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn has_push4(code: &[u8], sig: &str) -> bool {
    let mut want = vec![0x63u8];
    want.extend_from_slice(&keccak256(sig.as_bytes())[..4]);
    code.windows(5).any(|w| w == want.as_slice())
}

#[test]
fn abi_and_code_call_official_manager_functions() {
    // Решение 457: v3 — contracts/out/v3/, v4 — текущая сборка contracts/out/.
    let (v4, v3, v2) = (
        abi("contracts/out/ArbExecutor.abi"),
        abi("contracts/out/v3/ArbExecutor.abi"),
        abi("contracts/out/v2/ArbExecutor.abi"),
    );
    for sig in ["unlockCallback(bytes)", "lockAcquired(bytes)"] {
        let name = sig.split('(').next().unwrap();
        assert_eq!(v3.function(name).expect(sig)[0].signature(), sig);
        assert_eq!(v4.function(name).expect(sig)[0].signature(), sig);
        assert!(v2.function(name).is_none(), "{sig} — нет в v2");
    }
    let rt = code::runtime_v3().unwrap();
    let rt4 = code::runtime().unwrap();
    // Подписи — строки официальных интерфейсов (vendor-копии без изменений): v4-core IPoolManager.sol L114, L145, L171,
    // L179, L183, IExttload.sol L9; infinity-core Vault.sol / IVault.sol L56, ICLPoolManager.sol, IBinPoolManager.sol L191.
    for sig in [
        "unlock(bytes)",
        "lock(bytes)",
        "sync(address)",
        "settle()",
        "take(address,address,uint256)",
        "exttload(bytes32)",
        "currencyDelta(address,address)",
        "swap((address,address,uint24,int24,address),(bool,int256,uint160),bytes)",
        "swap((address,address,address,address,uint24,bytes32),(bool,int256,uint160),bytes)",
        "swap((address,address,address,address,uint24,bytes32),bool,int128,bytes)",
    ] {
        assert!(has_push4(&rt, sig), "{sig} в коде v3");
        let sel = &keccak256(sig.as_bytes())[..4];
        assert!(exec_callback::has_selector(&rt4, sel), "{sig} в коде v4");
    }
    let src = |p: &str| {
        std::fs::read_to_string(format!(
            "{}/contracts/evmtest/vendor/{p}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    };
    let pm = src("v4-core/src/interfaces/IPoolManager.sol");
    for line in [
        "function unlock(bytes calldata data) external returns (bytes memory);",
        "function sync(Currency currency) external;",
        "function take(Currency currency, address to, uint256 amount) external;",
        "function settle() external payable returns (uint256 paid);",
    ] {
        assert!(pm.contains(line), "v4-core: {line}");
    }
    let bin = src("infinity-core/src/pool-bin/interfaces/IBinPoolManager.sol");
    assert!(bin.contains(
        "function swap(PoolKey memory key, bool swapForY, int128 amountSpecified, bytes calldata hookData)"
    ));
    let vault = src("infinity-core/src/interfaces/IVault.sol");
    assert!(vault.contains(
        "function currencyDelta(address settler, Currency currency) external view returns (int256);"
    ));
}

#[tokio::test]
async fn registry_entry_becomes_singleton_step() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let base = q
        .reg
        .get(&PoolRef::Contract(route_common::WBNB_USDT_V2))
        .expect("пул записи");
    let (usdt, wbnb) = (route_common::USDT, route_common::WBNB);
    let manager = Address::repeat_byte(0x4e);
    // Uniswap V4: пул шага — PoolManager (адрес ссылки), ключ — из записи реестра.
    let uk = PoolKey::Uniswap(UniswapPoolKeyData {
        currency0: usdt,
        currency1: wbnb,
        fee: 3000,
        tick_spacing: I24::try_from(60i32).unwrap(),
        hooks: Address::ZERO,
    });
    let mut e = base.clone();
    e.family = families::make("x-395-v4", "uniswap_v4").unwrap();
    e.pool.reference = PoolRef::Singleton {
        manager,
        id: uk.id(),
    };
    e.key = Some(uk.clone());
    let (params, pool) = entry::params(&q.ctx, &e, usdt).expect("шаг UNI_V4");
    assert_eq!((params.kind(), pool), (19, manager));
    assert_eq!(params.data().to_vec(), uk.abi());
    // PoolId ключа не тот — шаг не строится.
    e.pool.reference = PoolRef::Singleton {
        manager,
        id: B256::repeat_byte(9),
    };
    assert!(entry::params(&q.ctx, &e, usdt).is_err());
    // Infinity CL: пул шага — Vault (dex.vault DEX); менеджер ключа = менеджер ссылки.
    let ik = PoolKey::Infinity(InfinityPoolKeyData {
        currency0: usdt,
        currency1: wbnb,
        hooks: Address::ZERO,
        manager,
        fee: 2500,
        parameters: B256::from(U256::from(10u32 << 16)),
    });
    let mut c = base.clone();
    c.family = families::make("x-395-cl", "pancakeswap_infinity_cl").unwrap();
    c.pool.reference = PoolRef::Singleton {
        manager,
        id: ik.id(),
    };
    c.key = Some(ik.clone());
    assert!(
        entry::params(&q.ctx, &c, usdt).is_err(),
        "dex.vault не задана"
    );
    let vault = Address::repeat_byte(0x5a);
    p.ctx
        .settings
        .set(
            "dex.vault",
            Scope::Dex("x-395-cl".into()),
            Some(json!(format!("{vault:#x}"))),
            "тест",
        )
        .await
        .unwrap();
    let (params, pool) = entry::params(&q.ctx, &c, wbnb).expect("шаг INF_CL");
    assert_eq!((params.kind(), pool), (20, vault));
    assert_eq!(params.data().to_vec(), ik.abi());
    c.pool.reference = PoolRef::Singleton {
        manager: Address::repeat_byte(0x4f),
        id: ik.id(),
    };
    assert!(
        entry::params(&q.ctx, &c, wbnb).is_err(),
        "менеджер ключа ≠ менеджер ссылки"
    );
    // Ревью t181 (Н1): ключ с нативной валютой (address 0) или не от пары записи — пул не исполним исполнителем.
    let mut n = e.clone();
    let nk = PoolKey::Uniswap(UniswapPoolKeyData {
        currency0: Address::ZERO,
        currency1: wbnb,
        fee: 3000,
        tick_spacing: I24::try_from(60i32).unwrap(),
        hooks: Address::ZERO,
    });
    n.pool.reference = PoolRef::Singleton {
        manager,
        id: nk.id(),
    };
    n.key = Some(nk);
    let why = entry::fits(&p.ctx, &n).expect_err("нативная валюта");
    assert!(why.contains("нативная валюта"), "{why}");
    let mut ok = e.clone();
    ok.pool.reference = PoolRef::Singleton {
        manager,
        id: uk.id(),
    };
    assert!(
        entry::fits(&p.ctx, &ok).is_ok(),
        "ключ USDT/WBNB = пара записи"
    );
    // Ревью t181 (Н2): у шага менеджера ссылка Contract — только Infinity Stable; V4 по адресу контракта — не шаг.
    assert!(entry::supported(ok.family, &PoolRef::Contract(manager)).is_err());
    let cl = families::make("x-395-cl", "pancakeswap_infinity_cl").unwrap();
    assert!(entry::supported(cl, &PoolRef::Contract(manager)).is_err());
    let st = families::make("x-395-st", "pancake_infinity_stable").unwrap();
    assert!(entry::supported(st, &PoolRef::Contract(manager)).is_ok());
}

/// Ревью t181 (Н3): правило «маршрут с шагом менеджера, собираемый бандлом, — бандлом» не трогает маршруты без шага
/// менеджера и маршруты с пулом без своего пути (DODO + V4 — только исполнитель). Случай «да» требует настроек каналов
/// бандла — этим тестом не покрыт (проверка собираемости — прежний код quote/graph_exec.rs).
#[tokio::test]
async fn bundle_first_only_for_manager_routes_with_old_path() {
    use arb::venues::bsc_dex::quote::graph_exec::bundle_first;
    let p = route_common::parts().await;
    let q = p.quoting();
    let v4 = families::make("x-395-b4", "uniswap_v4").unwrap();
    let dodo = families::make("x-395-dd", "dodo_dvm").unwrap();
    let v3 = families::make("x-395-b3", "pancakeswap_v3").unwrap();
    assert!(
        !bundle_first(&q.ctx, &[vec![v3], vec![v3]], false),
        "без шага менеджера — прежнее правило"
    );
    assert!(
        !bundle_first(&q.ctx, &[vec![dodo], vec![v4]], false),
        "DODO + V4 — прежнего пути нет"
    );
    // Ревью 2 t181: случай «да» — каналы бандла и dex.bundle_max_routers заданы (как tests/bsc_dex_bundle_tip_plan.rs),
    // пулы со своим Universal Router (строки DEX записи route_common: dex.universal_router задан у uniswap_v3 и
    // pancakeswap_v3). Маршрут «V4 (DEX uniswap_v3 видом uniswap_v4) + PancakeSwap V3» собирается бандлом — бандл раньше
    // исполнителя; без каналов бандла — нет (исполнитель).
    let v4ur = families::make("uniswap_v3", "uniswap_v4").unwrap();
    let pcs = families::make("pancakeswap_v3", "pancakeswap_v3").unwrap();
    assert!(
        !bundle_first(&q.ctx, &[vec![v4ur], vec![pcs]], false),
        "каналов бандла нет — бандл не собирается"
    );
    route_common::channel(&p, "club48", "bundle").await;
    p.ctx
        .settings
        .set(
            "dex.bundle_max_routers",
            Scope::Venue("bsc_dex".into()),
            Some(json!(5)),
            "тест",
        )
        .await
        .unwrap();
    assert!(
        bundle_first(&q.ctx, &[vec![v4ur], vec![pcs]], false),
        "V4 + PancakeSwap V3, бандл собирается — бандл (как у кошелька v2)"
    );
    assert!(
        !bundle_first(&q.ctx, &[vec![v4ur], vec![dodo]], false),
        "V4 + DODO (нет своего пути) — исполнитель"
    );
}

/// Ревью 2 t181, решение 400: запрет развёртывания снимается только записью, где принят хотя бы один класс шага; запись
/// о другом коде (в README нет SHA-256 текущего bin-runtime) не принимает ничего. Приёмка на настоящей записи —
/// tests/bsc_dex_executor_v3_live.rs.
#[test]
fn live_warning_tied_to_runtime_hash() {
    use arb::venues::bsc_dex::exec::executor::{deploy_gate, live_rule};
    let w = deploy_gate::refusal_for(&Ok(Vec::new())).expect("ничего не принято");
    assert!(w.contains(deploy_gate::NOT_ACCEPTED), "{w}");
    let other =
        live_rule::judge("README другой сборки", "[]", "[]", code::runtime_sha256()).unwrap();
    assert!(
        other
            .iter()
            .all(|x| !x.accepted && x.why.contains("запись о другом коде"))
    );
}
