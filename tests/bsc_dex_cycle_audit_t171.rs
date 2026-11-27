//! Файл: tests/bsc_dex_cycle_audit_t171.rs
//! Что проверяет: решение 385 — контроль отсева циклов DEX ↔ DEX (venues/bsc_dex/quote/cycle_audit.rs):
//! - отбор доли sieve.audit_share без случайности (hit): 0,1 — ровно каждый десятый, 0,3 — 30 из 100, доля не больше
//!   нуля — никогда, больше 1 — как 1; счёт точный, в целых (номер порядка 10^18 и доля с длинной мантиссой — без
//!   округления Decimal);
//! - сдвиг на проход (ревью t171, Н3): при неизменном порядке циклов выбранные позиции смещаются; доля 1/k, точно
//!   представимая десятичной дробью (k = 2, 4, 5, 10, 20), и k отсечённых за проход — за k проходов проверена каждая
//!   позиция, всего k проверок; доля 1/3 в Decimal (0,333…3, чуть меньше трети) — каждая позиция проверена за k + 1
//!   проходов, за 300 проходов по 3 отсечённых — 299 проверок (точно по формуле: первая точка отбора — номер 3, не 2);
//! - нарушение только при ρ < 1 И прибыли > 0 (violated): на РЕАЛЬНОМ цикле записи routes_2026-10-29 (USDT → CAKE → USDT
//!   двумя пулами V3; tests/route_common) отсев с контролем (Audit::gate) отдаёт ρ тех же ног, что cycle_rho (Leg несёт
//!   копию записи пула — тот же снимок); настоящий сигнал (выход < вход) — не нарушение; тот же сигнал с прибылью > 0
//!   (чистая логика: поля заменены) — нарушение при ρ < 1 и не нарушение при ρ ≥ 1;
//! - verify → flush: запись Error core.sieve_violation (вид «cycle») в журнале, и только после неё путь исключён из
//!   отсева (Audit::exempt, gate — полный счёт); повторные нарушения того же пути — та же одна запись (ревью t171, Н1, Н4).
//!
//! Кто вызывает: cargo test --test bsc_dex_cycle_audit_t171.
//! От чего зависит: tests/route_common, arb::venues::bsc_dex::quote::{bound_cycle, cycle, cycle_audit, route_paths,
//! select}, pools::graph, arb::settings, arb::storage::journal.

mod route_common;

use std::collections::HashMap;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use serde_json::json;

use arb::settings::scope::Scope;
use arb::venues::bsc_dex::pools::graph;
use arb::venues::bsc_dex::quote::bound_cycle::cycle_rho;
use arb::venues::bsc_dex::quote::cycle_audit::{Audit, Gate, flush, hit, key, report, violated};
use arb::venues::bsc_dex::quote::route_calc::Leg;
use arb::venues::bsc_dex::quote::select::Usable;
use arb::venues::bsc_dex::quote::{cycle, route_paths};

use route_common::{BTCB, CAKE, USDT, WBNB};

/// Выбор контроля на проходе со сдвигом `shift` среди `cuts` отсечённых.
fn pass(shift: u64, share: Decimal, cuts: u64) -> Vec<bool> {
    (0..cuts).map(|n| hit(shift + n, share)).collect()
}

#[test]
fn share_without_randomness() {
    let tenth = pass(0, Decimal::new(1, 1), 100);
    assert_eq!(tenth.iter().filter(|x| **x).count(), 10);
    assert!(
        tenth[9] && !tenth[..9].iter().any(|x| *x),
        "первый контроль — десятый"
    );
    let three = pass(0, Decimal::new(3, 1), 100);
    assert_eq!(three.iter().filter(|x| **x).count(), 30);
    for none in [Decimal::ZERO, Decimal::new(-1, 1)] {
        assert!(
            !pass(0, none, 50).iter().any(|x| *x),
            "доля ≤ 0 — контроля нет"
        );
    }
    assert!(pass(0, Decimal::TWO, 20).iter().all(|x| *x), "> 1 — как 1");
}

