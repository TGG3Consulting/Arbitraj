# Wombat Exchange, Arbitrum One — сверка развёрнутых реализаций с исходником (решение 484)

Получено 20.11.2026: Sourcify v2 `https://sourcify.dev/server/v2/contract/42161/<адрес>` (поля `proxyResolution`,
`sources`, `compilation`, `storageLayout`) через браузер; официальный исходник — `github.com/wombat-exchange/v1-core`,
коммит `2ac067b82dd94eee75cf71d922a33a3d5c8f2a79` (17.03.2023; последний коммит, менявший `pool/CoreV2.sol`,
`pool/PoolV2.sol`, `pool/HighCovRatioFeePoolV2.sol`, `pool/DynamicPoolV2.sol`, `asset/Asset.sol`). Адреса пулов —
docs.wombat.exchange/developers/contracts/arbitrum (раздел Pool).

## Пулы и реализации (proxyResolution; прокси — `OptimizedTransparentUpgradeableProxy`, exact_match)

| Пул | Адрес | Реализация | Контракт реализации |
| --- | --- | --- | --- |
| Cross Chain | 0xe78876C360716f2225F55A6726B32324FE1B1145 | 0xe1C846B7b347aC535745943c01da93c73aF1F454 | CrossChainPool (CoreV3 / PoolV3 — в v1-core нет; в код не взят) |
| Main | 0xc6bc781E20f9323012F6e422bdf552Ff06bA6CD1 | 0x82E62f4e174E3C5e1641Df670c91Ac6Ab8541518 | HighCovRatioFeePoolV2 |
| Overnight | 0xCF20fDA54e37f3fB456930f02fb07FccF49e4849 | 0x6A4b5988692F8C47eA3BcCFB8A127288db61f5E1 | HighCovRatioFeePoolV2 |
| MIM | 0x29eeB257a2A6eCDE2984aCeDF80A1B687f18eC91 | 0x30E27c4E1C9D8845a961CA703bf4B5e46e1F554c | HighCovRatioFeePoolV2 |
| BOB | 0x917caF2b4D6040a9D67A5F8CEfC4F89d1b214c1A | 0x3ccC0dC754E380cDA16EF94292F278bbc4Ff463E | HighCovRatioFeePoolV2 |
| mWOM | 0x90eCddEC4E4116E30769A4e1EA52c319aca338B6 | 0xC096FF2606152eD2A06dd12F15A3c0466Aa5A9fa | HighCovRatioFeePoolV2 |
| wmxWOM | 0xEE9b42b40852a53c7361F527e638B485D49750cD | 0xbEd0509801634CcC29Fa9f2faD764Bf194c7927d | HighCovRatioFeePoolV2 |
| qWOM | 0x12Fa5AB079CFf564d599466d39715D35d90Af978 | 0x9cbeE53A48390A3205B14F6B8e48Ca81Fe105dEE | HighCovRatioFeePoolV2 |
| frxETH | 0x20D7ee728900848752FA280fAD51aF40c47302f1 | 0x4d1CC84a33462c35ad3dd68756d330CBbA087053 | DynamicPoolV2 |
| wstETH | 0xe14302040c0A1eb6fB5A4A79EfA46D60029358d9 | 0x3DbF3e359a570D46CF5a663AC92DC7Cae77110FB | DynamicPoolV2 |
| ankrETH | 0xB9bdfE449Da096256Fe7954Ef61A18eE195Db77B | 0x3DbF3e359a570D46CF5a663AC92DC7Cae77110FB | DynamicPoolV2 |
| FRAX-MAI-USD+ | 0x4a8686df475D4c44324210FFA3Fc1DEA705296e0 | 0x38D2259fDeEdc94Eaf915a872139c985Ce36d5Ad | HighCovRatioFeePoolV2 |
| jUSDC | 0xc7a6bA5F28993BaDb566007bD2E0CB253c431974 | 0x7585bae98800C621b427Fe045EA3b4052559C11b | DynamicPoolV2 |
| fUSDC | 0x956454C7BE9318863297309183C79b793D370401 | 0x44A0e2AC2c99D5cB2910ec1784DD158b7fa3A642 | HighCovRatioFeePoolV2 |
| mPENDLE | 0xe7159f15e7b1d6045506B228A1ed2136dcc56F48 | 0x44A0e2AC2c99D5cB2910ec1784DD158b7fa3A642 | HighCovRatioFeePoolV2 |
| USDV | 0xa6eF6C45EbFDBc13f6D032fbDFeC9b389C1603E5 | 0x44A0e2AC2c99D5cB2910ec1784DD158b7fa3A642 | HighCovRatioFeePoolV2 |
| FRAX-USDV | 0x3cc8c886575968642Cab9F430261c81C5b044d4b | 0x44A0e2AC2c99D5cB2910ec1784DD158b7fa3A642 | HighCovRatioFeePoolV2 |

