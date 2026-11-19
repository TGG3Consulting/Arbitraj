//! Файл: tests/core_coin_account_416.rs
//! Что делает: решение 416 (требование Тиграна 19.11.2026) — сделки одной монеты на одной бирже через разные сети не
//! мешают друг другу и видят друг друга как соседей по ОДНОМУ остатку биржи; на DEX сеть разделяет полностью.
//! Монета — USDT: адреса BEP20 и ARBITRUM из записанного справочника сетей BingX
//! (tests/data/bingx_2026-10-24/config_getall.json, coin USDT, networkList: BEP20 0x55d3…7955, ARBITRUM 0xfd08…cbb9).
//! Проверяется: (1) без сети (шаг 1) — ключ счёта как раньше (CoinKey::of); (2) биржа не сообщила тикер монеты сети —
//! монеты разных сетей на её счёте разные (ничего не склеивается без факта); (3) биржа сообщила один тикер для обеих
//! сетей — один ключ счёта: один замок факта (credit_lock), одна очередь продажи (sell_turn), соседи по остатку
//! (remain_others: подтверждённое количество другой сети вычитается); (4) на DEX — разные ключи, разные замки и очереди,
//! соседей нет.
//! Ядро — t198_common (SQLite во временной папке, площадок в реестре нет; bingx, mexc — биржи, bsc_dex — DEX).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{coin_account, coin_key, credit_lock, sell_turn, remain_others, credit_fact, ledger}.

mod t198_common;

use alloy_primitives::{Address, address};
use arb::core::coin_account;
use arb::core::coin_key::CoinKey;
use arb::core::credit_fact::{Basis, confirm};
use arb::core::types::{Asset, ChainId, VenueId};
use arb::core::{credit_lock, sell_turn};
use t198_common::{credited, d, start};

/// USDT в сети BSC (BingX networkList, network BEP20).
const USDT_BSC: Address = address!("0x55d398326f99059ff775485246999027b3197955");
/// USDT в сети Arbitrum (BingX networkList, network ARBITRUM).
const USDT_ARB: Address = address!("0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9");

/// Монета USDT в сети `chain` с адресом `addr`.
fn usdt(chain: &str, addr: Address) -> Asset {
    Asset {
        ticker: "USDT".into(),
        chain: Some(ChainId(chain.into())),
        contract: Some(addr),
    }
}

#[tokio::test]
async fn exchange_coin_is_one_across_chains_dex_splits() {
    // Своя биржа теста (карта тикеров — общая на процесс; второй тест работает с mexc параллельно).
    let mexc = VenueId("bingx".into());
    let dex = VenueId("bsc_dex".into());
    let (bsc, arb) = (usdt("bsc", USDT_BSC), usdt("arbitrum", USDT_ARB));

    // (1) Шаг 1: сеть не указана — ключ счёта прежний.
    let old = Asset {
        ticker: "USDT".into(),
        chain: None,
        contract: Some(USDT_BSC),
    };
    assert_eq!(coin_account::key(&mexc, &old), CoinKey::of(&old));

    // (2) Биржа тикеры монет этих сетей не сообщила — разные монеты, замки независимы.
    assert_ne!(
        coin_account::key(&mexc, &bsc),
        coin_account::key(&mexc, &arb)
    );
    let g = credit_lock::fact(&mexc, &bsc).await;
    assert!(credit_lock::fact_try(&mexc, &arb).is_some());
    drop(g);

    // (3) Биржа сообщила один тикер «USDT» для обеих сетей — одна монета счёта.
    coin_account::remember(&mexc, &ChainId("bsc".into()), USDT_BSC, "USDT");
    coin_account::remember(&mexc, &ChainId("arbitrum".into()), USDT_ARB, "USDT");
    assert_eq!(
        coin_account::key(&mexc, &bsc),
        coin_account::key(&mexc, &arb)
    );
    assert_eq!(
        coin_account::key(&mexc, &bsc),
        CoinKey::Venue("USDT".into())
    );
    let g = credit_lock::fact(&mexc, &bsc).await;
    assert!(
        credit_lock::fact_try(&mexc, &arb).is_none(),
        "чтение факта одной сети ждёт продажи и чтения другой: остаток один"
    );
    drop(g);
    let t = sell_turn::take(&mexc, &arb).await;
    assert!(
        sell_turn::try_take(&mexc, &bsc).is_none(),
        "продажи одной монеты биржи из разных сетей — по одной"
    );
    drop(t);
    assert!(sell_turn::try_take(&mexc, &bsc).is_some());

    // (4) DEX: сеть разделяет — свои ключи, замки и очереди.
    assert_ne!(coin_account::key(&dex, &bsc), coin_account::key(&dex, &arb));
    assert_eq!(coin_account::key(&dex, &bsc), CoinKey::of(&bsc));
    let g = credit_lock::fact(&dex, &bsc).await;
    assert!(credit_lock::fact_try(&dex, &arb).is_some());
    drop(g);
    let t = sell_turn::take(&dex, &bsc).await;
    assert!(sell_turn::try_take(&dex, &arb).is_some());
    drop(t);
}

#[tokio::test]
async fn neighbours_on_one_exchange_rest_across_chains() {
    let core = start("coin-account-416").await;
    coin_account::remember(
        &VenueId("mexc".into()),
        &ChainId("bsc".into()),
        USDT_BSC,
        "USDT",
    );
    coin_account::remember(
        &VenueId("mexc".into()),
        &ChainId("arbitrum".into()),
        USDT_ARB,
        "USDT",
    );
    // Сделка через BSC ждёт доли на mexc; сделка через Arbitrum — подтверждено 40 на mexc.
    let mut a = credited("A416", "mexc", d("70"));
    a.direction.asset = usdt("bsc", USDT_BSC);
    arb::core::ledger::save(&core, &a).await;
    let mut c = credited("C416", "mexc", d("40"));
    c.direction.asset = usdt("arbitrum", USDT_ARB);
    confirm(&mut c, d("40"), Basis::Balance);
    arb::core::ledger::save(&core, &c).await;
    let o = arb::core::remain_others::on_account(&core, &a, &a.at);
    assert_eq!(
        o.qty,
        d("40"),
        "монеты сделки другой сети — соседи по остатку"
    );
    assert_eq!(o.deals, vec![("C416".to_string(), d("40"))]);

    // DEX: те же сделки на счёте DEX — разные монеты (сеть разделяет), соседей нет.
    let mut x = a.clone();
    x.id = "X416".into();
    x.at = VenueId("bsc_dex".into());
    arb::core::ledger::save(&core, &x).await;
    let mut y = c.clone();
    y.id = "Y416".into();
    y.at = VenueId("bsc_dex".into());
    arb::core::ledger::save(&core, &y).await;
    let o = arb::core::remain_others::on_account(&core, &x, &x.at);
    assert!(
        !o.deals.iter().any(|(id, _)| id == "Y416"),
        "на DEX сделка другой сети — не сосед: {:?}",
        o.deals
    );
}
