//! Файл: notify/reason_codes_range.rs
//! Что делает: решение 268 — человеческая формулировка кодов бирж «цена лимитного ордера вне диапазона, разрешённого
//! биржей» (код шлюза PriceOutOfRange). Коды — те же, что сопоставлены в адаптерах (venues/<биржа>/errors.rs, там же
//! ссылки на официальную документацию и живой ответ MEXC 13.11.2026); новых смыслов здесь нет. Kraken, KuCoin, Phemex —
//! кодов нет (не сопоставлены, docs/TASKS.md); Gate и Binance (итог ордера, не код ошибки) — текст даёт сам адаптер.
//! Кто вызывает: notify/reason_codes.rs (lookup — перед общим словарём).
//! От чего зависит: ничего.
//! Настройки: нет.

/// Одна формулировка на все биржи.
const TEXT: &str =
    "цена лимитного ордера вне диапазона, разрешённого биржей (коридор от рыночной цены)";

/// Человеческая формулировка кода. Принимает: id площадки, код (число или метка строкой). Возвращает: текст или None.
pub fn lookup(venue: &str, code: &str) -> Option<&'static str> {
    let hit = match venue {
        "mexc" => code == "30087",
        "bingx" => code == "100440",
        "xt" => matches!(
            code,
            "ORDER_F0501" | "ORDER_F0502" | "ORDER_F0503" | "ORDER_F0504" | "ORDER_F0401"
        ),
        "bybit" => matches!(code, "170192" | "170193" | "170194" | "170371" | "170372"),
        "poloniex" => matches!(code, "21310" | "21311"),
        "okx" => matches!(code, "51006" | "51137" | "51138"),
        "htx" => matches!(
            code,
            "order-limitorder-price-min-error" | "order-limitorder-price-max-error"
        ),
        "coinex" => matches!(
            code,
            "3606" | "3615" | "3616" | "3617" | "3618" | "3632" | "3633"
        ),
        "bitget" => matches!(
            code,
            "43008" | "43009" | "43039" | "43040" | "40815" | "40816"
        ),
        _ => false,
    };
    hit.then_some(TEXT)
}
