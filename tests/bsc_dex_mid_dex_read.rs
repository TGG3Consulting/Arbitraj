//! Файл: tests/bsc_dex_mid_dex_read.rs
//! Что делает: решение 276 и 270 (дополнение 2) — список промежуточных (pools/mids.rs read) и занятые тикеры подписи
//! (pools/mid_dex.rs busy) на настоящей SQLite во временной папке; адреса — из записи с цепи
//! tests/data/fstswap_fist_2026-11-14/calls.json (FIST, чужая обёртка fstswap 0x0EFb…). Узла в тесте нет: symbol() не
//! читается — подпись адресом и Warn с текстом ошибки узла. Проверки: (1) флаг не задан и флаг = нет — прежний отказ
//! текст в текст; флаг = да — монета в списке с подписью адресом; (2) чужая обёртка при флаге = да: dex.native_wrapped
//! задана (WBNB) — обёртка fstswap получает подпись, а не тикер нативной монеты, WBNB — тикер нативной; не задана —
//! подпись; (3) busy: занятыми оказываются dex.native_ticker, venue.quote_asset и venue.price_assets биржи.
//! Деление WETH() роутеров при флаге (wrapped::natives) на записанном WETH() fstswap — tests/bsc_dex_wrapped.rs.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools, parts}, arb::settings, tests/mid_dex_common.
//! Значения настроек — значения теста, не предложение.

use alloy_primitives::Address;
use serde_json::{Value, json};

use arb::settings::scope::Scope;
use arb::storage::journal::Level;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::mid_dex;
use arb::venues::bsc_dex::pools::mids::{self, Mid};

mod mid_dex_common;
use mid_dex_common::ctx;
use mid_dex_common::rec::{addr, rec};

/// WBNB в BSC — адрес из официальной документации BNB Chain (как tests/core_intermediate.rs).
const WBNB: &str = "0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c";

async fn set(c: &Ctx, name: &str, scope: Scope, v: Value) {
    c.settings.set(name, scope, Some(v), "тест").await.unwrap();
}

fn dex() -> Scope {
    Scope::Venue("bsc_dex".into())
}

/// Окружение с включёнными промежуточными и списком адресов.
async fn parts(name: &str, list: &[Address]) -> Parts {
    let p = Parts::new(ctx(name, false).await);
    set(&p.ctx, "dex.intermediate_enabled", dex(), json!(true)).await;
    let l: Vec<String> = list.iter().map(|a| format!("{a:#x}")).collect();
    set(&p.ctx, "dex.intermediate_tokens", dex(), json!(l)).await;
    set(&p.ctx, "dex.intermediate_label_max_len", dex(), json!(16)).await;
    p
}

/// Есть запись журнала уровня Warn: symbol() не прочитан, подпись адресом.
async fn warned(c: &Ctx, a: Address) -> bool {
    let head = format!("Промежуточная {a}: биржа не знает адрес; symbol() не прочитан: ");
    let tail =
        format!("— подпись адресом {a:#x}, цена — пулом с USDT (dex.intermediate_dex_priced)");
    let log = c.journal.list(100, None).await.unwrap();
    log.iter().any(|e| {
        matches!(e.level, Level::Warn) && e.message.starts_with(&head) && e.message.ends_with(&tail)
    })
}

#[tokio::test]
async fn read_flag_unset_no_yes() {
    let fist = addr(&rec(), "FIST");
    let p = parts("mid-dex-read", &[fist]).await;
    let c = &p.ctx;
    let old = format!(
        "Промежуточная монета {fist}: биржа не знает этот адрес и это не обёртка нативной монеты (WETH() роутера V2, dex.native_wrapped) — затраты не пересчитать, не используется"
    );
    // Флаг не задан, затем флаг = нет — прежний отказ, текст в текст.
    for flag in [None, Some(false)] {
        if let Some(f) = flag {
            set(c, "dex.intermediate_dex_priced", dex(), json!(f)).await;
        }
        assert!(
            mids::read(c, &p.tokens).await.unwrap().is_empty(),
            "{flag:?}"
        );
        let log = c.journal.list(50, None).await.unwrap();
        assert!(log.iter().any(|e| e.message == old), "{flag:?} {log:?}");
        assert!(
            !warned(c, fist).await,
            "{flag:?}: symbol() не читается при флаге нет"
        );
    }
    // Флаг = да — монета используется; узла нет — symbol() не прочитан (Warn), подпись адресом.
    set(c, "dex.intermediate_dex_priced", dex(), json!(true)).await;
    let got = mids::read(c, &p.tokens).await.unwrap();
    let want = Mid {
        address: fist,
        ticker: format!("{fist:#x}"),
    };
    assert_eq!(got, vec![want]);
    assert!(warned(c, fist).await);
}

#[tokio::test]
async fn foreign_wrapper_with_flag() {
    let v = rec();
    let wrapper = addr(&v, "WRAPPER");
    let wbnb: Address = WBNB.parse().unwrap();
    // dex.native_wrapped задана: тикер нативной — только у неё, обёртка fstswap — подпись (адрес: узла нет).
    let p = parts("mid-dex-wrap-set", &[wrapper, wbnb]).await;
    let c = &p.ctx;
    set(c, "dex.intermediate_dex_priced", dex(), json!(true)).await;
    set(c, "dex.native_wrapped", dex(), json!(WBNB)).await;
    set(c, "dex.native_ticker", dex(), json!("BNB")).await;
    let got = mids::read(c, &p.tokens).await.unwrap();
    let w = Mid {
        address: wrapper,
        ticker: format!("{wrapper:#x}"),
    };
    let n = Mid {
        address: wbnb,
        ticker: "BNB".into(),
    };
    assert_eq!(got, vec![w.clone(), n]);
    assert!(warned(c, wrapper).await);
    // dex.native_wrapped не задана: нативной обёртки нет — обёртка fstswap с подписью, тикер нативной не получает.
    let p = parts("mid-dex-wrap-unset", &[wrapper]).await;
    let c = &p.ctx;
    set(c, "dex.intermediate_dex_priced", dex(), json!(true)).await;
    set(c, "dex.native_ticker", dex(), json!("BNB")).await;
    assert_eq!(mids::read(c, &p.tokens).await.unwrap(), vec![w]);
    assert!(warned(c, wrapper).await);
}

#[tokio::test]
async fn busy_has_settings() {
    let p = Parts::new(ctx("mid-dex-busy", false).await);
    let c = &p.ctx;
    // «WBNB» — значение теста: отличается от тикера газа «BNB», чтобы проверка видела именно dex.native_ticker.
    set(c, "dex.native_ticker", dex(), json!("WBNB")).await;
    set(
        c,
        "venue.price_assets",
        Scope::Venue("gate".into()),
        json!(["GT"]),
    )
    .await;
    let busy = mid_dex::busy(c, &p.tokens, &["0xabc".into()]);
    for t in ["BNB", "WBNB", "USDT", "GT", "0xabc"] {
        assert!(busy.iter().any(|b| b == t), "{t} не занят: {busy:?}");
    }
    // symbol() чужой обёртки fstswap «WBNB» (запись) совпадает с dex.native_ticker — подпись адресом.
    let v = rec();
    let wrapper = addr(&v, "WRAPPER");
    let s = mid_dex_common::rec::symbol(&v, "WRAPPER.symbol()_0x95d89b41");
    let (label, why) = mid_dex::pick(s, wrapper, &busy, Some(16));
    assert_eq!(label, format!("{wrapper:#x}"));
    assert_eq!(why, Some(mid_dex::Why::Taken("WBNB".into())));
}
