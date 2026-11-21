//! Файл: settings/catalog/dex_elastic.rs
//! Что делает: настройки вида KyberSwap Elastic (kyber_elastic / kyber_elastic_v1) — решение 475: длина части чтения
//! тиков у TicksFeesReader. Отдельный файл: каталог dex.rs общий для параллельных задач семейств DEX (решение 433) —
//! так добавление не пересекается с чужими строками. Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, раздел «Контракты DEX»).

use super::{DX, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Настройки решения 475. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static DEX_ELASTIC: &[SettingDef] = &[
    d("Контракты DEX", "dex.elastic_ticks_chunk", K::Integer, L::Positive, "тиков", DX, "Решение 475: KyberSwap Elastic (kyber_elastic, kyber_elastic_v1) — сколько тиков читать одним вызовом TicksFeesReader.getTicksInRange(pool, start, length): тики пула читаются частями по связному списку (первая часть — от MIN_TICK, следующая — от последнего тика прошлой, до MAX_TICK). Вызов «всё сразу» (length = 0) выделяет массив на все возможные тики пула — у пулов с малым tickDistance «out of gas» (запись Arbitrum 20.11.2026: tickDistance 8 и 10). Целое от 2. Больше — меньше вызовов, но дороже газ одного eth_call (на тик не меньше 2 300 газа по EIP-2929 — холодный SLOAD initializedTicks 2 100 + тёплые EXTCODESIZE и STATICCALL по 100, плюс исполнение; память массива 3·n + n²/512; предел газа eth_call — у узла); совет — docs/FIRST_RUN.md. Не задана — DEX вида Elastic не подключается (в т. ч. на BSC)"),
];
