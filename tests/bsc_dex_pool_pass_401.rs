//! Файл: tests/bsc_dex_pool_pass_401.rs
//! Что делает: решение 401 (t186) — проба шага пула менеджера через исполнитель кошелька. Без сети, на записи узла
//! tests/data/executor_v3_live_2026-11-17 (eth_simulateV1 своего узла на H = 134171808; calls.json — итоги вызовов
//! execute, summary.json — квотер и остаток на исполнителе, сверенные с raw_rpc тестом tests/bsc_dex_executor_v3_live.rs):
//! 1. Разбор пробы (check/pass_judge.rs) на записанных ответах: «прошёл» — пулы без крючка и пул с крючком 0x5c9717dd…
//!    (покупка и продажа совпали с квотером, остаток 0); «отвергнут» — крючок 0x70201397… «Not allowed» (покупка),
//!    крючок 0xddc53a1a… «UA» (покупка у 4 пулов; у 0x25c3a0dd покупка прошла — продажа отвергнута); «не проверено» —
//!    квотер на H откатился (сверить не с чем) и пул Bin, где откатились и квотер, и исполнитель (пыль в бине).
//!    Тексты откатов из calls.json побайтно есть в raw_rpc.json.zst (ответ узла).
//! 2. WrappedError: селектор 0x90bfb865 = keccak256("WrappedError(address,bytes4,bytes,bytes)")[..4] — первые 4 байта
//!    отката крючка в записи; details — HookCallFailed() (0xa9e35b2f).
//! 3. Строгость: получено ≠ квотеру вне допуска — «отвергнут»; остаток на исполнителе не 0 — «отвергнут»; квотер 0 —
//!    «не проверено» (изменённые копии записанных итогов).
//! 4. Откат пробы сделки (pool_pass::on_sim): откат крючка — помечен только пул этого крючка; откат без крючка — только
//!    пулы, допущенные пробой (by_pass); успех — ничего.
//! 5. Правило допуска (pool_gate.rs через entry::fits): пул Infinity CL с крючком — не исполним, пока у исполнителя
//!    кошелька нет факта; «прошёл» — исполним (и сборка шага entry::params); «отвергнут» — не исполним и в принятом
//!    классе (без крючка); «не проверено» — как решение 400. Карточка пула (pass_view::details) — строки факта.
//!
//! Чего запись не проверяет (docs/TASKS.md, решение 401): саму сборку пробы через узел (check/pass_probe.rs) — запись
//! делалась на «виртуальном» исполнителе X с подменой кода, а проба 401 идёт настоящим адресом кошелька; откат по minOut.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{check::{pass_judge, simulate}, exec::executor::{call, entry, pass_view,
//! pool_pass}, pools::{families, key}}, tests/route_common, zstd, serde_json.

mod route_common;

use alloy_primitives::{Address, B256, U256, keccak256};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::types::PoolRef;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::check::pass_judge::{self, Outcome, Side};
use arb::venues::bsc_dex::check::simulate::CallResult;
use arb::venues::bsc_dex::exec::executor::call::ExecCall;
use arb::venues::bsc_dex::exec::executor::pool_pass::{self, Fact, Gated, State};
use arb::venues::bsc_dex::exec::executor::{entry, pass_view};
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::key::{InfinityPoolKeyData, PoolKey};

const DIR: &str = "tests/data/executor_v3_live_2026-11-17";

/// Текст файла записи. Ошибок нет (паника — файла нет).
fn text(name: &str) -> String {
    std::fs::read_to_string(format!("{}/{DIR}/{name}", env!("CARGO_MANIFEST_DIR"))).expect("запись")
}

/// JSON файла записи. Ошибок нет (паника).
fn json_of(name: &str) -> Value {
    serde_json::from_str(&text(name)).expect("JSON")
}

/// Десятичная строка → U256; null — None.
fn dec(v: &Value) -> Option<U256> {
    U256::from_str_radix(v.as_str()?, 10).ok()
}

