# contracts/evmtest — EVM-тест исполнителя ArbExecutor (решение 272)

Запуск: `cd contracts/evmtest && npm ci && node run.mjs` (версии — `package-lock.json`; сеть не нужна после `npm ci`).
Последняя строка `ВСЕ ПРОВЕРКИ ПРОШЛИ` — успех; любая ошибка — `НЕ ПРОШЛО: …` и код выхода ≠ 0.

| Файл | Что делает |
| --- | --- |
| `run.mjs` | Проверки (список — шапка файла): сборка = `contracts/out/`, продажа/покупка BNB `executeNative`, остаток, minOut, неверный wnative, не владелец, повторный вход, STABLE_U256_BNB, BNB напрямую, `execute` v2 и v1 |
| `lib.mjs` | Компиляция (solc 0.8.37 — как `build.mjs`; 0.5.16 и 0.6.6 — для официальных исходников), EVM ethereumjs Cancun, вызовы, разбор отката и событий |
| `vendor/pancake-swap-core/` | Официальные PancakePair / PancakeFactory / ERC20 — pancakeswap/pancake-swap-core@3b214306770e86bc3a64e67c2b5bdb566b4e94a7 `contracts/` без изменений |
| `vendor/v2-periphery/WETH9.sol` | WETH9 — Uniswap/v2-periphery@ed24991304291297c3b4a52818d02f46a17aa9a2 `contracts/test/WETH9.sol` без изменений (withdraw — `msg.sender.transfer`, стипендия 2300 газа) |
| `vendor/SHA256SUMS` | Хеши скопированных файлов |
| `fixtures/StableBnbFixture.sol` | ФИКСТУРА: пул StableSwap с BNB — кода Pancake Stable / Lista в репозитории нет; повторены только строки BNB официальных исходников (вход `dx == msg.value`, выход `call{gas: bnb_gas}`), курс 1:1 |
| `fixtures/ReentrantToken.sol` | ФИКСТУРА: ERC-20 с повторным входом в исполнитель из transfer |

Что НЕ проверяется здесь: математика пулов StableSwap (проверена тестами бота на записанных пулах), газ на живой сети
(снимается сухой сделкой — docs/notes/executor-v2-redeploy.md).
