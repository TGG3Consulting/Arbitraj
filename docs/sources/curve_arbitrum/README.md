# docs/sources/curve_arbitrum — пулы Curve (Arbitrum One), исходники Sourcify

Решение 481 (T472-10). Текст ниже шапки каждого файла — поле `sources["Vyper_contract.vy"]` ответа
`https://sourcify.dev/server/v2/contract/42161/<адрес>?fields=sources,compilation` без изменений (SHA-256 — в шапке;
совпал с посчитанным в браузере). Номера строк в комментариях кода бота — строки исходника (без шапки).

| Файл | Пул | Компилятор | Sourcify | Вид бота |
| --- | --- | --- | --- | --- |
| `0x3E01dD8a5E1fb3481F0F589056b428Fc308AF0Fb.vy` | WBTC/renBTC («ren»), LP — сам пул | Vyper 0.2.16 | creationMatch match, runtimeMatch match | `curve_stableswap_plain` (математика 2pool / Plain2Basic) |
| `0x6eB2dc694eB516B16Dc9FBc678C60052BbdD7d80.vy` | ETH/wstETH («wsteth»), LP — отдельный токен | Vyper 0.3.1 | creationMatch match, runtimeMatch null | `curve_stableswap_oracle` (курс wstETH — оракул пула) |

Адреса пулов — curvefi/curve-js ab3f1c2 src/constants/pools/arbitrum.ts L98 (ren), L152 (wsteth).