/// Допуск dex.quoter_max_diff_pct записи (README: «dex.quoter_max_diff_pct (живое значение) = 0.11 (%)»).
fn max_pct() -> Decimal {
    let readme = text("README.md");
    let mark = "dex.quoter_max_diff_pct (живое значение) = ";
    let at = readme.find(mark).expect("строка допуска в README") + mark.len();
    let num: String = readme[at..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    num.parse().expect("число")
}

/// Итог вызова из calls.json в форме ответа узла eth_simulateV1 (status / gasUsed — hex, error — code и message).
fn call_result(c: &Value) -> CallResult {
    let status = if c["status"] == 1 { "0x1" } else { "0x0" };
    let mut v = json!({
        "status": status,
        "returnData": c["returnData"],
        "gasUsed": format!("{:#x}", c["gasUsed"].as_u64().expect("gasUsed")),
    });
    if let Some(err) = c["error"].as_object() {
        v["error"] = json!({ "code": err["code"], "message": err["message"] });
    }
    serde_json::from_value(v).expect("итог вызова")
}

/// Остаток на исполнителе после вызова (summary.json x_left, ключи usdt и coin).
fn left(row: &Value) -> [Option<U256>; 2] {
    [dec(&row["x_left"]["usdt"]), dec(&row["x_left"]["coin"])]
}

/// Итог пробы по пулу записи: (крючок, итог). Сторона (а) — квотер на H, (б) — квотер после (а) в той же симуляции.
fn outcomes() -> Vec<(String, Outcome)> {
    let summary = json_of("summary.json");
    let calls = json_of("calls.json");
    let rows: Vec<&Value> = summary
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["case"] != "нет пулов")
        .collect();
    let calls = calls.as_array().unwrap();
    assert_eq!(rows.len(), calls.len());
    let max = max_pct();
    let mut out = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        if r["case"] != "a" {
            continue;
        }
        let pool = r["pool_id"].as_str().unwrap().to_string();
        let hooks: Address = r["hooks"].as_str().unwrap().parse().unwrap();
        let a = call_result(&calls[i]);
        let buy = Side {
            name: "покупка",
            swap: &a,
            quoted: dec(&r["quoter_out"]),
            left: left(r),
        };
        let sell_at = rows
            .iter()
            .position(|x| x["case"] == "b" && x["pool_id"] == r["pool_id"]);
        let b = sell_at.map(|j| call_result(&calls[j]));
        let sell = sell_at.zip(b.as_ref()).map(|(j, b)| Side {
            name: "продажа",
            swap: b,
            quoted: dec(&rows[j]["quoter_out_after_a"]),
            left: left(rows[j]),
        });
        out.push((pool, pass_judge::judge(&buy, sell.as_ref(), hooks, max)));
    }
    out
}

/// Итог пула по началу PoolId.
fn of<'a>(all: &'a [(String, Outcome)], prefix: &str) -> &'a Outcome {
    &all.iter()
        .find(|(p, _)| p.starts_with(prefix))
        .unwrap_or_else(|| panic!("пул {prefix} в записи"))
        .1
}

