//! Файл: tests/bsc_dex_settle_records_357.rs
//! Что делает: решение 357 на НАСТОЯЩИХ цепочках логов одного пула из квитанций блоков (tests/data/settle_2026-11-17,
//! записал серверный ИИ со своего узла 16.11.2026; README там же) — шаги «пула в обновлении» (pools/settle.rs,
//! settle_bins.rs) по логам блока: сколько логов держат пул и после какого он уходит ядру.
//! - Algebra Integral thena-v3 0x8829…9515, блок 134001089, одна tx: Fee(uint16) (logIndex 0x115), затем Swap с
//!   overrideFee / pluginFee (0x119). Fee → Refresh (events_algebra_integral.rs) — пул целиком на конце блока, Swap того
//!   же блока отбрасывается (has_applied): ядру уходит одно состояние. Чтение на N−1 и N ЗАПИСАНО (globalState, fee,
//!   liquidity) — состояние после «чтения на N» и состояние по одному логу Swap сверяются с ним; «в обновлении» эта пара
//!   не открывает (settle::opens — только SwapFee 1.2.10 и CompositionFees LB).
//! - Liquidity Book V2.1 BTC.b/WBNB 0x717d…073d, блок 133991544, одна tx: два Swap (бины 8399652 → 8399740, по
//!   возрастанию), других логов пула в блоке нет. Первый лог держит пул (Need::Active), второй — снимает: ядру одна
//!   отправка, чтений у узла два, оба на блоке события N: бин лога + активный бин (бин первого лога уже прочитан на
//!   конце N первым чтением — повторно не читается; копятся без чтения только бины логов, пришедших, пока пул ждёт).
//! - PancakeSwap Infinity Bin PoolId 0xa3aa…040f, блок 133943958: два Swap из РАЗНЫХ tx (0x1d, 0x1e), activeId обоих —
//!   8387666 (активный бин не сдвинулся): пул не держится, ядру после каждого лога.
//!
//! ЧЕГО В ЗАПИСИ НЕТ: состояния LB и Infinity Bin на N−1 и N (узел хранит ~128 блоков — «historical state … is not
//! available», README записи). Поэтому: (1) активный бин конца блока, который код берёт у узла (settle_read.rs), здесь =
//! бин ПОСЛЕДНЕГО Swap пула в блоке — по коду контрактов (LBPair.sol v2.1.0 L499-L550: активный бин меняет только обмен,
//! цикл кончается на бине с вводом и логом; Infinity Bin Swap несёт activeId после обмена) и по логам блока (после этого
//! Swap логов пула нет); (2) резервы бинов на N не записаны — в finish передаётся пустой список бинов (проверяется только
//! счёт шагов, а не резервы). Память LB до блока — настоящий пул той же пары, прочитанный загрузчиком на блоке
//! probe.json lb_v21_2026-11-15 (133716961); Infinity Bin — активный бин и комиссии из первого лога (лог сам ставит
//! активный бин — на исход шагов это не влияет).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/lb_node, arb::venues::bsc_dex::pools::{settle, settle_bins, settle_read,
//! events, load, load_lb, registry}.
//! Настройки теста: узел на записи (lb_node::attach — только для памяти LB), dex.bin_depth_usd = 3 [bsc_dex].

mod lb_node;
mod native_common;

use alloy::rpc::types::Log;
use alloy_primitives::{Address, B256, U256};
use serde_json::{Value, json};

use arb::core::types::{PoolRef, PoolState};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::pools::events::{self, Applied};
use arb::venues::bsc_dex::pools::load::{self, Loaded};
use arb::venues::bsc_dex::pools::registry::{Entry, Registry};
use arb::venues::bsc_dex::pools::settle::{self, Holds};
use arb::venues::bsc_dex::pools::settle_bins::{self, Plan, Settled};
use arb::venues::bsc_dex::pools::{load_lb, settle_read};
use arb::venues::bsc_dex::tokens::Tokens;
use lb_node::addr;

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/settle_2026-11-17/");

/// Файл записи.
fn rec(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{DIR}{name}")).unwrap()).unwrap()
}

