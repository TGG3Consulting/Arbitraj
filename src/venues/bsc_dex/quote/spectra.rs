//! Подбор входа для покупки в Curve-пуле Spectra. Проверяется выплата той же функции
//! точного входа, которая оценивает продажу. Вход и выход измеряются в атомах IBT/PT.

use alloy_primitives::U256;

use super::super::math::{full_math, swap_cl::Swapped, swap_solidly};
use super::super::pools::registry::Entry;
use crate::core::types::PoolState;

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
    if reserve_in.is_zero() {
        return None;
    }
    let seed = full_math::mul_div_up(wanted, reserve_in, reserve_out)?;
    let cap = U256::MAX - reserve_in;
    let (paid, received) = match &e.pool.state {
        PoolState::Crypto(data) => {
            let price = |dx| super::super::math::crypto_swap::exchange_out(data, usize::from(i), usize::from(j), dx);
            let hint = super::spectra_points::previous(e, zero_for_one, wanted);
            let result = bounded_input(hint, cap, wanted, &price, |max| {
                crypto_input(seed, max, wanted, data, usize::from(i), price)
            })?;
            super::spectra_points::remember(e, zero_for_one, result.0, result.1);
            result
        },
        _ => first_input(seed, cap, wanted, payout)?,
    };
    (received <= reserve_out).then_some(Swapped {
        amount_in: paid,
        amount_out: received,
        complete: true,
    })
}

/// Ранее найденный вход даёт предел списания только после подтверждения выплаты на текущем состоянии.
fn bounded_input(
    hint: Option<U256>, cap: U256, wanted: U256, payout: impl Fn(U256) -> Option<U256>,
    solve: impl FnOnce(U256) -> Option<(U256,U256)>,
) -> Option<(U256,U256)> {
    let known = hint.filter(|x| !x.is_zero() && *x<=cap)
        .and_then(|x|payout(x).filter(|y|*y>=wanted).map(|y|(x,y)));
    let upper = known.map_or(cap, |v|v.0);
    solve(upper).filter(|(x,y)| *x<=upper && *y>=wanted).or(known)
}

/// В legacy Curve целочисленный `newton_y` может уменьшить выплату при большем входе.
/// Масштаб проб берём из первой нормировки K0_i = 2·10^18·xp[i]/D самого контракта.
/// Каждая принятая цена проверяется прямой функцией; шаг нормировки задаёт сетку проб.
fn crypto_input(
    seed: U256,
    cap: U256,
    wanted: U256,
    data: &crate::core::types::CryptoData,
    i: usize,
    payout: impl Fn(U256) -> Option<U256>,
) -> Option<(U256, U256)> {
    let (mut best, mut out) = first_input(seed, cap, wanted, &payout)?;
    if best == U256::ONE {
        return Some((best, out));
    }
    let scale = if i == 0 {
        U256::from(10u64.pow(18))
    } else {
        *data.price_scale.get(i - 1)?
    };
    let precision = *data.precisions.get(i)?;
    if precision.is_zero() || scale.is_zero() {
        return None;
    }
    let quantum = precision
        .checked_mul(scale)
        .and_then(|v| v.checked_mul(U256::from(2)))
        .map_or(U256::ONE, |den| data.d.div_ceil(den).max(U256::ONE));
    let start = best;
    let window = quantum.saturating_mul(U256::from(4)).min(start - U256::ONE);
    // Предельные объёмы глубины обычно заданы долей резерва, а заявки — целыми
    // разрядами входной валюты. Такие входы проверяем наравне с промежуточными.
    // Дробь восстанавливаем из текущих балансов. Знаменатели и размеры ордеров не задаются.
    let reserve = *data.balances.get(i)?;
    let landmark = reserve_landmark(start, reserve, window);
    for point in [landmark, decimal_landmark(start, window)].into_iter().flatten() {
        for x in [point, point.saturating_sub(U256::ONE)] {
            if !x.is_zero() && x < best && let Some(y) = payout(x).filter(|v| *v >= wanted) {
                (best, out) = (x, y);
            }
        }
    }
    let mut previous = U256::ZERO;
    for k in 1..=40u64 {
        // Входы на единицах ERC-20 и равномерно по окрестности первой нормировки.
        let delta = if k <= 8 {
            U256::from(k)
        } else {
            full_math::mul_div_up(window, U256::from(k - 8), U256::from(32))?
        };
        if delta <= previous || delta >= start {
            continue;
        }
        previous = delta;
        let x = start - delta;
        if x < best && let Some(y) = payout(x).filter(|y| *y >= wanted) {
            (best, out) = (x, y);
        }
    }
    // Если у найденной точки работает и предшествующий атом, сужаем цену за счёт известных выплат.
    if best < start && best > U256::ONE && payout(best - U256::ONE).is_some_and(|y| y >= wanted) {
        let mut step = U256::ONE;
        let hi = best;
        loop {
            let x = hi.saturating_sub(step).max(U256::ONE);
            if let Some(y) = payout(x).filter(|y| *y >= wanted) {
                (best, out) = (x, y);
                if x == U256::ONE {
                    break;
                }
                step = step.saturating_mul(U256::from(2));
            } else {
                let mut low = x;
                while best - low > U256::ONE {
                    let mid = low + (best - low) / U256::from(2);
                    if let Some(y) = payout(mid).filter(|y| *y >= wanted) {
                        (best, out) = (mid, y);
                    } else {
                        low = mid;
                    }
                }
                break;
            }
        }
    }
    Some((best, out))
}