#[test]
fn exact_on_large_numbers() {
    // Доля 0,333…3 (28 знаков): 3·10^18 · доля = 10^18 − 3·10^−10 — пол 10^18 − 1, а Decimal округлил бы до 10^18.
    let third = Decimal::ONE / Decimal::from(3);
    let n = 3_000_000_000_000_000_000u64;
    assert!(!hit(n - 1, third), "⌊3·10^18·доля⌋ = ⌊(3·10^18 − 1)·доля⌋");
    assert!(hit(n, third), "⌊(3·10^18 + 1)·доля⌋ больше ⌊3·10^18·доля⌋");
    let tenth = Decimal::new(1, 1);
    assert!(hit(9_999_999_999_999_999_999, tenth));
    assert!(!hit(9_999_999_999_999_999_998, tenth));
}

/// Сколько раз проверена каждая позиция за `passes` проходов по `cuts` отсечённых (сдвиг — номер прохода) и всего.
fn rotation(share: Decimal, cuts: u64, passes: u64) -> (Vec<u64>, u64) {
    let mut seen = vec![0u64; cuts as usize];
    for p in 0..passes {
        for (i, h) in pass(p, share, cuts).into_iter().enumerate() {
            if h {
                seen[i] += 1;
            }
        }
    }
    let total = seen.iter().sum();
    (seen, total)
}

#[test]
fn shift_rotates_positions_between_passes() {
    // Доли 1/k, точно представимые десятичной дробью.
    for k in [2u64, 4, 5, 10, 20] {
        let share = Decimal::ONE / Decimal::from(k);
        assert_eq!(share * Decimal::from(k), Decimal::ONE, "1/{k} точна");
        let (seen, total) = rotation(share, k, k);
        assert!(
            seen.iter().all(|c| *c == 1),
            "доля 1/{k}: за {k} проходов каждая позиция проверена ровно раз: {seen:?}"
        );
        assert_eq!(total, k, "доля 1/{k}: всего проверок — {k} из {}", k * k);
    }
    // 1/3 в Decimal — 0,333…3 (меньше трети): 3·доля < 1, первая точка отбора — номер 3.
    let third = Decimal::ONE / Decimal::from(3);
    assert!(third * Decimal::from(3) < Decimal::ONE);
    let (seen, total) = rotation(third, 3, 3);
    assert_eq!(
        seen,
        [0, 1, 1],
        "за 3 прохода точка номер 3 — в позициях 2 и 1"
    );
    assert_eq!(total, 2);
    let (seen, _) = rotation(third, 3, 4);
    assert!(
        seen.iter().all(|c| *c > 0),
        "за 4 прохода — каждая позиция: {seen:?}"
    );
    let (_, total) = rotation(third, 3, 300);
    assert_eq!(
        total, 299,
        "300 проходов по 3: Σ (⌊(p+3)·доля⌋ − ⌊p·доля⌋) = 299"
    );
}

/// Нога по шагу цикла с явным допуском (без крючка и налога), decimals 18 — если проверки записи пул не допустили.
fn leg_of(step: &route_paths::Step) -> Leg {
    Leg {
        entry: step.edge.entry.clone(),
        usable: Usable {
            one_share_only:false,
            gas: 120_000,
            hook_pct: Decimal::ZERO,
            tax_pct: Decimal::ZERO,
            hook: Default::default(),
        },
        token_in: step.token_in,
        token_out: step.token_out,
        dec_in: 18,
        dec_out: 18,
    }
}

/// Записи core.sieve_violation в журнале окружения.
async fn violations(p: &arb::venues::bsc_dex::parts::Parts) -> Vec<serde_json::Value> {
    let all = p
        .ctx
        .journal
        .list(100, None)
        .await
        .unwrap_or_else(|e| panic!("журнал не читается: {e:?}"));
    all.into_iter()
        .filter(|e| e.kind == "core.sieve_violation")
        .map(|e| e.data.unwrap_or_default())
        .collect()
}

