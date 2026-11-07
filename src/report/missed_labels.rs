//! Файл: report/missed_labels.rs
//! Что делает: подписи отчёта «Упущено» (решения 215, 217): что ограничило объём (коды Record::limited_by — как
//! ui/src/lib/uncappedLabels.js LIMITED), вердикт расчёта на нашем бюджете (VERDICT), на каких данных посчитано
//! (Record::basis — BASIS: снимок рынка момента решения / после покупки — занижено / живые котировки / решение 218 —
//! пересчитано по окну сделки).
//! Кто вызывает: report/missed_rows.rs.
//! От чего зависит: ничего.
//! Настройки: нет.

/// Что ограничило объём — по коду Record::limited_by (как ui/src/lib/uncappedLabels.js LIMITED). Ошибок нет.
pub fn limited_label(k: &str) -> String {
    match k {
        "per_trade" => "лимит на сделку money.max_per_trade_usd",
        "per_coin" => "лимит на монету money.max_per_coin_usd",
        "in_transit" => "лимит в пути money.max_in_transit_usd",
        "dry" => "виртуальный бюджет dry.budget_usdt",
        "free" => "свободный остаток на площадке покупки",
        "budget" => "бюджет (составляющие не прочитаны)",
        "optimum" => "ничего: оптимум внутри бюджета",
        "liquidity" => "ликвидность площадок",
        "search_cap" => "предел поиска card.max_search_usdt",
        other => other,
    }
    .to_string()
}

/// Вердикт расчёта на нашем бюджете (Battle::verdict) — как uncappedLabels.js VERDICT. Ошибок нет.
pub fn verdict_label(v: &str) -> String {
    match v {
        "deal" => "сделка",
        "accept" => "прошёл, сделка не запущена",
        "reject" => "отказ",
        other => other,
    }
    .to_string()
}

/// Решение 217: источник данных записи (Record::basis) — подпись. Ошибок нет.
pub fn basis_label(b: &str) -> String {
    match b {
        "decision_snapshot" => "рынок на момент решения (до покупки)",
        "live_after_buy" => "посчитано после покупки — занижено",
        "live" => "живые котировки",
        // Решение 218: старая запись пересчитана по окну сделки (рынок до входа).
        "recomputed_window" => "пересчитано по окну: стаканы на момент решения (до покупки)",
        "recomputed_window_dex_plan" => {
            "пересчитано по окну: биржа — стакан на момент решения, DEX — из плана (приблизительно)"
        }
        other => other,
    }
    .to_string()
}