/// Самый крупный десятичный разряд, округление вниз к которому попадает в исследуемый диапазон.
fn decimal_landmark(point: U256, width: U256) -> Option<U256> {
    let low = point.saturating_sub(width).max(U256::ONE);
    let mut unit = U256::ONE;
    loop {
        let next = unit.checked_mul(U256::from(10))?;
        if point - point % next < low { return Some(point - point % unit); }
        unit = next;
    }
}

/// Первая подходящая дробь цепного разложения входа относительно текущего резерва.
/// После перевода дроби в атомы её исполнимость всегда перепроверяет сам пул.
fn reserve_landmark(point: U256, reserve: U256, width: U256) -> Option<U256> {
    let (mut n, mut d) = (point, reserve);
    let (mut prev_n, mut cur_n) = (U256::ZERO, U256::ONE);
    let (mut prev_d, mut cur_d) = (U256::ONE, U256::ZERO);
    while !d.is_zero() {
        let (q, r) = n.div_rem(d);
        let num = q.checked_mul(cur_n)?.checked_add(prev_n)?;
        let den = q.checked_mul(cur_d)?.checked_add(prev_d)?;
        if !num.is_zero() {
            let x = full_math::mul_div(reserve, num, den)?;
            if x.abs_diff(point) <= width { return Some(x); }
        }
        (n, d) = (d, r);
        (prev_n, cur_n) = (cur_n, num);
        (prev_d, cur_d) = (cur_d, den);
    }
    None
}

