//! Файл: tests/remeasure_247.rs
//! Что проверяет: решение 247 (Тигран 10.11: согласовано) на числах живого случая PIG 09.11 (сделка
//! 20261109202626272PIG, mexc → bsc_dex): факты журнала бота — «код 3: execution reverted: minOut; доля 1: minOut
//! 101.590829 USDT, ожидалось 102.101336 USDT, модельный налог монеты 4.0003%», допуск exec.dex_slippage_pct 0.5 %
//! (101.590829 = 102.101336 × 0.995); куплено 7453353189 PIG, зачислено 7065220091; ручная продажа — 101.1307 USDT;
//! в сети перевод кошелёк → пул удержал ровно 5.0 % (квитанции ручных продаж через свой узел).
//! 1) распознавание отката по minOut (код и текст узла — как в журнале бота; иные require — не minOut);
//! 2) новый minOut по замеру и граница по налогу в сети (наименьшее; PIG: граница по сети 101.038… ≤ 101.1307);
//! 3) граница пере-замера ядра — ровно порог убытка (loss_pct_if на границе = порог), кнопка — 0, «Ожидание» по
//!    истечении — без пере-замера;
//! 4) налог, наблюдённый в сети по переводу сделки (комиссия вывода не сообщена — вся недостача; сообщена — сверх неё);
//! 5) подписи «Прибыль/Убыток при продаже сейчас» по знаку, пометка «по модели», заголовок по причине;
//! 6) стоимость транзакции для проверки BNB на газ (п. З).
//!
//! Не покрыто (записанного ответа узла на симуляцию отката minOut в tests/data нет): сама симуляция пере-замера на
//! узле — CallResult здесь собран из текста журнала бота, а не из записанного сырого ответа eth_simulateV1.

use std::str::FromStr;

use alloy_primitives::U256;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::deal::Deal;
use arb::core::remeasure::{Guard, floor, of, tax_seen_pct};
use arb::core::types::{Asset, Cost, CostKind, Direction, Transfer, TransferStatus, VenueId};
use arb::notify::human as hu;
use arb::venues::bsc_dex::check::simulate::CallResult;
use arb::venues::bsc_dex::exec::executor::remeasure::{
    bound_text, is_min_out, min_out_of, seen_out,
};
use arb::venues::bsc_dex::exec::gas_fund::need_wei;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Итог вызова eth_simulateV1 с ошибкой (execution-apis CallResult: status 0x0, error {code, message}).
fn reverted(code: i64, message: &str) -> CallResult {
    serde_json::from_value(json!({
        "status": "0x0", "returnData": "0x", "gasUsed": "0x32fd7",
        "error": {"code": code, "message": message}
    }))
    .unwrap()
}

#[test]
fn min_out_revert_is_recognized_exactly() {
    // Журнал бота PIG: «код 3: execution reverted: minOut».
    assert!(is_min_out(&reverted(3, "execution reverted: minOut")));
    // Иные require исполнителя (contracts/ArbExecutor.sol) — не minOut.
    assert!(!is_min_out(&reverted(3, "execution reverted: deadline")));
    assert!(!is_min_out(&reverted(3, "execution reverted: out")));
    assert!(!is_min_out(&reverted(3, "execution reverted: token call")));
    // Не откат (ошибка VM другого кода).
    assert!(!is_min_out(&reverted(-32015, "execution reverted: minOut")));
    let ok: CallResult =
        serde_json::from_value(json!({"status": "0x1", "returnData": "0x", "gasUsed": "0x1"}))
            .unwrap();
    assert!(!is_min_out(&ok));
}

#[test]
fn new_min_out_from_measure_and_network_tax() {
    // Модель: старый minOut = ожидаемое × (1 − 0.5 %) — ровно число журнала.
    let old = min_out_of(d("102.101336"), d("0.5"), 6).unwrap();
    assert_eq!(old, U256::from(101_590_829u64));
    // Налог в сети 5.0 % при модели 4.0003 %: нижняя граница выхода 102.101336 × 95 / 95.9997.
    let exp = [(d("102.101336"), d("4.0003"))];
    let w = seen_out(&exp, Some(d("5.0"))).unwrap();
    assert!(w > d("101.038") && w < d("101.039"), "{w}");
    assert!(
        w <= d("101.1307"),
        "граница по сети не выше реальной ручной продажи"
    );
    // Симуляция «показала» выход предпроверки 102.161649 (выше модели) — граница по сети, в тексте — «занижает налог».
    let (b, t) = bound_text(&exp, d("102.161649"), Some(d("5.0")));
    assert_eq!(b, w);
    assert!(t.contains("симуляция занижает налог"), "{t}");
    assert!(t.contains("модель 102.101336 USDT"), "{t}");
    // Замер ниже границы по сети (101.0 < 101.038…) — граница = замер, без пометки.
    let (b, t) = bound_text(&exp, d("101.0"), Some(d("5.0")));
    assert_eq!(b, d("101.0"));
    assert!(!t.contains("занижает"), "{t}");
    // Налог в сети не наблюдался — граница = замер.
    assert_eq!(bound_text(&exp, d("101.1307"), None).0, d("101.1307"));
    // Новый minOut по ручной продаже 101.1307 × 0.995 = 100.6250465 → 100.625046 (вниз, USDT BSC — 18 знаков,
    // здесь 6 — для наглядности округления вниз).
    assert_eq!(
        min_out_of(d("101.1307"), d("0.5"), 6).unwrap(),
        U256::from(100_625_046u64)
    );
    // Граница 0 — minOut 0 не отправляется.
    assert!(min_out_of(Decimal::ZERO, d("0.5"), 18).is_none());
}

