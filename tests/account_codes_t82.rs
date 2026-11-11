//! Файл: tests/account_codes_t82.rs
//! Что проверяет: решение 253, дополнение t82 (Тигран 10.11) — отказы вывода разнесены по официальным таблицам ошибок
//! бирж на «адрес / белый список» (InvalidAddress), «аккаунт» (новый код шлюза AccountRestricted) и «монета / сеть»
//! (как раньше — WithdrawRejected, закрытие входа по монете):
//! (1) разбор адаптеров: код и текст — из официальной таблицы (ссылки — в venues/<биржа>/errors.rs у кода), Bitget
//!     40938 — живые ответы tests/data/bitget_40938_2026-11-07; у не-вывода — прежний смысл;
//! (2) у каждого кода «аккаунт / адрес» есть человеческая причина (notify/reason_codes_account.rs);
//! (3) ядро ведёт AccountRestricted как InvalidAddress: отказ вывода, без повтора по правилу и без паузы, не ошибка
//!     подряд, вывод не создан; монета НЕ закрывается — журнал «проблема в аккаунте» (core/withdraw_block.rs).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::*::errors, arb::notify::reason, arb::core (withdraw_block, withdraw_refused, risk_count,
//! auto_pause, op_error), arb::gateway::rules. Моков нет.

mod usdt_return_common;

use arb::core::op_error::OpError;
use arb::core::types::{Asset, Direction, VenueId};
use arb::gateway::errors::{ErrorCode as C, GatewayError};
use arb::venues;
use usdt_return_common::{dir, entries, start};

/// Проверка одного разбора: ожидаемый код и человеческая причина (известный код — без «ответа биржи как есть»).
fn check(e: &GatewayError, want: C, human: &str) {
    assert_eq!(e.code, want, "{}", e.message);
    let r = arb::notify::reason::human(&e.message);
    assert!(
        r.answer.is_none(),
        "код не в словаре: {} → {r:?}",
        e.message
    );
    assert!(r.text.contains(human), "{} → {r:?}", e.message);
}

#[test]
fn binance_wallet_codes() {
    use venues::binance::errors::{Op, from_code};
    let acc = [
        (-4004, "You don't login or auth.", "не вошёл"),
        (-4006, "Support main account only.", "основного аккаунта"),
        (-4013, "2FA is not opened.", "2FA"),
        (
            -4014,
            "Withdraw is not allowed within 2 min login.",
            "2 минуты",
        ),
        (-4015, "Withdraw is limited.", "ограничен"),
        (
            -4016,
            "Within 24 hours after password modification, withdrawal is prohibited.",
            "пароля",
        ),
        (
            -4017,
            "Within 24 hours after the release of 2FA, withdrawal is prohibited.",
            "2FA",
        ),
        (-4039, "The user does not exist.", "пользователя"),
        (-4046, "Agreement not confirmed.", "соглашение"),
    ];
    for (code, msg, h) in acc {
        let raw = serde_json::json!({"code": code, "msg": msg}).to_string();
        check(
            &from_code(code, msg, Op::Withdraw, &raw),
            C::AccountRestricted,
            h,
        );
        // Не вывод — прежний смысл (не «аккаунт»).
        assert_ne!(
            from_code(code, msg, Op::Other, &raw).code,
            C::AccountRestricted
        );
    }
    let e = from_code(
        -4008,
        "Address tag validation is not passed.",
        Op::Withdraw,
        "{}",
    );
    check(&e, C::InvalidAddress, "тег");
    // Монета — как раньше: −4019 «Current asset is not open for withdrawal», −4018 «We don't have this asset».
    for (code, msg) in [
        (-4019, "Current asset is not open for withdrawal."),
        (-4018, "We don't have this asset."),
    ] {
        assert_eq!(
            from_code(code, msg, Op::Withdraw, "{}").code,
            C::WithdrawRejected
        );
    }
}

