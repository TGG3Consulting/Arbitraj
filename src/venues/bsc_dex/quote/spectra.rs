//! Подбор входа для покупки в Curve-пуле Spectra. Проверяется выплата той же функции
//! точного входа, которая оценивает продажу. Вход и выход измеряются в атомах IBT/PT.

use alloy_primitives::U256;

use crate::core::types::PoolState;
use super::super::math::{full_math, swap_cl::Swapped, swap_solidly};
use super::super::pools::registry::Entry;

pub(super) fn target(
    e: &Entry,
    zero_for_one: bool,
    wanted: U256,
    payout: impl Fn(U256) -> Option<U256>,
) -> Option<Swapped> {
    let (a, b, balances) = match &e.pool.state {
        PoolState::StableSwap(d) if !d.halted && d.ng.is_some() => (d.i0, d.i1, &d.balances),
        PoolState::Crypto(d) if !d.halted && !d.d.is_zero() => (d.i0, d.i1, &d.balances),
        _ => return None,
    };
    let (i, j) = if zero_for_one { (a, b) } else { (b, a) };
    let reserve_out = *balances.get(usize::from(j))?;
    if wanted.is_zero() || wanted >= reserve_out {
        return None;
    }
    // Отношение запасов задаёт только начальный размер поиска. Каждый последующий шаг — расчёт на том же состоянии.
    let reserve_in = *balances.get(usize::from(i))?;
    if reserve_in.is_zero() { return None; }
    let seed = full_math::mul_div_up(wanted, reserve_in, reserve_out)?;
    let cap = U256::MAX - reserve_in;
    let (paid, received) = first_input(seed, cap, wanted, payout)?;
    (received <= reserve_out).then_some(Swapped {
        amount_in: paid,
        amount_out: received,
        complete: true,
    })
}

