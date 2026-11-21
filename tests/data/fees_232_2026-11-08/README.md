# Монета комиссии торговли остальных бирж — 08.11.2026 (решение 232)

- `live_deals_facts.json` — исполненные ордера живого бота (GET http://127.0.0.1:8787/api/deals, 08.11): покупки KuCoin
  (feeCurrency USDT, комиссия = 0.003 × сумма) и XT (WIN, комиссия = 0.002 × количество). Поля — из записи сделки бота
  (request.venue/side/asset, status, filled_qty, avg_price, fee, fee_asset, venue_ref), не сырой ответ биржи.
- `poloniex_order_trades_doc.json` — пример ответа документации Poloniex «Trades by Order Id» (GET /orders/{id}/trades,
  https://api-docs.poloniex.com/spot/api/private/trade): BUY LINK_USDT, feeCurrency LINK. Не живой ответ.
- `coinex_order_status_doc_fees.json` — поля комиссии из примера документации CoinEx «Get Order Status»
  (https://docs.coinex.com/api/v2/spot/order/http/get-order-status): buy CETUSDT, base_fee 11.6582326221, quote_fee 0.
  Не живой ответ, только поля комиссии и стороны.

Живых ордеров OKX, CoinEx, Phemex, Poloniex, Kraken, HTX в журнале бота нет — заменить сырыми ответами при первых ордерах.

Используется: tests/fees_232.rs.