#[tokio::test]
async fn violation_only_below_one_with_profit_same_snapshot() {
    let p = route_common::parts_from("tests/data/routes_2026-10-29").await;
    route_common::executor(&p, "0x00000000000000000000000000000000000000ee").await;
    let g = graph::build(&p.ctx, &p.registry, &p.tokens, 18)
        .unwrap_or_else(|e| panic!("граф записи не строится: {e:?}"));
    let q = p.quoting();
    let dec: HashMap<Address, u8> = [USDT, CAKE, WBNB, BTCB]
        .into_iter()
        .map(|t| (t, 18))
        .collect();
    let cycles = route_paths::enumerate_cycles(&g, USDT, 2, 100);
    let Some(cyc) = cycles.first() else {
        panic!("в графе записи нет двухшагового цикла USDT → CAKE → USDT");
    };
    let legs = cycle::legs_for_cycle(&q, cyc, 18, &dec)
        .unwrap_or_else(|_| cyc.steps.iter().map(leg_of).collect());
    let rho = cycle_rho(&p.ctx, &legs)
        .unwrap_or_else(|e| panic!("у цикла двух пулов V3 нет строгого верха: {e}"));
    assert!(
        rho < Decimal::ONE,
        "два пула с комиссией на одном блоке записи: ρ < 1 (ρ = {rho})"
    );
    // Доля не задана — отсечённый не считается; доля 1 — контроль с ρ тех же ног.
    assert_eq!(Audit::start(&p.ctx).gate(&p.ctx, &legs), Gate::Cut);
    p.ctx
        .settings
        .set("sieve.audit_share", Scope::Global, Some(json!("1")), "тест")
        .await
        .unwrap_or_else(|e| panic!("доля контроля не задаётся: {e:?}"));
    let audit = Audit::start(&p.ctx);
    assert_eq!(
        audit.gate(&p.ctx, &legs),
        Gate::Calc(Some(rho)),
        "контроль сверяет с ρ тех же ног"
    );
    let path = cycle::eval(&p.ctx, &legs, Decimal::from(100))
        .unwrap_or_else(|e| panic!("цикл не считается на входе 100: {e:?}"));
    let gas = U256::from(1_000_000_000u64);
    let sig = cycle::build_signal(
        &p.ctx,
        USDT,
        "USDT",
        &path,
        &legs,
        gas,
        Decimal::ZERO,
        Some(Decimal::from(600)),
    )
    .unwrap_or_else(|e| panic!("сигнал цикла не собирается: {e:?}"));
    assert!(
        sig.gross_profit < Decimal::ZERO,
        "ρ < 1 ⇒ выход < вход: {}",
        sig.gross_profit
    );
    assert!(!violated(rho, &sig), "настоящий сигнал — не нарушение");
    audit.verify(rho, &sig, &legs);
    flush(&p.ctx, &p.stats).await;
    assert!(
        violations(&p).await.is_empty(),
        "нет нарушения — нет записи"
    );
    assert!(!Audit::start(&p.ctx).exempt(&legs));

    // Чистая логика: тот же сигнал с прибылью > 0.
    let mut won = sig.clone();
    won.output = won.input + Decimal::ONE;
    won.gross_profit = Decimal::ONE;
    won.net_profit = Some(Decimal::new(5, 1));
    assert!(violated(rho, &won));
    assert!(
        !violated(Decimal::ONE, &won),
        "ρ ≥ 1 — граница прибыль не исключала"
    );
    let mut gross_only = won.clone();
    gross_only.net_profit = None;
    assert!(
        violated(rho, &gross_only),
        "валовая > 0 при ρ < 1 — уже нарушение"
    );
    let mut zero = won.clone();
    zero.gross_profit = Decimal::ZERO;
    zero.net_profit = Some(Decimal::ZERO);
    assert!(!violated(rho, &zero), "ноль — не прибыль");

    let r = report(rho, &won, key(&legs));
    assert!(r.text.contains(&rho.to_string()) && r.text.contains("USDT"));
    assert_eq!(r.data["kind"], "cycle");

    // Нарушение: до записи путь не исключён; после flush — одна запись и исключение.
    audit.verify(rho, &won, &legs);
    audit.verify(rho, &won, &legs);
    assert!(
        !Audit::start(&p.ctx).exempt(&legs),
        "до записи в журнал путь не исключён"
    );
    flush(&p.ctx, &p.stats).await;
    let v = violations(&p).await;
    assert_eq!(v.len(), 1, "одна запись на путь");
    assert_eq!(v[0]["kind"], "cycle");
    let after = Audit::start(&p.ctx);
    assert!(
        after.exempt(&legs),
        "после записи путь дальше не отсекается"
    );
    assert_eq!(after.gate(&p.ctx, &legs), Gate::Calc(None));
    after.verify(rho, &won, &legs);
    flush(&p.ctx, &p.stats).await;
    assert_eq!(violations(&p).await.len(), 1, "повтор — без второй записи");
}