/// Ищет границу только по прямым котировкам. Слишком маленький и чрезмерный вход могут оба дать откат.
fn first_input(
    seed: U256,
    cap: U256,
    wanted: U256,
    payout: impl Fn(U256) -> Option<U256>,
) -> Option<(U256, U256)> {
    if cap.is_zero() || wanted.is_zero() {
        return None;
    }
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
    // Секущая даёт предполагаемый вход, но цену подтверждает только прямой расчёт.
    // Из каждой пары сохраняем самую дешёвую уже проверенную выплату.
    let mut known = (out >= wanted).then_some((paid,out));
    let mut proposal = full_math::mul_div_up(wanted, paid, out);
    for _ in 0..5 {
        let Some(x) = proposal.map(|x| x.clamp(U256::ONE, cap)).filter(|x| *x != paid) else { break };
        let Some(y) = payout(x).filter(|v| !v.is_zero()) else { break };
        if y>=wanted && known.is_none_or(|(old,_)| x<old) { known=Some((x,y)); }
        if y==wanted { break; }
        if (x>paid) != (y>out) || y==out { break; }
        let offset = full_math::mul_div_up(wanted.abs_diff(y), x.abs_diff(paid), y.abs_diff(out));
        proposal=offset.map(|v| if y<wanted {x.saturating_add(v)} else {x.saturating_sub(v)});
        (paid,out)=(x,y);
    }
    if let Some((x,y)) = known { (paid,out)=(x,y); }
    if out >= wanted && (paid == U256::ONE || payout(paid - U256::ONE).is_none_or(|v| v < wanted)) {
        return Some((paid, out));
    }
    // Если удвоение перескочило последний допустимый вход, сужаем диапазон к известной выполнимой точке.
    let mut end = cap;
    while out < wanted {
        if paid >= end {
            return None;
        }
        let mut next = paid.saturating_mul(U256::from(2)).min(end);
        if let Some(y) = payout(next) {
            (paid, out) = (next, y);
        } else {
            end = next - U256::ONE;
            if paid >= end {
                return None;
            }
            next = paid + (end - paid).div_ceil(U256::from(2));
            match payout(next) {
                Some(y) => (paid, out) = (next, y),
                None => end = next - U256::ONE,
            }
        }
    }
    // Близкую границу ищем от оплаченного входа. Если цена далека, обычное деление пополам остаётся ограниченным.
    let mut span = U256::ONE;
    let top = paid;
    for _ in 0..8 {
        let below = top.saturating_sub(span);
        match (!below.is_zero()).then(||payout(below)).flatten() {
            Some(y) if y >= wanted => { (paid,out)=(below,y); span=span.saturating_mul(U256::from(2)); },
            _ => {
                let mut low=below;
                while paid-low>U256::ONE {
                    let mid=low+(paid-low)/U256::from(2);
                    if let Some(y) = payout(mid).filter(|y|*y>=wanted) { (paid,out)=(mid,y); }
                    else { low=mid; }
                }
                return Some((paid,out));
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
#[path = "spectra_tests.rs"]
mod tests;

#[cfg(test)]
fn original_input(
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
mod comparisons {
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
    fn control_all_paid_inputs() {
        use crate::venues::bsc_dex::{math::crypto_swap, pools::load_crypto_v1};
        use alloy_primitives::Bytes;

        let raw = include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/spectra_base_math.json.gz");
        let archive: Value = serde_json::from_reader(flate2::read::GzDecoder::new(raw.as_slice())).unwrap();
        let num = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
        let extra: Value = serde_json::from_reader(flate2::read::GzDecoder::new(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/spectra_base_inverse_intermediate.json.gz").as_slice())).unwrap();
        let blind: Value = serde_json::from_reader(flate2::read::GzDecoder::new(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/spectra_base_inverse_blind.json.gz").as_slice())).unwrap();
        let mut differences = Vec::new();
        let mut latency: [Vec<u128>;2]=[vec![],vec![]]; let mut original_misses=[0;2];
        let mut peak = [0; 2];
        let mut checked = [0; 2];
        for m in archive["markets"].as_array().unwrap() {
            let mut all = m["quotes"].as_array().unwrap().clone();
            for extra in extra["points"].as_array().unwrap().iter().chain(blind["points"].as_array().unwrap()).filter(|x|x[0]==m["pool"]) {
                all.push(serde_json::json!({"i":extra[1],"dx":extra[2],"dy":extra[3]}));
            }
            let quotes: Vec<_> = all.iter()
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
                let start=std::time::Instant::now();
                let old=original_input(seed,U256::MAX-balances[i],target,payout).unwrap();
                latency[0].push(start.elapsed().as_nanos());
                original_misses[family]+=usize::from(old.0>num(&q["dx"]));
                n.set(0);
                let start=std::time::Instant::now();
                let (paid, net) = if let Some(data) = &crypto {
                    crypto_input(seed, U256::MAX - balances[i], target, data, i, payout)
                } else { first_input(seed, U256::MAX - balances[i], target, payout) }.unwrap();
                latency[1].push(start.elapsed().as_nanos()); let reference=num(&q["dx"]);
                if paid>reference || net<target {differences.push(serde_json::json!({"pool":m["pool"],"reference":q["dx"],"target":q["dy"],"paid":paid.to_string()}));}
                peak[family] = peak[family].max(n.get());
                checked[family] += 1;
            }
        }
        eprintln!("Spectra: максимум прямых расчётов на архивную цель у legacy {}, у NG {}",peak[0],peak[1]);
        assert_eq!(checked, [230,124]);
        for values in &mut latency { values.sort(); } let elapsed=latency.map(|v|[v[v.len()/2],v[v.len()*95/100]]); std::fs::write(std::env::var("SPECTRA_REPORT").unwrap(),serde_json::to_vec(&serde_json::json!({"counts":checked,"peak":peak,"mismatches":differences,"old_misses":original_misses,"debug_elapsed_ns":elapsed})).unwrap()).unwrap();
    }
}