#[test]
fn judge_on_recorded_node_answers() {
    let all = outcomes();
    for p in [
        "0x92e5d26c",
        "0xb12f0c5f",
        "0xc89b1f76",
        "0xa005f46e",
        "0xdfe5344e",
        "0xed4de4a3",
        "0xc6104625",
    ] {
        assert_eq!(of(&all, p), &Outcome::Passed, "{p}");
    }
    let rejected = |p: &str| match of(&all, p) {
        Outcome::Rejected(w) => w.clone(),
        o => panic!("{p}: ожидался «отвергнут», итог {o:?}"),
    };
    let not_allowed = rejected("0xcadf2018");
    assert!(
        not_allowed.starts_with("покупка: крючок 0x70201397845fbe8283ed503a242d075d21abe1cf")
            && not_allowed.ends_with(": Not allowed"),
        "{not_allowed}"
    );
    for p in ["0x5ef34cf9", "0xeeb1677d", "0xed367707", "0x5a3edbc1"] {
        let w = rejected(p);
        assert!(
            w.starts_with("покупка: крючок 0xddc53a1a65d96772076f983fea1b53971d7ca5db")
                && w.ends_with(": UA"),
            "{p}: {w}"
        );
    }
    let sell_ua = rejected("0x25c3a0dd");
    assert!(
        sell_ua.starts_with("продажа: крючок 0xddc53a1a65d96772076f983fea1b53971d7ca5db")
            && sell_ua.ends_with(": UA"),
        "покупка прошла — продажа отвергнута: {sell_ua}"
    );
    let unchecked = |p: &str| match of(&all, p) {
        Outcome::Unchecked(w) => w.clone(),
        o => panic!("{p}: ожидался «не проверено», итог {o:?}"),
    };
    for p in ["0x758264e8", "0xa1512dbb"] {
        let w = unchecked(p);
        assert!(w.contains("квотер DEX не ответил"), "{p}: {w}");
    }
    let bin = unchecked("0x9994aa29");
    assert!(
        bin.contains("откатились и квотер, и исполнитель"),
        "Bin: {bin}"
    );
    // Тексты откатов — ответ узла: каждое error.message calls.json есть в raw_rpc.json.zst.
    let p = format!("{}/{DIR}/raw_rpc.json.zst", env!("CARGO_MANIFEST_DIR"));
    let bytes = zstd::decode_all(std::fs::File::open(p).expect("запись")).expect("zstd");
    let raw = String::from_utf8(bytes).expect("utf-8");
    for c in json_of("calls.json").as_array().unwrap() {
        if let Some(m) = c["error"]["message"].as_str() {
            assert!(raw.contains(m), "откат calls.json не из ответа узла: {m}");
        }
    }
}

#[test]
fn wrapped_error_of_hook() {
    let wrapped = keccak256("WrappedError(address,bytes4,bytes,bytes)");
    let sel = &wrapped[..4];
    let calls = json_of("calls.json");
    let hooked: Vec<&Value> = calls
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| {
            c["status"] == 0
                && c["revert"]
                    .as_str()
                    .is_some_and(|r| r.starts_with("WrappedError"))
        })
        .collect();
    assert_eq!(hooked.len(), 6, "6 откатов крючков в записи");
    let failed = keccak256("HookCallFailed()");
    let hook_call_failed = &failed[..4];
    for c in hooked {
        let r = call_result(c);
        let data = pass_judge::revert_data(&r).expect("данные отката в тексте узла");
        assert_eq!(&data[..4], sel, "селектор WrappedError");
        let hex = alloy_primitives::hex::encode(hook_call_failed);
        assert!(
            alloy_primitives::hex::encode(&data).contains(&hex),
            "details — HookCallFailed()"
        );
        // Другой крючок — откат не «этим крючком».
        assert!(pass_judge::hook_revert(&r, Address::repeat_byte(0x11)).is_none());
        assert!(pass_judge::revert_text(&r).starts_with("WrappedError: контракт 0x"));
    }
}