/// Ищет границу только по прямым котировкам. Слишком маленький и чрезмерный вход могут оба дать откат.
fn first_input(
    seed: U256, cap: U256, wanted: U256, payout: impl Fn(U256) -> Option<U256>,
) -> Option<(U256, U256)> {
    if cap.is_zero() || wanted.is_zero() { return None; }
    let mut high = seed.clamp(U256::ONE, cap);
    let mut low = high;
    let mut sample = payout(high).filter(|x| !x.is_zero()).map(|out| (high, out));
    while sample.is_none() && (low > U256::ONE || high < cap) {
        if low > U256::ONE {
            low >>= 1;
            sample = payout(low).filter(|x| !x.is_zero()).map(|out| (low, out));
        }
        if sample.is_none() && high < cap {
            high = high.saturating_mul(U256::from(2)).min(cap);
            sample = payout(high).filter(|x| !x.is_zero()).map(|out| (high, out));
        }
    }
    let (mut paid, mut out) = sample?;
    if let Some(projected) = full_math::mul_div_up(wanted, paid, out) {
        let estimate = projected.clamp(U256::ONE, cap);
        if estimate != paid && let Some(y) = payout(estimate).filter(|y| !y.is_zero()) {
            (paid, out) = (estimate, y);
        }
    }
    if out >= wanted && (paid == U256::ONE || payout(paid - U256::ONE).is_none_or(|v| v < wanted)) {
        return Some((paid, out));
    }
    // Если удвоение перескочило последний допустимый вход, сужаем диапазон к известной выполнимой точке.
    let mut end = cap;
    while out < wanted {
        if paid >= end { return None; }
        let mut next = paid.saturating_mul(U256::from(2)).min(end);
        if let Some(y) = payout(next) {
            (paid, out) = (next, y);
        } else {
            end = next - U256::ONE;
            if paid >= end { return None; }
            next = paid + (end - paid).div_ceil(U256::from(2));
            match payout(next) {
                Some(y) => (paid, out) = (next, y),
                None => end = next - U256::ONE,
            }
        }
    }
    let minimum = swap_solidly::input_for(paid, wanted, &payout)?;
    let received = payout(minimum)?;
    (received >= wanted
        && (minimum == U256::ONE || payout(minimum - U256::ONE).is_none_or(|v| v < wanted)))
        .then_some((minimum, received))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venues::bsc_dex::math::stableswap_ng;
    use serde_json::Value;
    use std::cell::Cell;

    #[test]
    fn search_respects_both_ends_of_the_executable_interval() {
        let credit = |q| (q >= U256::from(7) && q <= U256::from(19)).then_some(q);
        for (seed, target) in [(1, 7), (128, 16), (9, 19)] {
            let wanted = U256::from(target);
            assert_eq!(first_input(U256::from(seed), U256::MAX, wanted, credit), Some((wanted, wanted)));
        }
        assert!(first_input(U256::from(9), U256::from(25), U256::from(20), credit).is_none());
        let rounded = |q| (q <= U256::from(19)).then_some(if q < U256::from(7) { U256::ZERO } else { q });
        assert_eq!(first_input(U256::ONE, U256::from(25), U256::from(17), rounded), Some((U256::from(17), U256::from(17))));
    }

    #[test]
    fn recorded_targets_bound_the_forward_calls_for_both_pool_families() {
        use crate::venues::bsc_dex::{math::crypto_swap, pools::load_crypto_v1};
        use alloy_primitives::Bytes;

        let raw = include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/spectra_base_math.json.gz");
        let archive: Value = serde_json::from_reader(flate2::read::GzDecoder::new(raw.as_slice())).unwrap();
        let num = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
        let mut peak = [0; 2];
        let mut checked = [0; 2];
        for m in archive["markets"].as_array().unwrap() {
            let quotes: Vec<_> = m["quotes"].as_array().unwrap().iter()
                .filter(|q| q["dy"].is_string() && !num(&q["dy"]).is_zero()).collect();
            if quotes.is_empty() { continue; }
            let c = &m["calls"];
            let legacy = m["kind"] == "legacy";
            let family = usize::from(!legacy);
            let crypto = legacy.then(|| {
                let pool = m["pool"].as_str().unwrap().parse().unwrap();
                let got = load_crypto_v1::calls_with(pool, 2, false).iter().map(|(_, call)| {
                    c[format!("{call}")]["result"].as_str().unwrap().parse::<Bytes>().map_err(|e| e.to_string())
                }).collect();
                let decimals = m["decimals"].as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as u8).collect();
                let input = load_crypto_v1::Raw { pool, n: 2, got, now: archive["timestamp"].as_u64().unwrap(), decimals, killed: false };
                load_crypto_v1::decode(&input).unwrap().1
            });
            let rates = (!legacy).then(|| c["stored_rates()"].as_array().unwrap().iter().map(num).collect::<Vec<_>>());
            let balances: Vec<_> = m["balances"].as_array().unwrap().iter().map(num).collect();
            for q in quotes {
                let target = num(&q["dy"]);
                let i = q["i"].as_u64().unwrap() as usize;
                let j = 1 - i;
                let n = Cell::new(0);
                let payout = |dx| {
                    n.set(n.get() + 1);
                    if let Some(data) = &crypto {
                        crypto_swap::exchange_out(data,i,j,dx)
                    } else {
                        stableswap_ng::exchange_received(i,j,dx,&balances,rates.as_ref()?,
                            num(&c["A_precise()"]),num(&c["fee()"]),num(&c["offpeg_fee_multiplier()"])).map(|o| o.0)
                    }
                };
                let seed = full_math::mul_div_up(target, balances[i], balances[j]).unwrap();
                let (paid, net) = first_input(seed, U256::MAX - balances[i], target, payout).unwrap();
                assert!(paid <= num(&q["dx"]) && net >= target, "{} {q}", m["pool"]);
                peak[family] = peak[family].max(n.get());
                checked[family] += 1;
            }
        }
        eprintln!("Spectra: максимум прямых расчётов на архивную цель у legacy {}, у NG {}",peak[0],peak[1]);
        assert_eq!(checked, [92, 124]);
        assert!(peak.into_iter().all(|n| (1..=128).contains(&n)), "{peak:?}");
    }
}
