//! Файл: tests/exit_ur_248.rs
//! Что проверяет: решение 248 (техлид 10.11 по поручению Тиграна, кейс SMARS 10.11, монета 0xc0366a10…, mexc →
//! bsc_dex) на фактах журнала бота и официальных исходниках Universal Router:
//! 1) селекторы ошибок «мало на выходе» = keccak256 сигнатуры из исходника (exec/ur_short.rs — ссылки на файлы и
//!    строки); V2TooLittleReceived() = 0x849eaf98 — ровно данные отката из журнала SMARS;
//! 2) распознавание отката по тексту узла из журнала («код 3: execution reverted: 0x849eaf98», returnData пуст — так
//!    записан CallResult в журнале) и по данным отката (ExecutionFailed(BalanceTooLow) — abi из IUniversalRouter);
//!    иные откаты (срок, minOut исполнителя, ExecutionFailed с другим сообщением) — не «мало на выходе»;
//! 3) ядро (core/exit_gap.rs): насколько замер ниже модели, налог по замеру (нижняя граница), сходится ли новый
//!    налог с замером — на числах сделки 1 SMARS (выход модели предпроверки 6.841205 USDT, модельный налог продажи
//!    7.669 %, продано руками за 6.69 USDT);
//! 4) событие MarketEvent::ExitGap проходит рыночное состояние ядра как Touched::ExitGap.
//!
//! Не покрыто (записанного сырого ответа eth_simulateV1 с этим откатом и замера R в tests/data нет): сама симуляция
//! пере-замера Universal Router на узле (exec/ur_remeasure.rs measure), сборка плана с минимумами 1 ед. и запись
//! настроек ядром (on_gap / on_check — нужен живой Core с хранилищем); откат Infinity/V4 через vault.lock — по данным
//! узла не проверено (селектор — по исходнику).

use std::str::FromStr;

use alloy::sol_types::SolError;
use alloy_primitives::{Bytes, U256, hex, keccak256};
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::exit_gap::{gap_pct, matches, seen_tax_pct};
use arb::core::market_state::MarketState;
use arb::core::touched::Touched;
use arb::core::types::{Asset, ExitGap, VenueId};
use arb::gateway::events::MarketEvent;
use arb::venues::bsc_dex::check::simulate::CallResult;
use arb::venues::bsc_dex::exec::ur_short::{
    BalanceTooLow, ExecutionFailed, StableTooLittleReceived, TooLittleReceived,
    V2TooLittleReceived, V3TooLittleReceived, V4TooLittleReceived, too_little,
};

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn sel(sig: &str) -> [u8; 4] {
    keccak256(sig.as_bytes())[..4].try_into().unwrap()
}

/// Итог вызова eth_simulateV1 с откатом (execution-apis CallResultFailure: status 0x0, returnData, error).
fn reverted(data: &str, message: &str) -> CallResult {
    serde_json::from_value(json!({
        "status": "0x0", "returnData": data, "gasUsed": "0x1",
        "error": {"code": 3, "message": message}
    }))
    .unwrap()
}

fn short(r: &CallResult) -> Option<&'static str> {
    let text = r
        .error
        .as_ref()
        .map(|e| e.message.as_str())
        .unwrap_or_default();
    too_little(&r.return_data, text)
}

#[test]
fn selectors_match_official_signatures() {
    assert_eq!(V2TooLittleReceived::SELECTOR, sel("V2TooLittleReceived()"));
    assert_eq!(hex::encode(V2TooLittleReceived::SELECTOR), "849eaf98");
    assert_eq!(V3TooLittleReceived::SELECTOR, sel("V3TooLittleReceived()"));
    assert_eq!(hex::encode(V3TooLittleReceived::SELECTOR), "39d35496");
    assert_eq!(
        StableTooLittleReceived::SELECTOR,
        sel("StableTooLittleReceived()")
    );
    assert_eq!(
        TooLittleReceived::SELECTOR,
        sel("TooLittleReceived(uint256,uint256)")
    );
    assert_eq!(
        V4TooLittleReceived::SELECTOR,
        sel("V4TooLittleReceived(uint256,uint256)")
    );
    assert_eq!(BalanceTooLow::SELECTOR, sel("BalanceTooLow()"));
    assert_eq!(hex::encode(ExecutionFailed::SELECTOR), "2c4029e9");
}

