# Импорт настроек Base и Ethereum (сборка e35c9c95)

`base-ethereum-extras-e35c9c95.json` — значения поверх справочника `config/first_run_dex.json` (той же сборки):
настройки площадок `base_dex`/`eth_dex` из таблицы O-68 (`docs/FIRST_RUN.md`) и раздела «Регистрация, узлы и управление»
(`docs/BASE_ETHEREUM_SETTINGS.md`), каналы `base_public`/`ethereum_public`, особые значения DEX-профилей, `ryze.*`.
Формат: `[имя, область, значение]`. Каждый профиль справочника дополнительно получает `dex.pool_enabled = true`
(кроме явных значений в файле). Секретов нет. BSC и Arbitrum не затрагиваются. Ethereum: `venue.enabled = false`.
Полный набор: 2933 записи, sha256 канонического списка `cbaf5d0721524654035aa64c23990d752078b42aebc3191503428c900d5c1648`.
