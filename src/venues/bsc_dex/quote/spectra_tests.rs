//! Обратный подбор по архивным вызовам get_dy для обеих фабрик Spectra.
use super::*;
use crate::venues::bsc_dex::math::stableswap_ng;
use serde_json::Value;
use std::cell::Cell;

#[test]
fn earlier_paid_input_is_rechecked_on_the_current_state() {
    let wanted = U256::from(10);
    let spend = U256::from(12);
    let cap = U256::from(100);
    let hint = Some(spend);
    let measured = |x| (x == spend).then_some(wanted);
    assert_eq!(
        bounded_input(hint, cap, wanted, measured, |limit| {
            assert_eq!(limit, spend);
            None
        }),
        Some((spend, wanted))
    );
    assert!(
        bounded_input(
            hint,
            cap,
            wanted,
            |_| Some(wanted - U256::ONE),
            |limit| {
                assert_eq!(limit, cap);
                None
            }
        )
        .is_none()
    );
    assert!(
        bounded_input(
            hint,
            spend - U256::ONE,
            wanted,
            |_| panic!("outside cap"),
            |_| None
        )
        .is_none()
    );
}

#[test]
fn search_respects_both_ends_of_the_executable_interval() {
    let credit = |q| (q >= U256::from(7) && q <= U256::from(19)).then_some(q);
    for (seed, target) in [(1, 7), (128, 16), (9, 19)] {
        let wanted = U256::from(target);
        assert_eq!(
            first_input(U256::from(seed), U256::MAX, wanted, credit),
            Some((wanted, wanted))
        );
    }
    assert!(first_input(U256::from(9), U256::from(25), U256::from(20), credit).is_none());
    // Оценка по пропорции может недодать. Сохраняем уже проверенный доступный вход.
    let isolated = |q| {
        if q == U256::from(10) {
            Some(U256::from(12))
        } else if q == U256::from(9) {
            Some(U256::from(8))
        } else {
            None
        }
    };
    assert_eq!(
        first_input(U256::from(10), U256::from(20), U256::from(10), isolated),
        Some((U256::from(10), U256::from(12)))
    );
    let rounded =
        |q| (q <= U256::from(19)).then_some(if q < U256::from(7) { U256::ZERO } else { q });
    assert_eq!(
        first_input(U256::ONE, U256::from(25), U256::from(17), rounded),
        Some((U256::from(17), U256::from(17)))
    );
}

