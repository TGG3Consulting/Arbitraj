//! Поиск интервалов gross, в которых нет целого входа после удержания комиссии.
//! Вызывает mooniswap_inverse при подборе первого входа. Из формулы 1inch
//! Liquidity Protocol (Mooniswap.sol, 48d3db12) следуют две границы входа:
//! L=x*g/(y−g), U=D*x*(g−w)/(D*w−(D−s)*g). При положительном знаменателе
//! обе функции выпуклы. Касательная L снизу и секущая U сверху содержат все
//! возможные целые точки. Все произведения точны: размерность максимального
//! слагаемого ограничена 4 словами uint256 с учётом D=10^18.
//! https://github.com/1inch/liquidity-protocol/blob/48d3db12/contracts/Mooniswap.sol
use super::{U256, scale};
type W = alloy_primitives::Uint<1024, 16>;

/// Сумма floor((a*i+b)/m) при 0 <= i < n. После отделения целых частей
/// меняем строки и столбцы оставшегося треугольника (алгоритм Евклида).
fn floor_sum(mut n: W, mut m: W, mut a: W, mut b: W) -> W {
    let mut sum = W::ZERO;
    loop {
        let (integral, remainder) = a.div_rem(m);
        sum += (n * (n - W::ONE) / W::from(2)) * integral;
        a = remainder;
        let (integral, remainder) = b.div_rem(m);
        sum += n * integral;
        b = remainder;
        let (rows, remainder) = (a * n + b).div_rem(m);
        if rows.is_zero() {
            return sum;
        }
        n = rows;
        b = remainder;
        std::mem::swap(&mut a, &mut m);
    }
}

/// Даёт следующий gross после доказанно пустого префикса [start,last].
pub(super) fn skip(x: U256, y: U256, want: U256, fee: U256, start: U256, last: U256) -> U256 {
    let (x, y, w, g, d) = (
        W::from(x),
        W::from(y),
        W::from(want),
        W::from(start),
        W::from(scale()),
    );
    let keep = d - W::from(fee);
    let Some(b0) = (d * w).checked_sub(keep * g) else {
        return start;
    };
    if b0.is_zero() {
        return start;
    }
    let v = y - g;
    let m = v * v;
    let lower_offset = x * g * v + m - W::ONE; // Переносим ceil касательной в floor.
    let upper_offset = d * x * (g - w);
    let upper_step = d * x * W::from(fee) * w;
    let empty = |end: U256| {
        let r = W::from(end);
        let Some(br) = b0.checked_sub(keep * r) else {
            return false;
        };
        if br.is_zero() {
            return false;
        }
        let n = r + W::ONE;
        // U(g+i) <= U(g) + i*D*x*s*w/(b0*br). В пределах исходной огибающей
        // U>=L, поэтому каждый столбец даёт неотрицательное число целых точек.
        let upper = floor_sum(n, b0 * br, upper_step, upper_offset * br);
        let lower = floor_sum(n, m, x * y, lower_offset);
        upper + n == lower
    };
    let span = last - start;
    if span.is_zero() || !empty(U256::ONE) {
        return start;
    }
    let mut excluded = U256::ONE;
    loop {
        if excluded == span {
            return last + U256::ONE;
        }
        let end = excluded.saturating_add(excluded).min(span);
        if empty(end) {
            excluded = end;
            continue;
        }
        let mut possible = end;
        while possible - excluded > U256::ONE {
            let mid = excluded + (possible - excluded) / U256::from(2);
            if empty(mid) {
                excluded = mid;
            } else {
                possible = mid;
            }
        }
        return start + excluded + U256::ONE;
    }
}