Router `0xc4B2F992496376C6127e73F1211450322E580668` — WombatRouter, CoreV3 `0xae5E8171707d1eF76208700555a32F572A5ECBdC` —
CoreV3 (обе — exact_match; бот их не вызывает). Реализации пулов HighCovRatioFeePoolV2 / DynamicPoolV2 — exact_match,
CrossChainPool — match; solc 0.8.18.

## Сверка текстов (SHA-256 первых 6 hex текста файла)

| Файл | Реализации Main…qWOM, FRAX-MAI-USD+, frxETH | wstETH / ankrETH, jUSDC, fUSDC…FRAX-USDV | v1-core 2ac067b8 |
| --- | --- | --- | --- |
| CoreV2.sol | b3baeb | aa2277 (jUSDC, wstETH, fUSDC…) | b3baeb |
| PoolV2.sol | 53238c | ad2cec (wstETH, fUSDC…); jUSDC — 53238c | d1bd84 |
| HighCovRatioFeePoolV2.sol | 5d5c75 | 5d5c75 | 5d5c75 |
| DynamicPoolV2.sol | f19a24 | f19a24 | f19a24 |
| PausableAssets.sol | c39623 | fd245f | c39623 |
| DSMath.sol / SignedSafeMath.sol / IAsset.sol | f79720 / 66c353 / e5d2e9 | те же | те же |
| Asset.sol (активы всех проверенных пулов) | bcb68e | bcb68e | bcb68e |

Отличия (построчное сравнение в браузере):

- PoolV2.sol 53238c против d1bd84: `_mintFeeIfNeeded` (порог mintFeeThreshold) вместо `_mintFee` в deposit / withdraw,
  `_mintFee` без порога возвращает собранное, `mintFee(token)` возвращает сумму; строка «remainings are sent to the
  tipbucket». Путь обмена (_quoteFactor, _quoteFrom, _swap, swap, quotePotentialSwap) — без отличий.
- PoolV2.sol ad2cec против 53238c: одна строка — `if (!_containsAsset(token)) revert WOMBAT_ASSET_NOT_EXISTS();` в
  pauseAsset (L323). Путь обмена — без отличий.
- CoreV2.sol aa2277 против b3baeb: только комментарий («TODO (if any) from Yellow Paper» → «From Yellow Paper»).
- PausableAssets.sol fd245f против c39623: параметры событий и mapping названы token, добавлен геттер
  `isPaused(address token)`; ключ mapping — монета в обоих (PoolV2.pauseAsset(token) → _pauseAsset(token)).
- DynamicAsset.sol активов динамических пулов (052671) ≠ v1-core (a55783): getRelativePrice — у наследников
  (PriceFeedAsset, WstETHAsset, AnkrStakingAsset, jUsdcAsset) от внешних источников; бот читает getRelativePrice()
  вызовом и формулу цены не повторяет.

## Раскладка хранилища (storageLayout Sourcify, все 12 реализаций)

`_pausedAssets` — слот 201, `ampFactor` — 202, `startCovRatio` / `endCovRatio` — слот 264 (смещения 0 / 16). Бот читает
`_pausedAssets[монета]` слотом keccak256(abi.encode(монета, 201)) — у части реализаций геттера isPaused нет.