#[test]
fn mexc_bingx_kucoin_codes() {
    use venues::{bingx, kucoin, mexc};
    let w = mexc::errors::Op::Withdraw;
    let msg =
        "This withdrawal address is not on the commonly used address list or has been invalidated";
    check(
        &mexc::errors::from_code(10212, msg, w, "{}"),
        C::InvalidAddress,
        "списке разрешённых",
    );
    let msg = "Withdrawal is currently locked by withdrawal protection.";
    check(
        &mexc::errors::from_code(157005, msg, w, "{}"),
        C::AccountRestricted,
        "защитой",
    );
    let msg = "Due to risk control, withdrawal is unavailable, please try again later";
    check(
        &mexc::errors::from_code(10265, msg, w, "{}"),
        C::AccountRestricted,
        "риск-контролем",
    );
    let msg = "risk control system detected abnormal";
    check(
        &mexc::errors::from_code(10098, msg, w, "{}"),
        C::AccountRestricted,
        "риск-контролем",
    );
    check(
        &mexc::errors::from_code(730100, "Unusual user status", w, "{}"),
        C::AccountRestricted,
        "необычным",
    );
    let msg = "currency does not exist";
    assert_eq!(
        mexc::errors::from_code(10232, msg, w, "{}").code,
        C::WithdrawRejected
    );

    let w = bingx::errors::Op::Withdraw;
    let msg = "The current account is not the main account.";
    check(
        &bingx::errors::from_code(100403, msg, w, "{}"),
        C::AccountRestricted,
        "субаккаунта",
    );
    let msg = "Account is abnormal or advanced identity verification is required.";
    check(
        &bingx::errors::from_code(100441, msg, w, "{}"),
        C::AccountRestricted,
        "KYC",
    );
    let t = bingx::errors::Op::Trade;
    assert_eq!(
        bingx::errors::from_code(100441, msg, t, "{}").code,
        C::OrderRejected
    );

    let w = kucoin::errors::Op::Withdraw;
    let msg = "Withdrawal limit exceeded. The withdrawal request hit the limit.";
    // Дополнение t85: суточный лимит, сбросится сам — WithdrawLimit (tests/withdraw_limit_t85.rs).
    check(
        &kucoin::errors::from_code("110003", msg, w, "{}"),
        C::WithdrawLimit,
        "суточный лимит",
    );
    // 260210 — монета или аккаунт по одному коду: как раньше.
    let msg =
        "withdraw.disabled -- Currency/Chain withdraw is closed, or user is frozen to withdraw";
    assert_eq!(
        kucoin::errors::from_code("260210", msg, w, "{}").code,
        C::WithdrawRejected
    );
}

#[test]
fn bitget_40938_recorded_is_address() {
    use venues::bitget::errors::{Op, from_code};
    let text =
        std::fs::read_to_string("tests/data/bitget_40938_2026-11-07/withdraw_rejected.jsonl")
            .unwrap();
    for raw in text.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();
        let (code, msg) = (v["code"].as_str().unwrap(), v["msg"].as_str().unwrap());
        check(
            &from_code(400, code, msg, Op::Withdraw, raw),
            C::InvalidAddress,
            "адресной книги",
        );
    }
    let msg =
        "Withdrawal whitelist is enabled; your account can only withdraw to address-book addresses";
    check(
        &from_code(400, "11000", msg, Op::Withdraw, "{}"),
        C::InvalidAddress,
        "адресной книги",
    );
}

#[test]
fn bybit_okx_codes() {
    use venues::{bybit, okx};
    let w = bybit::errors::Op::Withdraw;
    let acc = [
        (131065, "Your KYC information is incomplete", "KYC"),
        (131084, "Withdraw failed because of Uta Upgrading", "UTA"),
        (
            131086,
            "Withdrawal amount exceeds risk limit",
            "риск-контролем",
        ),
        (
            131087,
            "your current account spot risk level is too high, withdrawal is prohibited",
            "риск",
        ),
        (
            131088,
            "The withdrawal amount exceeds the remaining withdrawal limit of your identity verification level",
            "верификации",
        ),
        (
            131089,
            "User sensitive operation, withdrawal is prohibited within 24 hours",
            "24 часа",
        ),
        (131094, "UserId is not in the whitelist", "белом списке"),
    ];
    for (code, msg, h) in acc {
        check(
            &bybit::errors::from_code(code, msg, w, "{}"),
            C::AccountRestricted,
            h,
        );
    }
    let msg = "Withdrawal of this currency has been closed";
    assert_eq!(
        bybit::errors::from_code(131097, msg, w, "{}").code,
        C::WithdrawRejected
    );

    let w = okx::errors::Op::Withdraw;
    let addr = [
        ("58203", "Please add a withdrawal address.", "не добавлен"),
        (
            "58207",
            "Withdrawal address isn't on the verified address list.",
            "проверенных",
        ),
        ("58222", "Invalid withdrawal address", "недопустимым"),
    ];
    for (code, msg, h) in addr {
        check(
            &okx::errors::from_code(code, msg, w, "{}"),
            C::InvalidAddress,
            h,
        );
    }
    let acc = [
        (
            "58204",
            "Withdrawal suspended due to your account activity triggering risk control.",
            "риск",
        ),
        (
            "58219",
            "You cannot withdraw crypto within 24 hours after changing your mobile number, email address, or Google Authenticator.",
            "24 часа",
        ),
        (
            "58230",
            "According to the OKX compliance policy, you will need to complete your identity verification (Level 1) in order to withdraw",
            "KYC",
        ),
    ];
    for (code, msg, h) in acc {
        check(
            &okx::errors::from_code(code, msg, w, "{}"),
            C::AccountRestricted,
            h,
        );
    }
    let msg = "Withdrawal from {0} to {1} is currently not supported for this currency.";
    assert_eq!(
        okx::errors::from_code("58200", msg, w, "{}").code,
        C::WithdrawRejected
    );
    // Не вывод — как раньше.
    let o = okx::errors::Op::Other;
    assert_eq!(
        okx::errors::from_code("58201", "x", o, "{}").code,
        C::WithdrawRejected
    );
}