fn pig() -> Deal {
    let dir = Direction {
        asset: Asset {
            ticker: "PIG".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("mexc".into()),
        sell_on: VenueId("bsc_dex".into()),
    };
    let mut x = Deal::new(
        "20261109202626272PIG".into(),
        dir,
        json!(null),
        Decimal::ZERO,
    );
    x.at = VenueId("bsc_dex".into());
    x.qty = d("7065220091");
    x
}

fn transfer(venue: &str, amount: &str, fee: Option<Cost>) -> Transfer {
    Transfer {
        venue: VenueId(venue.into()),
        asset: pig().direction.asset,
        network: "BSC".into(),
        amount: d(amount),
        status: TransferStatus::Credited,
        venue_ref: String::new(),
        tx_hash: None,
        fee,
        updated_at: Utc::now(),
    }
}

#[test]
fn floor_is_exactly_the_loss_threshold() {
    // Вложено 99.6 (порядок числа из постановки — «в плюс к вложенным ~99.6»), затраты вне цены у котировки 0.05.
    let mut x = pig();
    x.invested = d("99.6");
    let (value, gross) = (d("102.05"), d("102.10"));
    for max in ["5", "0", "-1"] {
        let f = floor(&x, value, gross, Guard::MaxLoss(d(max))).unwrap();
        // На границе: чистыми f − (gross − value), убыток ровно порог.
        assert_eq!(x.loss_pct_if(f - (gross - value)), d(max), "порог {max}");
    }
    assert_eq!(floor(&x, value, gross, Guard::Any), Some(Decimal::ZERO));
    assert_eq!(floor(&x, value, gross, Guard::Off), None);
    // Убыток при ручной продаже 101.1307: прибыль — порог «выгодно» (0) пропускает.
    let f0 = floor(&x, value, gross, Guard::MaxLoss(Decimal::ZERO)).unwrap();
    assert!(d("101.1307") >= f0, "{f0}");
}

#[test]
fn network_tax_from_deal_transfer() {
    let mut x = pig();
    // Комиссия вывода MEXC не сообщена — вся недостача: (7453353189 − 7065220091) / 7453353189 = 5.2074…%.
    x.transfers = vec![
        transfer("mexc", "7453353189", None),
        transfer("bsc_dex", "7065220091", None),
    ];
    let t = tax_seen_pct(&x).unwrap();
    assert!(t > d("5.2074") && t < d("5.2075"), "{t}");
    // Комиссия вывода в монете известна — недостача сверх неё (число комиссии здесь условное: в ответе MEXC по этой
    // сделке её нет в данных постановки).
    let fee = Cost {
        kind: CostKind::WithdrawFee,
        amount: d("15000000"),
        asset: "PIG".into(),
        in_price: false,
        step: None,
        paid_in: None,
    };
    x.transfers[0].fee = Some(fee);
    let t2 = tax_seen_pct(&x).unwrap();
    assert!(t2 < t && t2 > d("5.0"), "{t2}");
    // В запрос — вместе с границей.
    let r = of(&x, d("102.05"), d("102.10"), Guard::Any).unwrap();
    assert_eq!(r.floor, Decimal::ZERO);
    assert_eq!(r.tax_seen_pct, Some(t2));
    // Перевода нет — налога в сети нет.
    assert_eq!(tax_seen_pct(&pig()), None);
}

#[test]
fn sale_now_labels_by_sign() {
    // PIG: ядро −2.03 (плюс — убыток) — на деле прибыль.
    assert_eq!(
        hu::sale_now(d("-2.03")),
        ("Прибыль при продаже сейчас", "+2.03%".to_string())
    );
    assert_eq!(
        hu::sale_now(d("3.5")),
        ("Убыток при продаже сейчас", "3.50%".to_string())
    );
    let err = "проба исполнителя откатилась — код 3: execution reverted: minOut; доля 1: minOut 101.590829 USDT";
    let (k, v) = hu::sale_now_alert(Some(d("-2.03")), Some(err));
    assert_eq!(k, "Прибыль при продаже сейчас");
    assert!(v.contains("по модели — продажа по ней откатилась"), "{v}");
    assert!(hu::min_out_hint(Some(err)).is_some());
    assert!(hu::min_out_hint(Some("нет маршрута")).is_none());
    assert_eq!(
        hu::alert_head(Some(d("-2.03")), true),
        "Нужно ваше решение: продажа не прошла"
    );
    assert_eq!(
        hu::alert_head(Some(d("4")), false),
        "Нужно ваше решение: монета застряла с убытком"
    );
    assert_eq!(
        hu::alert_head(Some(d("-1")), false),
        "Нужно ваше решение: монета застряла"
    );
    // Порог: убыток — прежний текст; прибыль — по знаку.
    assert_eq!(
        hu::loss_vs_max(d("10"), d("20"), true),
        "убыток 10.00% не больше порога 20.00%"
    );
    assert_eq!(
        hu::loss_vs_max(d("-2.03"), d("5"), true),
        "прибыль +2.03% (порог убытка 5.00%)"
    );
}

#[test]
fn gas_need_includes_value_gas_and_tip() {
    // Газ ручной продажи PIG в сети — 208903 (квитанции); цена 0.05 gwei, чаевые 0.0001 BNB с переводом 21000 газа.
    let price = U256::from(50_000_000u64);
    let base = need_wei(U256::ZERO, price, 208_903, None);
    assert_eq!(base, U256::from(208_903u64 * 50_000_000));
    let tip = U256::from(100_000_000_000_000u64);
    let all = need_wei(U256::ZERO, price, 208_903, Some((tip, 21_000)));
    assert_eq!(all, base + tip + U256::from(21_000u64 * 50_000_000));
}
