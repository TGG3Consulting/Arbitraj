# XT — пример ответа из официальной документации, 17.11.2026

`support_currency_example.json` — пример ответа GET /v4/public/wallet/support/currency со страницы
https://doc.xt.com/docs/spot/Deposit&Withdrawal/GetSupportedCurrencies (прочитана 17.11.2026, задача t163): первая
монета примера (BTC, сеть Bitcoin) с полями и значениями, как на странице; комментарии `// …` страницы убраны (JSON их
не допускает), обёртка {rc, mc, ma, result} — как у всех ответов XT. Это пример документации, не живой ответ: живой
ответ записывается на сервере бота (tests/README.md, раздел XT, файл cex_samples_2026_11_05/xt_support_currency.json).
Нужен тесту tests/xt_deposit_min_t163.rs (решение 365: минимум депозита — depositMinAmount).