#[test]
fn gate_xt_poloniex_codes() {
    use venues::{gate, poloniex, xt};
    // Статус ответа в таблице меток Gate не указан; на разбор метки (кроме 429 / 5xx) он не влияет.
    let w = gate::errors::Op::Withdraw;
    let e = gate::errors::from_label(400, "ACCOUNT_EXCEPTION", "Abnormal account", w, "{}");
    check(&e, C::AccountRestricted, "необычным");
    let e = gate::errors::from_label(
        400,
        "WITHDRAWAL_OVER_LIMIT",
        "Withdrawal limit exceeded",
        w,
        "{}",
    );
    assert_eq!(
        e.code,
        C::WithdrawRejected,
        "чей лимит — вопрос, как раньше"
    );

    let w = xt::errors::Op::Withdraw;
    check(
        &xt::errors::from_code("WITHDRAW_004", "", w, "{}"),
        C::InvalidAddress,
        "адресную книгу",
    );
    check(
        &xt::errors::from_code("WITHDRAW_002", "", w, "{}"),
        C::InvalidAddress,
        "недопустимым",
    );
    check(
        &xt::errors::from_code("WITHDRAW_014", "", w, "{}"),
        C::InvalidAddress,
        "внутренние",
    );
    check(
        &xt::errors::from_code("WITHDRAW_003", "", w, "{}"),
        C::AccountRestricted,
        "безопасности",
    );
    // Дополнение t85: дневные лимиты XT — WithdrawLimit (tests/withdraw_limit_t85.rs).
    for mc in [
        "WITHDRAW_012",
        "WITHDRAW_013",
        "WITHDRAW_020",
        "WITHDRAW_021",
    ] {
        check(
            &xt::errors::from_code(mc, "", w, "{}"),
            C::WithdrawLimit,
            "суточный лимит",
        );
    }
    assert_eq!(
        xt::errors::from_code("WITHDRAW_008", "", w, "{}").code,
        C::WithdrawRejected
    );

    let w = poloniex::errors::Op::Withdraw;
    let e = poloniex::errors::from_code(400, "21601", "Account locked. Contact support", w, "{}");
    check(&e, C::AccountRestricted, "заблокирован");
    let e = poloniex::errors::from_code(400, "21711", "Currency locked. Contact support", w, "{}");
    assert_eq!(e.code, C::WithdrawRejected);
}

#[test]
fn core_treats_account_like_address() {
    use arb::core::withdraw_refused::refusal;
    let e = GatewayError::new(C::AccountRestricted, Some(VenueId("binance".into())), "x");
    assert!(refusal(Some(C::AccountRestricted)));
    assert!(arb::gateway::rules::withdraw_refusal(C::AccountRestricted));
    assert!(arb::core::auto_pause::no_pause(C::AccountRestricted));
    assert!(!arb::core::risk_count::counts(&e));
    assert!(OpError::from(&e).withdraw_unsent());
    let codes = arb::api::rules_codes::codes();
    assert!(
        codes
            .iter()
            .any(|c| c.code == C::AccountRestricted && c.no_retry)
    );
}

/// Отказ вывода Binance −4013 (2FA не включена) — монета не закрывается, журнал «проблема в аккаунте».
#[tokio::test]
async fn account_refusal_does_not_close_coin() {
    use venues::binance::errors::{Op, from_code};
    let d = dir("t82acc");
    let (core, j) = start(&d, None).await;
    let binance = VenueId("binance".into());
    let coin = Asset {
        ticker: "CYBER".into(),
        bsc_contract: Some(alloy_primitives::address!(
            "0x0000000000000000000000000000000000082a01"
        )),
    };
    let raw = serde_json::json!({"code": -4013, "msg": "2FA is not opened."}).to_string();
    let e = OpError::from(&from_code(-4013, "2FA is not opened.", Op::Withdraw, &raw));
    arb::core::withdraw_block::close(&core, "20261111100000000CYBER", &coin, &binance, &e).await;
    let d = Direction {
        asset: coin,
        buy_on: binance,
        sell_on: VenueId("bsc_dex".into()),
    };
    assert!(
        arb::core::withdraw_block::check(&core, &d).is_ok(),
        "монета не закрыта"
    );
    let logs = entries(&j, "core.withdraw_block", "проблема в аккаунте binance").await;
    assert_eq!(logs.len(), 1);
}
