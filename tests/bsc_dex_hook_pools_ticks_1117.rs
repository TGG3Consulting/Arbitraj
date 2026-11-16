//! Файл: tests/bsc_dex_hook_pools_ticks_1117.rs
//! Что проверяет: досъёмка узла 17.11 (tests/data/hook_pools_ticks_2026-11-17, свой узел BSC):
//! 1. Полный обмен НАШИМ кодом (math/swap_cl.rs swap, версия V4, карта тиков из ответов узла: битмап разобран
//!    pools/load_cl.rs set_ticks, данные тиков — node/call.rs decode getPoolTickInfo, комиссия quote/pool_quote.rs
//!    swap_fee) против CLQuoter на 1, 20, 100 USDT в обе стороны для пулов Infinity CL GAIB, BTCMT, CAKE 0xc528 на
//!    блоке 134000292: совпадение до wei везде, кроме покупки GAIB и BTCMT на 20 и 100 USDT — там квотер = наш
//!    обмен до wei с подменой lpFee крючком (lpFeeOverride, зависит от суммы: GAIB 1492 / 7332, BTCMT 8057 / 36193 —
//!    найдено перебором по записи), и покупки CAKE на 20 и 100 USDT — цена уходит за слова, снятые серверным ИИ
//!    (±10 от слова тика), наш расчёт честно неполный (complete = false; бот читает шире — dex.tick_read_band_pct).
//!
//! Пакет Multicall3 против отдельных вызовов — tests/bsc_dex_hook_pools_batch_1117.rs.
//!
//! Кто вызывает: cargo test. Чтение записи и наш обмен — tests/hook_ticks_common.
//! От чего зависит: arb::venues::bsc_dex::{math::swap_cl, pools::{abi_v4, load_cl, ticks}, node::call,
//! quote::pool_quote}.

mod hook_ticks_common;

use arb::venues::bsc_dex::pools::load_cl::set_ticks;
use hook_ticks_common::{load, map, ours, pools, quotes};

/// Подмена lpFee крючком 0x32c59d…, при которой наш обмен = квотеру до wei (перебор по записи): (пул, сумма USDT, lpFee).
const OVERRIDE: [(&str, u64, u32); 4] = [
    ("0x5c1b4d20", 20, 1492),
    ("0x5c1b4d20", 100, 7332),
    ("0x62166b8a", 20, 8057),
    ("0x62166b8a", 100, 36193),
];

#[test]
fn full_swap_vs_quoter() {
    let all = load();
    let pools = pools(&all);
    assert_eq!(pools.len(), 3);
    let (mut exact, mut over, mut short) = (0, 0, 0);
    for q in quotes(&all, &pools) {
        let p = &pools[&q.id];
        let m = map(p);
        // Битмап записи разобран нашим кодом — те же тики, что прочитаны getPoolTickInfo.
        let words: Vec<_> = p.words.values().copied().collect();
        assert_eq!(
            set_ticks(m.words.0, &words, m.spacing),
            p.ticks.keys().copied().collect::<Vec<_>>()
        );
        let lp = OVERRIDE
            .iter()
            .find(|(pre, a, _)| q.id.starts_with(pre) && q.buy && *a == q.usd)
            .map_or(p.lp, |o| o.2);
        let r = ours(p, &m, q.z, true, q.amount, lp);
        // CAKE 0xc528, покупка 20 и 100 USDT: цена уходит выше слова 28 — дальше слов, снятых серверным ИИ (±10).
        if q.id.starts_with("0xc528f2c0") && q.buy && q.usd >= 20 {
            assert!(!r.complete, "цена ушла за прочитанные слова записи");
            short += 1;
            continue;
        }
        assert!(r.complete, "{} {}", q.id, q.amount);
        assert_eq!(r.amount_out, q.remote, "{} z={} {}", q.id, q.z, q.amount);
        if lp == p.lp { exact += 1 } else { over += 1 }
    }
    assert_eq!((exact, over, short), (12, 4, 2));
}
