//! Файл: settings/catalog/dex_ekubo.rs
//! Что делает: настройки вида пулов Ekubo (решение 447, dex.kind = ekubo; DEX ekubo-v3-arbitrum и любые DEX того же
//! семейства): адреса ядра Core, реестра ключей PoolKeyIndex, расширения MEVCapture и сколько слов битмапа тиков читать.
//! Решение 454: адреса расширений TWAMM, Oracle, BoostedFees, горизонт виртуальных ордеров TWAMM, буфер общей
//! подписки ядра (хаб).
//! Отдельный файл: каталог dex.rs правят параллельно другие семейства. Область — «DEX» (DX). Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, группа «Контракты DEX»).
//!
//! Адреса — официальные: docs.ekubo.org/reference/contracts/evm-v3 (Core, MEVCapture, MEVCaptureRouter — один адрес во
//! всех сетях, Arbitrum — Live); PoolKeyIndex — EkuboProtocol/evm-contracts broadcast/DeployAll.s.sol/42161
//! (run-1785724793522.json, commit 02e0d82f). Решение 454: TWAMM 0xd4f1060cb9c1a13e1d2d20379b8aa2cf7541ed9b (61d09daa,
//! run-1767194711580.json) и 0xd47f1b1edcfeabb08f6ebd8fc337c27e636c75ba (8c1b721, run-1777073880286.json; он же на
//! docs.ekubo.org), Oracle 0x517e506700271aea091b02f42756f5e174af5230 (61d09daa), BoostedFees
//! 0xd4b54d0ca6979da05f25895e6e269e678ba00f9e (concentrated) и 0x948b9c2c99718034954110cb61a6e08e107745f9 (stableswap)
//! (69b642d, run-1770053979549.json; docs.ekubo.org). Вводит Тигран.

use super::{DX, V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Настройки решения 447. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static DEX_EKUBO: &[SettingDef] = &[
    d("Контракты DEX", "dex.ekubo_core", K::Address, L::None, "адрес", DX, "Решение 447: Ekubo (ekubo) — ядро Core: в нём все пулы (состояние, тики, битмап читаются sload ядра), события пулов (анонимный лог обмена и PositionUpdated) и обмен (шаг EKUBO исполнителя: lock → swap). Официальный адрес (docs.ekubo.org evm-v3, все сети, Arbitrum) — 0x00000000000014aA86C5d3c41765bb24e11bd701. Не задан — DEX не подключена"),
    d("Контракты DEX", "dex.ekubo_key_index", K::Address, L::None, "адрес", DX, "Решение 447: Ekubo — реестр ключей пулов PoolKeyIndex: ключ пула (token0, token1, config) по PoolId из справочника индексатора (poolKeyById); ключ должен дать тот же PoolId. Пул, которого нет в реестре, не берётся (причина на экране). Arbitrum — 0x898956fc2aed01d5f81f556ff5dcb10534285718 (EkuboProtocol/evm-contracts broadcast DeployAll.s.sol/42161, commit 02e0d82f; на странице контрактов docs.ekubo.org адреса нет). Не задан — DEX не подключена"),
    d("Контракты DEX", "dex.ekubo_mev_capture", K::Address, L::None, "адрес", DX, "Решение 447: Ekubo — расширение MEVCapture: пулы с этим расширением берутся (обмен — через forward ядра к расширению, расчёт — с доплатой за пройденные тики, math/ekubo_mev.rs). Официальный адрес (docs.ekubo.org evm-v3) — 0x5555fF9Ff2757500BF4EE020DcfD0210CFfa41Be. Не задан — пулы с MEVCapture не берутся (пулы без расширения — берутся). Пулы с другими расширениями — по их настройкам (решение 454: dex.ekubo_twamm, dex.ekubo_oracle, dex.ekubo_boosted_fees)").optional(),
    d("Контракты DEX", "dex.ekubo_bitmap_words", K::Integer, L::Positive, "слов", DX, "Решение 447: Ekubo — сколько слов битмапа тиков читать по каждую сторону от слова текущего тика (слово — 256 шагов тика пула). Расчёт обмена за прочитанными словами не идёт — он частичный (больше слов — глубже расчёт и больше работы узла на перечитывании). Не задана — DEX не подключена"),
    d("Контракты DEX", "dex.ekubo_twamm", K::AddressList, L::None, "адреса", DX, "Решение 454: Ekubo — адреса расширения TWAMM: пулы с ним берутся; перед нашим обменом ядро исполняет виртуальные ордера TWAMM — котировка проводит их сама (math/ekubo_virtual.rs) к двум возможным временам блока сделки и берёт худшее; состояние расширения читается вместе с пулом, любое событие такого пула — перечитывание. Arbitrum (EkuboProtocol/evm-contracts broadcast DeployAll.s.sol/42161): 0xd4f1060cb9c1a13e1d2d20379b8aa2cf7541ed9b (commit 61d09daa) и 0xd47f1b1edcfeabb08f6ebd8fc337c27e636c75ba (commit 8c1b721, он же на docs.ekubo.org evm-v3). Не задана — пулы с TWAMM не берутся (причина на экране)").optional(),
    d("Контракты DEX", "dex.ekubo_twamm_horizon_s", K::Integer, L::Positive, "секунд", DX, "Решение 454: Ekubo TWAMM — на сколько секунд после блока чтения пула читаются времена ордеров TWAMM (слова битмапа времён и их TimeInfo). Котировка такого пула считает виртуальные ордера только до этого времени; позже (пул без событий дольше горизонта) — пул не котируется с причиной «перечитать пул», пока событие пула или переподключение его не перечитает. Больше — дольше котируется тихий пул, но больше слотов читается при каждом перечитывании. Не задана — пулы с TWAMM не читаются (причина на экране)").optional(),
    d("Контракты DEX", "dex.ekubo_oracle", K::Address, L::None, "адрес", DX, "Решение 454: Ekubo — адрес расширения Oracle: пулы с ним берутся (Oracle перед обменом только записывает снимок цены в своё хранилище — обмен не меняет, Oracle.sol L193-L201). Arbitrum — 0x517E506700271AEa091b02f42756F5E174Af5230 (docs.ekubo.org evm-v3; broadcast DeployAll.s.sol/42161, commit 61d09daa). Пулы Oracle — всегда с нативным ETH (нужна dex.native_wrapped). Не задан — пулы с Oracle не берутся").optional(),
    d("Контракты DEX", "dex.ekubo_boosted_fees", K::AddressList, L::None, "адреса", DX, "Решение 454: Ekubo — адреса расширения BoostedFees: пулы с ним берутся (перед обменом расширение только начисляет пожертвованные комиссии — Core.accumulateAsFees меняет комиссии на ликвидность, не цену и ликвидность; BoostedFees.sol L100-L102, L138-L237). Arbitrum (docs.ekubo.org evm-v3): 0xd4B54d0ca6979Da05F25895E6e269E678ba00f9e (concentrated) и 0x948b9C2C99718034954110cB61a6e08e107745f9 (stableswap). Не задана — пулы с BoostedFees не берутся").optional(),
    d("DEX", "dex.ekubo_hub_buffer", K::Integer, L::Positive, "сообщений", V, "Решение 454: Ekubo — одна подписка на логи ядра (и TWAMM) на всю площадку с раздачей по пачкам пулов (источник событий ws): сколько логов может ждать самая медленная пачка; отстала больше — пачка переподключается и перечитывает свои пулы (как при обрыве). Не задана — пачки пулов Ekubo не подписываются (причина в журнале)").dex_venue(),
];