/// Логи записи как логи подписки (тот же разбор, что у сессии: serde_json → alloy Log).
fn logs(v: &Value) -> Vec<Log> {
    v["logs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| serde_json::from_value(l.clone()).unwrap())
        .collect()
}

/// Слово `i` (32 байта) ответа eth_call.
fn word(raw: &str, i: usize) -> U256 {
    let h = raw.trim_start_matches("0x");
    U256::from_str_radix(&h[i * 64..(i + 1) * 64], 16).unwrap()
}

/// Состояние Algebra из записанного чтения на блоке: globalState (цена, тик), liquidity(), fee().
fn algebra_at(state: &Value, block: u64) -> PoolState {
    let s = &state[block.to_string()];
    let gs = s["globalState"].as_str().unwrap();
    // int24 в слове со знаковым расширением: младшие 32 бита — то же число в дополнительном коде.
    let tick = (word(gs, 1) & U256::from(u32::MAX)).to::<u32>() as i32;
    PoolState::Algebra {
        sqrt_price_x96: word(gs, 0),
        tick,
        liquidity: word(s["liquidity"].as_str().unwrap(), 0).to::<u128>(),
        fee: word(s["fee"].as_str().unwrap(), 0).to::<u32>(),
    }
}

/// Пара Fee + Swap Algebra Integral одной tx: ядру уходит одно состояние = записанному на конце блока.
#[test]
fn algebra_fee_then_swap_one_state_equals_block_end() {
    let v = rec("algebra_134001089.json");
    let n = v["block"].as_u64().unwrap();
    let all = logs(&v);
    assert_eq!(all.len(), 2, "в блоке у пула два лога");
    let fee_t0: B256 = "0x598b9f043c813aa6be3426ca60d1c65d17256312890be5118dab55b0775ebe2a"
        .parse()
        .unwrap();
    assert_eq!(all[0].topic0(), Some(&fee_t0), "первый — Fee(uint16)");
    assert_eq!(all[0].transaction_hash, all[1].transaction_hash, "одна tx");
    let mut e = native_common::entry("algebra_integral", None);
    let r = PoolRef::Contract(addr(&v["pool"]));
    e.pool.reference = r.clone();
    e.pool.state = algebra_at(&v["state"], n - 1);
    e.pool.block = n - 1;
    e.synced_block = n - 1;
    let before = e.clone();
    let (reg, holds) = (Registry::default(), Holds::default());
    reg.put(e);
    // Fee → перечитать пул целиком (на конце блока); «в обновлении» не открывается.
    let a = reg.update(&r, |x| events::apply(x, &all[0])).unwrap();
    assert!(
        matches!(a, Applied::Refresh(_)),
        "Fee — Refresh, а не {a:?}"
    );
    assert!(settle::opens(before.family, &all[0], &a).is_none());
    assert!(!holds.step(&reg, (&r, before.family), &all[0], &a, false));
    // Чтение пула на N (listen_log.rs ветка Refresh → load::renewed): записанные globalState / liquidity / fee на N.
    let at_n = algebra_at(&v["state"], n);
    let read = load::renewed(
        &reg.get(&r).unwrap(),
        n,
        Loaded {
            state: at_n.clone(),
            ticks: None,
            bins: None,
            fee_pips: None,
        },
    );
    reg.put(read);
    let e = reg.get(&r).unwrap();
    assert_eq!(e.pool.state, at_n, "ядру — состояние конца блока N");
    // Swap той же tx уже учтён чтением на N — отбрасывается (listen_log.rs has_applied), второй отправки нет.
    let (b, i) = (all[1].block_number.unwrap(), all[1].log_index.unwrap());
    assert!(e.has_applied(b, i));
    // Тот же Swap сам по себе (без Fee) даёт то же состояние конца блока: цена, тик, ликвидность, комиссия = N.
    let mut alone = before.clone();
    let a = events::apply(&mut alone, &all[1]);
    assert_eq!(a, Applied::Updated);
    assert!(settle::opens(before.family, &all[1], &a).is_none());
    assert_eq!(
        alone.pool.state, at_n,
        "Swap несёт состояние после обмена = чтению на N"
    );
    // Комиссия на N−1 и N (fee()) та же — 1204; lastFee globalState 1206 → 1204 = значение лога Fee.
    let lf = |b: u64| {
        word(
            v["state"][b.to_string()]["globalState"].as_str().unwrap(),
            2,
        )
    };
    assert_eq!(
        (lf(n - 1), lf(n)),
        (U256::from(1206u32), U256::from(1204u32))
    );
}

/// Обмен LB через два бина одной tx: первый лог держит пул, второй снимает — ядру одна отправка.
#[tokio::test]
async fn lb_two_bins_one_swap_held_then_once() {
    let v = rec("lb_133991544.json");
    let n = v["block"].as_u64().unwrap();
    let all = logs(&v);
    let ids: Vec<u32> = all
        .iter()
        .map(|l| {
            let d = l.inner.data.data.as_ref();
            U256::from_be_slice(&d[0..32]).to::<u32>()
        })
        .collect();
    assert_eq!(
        ids,
        vec![8_399_652, 8_399_740],
        "бины по порядку обмена, по возрастанию"
    );
    assert!(
        all.iter()
            .all(|l| l.transaction_hash == all[0].transaction_hash)
    );
    // Активный бин конца блока — бин последнего Swap (у узла не записан; см. шапку).
    let end = *ids.last().unwrap();

    // Память — настоящий пул этой пары, прочитанный загрузчиком на probe.json (lb_v21_2026-11-15).
    let c = native_common::ctx("settle-357-rec-lb").await;
    let url = lb_node::node(vec![lb_node::rec("probe.json")]).await;
    lb_node::attach(&c, &url).await;
    let s = Scope::Venue("bsc_dex".into());
    c.settings
        .set("dex.bin_depth_usd", s, Some(json!(3)), "тест")
        .await
        .unwrap();
    let probe = lb_node::rec("probe.json");
    let a: Address = v["pool"].as_str().unwrap().parse().unwrap();
    let p = &probe.pools[&a];
    let t = Tokens::default();
    let d = |k: &str| p[k].as_str().unwrap().parse::<u8>().unwrap();
    let (x, y) = (addr(&p["tokenX"]), addr(&p["tokenY"]));
    t.remember_decimals(x, d("decimalsX"));
    t.remember_decimals(y, d("decimalsY"));
    let pair = (x.min(y), x.max(y));
    let (state, bins) = load_lb::load(&c, &t, a, pair, y, probe.block)
        .await
        .unwrap();
    let mut e: Entry = native_common::entry("lb_v21", None);
    let r = PoolRef::Contract(a);
    e.pool.reference = r.clone();
    e.pool.state = state;
    e.bins = Some(bins);
    e.synced_block = probe.block;
    let family = e.family;
    let (reg, holds) = (Registry::default(), Holds::default());
    reg.put(e);

    let mut sent = 0;
    let mut reads = Vec::new();
    for log in &all {
        let applied = reg.update(&r, |x| events::apply(x, log)).unwrap();
        assert!(!holds.step(&reg, (&r, family), log, &applied, false));
        let Applied::Bins(b) = &applied else {
            panic!("Swap LB — Bins");
        };
        let res = match settle_bins::plan(&holds, &reg, &r, b, n) {
            Plan::Held => Settled::Held,
            Plan::Read(list) => {
                reads.push((list.clone(), settle_read::calls(&r, &list).unwrap().len()));
                // Резервов на N в записи нет — пустой список (см. шапку); активный бин конца блока = end.
                settle_bins::finish(&holds, &reg, &r, n, (Vec::new(), end))
            }
        };
        if res == Settled::Done {
            sent += 1;
        }
        let held = reg.get(&r).map(|e| !e.valid).unwrap();
        assert_eq!(held, res == Settled::Held);
        if res == Settled::Held {
            // Прочитанные бины из отметки сняты (уже на конце N), отметка — на блоке N, ждёт end.
            let h = holds.get(&r).unwrap();
            assert!(h.bins.is_empty());
            assert_eq!(h.block, n);
        }
    }
    // Лог 1: getBin 8399652 + getActiveId на N → «в обновлении» (Need::Active(8399740)); лог 2: getBin 8399740 +
    // getActiveId на N (бин 8399652 уже прочитан на конце N) → снято. Ядру — одна отправка, после последнего лога обмена.
    assert_eq!(reads, vec![(vec![8_399_652], 2), (vec![8_399_740], 2)]);
    assert_eq!(sent, 1);
    assert!(holds.get(&r).is_none());
    assert!(reg.get(&r).unwrap().valid);
}

/// Infinity Bin: два Swap одного пула из разных tx, активный бин не сдвинулся — пул не держится.
#[test]
fn infinity_bin_two_txs_same_active_not_held() {
    let v = rec("bin_133943958.json");
    let n = v["block"].as_u64().unwrap();
    let all = logs(&v);
    assert_eq!(all.len(), 2);
    assert_ne!(
        all[0].transaction_hash, all[1].transaction_hash,
        "разные tx"
    );
    // Swap(id, sender, amount0, amount1, activeId, fee, protocolFee): activeId — третье слово данных.
    let active = |l: &Log| U256::from_be_slice(&l.inner.data.data[64..96]).to::<u32>();
    let word = |l: &Log, i: usize| {
        U256::from_be_slice(&l.inner.data.data[i * 32..(i + 1) * 32]).to::<u32>()
    };
    assert_eq!((active(&all[0]), active(&all[1])), (8_387_666, 8_387_666));
    let end = active(&all[1]);
    let mut e = native_common::entry("pancakeswap_infinity_bin", None);
    let r = PoolRef::Singleton {
        manager: addr(&v["manager"]),
        id: v["poolId"].as_str().unwrap().parse().unwrap(),
    };
    e.pool.reference = r.clone();
    // Память до блока не записана: активный бин и комиссии — из первого лога (лог сам ставит активный бин).
    e.pool.state = PoolState::Bin {
        active_id: active(&all[0]),
        protocol_fee: word(&all[0], 4),
        lp_fee: word(&all[0], 3),
    };
    let family = e.family;
    let (reg, holds) = (Registry::default(), Holds::default());
    reg.put(e);
    let mut sent = 0;
    for log in &all {
        let applied = reg.update(&r, |x| events::apply(x, log)).unwrap();
        assert!(!holds.step(&reg, (&r, family), log, &applied, false));
        let Applied::Bins(b) = &applied else {
            panic!("Swap Infinity Bin — Bins");
        };
        let Plan::Read(list) = settle_bins::plan(&holds, &reg, &r, b, n) else {
            panic!("пул не держится — чтение");
        };
        assert_eq!(settle_read::calls(&r, &list).unwrap().len(), list.len() + 1);
        let res = settle_bins::finish(&holds, &reg, &r, n, (Vec::new(), end));
        assert_eq!(res, Settled::Done);
        sent += 1;
    }
    assert_eq!(
        sent, 2,
        "ядру после каждого лога — активный бин уже конечный"
    );
    assert!(holds.get(&r).is_none());
}