#[test]
fn judge_is_strict_on_altered_answers() {
    let summary = json_of("summary.json");
    let calls = json_of("calls.json");
    let max = max_pct();
    // Пул без крючка 0x92e5… (а): получено = квотер.
    let i = summary
        .as_array()
        .unwrap()
        .iter()
        .position(|r| r["case"] == "a" && r["pool_id"].as_str().unwrap().starts_with("0x92e5d26c"))
        .unwrap();
    let row = &summary[i];
    let a = call_result(&calls[i]);
    let got = dec(&row["got"]).unwrap();
    let side = |quoted: Option<U256>, rest: [Option<U256>; 2]| Side {
        name: "покупка",
        swap: &a,
        quoted,
        left: rest,
    };
    let zero = [Some(U256::ZERO), Some(U256::ZERO)];
    assert_eq!(
        pass_judge::side(&side(Some(got), zero), Address::ZERO, max),
        Ok(got)
    );
    // Квотер на 0,2 % больше полученного — вне допуска 0,11 %.
    let q = got + got / U256::from(500u32);
    let far = pass_judge::side(&side(Some(q), zero), Address::ZERO, max);
    assert!(
        matches!(&far, Err(Outcome::Rejected(w)) if w.contains("≠ квотер")),
        "{far:?}"
    );
    // Остаток на исполнителе.
    let dust = [Some(U256::from(1u8)), Some(U256::ZERO)];
    let rest = pass_judge::side(&side(Some(got), dust), Address::ZERO, max);
    assert!(
        matches!(&rest, Err(Outcome::Rejected(w)) if w.contains("остались")),
        "{rest:?}"
    );
    // Остаток не прочитан / квотер 0 — «не проверено».
    let unread = pass_judge::side(
        &side(Some(got), [None, Some(U256::ZERO)]),
        Address::ZERO,
        max,
    );
    assert!(matches!(unread, Err(Outcome::Unchecked(_))));
    let nil = pass_judge::side(&side(Some(U256::ZERO), zero), Address::ZERO, max);
    assert!(matches!(nil, Err(Outcome::Unchecked(_))));
    // Продажа не пробовалась — пул не «прошёл».
    assert!(matches!(
        pass_judge::judge(&side(Some(got), zero), None, Address::ZERO, max),
        Outcome::Unchecked(_)
    ));
}

/// Вызов исполнителя с пулами шагов менеджера (шаги не нужны: on_sim смотрит только gated и итог).
fn exec_call(executor: Address, gated: Vec<Gated>) -> ExecCall {
    ExecCall {
        executor,
        token_in: Address::repeat_byte(1),
        amount_in: U256::from(1u8),
        token_out: Address::repeat_byte(2),
        min_out: U256::from(1u8),
        deadline: U256::from(1u8),
        steps: Vec::new(),
        native: None,
        gated,
    }
}

/// Итог вызова записи по началу PoolId и case.
fn recorded(prefix: &str, case: &str) -> CallResult {
    let calls = json_of("calls.json");
    let c = calls
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["case"] == case && c["pool_id"].as_str().unwrap().starts_with(prefix))
        .expect("вызов записи")
        .clone();
    call_result(&c)
}

#[test]
fn deal_revert_marks_pools() {
    let ddc5: Address = "0xddc53a1a65d96772076f983fea1b53971d7ca5db"
        .parse()
        .unwrap();
    let other: Address = "0x5c9717dd0000000000000000000000000000000a"
        .parse()
        .unwrap();
    let pool = |b: u8| PoolRef::Singleton {
        manager: Address::repeat_byte(0x4e),
        id: B256::repeat_byte(b),
    };
    let gated = vec![
        Gated {
            pool: pool(1),
            hooks: ddc5,
            by_pass: true,
        },
        Gated {
            pool: pool(2),
            hooks: other,
            by_pass: true,
        },
        Gated {
            pool: pool(3),
            hooks: Address::ZERO,
            by_pass: false,
        },
    ];
    // Откат крючка 0xddc5… (продажа 0x25c3 записи) — помечен только пул этого крючка.
    let x1 = Address::repeat_byte(0xa1);
    let marked = pool_pass::on_sim(&exec_call(x1, gated.clone()), &recorded("0x25c3a0dd", "b"));
    assert_eq!(marked, vec![pool(1)]);
    let f = pool_pass::get(x1, &pool(1)).expect("факт");
    assert!(
        matches!(&f.state, State::Rejected { deal: true, why } if why.contains("UA")),
        "{f:?}"
    );
    assert!(pool_pass::get(x1, &pool(3)).is_none());
    // Откат без крючка (Bin 0x96aa65ad) — только пулы, допущенные пробой.
    let x2 = Address::repeat_byte(0xa2);
    let marked = pool_pass::on_sim(&exec_call(x2, gated.clone()), &recorded("0x9994aa29", "a"));
    assert_eq!(marked, vec![pool(1), pool(2)]);
    // Успех — ничего.
    let x3 = Address::repeat_byte(0xa3);
    let marked = pool_pass::on_sim(&exec_call(x3, gated), &recorded("0x92e5d26c", "a"));
    assert!(marked.is_empty() && pool_pass::get(x3, &pool(1)).is_none());
}

