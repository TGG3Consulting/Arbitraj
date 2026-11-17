//! Файл: tests/settings_before_send_guard.rs
//! Что делает: страж решения 398 по исходникам — настройки (и ключ кошелька, эталон кода) читаются ДО необратимого
//! действия, а не после: иначе при незаданной настройке транзакция или ордер уже ушли, а бот получает ошибку
//! «не задана» и не узнаёт итог (кейс kyber.receipt_poll_ms / kyber.receipt_timeout_ms, kyberswap/tx.rs).
//! Критерий (простой и проверяемый глазами):
//! 1) AFTER — пары «файл, функция, метка отправки». Тело функции — от строки её объявления до первой строки, равной
//!    отступу объявления + «}» (код отформатирован rustfmt). Метка — подстрока строки отправки (eth_sendRawTransaction,
//!    dispatch в каналы, POST ордера / вывода); пустая метка — функция целиком после отправки (трекер, отмена, итог),
//!    «*» вместо функции — весь файл. От первой строки с меткой до конца тела (без комментариев «//») не должно быть
//!    чтений из READS: настройка (ctx.int( / ctx.dec( / ctx.text( / ctx.flag( / … / .settings.), сроки целиком
//!    (SendCfg::read(, Wait::read(, times(ctx), ключ и кошелёк (signer::signer(, signer::sign(, ctx.wallet(,
//!    vault.get(), лимит газа с чтением запаса (tx::limit( — после отправки только tx::limit_by), эталон кода
//!    (code::runtime().
//! 2) Биржи: у каждой из 14 бирж сроки ожидания итога ордера (order_state::times) читаются до постановки — либо в
//!    trade.rs до строки постановки (rest::signed( / rest::post( / rest::private(), либо и в order_rules.rs, и в
//!    market_sell.rs (подготовка ордера, которую trade получает готовой).
//!
//! Новый путь отправки — новая строка в AFTER. Чтение после отправки, которое нельзя перенести, сюда не попадает —
//! оно записано в docs/TASKS.md (решение 398) с причиной.
//! Кто вызывает: cargo test.
//! От чего зависит: только исходники src/ (чтение файлов). Моков нет.

use std::path::Path;

/// Что считается чтением настройки / ключа / эталона (подстрока кода строки).
const READS: [&str; 20] = [
    "ctx.int(",
    "ctx.dec(",
    "ctx.text(",
    "ctx.flag(",
    "ctx.addr(",
    "ctx.venue_text(",
    "ctx.venue_flag(",
    "ctx.dex_",
    "ctx.global_int(",
    ".settings.",
    "SendCfg::read(",
    "Wait::read(",
    "times(ctx",
    "signer::signer(",
    "signer::sign(",
    "ctx.wallet(",
    "vault.get(",
    "code::runtime(",
    "tx::limit(",
    "node_url(",
];

/// (файл от корня, функция или «*» — весь файл, метка отправки; пусто — вся функция после отправки).
const AFTER: [(&str, &str, &str); 37] = [
    ("src/venues/kyberswap/tx.rs", "send", "node::send_raw("),
    ("src/venues/kyberswap/tx.rs", "track", ""),
    (
        "src/venues/kyberswap/trade.rs",
        "execute",
        "let d = tx::send(",
    ),
    ("src/venues/kyberswap/trade.rs", "outcome_ok", ""),
    ("src/venues/kyberswap/transfer.rs", "withdraw", "tx::send("),
    ("src/venues/kyberswap/trade_fact.rs", "*", ""),
    (
        "src/venues/bsc_dex/exec/txs.rs",
        "send_capped",
        "send::dispatch(",
    ),
    (
        "src/venues/bsc_dex/exec/bundle_send.rs",
        "send",
        "send::dispatch_slice(",
    ),
    ("src/venues/bsc_dex/exec/bundle_send.rs", "confirm", ""),
    ("src/venues/bsc_dex/exec/bundle_send.rs", "all_receipts", ""),
    ("src/venues/bsc_dex/send/track.rs", "*", ""),
    ("src/venues/bsc_dex/send/track_util.rs", "*", ""),
    ("src/venues/bsc_dex/send/settle.rs", "*", ""),
    ("src/venues/bsc_dex/send/receipt.rs", "*", ""),
    ("src/venues/bsc_dex/send/cancel.rs", "send", ""),
    ("src/venues/bsc_dex/exec/chain_tx.rs", "*", ""),
    ("src/venues/bsc_dex/exec/result.rs", "*", ""),
    ("src/venues/bsc_dex/exec/executor/fact.rs", "*", ""),
    (
        "src/venues/bsc_dex/exec/trade_direct.rs",
        "trade",
        "send_swap(",
    ),
    (
        "src/venues/bsc_dex/exec/trade_smart.rs",
        "trade",
        "send_swap(",
    ),
    (
        "src/venues/bsc_dex/exec/executor/trade.rs",
        "trade",
        "txs::send_capped(",
    ),
    (
        "src/venues/bsc_dex/exec/cycle_exec.rs",
        "trade",
        "txs::send_capped(",
    ),
    (
        "src/venues/bsc_dex/exec/bundle_trade.rs",
        "trade",
        "bundle_send::send(",
    ),
    (
        "src/venues/bsc_dex/exec/bundle_trade.rs",
        "bundle_result",
        "",
    ),
    (
        "src/venues/bsc_dex/exec/cycle_bundle_trade.rs",
        "trade",
        "bundle_send::send(",
    ),
    (
        "src/venues/bsc_dex/exec/executor/deploy.rs",
        "deploy",
        "txs::send(",
    ),
    (
        "src/venues/bsc_dex/exec/executor/deploy.rs",
        "on_chain_ok",
        "",
    ),
    (
        "src/venues/bsc_dex/wallet/transfer.rs",
        "withdraw",
        "txs::send(",
    ),
    ("src/venues/bingx/trade.rs", "trade", "rest::signed("),
    ("src/venues/bingx/order_state.rs", "settle", ""),
    ("src/venues/bingx/order_state.rs", "poll_final", ""),
    ("src/venues/bingx/order_state.rs", "result", ""),
    ("src/venues/kraken/trade.rs", "trade", "rest::private("),
    ("src/venues/kraken/order_state.rs", "result", ""),
    (
        "src/venues/kraken/transfers.rs",
        "withdraw",
        "rest::funding(",
    ),
    ("src/venues/kraken/transfers.rs", "find", ""),
    ("src/venues/kraken/transfers.rs", "recent", ""),
];

