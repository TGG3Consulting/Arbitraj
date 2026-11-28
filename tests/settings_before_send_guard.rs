//! Файл: tests/settings_before_send_guard.rs
//! Что делает: страж решения 398 по исходникам — настройки (и ключ кошелька, эталон кода) читаются ДО необратимого
//! действия, а не после: иначе при незаданной настройке транзакция или ордер уже ушли, а бот получает ошибку
//! «не задана» и не узнаёт итог (кейс kyber.receipt_poll_ms / kyber.receipt_timeout_ms, kyberswap/tx.rs).
//! Критерий (простой и проверяемый глазами):
//! 1) AFTER — пары «файл, функция, метка отправки». Тело функции — от строки её объявления до первой строки, равной
//!    отступу объявления + «}» (код отформатирован rustfmt). Строки тела очищаются от комментария «//» (вне строковых
//!    литералов) и склеиваются: строка, начинающаяся с «.», дописывается к предыдущей (цепочки, перенесённые rustfmt).
//!    Метка — подстрока строки отправки (eth_sendRawTransaction, dispatch в каналы, POST ордера / вывода); пустая метка —
//!    функция целиком после отправки (трекер, отмена, итог); «*» вместо функции — весь файл. От первой склеенной строки
//!    с меткой до конца тела не должно быть чтений из READS: настройка (ctx.int( / ctx.dec( / … / .settings. /
//!    integer_in( / get_exact( / …), сроки целиком (SendCfg::read(, Wait::read(, times(ctx), ключ и кошелёк
//!    (signer::signer(, signer::sign(, ctx.wallet(, vault.get(, ctx.keys(), лимит газа с чтением запаса (tx::limit( —
//!    после отправки только tx::limit_by), эталон кода (code::runtime().
//! 2) Биржи: у каждой из 14 бирж сроки ожидания итога ордера (order_state::times) читаются до постановки — либо в
//!    trade.rs до строки постановки (rest::signed( / rest::post( / rest::private(), либо и в order_rules.rs, и в
//!    market_sell.rs (подготовка ордера, которую trade получает готовой).
//!
//! Чего страж НЕ ловит (честно; это не анализатор кода):
//! - вложенные вызовы: функция после метки, которая сама читает настройки и не стоит в AFTER (rest::signed — окно
//!   запроса и ключи API, settle и fills у 13 бирж, поиск вывода Binance → history, rpc::call → адрес узла, r.send
//!   каналов, swap_ctx::*) — известные перечислены в docs/TASKS.md (решение 398, п. 2);
//! - чтение через локальную переменную (`let s = &core.inner.deps.settings;` затем `s.integer(` — ловится только
//!   «.integer(» / «.decimal(» и т. п. из READS; `let st = &ctx.settings;` затем свой метод — нет) и через методы,
//!   которых нет в READS;
//! - файлы и функции вне AFTER: вывод у 13 бирж, settle / cancel у 13 бирж, swap_send.rs, ядро (core/*), новые пути;
//! - в п. 2 — что после постановки сроки не перечитываются (только что они прочитаны до неё);
//! - body() берёт первую функцию с таким именем в файле и знает только перечисленные заголовки (без `pub(in …)`,
//!   `const fn`, `unsafe fn`); строковые литералы на несколько строк и `'"'` разбираются упрощённо.
//!
//! Новый путь отправки — новая строка в AFTER. Чтение после отправки, которое нельзя перенести, сюда не попадает —
//! оно записано в docs/TASKS.md (решение 398) с причиной.
//! Кто вызывает: cargo test.
//! От чего зависит: только исходники src/ (чтение файлов). Моков нет.

use std::path::Path;

/// Что считается чтением настройки / ключа / эталона (подстрока кода строки).
const READS: [&str; 31] = [
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
    ".integer(",
    ".decimal(",
    "integer_in(",
    "decimal_in(",
    "text_in(",
    "flag_in(",
    "address_in(",
    "get_exact(",
    "SendCfg::read(",
    "Wait::read(",
    "times(ctx",
    "signer::signer(",
    "signer::sign(",
    "ctx.wallet(",
    "vault.get(",
    "ctx.keys(",
    "code::runtime(",
    "tx::limit(",
    "node_url(",
    // Решение 416 (ревью М-4): chainId сети и сверка сети узлов — только до отправки.
    "chain_id::chain_id(",
    "chain_seen::ensure(",
];

/// (файл от корня, функция или «*» — весь файл, метка отправки; пусто — вся функция после отправки).
const AFTER: [(&str, &str, &str); 39] = [
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
        // Обычные отправки и измеренная Tokenlon проходят через одно тело перед dispatch.
        "send_with_gas",
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
    // Решение 437: трекер сети eip1559_l1 без отмен — весь файл после отправки.
    ("src/venues/bsc_dex/send/track_l1.rs", "*", ""),
    ("src/venues/bsc_dex/send/track_util.rs", "*", ""),
    ("src/venues/bsc_dex/send/settle.rs", "*", ""),
    ("src/venues/bsc_dex/send/receipt.rs", "*", ""),
    ("src/venues/bsc_dex/send/cancel.rs", "send", ""),
    // Решение 435 (433-д): цена и подпись отмены по модели газа — без чтения настроек (всё из Watch).
    ("src/venues/bsc_dex/send/tx_price.rs", "*", ""),
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
        "txs::send_tipped(",
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

/// Код строки без комментария «//» вне строкового литерала (кавычки с экранированием «\"»). Ошибок нет.
fn code(l: &str) -> &str {
    let b = l.as_bytes();
    let (mut in_str, mut i) = (false, 0);
    while i < b.len() {
        match b[i] {
            b'\\' if in_str => i += 1,
            b'"' => in_str = !in_str,
            b'/' if !in_str && b.get(i + 1) == Some(&b'/') => return &l[..i],
            _ => {}
        }
        i += 1;
    }
    l
}

/// Строки тела без комментариев, цепочки склеены: строка, начинающаяся с «.», — к предыдущей. Ошибок нет.
fn joined(lines: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in lines {
        let c = code(l).trim();
        match out.last_mut() {
            Some(last) if c.starts_with('.') => last.push_str(c),
            _ => out.push(c.to_string()),
        }
    }
    out
}

#[test]
fn no_setting_reads_after_send() {
    let mut bad = Vec::new();
    for (file, name, mark) in AFTER {
        let src = read(file);
        let raw = body(&src, name).unwrap_or_else(|| panic!("{file}: нет функции {name}"));
        let lines = joined(&raw);
        let from = if mark.is_empty() {
            0
        } else {
            lines
                .iter()
                .position(|l| l.contains(mark))
                .unwrap_or_else(|| panic!("{file} {name}: нет строки отправки «{mark}»"))
        };
        for l in &lines[from..] {
            for r in READS {
                if l.contains(r) {
                    bad.push(format!("{file} {name}: «{r}» после отправки: {l}"));
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
