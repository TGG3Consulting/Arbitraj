//! Файл: tests/bsc_dex_sim_time.rs
//! Что делает: время симулированного блока eth_simulateV1 (решение 127, check/sim_base.rs, check/simulate.rs params).
//! 1) Тело запроса без времени (Base::from) совпадает с ЖИВЫМ записанным запросом пробы переводов USDT
//!    (tests/data/service_tax_2026-10-25/responses.json, блок 0x7bb0bc5) — params() собирает ровно то, что ушло на узел.
//! 2) Base::timed (голова + 1 с) добавляет только blockStateCalls[0].blockOverrides.time (hex-секунды) — остальное тело
//!    то же (execution-apis 16caee27 src/schemas/execute.yaml#L100-L117).
//! 3) Срок вызова исполнителя при dex.deadline_s = 3 (рекомендация решения 127) не меньше времени симулированного
//!    блока — проба не откатывается по сроку (раньше при +12 с узла BSC откатывалась бы всегда).
//!
//! Без сети. Сеть (узел принимает time = голова + 1 и отвергает time = голова) — не покрыто, нужна запись на сервере
//! (docs/TASKS.md, решение 127).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::check::{service_tax, simulate, sim_base}, exec::executor::call,
//! tests/route_common.

mod route_common;

use alloy_primitives::{Address, U256, address};
use serde_json::{Value, json};

use arb::core::types::PoolRef;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::check::service_tax;
use arb::venues::bsc_dex::check::simulate::{self, Base};
use arb::venues::bsc_dex::exec::chain::Step;
use arb::venues::bsc_dex::exec::executor::call;
use arb::venues::bsc_dex::exec::plan::Order;

use route_common::{CAKE, CAKE_USDT_PCS, USDT as R_USDT};

const DIR: &str = "tests/data/service_tax_2026-10-25/";
/// USDT BSC, источник и кошелёк — как в записи (tests/bsc_dex_service_tax.rs).
const USDT: Address = address!("0x55d398326f99059fF775485246999027B3197955");
const SOURCE: Address = address!("0x16b9a82891338f9ba80e2d6970fdda79d1eb0dae");
const WALLET: Address = address!("0x8894e0a0c962cb723c1976a4421c95949be2d4e3");

fn record() -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{DIR}responses.json")).unwrap()).unwrap()
}

/// Адреса в записи — строчными, у кода — с контрольной суммой: сравниваем без регистра.
fn lower(v: &Value) -> String {
    v.to_string().to_lowercase()
}

fn hex(v: &Value) -> u64 {
    u64::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

fn amount() -> U256 {
    U256::from(5u8) * U256::from(10u8).pow(U256::from(18u8))
}

#[test]
fn untimed_body_matches_recorded_request() {
    let rec = record();
    let block = hex(&rec["block"]);
    let calls = service_tax::calls(USDT, SOURCE, WALLET, amount());
    let body = simulate::params(&calls, &[], &[], Base::from(block));
    assert_eq!(lower(&body), lower(&rec["request"]["params"]));
    assert!(
        body[0]["blockStateCalls"][0]
            .get("blockOverrides")
            .is_none()
    );
}

#[test]
fn timed_body_adds_only_block_time() {
    let rec = record();
    let block = hex(&rec["block"]);
    // Симулированный блок записи — номер базового + 1; время узел поставил сам (у BSC — базовое + 12 с,
    // simulate.go#L56-L57): время головы восстанавливается отсюда только для примера числа.
    let sim = &rec["response"]["result"][0];
    assert_eq!(hex(&sim["number"]), block + 1);
    let head_time = hex(&sim["timestamp"]) - 12;
    let calls = service_tax::calls(USDT, SOURCE, WALLET, amount());
    let base = Base::timed(block, head_time);
    assert_eq!(
        base,
        Base {
            number: block,
            time: Some(head_time + 1)
        }
    );
    let mut body = simulate::params(&calls, &[], &[], base);
    let ov = body[0]["blockStateCalls"][0]
        .as_object_mut()
        .unwrap()
        .remove("blockOverrides")
        .expect("blockOverrides задан");
    assert_eq!(ov, json!({ "time": format!("{:#x}", head_time + 1) }));
    assert_eq!(body[1], json!(format!("{block:#x}")));
    // Без blockOverrides — ровно записанный запрос.
    assert_eq!(lower(&body), lower(&rec["request"]["params"]));
}

#[test]
fn untimed_base_has_no_time() {
    assert_eq!(
        Base::from(7),
        Base {
            number: 7,
            time: None
        }
    );
    assert_eq!(Base::timed(7, u64::MAX).time, Some(u64::MAX));
}

#[tokio::test]
async fn short_deadline_passes_simulated_block_time() {
    const NOW: u64 = 1_000_000;
    let p = route_common::parts().await;
    let v = Scope::Venue("bsc_dex".into());
    p.ctx
        .settings
        .set("dex.deadline_s", v, Some(json!(3)), "тест")
        .await
        .unwrap();
    let q = p.quoting();
    let e = q.reg.get(&PoolRef::Contract(CAKE_USDT_PCS)).unwrap();
    let step = Step {
        entry: e,
        order: Order {
            token_in: R_USDT,
            token_out: CAKE,
            amount_in: U256::from(1_000u32),
            min_out: U256::from(1u8),
        },
        from_wallet: true,
        to_wallet: true,
    };
    let exec = address!("0x00000000000000000000000000000000000000ee");
    let c = call::build(&q.ctx, &[vec![step]], exec, NOW).unwrap();
    let sim_time = Base::timed(0, NOW).time.unwrap();
    // require(block.timestamp <= deadline): 1 000 001 ≤ 1 000 003 — проходит; с +12 с узла BSC (1 000 012) — нет.
    assert!(U256::from(sim_time) <= c.deadline);
    assert!(U256::from(NOW + 12) > c.deadline);
}