#[test]
fn smars_journal_text_is_v2_too_little() {
    // Журнал бота SMARS 10.11 05:03:10: «шаг 0 откатился — код 3: execution reverted: 0x849eaf98».
    let r = reverted("0x", "execution reverted: 0x849eaf98");
    assert_eq!(short(&r), Some("V2TooLittleReceived"));
    // Те же данные в returnData (CallResultFailure.returnData) — тоже.
    let r = reverted("0x849eaf98", "execution reverted");
    assert_eq!(short(&r), Some("V2TooLittleReceived"));
}

#[test]
fn balance_check_wrapped_in_execution_failed() {
    let low = Bytes::from(BalanceTooLow::SELECTOR.to_vec());
    let data = ExecutionFailed {
        commandIndex: U256::from(2u8),
        message: low,
    }
    .abi_encode();
    let r = reverted(&format!("0x{}", hex::encode(&data)), "execution reverted");
    assert_eq!(short(&r), Some("ExecutionFailed(BalanceTooLow)"));
    let r = reverted(
        "0x",
        &format!("execution reverted: 0x{}", hex::encode(&data)),
    );
    assert_eq!(short(&r), Some("ExecutionFailed(BalanceTooLow)"));
}

#[test]
fn other_reverts_are_not_too_little() {
    // TransactionDeadlinePassed() (IUniversalRouter) — срок, не выход.
    let r = reverted(
        "0x",
        &format!(
            "execution reverted: 0x{}",
            hex::encode(sel("TransactionDeadlinePassed()"))
        ),
    );
    assert_eq!(short(&r), None);
    // require исполнителя «minOut» — распознаёт exec/executor/remeasure.rs, не Universal Router.
    assert_eq!(short(&reverted("0x", "execution reverted: minOut")), None);
    // ExecutionFailed с другим сообщением (V2InvalidPath) — не проверка баланса.
    let other = Bytes::from(sel("V2InvalidPath()").to_vec());
    let data = ExecutionFailed {
        commandIndex: U256::ZERO,
        message: other,
    }
    .abi_encode();
    assert_eq!(
        short(&reverted(
            &format!("0x{}", hex::encode(&data)),
            "execution reverted"
        )),
        None
    );
    assert_eq!(short(&reverted("0x", "execution reverted")), None);
}

#[test]
fn core_gap_and_seen_tax_on_smars_deal_1() {
    // Сделка 1 SMARS: выход модели предпроверки 6.841205 USDT при налоге продажи 7.669 %, продано руками за 6.69.
    let (model, real, t) = (d("6.841205"), d("6.69"), d("7.669"));
    let gap = gap_pct(model, real).unwrap();
    assert_eq!(gap.round_dp(4), d("2.2102"));
    let seen = seen_tax_pct(model, real, t);
    assert_eq!(seen.round_dp(4), d("9.7097"));
    // Перепроверка дала тот же налог 7.669 % — не сходится при допуске 0.5 % (модель выше замера на 2.21 %).
    assert!(!matches(seen, t, d("0.5")));
    // Налог перепроверки равен налогу по замеру — сходится.
    assert!(matches(seen, seen, d("0.5")));
    // Замер не ниже модели — налог модели, разрыва нет.
    assert_eq!(seen_tax_pct(model, model, t), t);
    assert!(gap_pct(model, model).unwrap().is_zero());
    assert_eq!(gap_pct(Decimal::ZERO, real), None);
}

#[test]
fn exit_gap_event_reaches_core_state() {
    let g = ExitGap {
        venue: VenueId("bsc_dex".into()),
        asset: Asset {
            ticker: "SMARS".into(),
            bsc_contract: Some(
                "0xc0366a104b429f0806bfa98d0008daa9555b2bed"
                    .parse()
                    .unwrap(),
            ),
        },
        deal_id: "20261110143720525SMARS".into(),
        model_usdt: d("5.582813"),
        real_usdt: d("5.4"),
        model_tax_pct: d("7.6687"),
    };
    let ev = MarketEvent::ExitGap(g.clone());
    let back: MarketEvent = serde_json::from_str(&serde_json::to_string(&ev).unwrap()).unwrap();
    let mut st = MarketState::default();
    match st.apply(back) {
        Touched::ExitGap(x) => assert_eq!(x, g),
        other => panic!("ожидался Touched::ExitGap, получено {other:?}"),
    }
}