#[tokio::test]
async fn gate_follows_pool_fact() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let base = q
        .reg
        .get(&PoolRef::Contract(route_common::WBNB_USDT_V2))
        .expect("пул записи");
    let (usdt, wbnb) = (route_common::USDT, route_common::WBNB);
    let manager = Address::repeat_byte(0x4e);
    let cl = families::make("x-401-cl", "pancakeswap_infinity_cl").unwrap();
    let key = |hooks: Address| {
        PoolKey::Infinity(InfinityPoolKeyData {
            currency0: usdt,
            currency1: wbnb,
            hooks,
            manager,
            fee: 2500,
            parameters: B256::from(U256::from(10u32 << 16)),
        })
    };
    let entry_of = |k: PoolKey| {
        let mut e = base.clone();
        e.family = cl;
        e.pool.reference = PoolRef::Singleton {
            manager,
            id: k.id(),
        };
        e.key = Some(k);
        e
    };
    let hooked = entry_of(key("0xddc53a1a65d96772076f983fea1b53971d7ca5db"
        .parse()
        .unwrap()));
    let plain = entry_of(key(Address::ZERO));
    p.ctx
        .settings
        .set(
            "dex.vault",
            Scope::Dex("x-401-cl".into()),
            Some(json!(format!("{:#x}", Address::repeat_byte(0x5a)))),
            "тест",
        )
        .await
        .unwrap();
    // Без исполнителя у кошелька: класс с крючком записью не принят (решение 400), класс без крючка — принят.
    assert!(entry::fits(&q.ctx, &hooked).is_err());
    assert!(entry::fits(&q.ctx, &plain).is_ok());
    // Кошелёк строки «Пробы» с исполнителем v3.
    let x = "0x00000000000000000000000000000000000401a1";
    route_common::executor(&p, x).await;
    let a = p.ctx.settings.accounts();
    let w = a.wallets().into_iter().find(|w| w.name == "тест").unwrap();
    a.set_executor_version(&w.id, x, Some("v3".into()), None, "тест")
        .await
        .unwrap();
    let xa: Address = x.parse().unwrap();
    let why = entry::fits(&q.ctx, &hooked).expect_err("факта нет");
    assert!(why.contains("не проверено"), "{why}");
    assert!(
        pass_view::details(&q.ctx, &hooked)
            .iter()
            .any(|l| l.contains("записью не принят"))
    );
    // Прошёл обе стороны — исполним, шаг строится (котировка и сделка — одно правило).
    let fact = |state: State| Fact {
        state,
        at: chrono::Utc::now(),
        block: Some(134171808),
    };
    pool_pass::put(xa, &hooked.pool.reference, fact(State::Passed));
    assert!(entry::fits(&q.ctx, &hooked).is_ok());
    assert!(entry::params(&q.ctx, &hooked, usdt).is_ok());
    assert!(
        pass_view::details(&q.ctx, &hooked)
            .iter()
            .any(|l| l.contains("проходит обе стороны"))
    );
    // Отвергнут — не исполним; и пул принятого класса тоже.
    let no = State::Rejected {
        why: "покупка: крючок отверг вызов".into(),
        deal: false,
    };
    pool_pass::put(xa, &hooked.pool.reference, fact(no.clone()));
    let rejected = entry::fits(&q.ctx, &hooked).expect_err("отвергнут");
    assert!(rejected.contains("отвергнут"), "{rejected}");
    assert!(entry::params(&q.ctx, &hooked, usdt).is_err());
    pool_pass::put(xa, &plain.pool.reference, fact(no));
    assert!(entry::fits(&q.ctx, &plain).is_err());
    // Не проверено — как решение 400.
    let unchecked = State::Unchecked("квотер не ответил".into());
    pool_pass::put(xa, &plain.pool.reference, fact(unchecked.clone()));
    assert!(entry::fits(&q.ctx, &plain).is_ok());
    pool_pass::put(xa, &hooked.pool.reference, fact(unchecked));
    assert!(entry::fits(&q.ctx, &hooked).is_err());
}
