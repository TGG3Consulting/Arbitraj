# Живые ответы XT, 05.11.2026

`currencies.json` — ответ GET https://sapi.xt.com/v4/public/currencies (публичный, снят в браузере 05.11.2026 ~18:20 UTC).
Из 1645 монет оставлены две (btc, cake) и из каждой — поля, которые читает бот (id, currency, displayName,
depositStatus, withdrawStatus, maxPrecision, isListing); остальные поля (logo, chainRelation и т. п.) вырезаны.
Главное: `result` — ОБЪЕКТ {time, version, currencies[...]}, а не массив, как в документации (`_balance/currenciesGet.md`);
статусы — числа (1), а не строки. Решение 184.