#[test]
fn recorded_targets_bound_the_forward_calls_for_both_pool_families() {
    use crate::venues::bsc_dex::{math::crypto_swap, pools::load_crypto_v1};
    use alloy_primitives::Bytes;

    let raw =
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/spectra_base_math.json.gz");
    let archive: Value =
        serde_json::from_reader(flate2::read::GzDecoder::new(raw.as_slice())).unwrap();
    let num = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let extra: Value = serde_json::from_reader(flate2::read::GzDecoder::new(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/spectra_base_inverse_intermediate.json.gz"
    ).as_slice())).unwrap();
    assert_eq!(extra["selector"], archive["selector"]);
    let blind: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/spectra_base_inverse_blind.json.gz"
        )
        .as_slice(),
    ))
    .unwrap();
    assert_eq!(blind["selector"], archive["selector"]);
    let mut peak = [0; 2];
    let mut checked = [0; 2];
    let mut cold_gaps = Vec::new();
    for m in archive["markets"].as_array().unwrap() {
        let mut points = m["quotes"].as_array().unwrap().clone();
        for row in extra["points"]
            .as_array()
            .unwrap()
            .iter()
            .chain(blind["points"].as_array().unwrap())
            .filter(|r| r[0] == m["pool"])
        {
            points.push(serde_json::json!({"i":row[1],"dx":row[2],"dy":row[3]}));
        }
        let quotes: Vec<_> = points
            .iter()
            .filter(|q| q["dy"].is_string() && !num(&q["dy"]).is_zero())
            .collect();
        if quotes.is_empty() {
            continue;
        }
        let c = &m["calls"];
        let legacy = m["kind"] == "legacy";
        let family = usize::from(!legacy);
        let crypto = legacy.then(|| {
            let pool = m["pool"].as_str().unwrap().parse().unwrap();
            let got = load_crypto_v1::calls_with(pool, 2, false)
                .iter()
                .map(|(_, call)| {
                    c[format!("{call}")]["result"]
                        .as_str()
                        .unwrap()
                        .parse::<Bytes>()
                        .map_err(|e| e.to_string())
                })
                .collect();
            let decimals = m["decimals"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect();
            let input = load_crypto_v1::Raw {
                pool,
                n: 2,
                got,
                now: archive["timestamp"].as_u64().unwrap(),
                decimals,
                killed: false,
            };
            load_crypto_v1::decode(&input).unwrap().1
        });
        let rates = (!legacy).then(|| {
            c["stored_rates()"]
                .as_array()
                .unwrap()
                .iter()
                .map(num)
                .collect::<Vec<_>>()
        });
        let balances: Vec<_> = m["balances"].as_array().unwrap().iter().map(num).collect();
        for q in quotes {
            let target = num(&q["dy"]);
            let i = q["i"].as_u64().unwrap() as usize;
            let j = 1 - i;
            let n = Cell::new(0);
            let payout = |dx| {
                n.set(n.get() + 1);
                if let Some(data) = &crypto {
                    crypto_swap::exchange_out(data, i, j, dx)
                } else {
                    stableswap_ng::exchange_received(
                        i,
                        j,
                        dx,
                        &balances,
                        rates.as_ref()?,
                        num(&c["A_precise()"]),
                        num(&c["fee()"]),
                        num(&c["offpeg_fee_multiplier()"]),
                    )
                    .map(|o| o.0)
                }
            };
            let seed = full_math::mul_div_up(target, balances[i], balances[j]).unwrap();
            let solve = |cap| match &crypto {
                Some(data) => crypto_input(seed, cap, target, data, i, payout),
                None => first_input(seed, cap, target, payout),
            };
            let (paid, net) = solve(U256::MAX - balances[i]).unwrap();
            let reference = num(&q["dx"]);
            assert!(net >= target, "{} {q}", m["pool"]);
            if paid > reference {
                // Отдельно учитываем холодную оценку без известного лимита. Разрешён
                // только ранее измеренный остаток на независимой точке; его уменьшение допустимо.
                cold_gaps.push((
                    m["pool"].as_str().unwrap().to_owned(),
                    i,
                    reference,
                    paid - reference,
                ));
            }
            peak[family] = peak[family].max(n.get());
            let known = num(&q["dx"]);
            let (from_known, credit) =
                bounded_input(Some(known), U256::MAX - balances[i], target, payout, solve).unwrap();
            assert!(from_known <= known && credit >= target, "{} {q}", m["pool"]);
            checked[family] += 1;
        }
    }
    eprintln!(
        "Spectra: максимум прямых расчётов на архивную цель у legacy {}, у NG {}",
        peak[0], peak[1]
    );
    assert_eq!(checked, [230, 124]);
    // 216 исходных и 92 промежуточных ответа обязаны пройти без превышения входа.
    // Из 46 независимых вызовов у одного известна разница 155 минимальных единиц.
    // Для всех 354 тот же тест выше проверяет точную оплату при известном пределе.
    assert!(cold_gaps.len() <= 1, "{cold_gaps:?}");
    for (pool, side, reference, excess) in cold_gaps {
        assert!(
            blind["points"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r[0].as_str() == Some(pool.as_str())
                    && r[1].as_u64() == Some(side as u64)
                    && num(&r[2]) == reference)
        );
        assert_eq!(pool, "0x5e3a444cbbabf92d619fa9fcaeef99c24ead3ba0");
        assert_eq!(reference, U256::from(151_022_579_325_132_638u64));
        assert!(excess <= U256::from(155), "{excess}");
    }
    assert!(peak.into_iter().all(|n| (1..=128).contains(&n)), "{peak:?}");
}