/// Биржи (адаптеры с ордерами на споте).
const CEX: [&str; 14] = [
    "binance", "bingx", "bitget", "bybit", "coinex", "gate", "htx", "kraken", "kucoin", "mexc",
    "okx", "phemex", "poloniex", "xt",
];

/// Текст файла от корня репозитория. Паника с именем файла — файла нет (страж должен знать о переносе).
fn read(rel: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// Строки тела функции `name` (см. шапку); «*» — весь файл. None — функции нет.
fn body<'a>(src: &'a str, name: &str) -> Option<Vec<&'a str>> {
    let lines: Vec<&str> = src.lines().collect();
    if name == "*" {
        return Some(lines);
    }
    let heads = [
        "pub async fn ",
        "async fn ",
        "pub fn ",
        "fn ",
        "pub(super) async fn ",
        "pub(super) fn ",
        "pub(crate) async fn ",
        "pub(crate) fn ",
    ];
    for (i, l) in lines.iter().enumerate() {
        let decl = l.trim_start();
        let found = heads.iter().any(|h| {
            decl.starts_with(&format!("{h}{name}(")) || decl.starts_with(&format!("{h}{name}<"))
        });
        if !found {
            continue;
        }
        let end = format!("{}}}", &l[..l.len() - decl.len()]);
        let last = lines[i + 1..].iter().position(|x| *x == end)?;
        return Some(lines[i..=i + 1 + last].to_vec());
    }
    None
}

/// Код строки без комментария «//». Ошибок нет.
fn code(l: &str) -> &str {
    l.split("//").next().unwrap_or("")
}

#[test]
fn no_setting_reads_after_send() {
    let mut bad = Vec::new();
    for (file, name, mark) in AFTER {
        let src = read(file);
        let lines = body(&src, name).unwrap_or_else(|| panic!("{file}: нет функции {name}"));
        let from = if mark.is_empty() {
            0
        } else {
            lines
                .iter()
                .position(|l| code(l).contains(mark))
                .unwrap_or_else(|| panic!("{file} {name}: нет строки отправки «{mark}»"))
        };
        for l in &lines[from..] {
            for r in READS {
                if code(l).contains(r) {
                    bad.push(format!("{file} {name}: «{r}» после отправки: {}", l.trim()));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "решение 398: читайте до отправки и передавайте значением:\n{}",
        bad.join("\n")
    );
}

#[test]
fn order_wait_times_read_before_order() {
    let mut missing = Vec::new();
    for v in CEX {
        let trade = read(&format!("src/venues/{v}/trade.rs"));
        let lines = body(&trade, "trade").unwrap_or_else(|| panic!("{v}: нет trade::trade"));
        let place = ["rest::signed(", "rest::post(", "rest::private("];
        let before: Vec<&str> = lines
            .iter()
            .take_while(|l| !place.iter().any(|p| code(l).contains(p)))
            .copied()
            .collect();
        assert!(
            before.len() < lines.len(),
            "{v}: в trade.rs нет постановки ордера"
        );
        let pre = "order_state::times(ctx)?";
        let in_trade = before.iter().any(|l| code(l).contains(pre));
        let rules = read(&format!("src/venues/{v}/order_rules.rs"));
        let market = read(&format!("src/venues/{v}/market_sell.rs"));
        let ok = in_trade || (rules.contains(pre) && market.contains(pre));
        if !ok {
            missing.push(v);
        }
    }
    assert!(
        missing.is_empty(),
        "решение 398: сроки ожидания итога ордера не читаются до постановки у {missing:?}"
    );
}
